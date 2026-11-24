"""用同一组 Bayer 工件对照 LyFlow 原生混胶算子与 Python 离线量测。"""

import argparse
import ctypes as ct
import hashlib
import json
import tempfile
from contextlib import contextmanager
from pathlib import Path

import numpy as np

from mix_eval import evaluate, metadata, read_frame, sample_rectangles


class ImageView(ct.Structure):
    _fields_ = [(name, ct.c_uint32) for name in (
        "width", "height", "channels", "depth", "level", "full_width", "full_height",
        "row_offset", "row_count", "row_bytes",
    )] + [("pixels", ct.c_void_p), ("handle", ct.c_void_p)]


EventCallback = ct.CFUNCTYPE(None, ct.c_char_p, ct.c_void_p)


class NativeCore:
    def __init__(self, path):
        self.lib = ct.CDLL(str(path.resolve()))
        string, pointer, uint = ct.c_char_p, ct.c_void_p, ct.c_uint32
        signatures = {
            "version": (string, []),
            "abi_version_string": (string, []),
            "manifest_json": (pointer, []),
            "run_start": (pointer, [string, pointer, EventCallback, pointer]),
            "run_join": (None, [pointer]),
            "run_free": (None, [pointer]),
            "run_summary": (pointer, [string]),
            "run_outputs": (pointer, [string]),
            "string_free": (None, [pointer]),
            "output_image": (ct.c_int, [string, string, string, uint, uint, uint, ct.POINTER(ImageView)]),
            "image_view_free": (None, [ct.POINTER(ImageView)]),
        }
        for name, (result, arguments) in signatures.items():
            function = getattr(self.lib, "lyflow_" + name)
            function.restype = result
            function.argtypes = arguments

    def json_result(self, pointer):
        if not pointer:
            raise ValueError("LyFlow 返回空结果")
        try:
            return json.loads(ct.string_at(pointer))
        finally:
            self.lib.lyflow_string_free(pointer)

    @contextmanager
    def run(self, graph):
        finished = {}

        @EventCallback
        def receive(raw_event, _):
            event = json.loads(raw_event)
            if event["kind"] == "run_finished":
                finished.update(event)

        handle = self.lib.lyflow_run_start(json.dumps(graph).encode(), None, receive, None)
        if not handle:
            raise ValueError("LyFlow 无法启动图")
        try:
            self.lib.lyflow_run_join(handle)
            run_id = finished["runId"].encode()
            summary = self.json_result(self.lib.lyflow_run_summary(run_id))
            if summary["status"] != "ok":
                raise ValueError(f"LyFlow 图执行失败：{finished}，{summary}")
            yield run_id, self.json_result(self.lib.lyflow_run_outputs(run_id))
        finally:
            self.lib.lyflow_run_free(handle)

    def image(self, run_id, port, dtype):
        view = ImageView()
        if self.lib.lyflow_output_image(run_id, b"map", port.encode(), 0, 0, 0, ct.byref(view)):
            raise ValueError(f"无法读取 map.{port}")
        try:
            if view.channels != 1 or view.depth != np.dtype(dtype).itemsize or view.row_count != view.height:
                raise ValueError(f"map.{port} 的图像格式有误")
            data = ct.string_at(view.pixels, view.row_bytes * view.height)
            return np.frombuffer(data, dtype=dtype).reshape(view.height, view.width)
        finally:
            self.lib.lyflow_image_view_free(ct.byref(view))


def node(node_id, op, params):
    return {"id": node_id, "op": op, "opVersion": "1.0.0", "params": params}


def graph(name, nodes, links, outputs):
    def port(address):
        name, field = address.split(".", 1)
        return {"node": name, "port": field}
    return {
        "schemaVersion": 1, "id": name, "name": name, "nodes": nodes,
        "edges": [{"id": f"e{i}", "from": port(a), "to": port(b)} for i, (a, b) in enumerate(links)],
        "outputs": {key: port(address) for key, address in outputs.items()},
    }


def read_record(outputs, name, kind):
    value = outputs[name]["value"]
    if value["kind"] != "Record" or value["type"] != kind:
        raise ValueError(f"{name} 缺少预期的 {kind} 记录")
    return value["data"]


def file_node(name, path):
    return node(name, "io.load_image", {"source": "file", "path": str(path.resolve())})


def training_graph(spec_path, spec, directory, white_balance):
    images, labels = [], []
    for entry in spec["fit"]:
        part = (spec_path.parent / entry["part"]).resolve()
        rgb, _, _ = read_frame(part, metadata(part), entry["seq"], spec["cameraId"], spec["datasetKind"])
        mask = np.zeros(rgb.shape[:2], dtype=np.uint8)
        for code, role in enumerate(("green", "yellow", "blue", "background"), 1):
            if role in entry["samples"]:
                selected = sample_rectangles(mask.shape, entry["samples"][role])
                if np.any(mask[selected]):
                    raise ValueError(f"{part} 包含角色相互重叠的取样")
                mask[selected] = code
        images.append(rgb)
        labels.append(mask)
    h, w = images[0].shape[:2]
    if any(image.shape != images[0].shape for image in images):
        raise ValueError("同一相机的拟合帧需要相同尺寸")
    atlas = directory / "fit.ppm"
    label_file = directory / "labels.pgm"
    for path, magic, stack in ((atlas, "P6", images), (label_file, "P5", labels)):
        path.write_bytes(f"{magic}\n{w} {h * len(images)}\n255\n".encode() + np.concatenate(stack).tobytes())
    template_path = Path(__file__).resolve().parents[2] / "src-tauri/graphs/mix_teach.lyflow.json"
    template = metadata(template_path)
    nodes = {n["id"]: n for n in template["nodes"]}
    for name, path in (("n_load", atlas), ("n_labels", label_file)):
        nodes[name]["params"].update(source="file", path=str(path.resolve()))
    template["params"]["frameHeight"]["default"] = h
    template["params"]["whiteBalance"]["default"] = json.dumps(white_balance)
    nodes["fit"]["params"]["highlightRadiusPx"] = spec["measurement"].get("highlightRadiusPx", 1)
    return template


def measurement_graph(image, case, settings, model):
    return graph("mix-measure", [
        file_node("rgb", image),
        node("line", "glue.polyline", {"points": json.dumps(case["line"])}),
        node("map", "glue.mix_map", {"model": json.dumps(model), "offAxisLimit": settings["offAxisLimit"],
                                     "backgroundRects": json.dumps(case["background"])}),
        node("measure", "glue.mix_regions", {k: settings[k] for k in (
            "halfWidthPx", "smoothPx", "axisThreshold", "maxHighlightGapPx")}),
    ], [("rgb.image", "map.image"), ("map.maps", "measure.maps"), ("line.line", "measure.line")],
        {"observation": "measure.observation"})


def compare_value(actual, expected, name):
    if isinstance(expected, dict):
        for key, value in expected.items():
            compare_value(actual[key], value, f"{name}.{key}")
    elif isinstance(expected, list):
        if len(actual) != len(expected):
            raise AssertionError(f"{name} 数量不一致：{len(actual)} / {len(expected)}")
        for index, (a, b) in enumerate(zip(actual, expected)):
            compare_value(a, b, f"{name}[{index}]")
    elif isinstance(expected, float):
        if not np.isclose(actual, expected, atol=5e-5, rtol=2e-5):
            raise AssertionError(f"{name} 数值不符：{actual} / {expected}")
    elif actual != expected:
        raise AssertionError(f"{name} 不符：{actual} / {expected}")


def ordered_regions(regions):
    def ring_key(points):
        vertices = [tuple(p) for p in points]
        start = min(vertices)
        cycles = []
        for ordering in (vertices, vertices[::-1]):
            for i, point in enumerate(ordering):
                if point == start:
                    cycles.append(ordering[i:] + ordering[:i])
        return min(cycles)

    normalized = [dict(r, contours=sorted(ring_key(c) for c in r["contours"])) for r in regions]
    return sorted(normalized, key=lambda r: (r["direction"], r["contours"]))


def compare_frame(core, run_id, outputs, ref_dir, title):
    ref = metadata(ref_dir / "observation.json")
    obs = read_record(outputs, "observation", "glue.MixObservation.v1")
    compare_value(ordered_regions(obs["regions"]), ordered_regions(ref["regions"]), title + ".regions")
    compare_value(obs["stations"], ref["stations"], title + ".stations")
    background = {k: v for k, v in ref["background"].items() if k != "classifiedSurfaceFraction"}
    compare_value(obs["background"], background, title + ".background")
    errors = {}
    with np.load(ref_dir / "maps.npz") as expected:
        for field, port, dtype in (("state", "maps.state", np.uint8), ("axis", "maps.axis", np.float32),
                                   ("off_axis", "maps.offAxis", np.float32)):
            actual = core.image(run_id, port, dtype)
            reference = expected[field]
            np.testing.assert_allclose(actual, reference, rtol=2e-5, atol=5e-5, equal_nan=True,
                                       err_msg=f"{title}.{field}")
            if field != "state":
                finite = np.isfinite(reference)
                errors[field] = float(np.max(np.abs(actual[finite] - reference[finite]), initial=0))
        score = np.exp(-0.5 * expected["off_axis"] ** 2)
        score[expected["state"] == 3] = 0
        np.testing.assert_allclose(core.image(run_id, "colorMaps.score", np.float32), score,
                                   rtol=2e-5, atol=5e-5, equal_nan=True, err_msg=title + ".score")
    return {"cycleId": ref["cycleId"], "seq": ref["seq"], "regions": len(obs["regions"]),
            "stations": len(obs["stations"]), "maxAbsoluteError": errors}


def verify(spec_path, dll, directory):
    spec = metadata(spec_path)
    core = NativeCore(dll)
    operators = core.json_result(core.lib.lyflow_manifest_json())["operators"]
    required = {"glue.mix_model", "glue.mix_map", "glue.mix_regions"}
    missing = required - {op["id"] for op in operators}
    if missing:
        raise ValueError(f"指定的 DLL 缺少算子：{', '.join(sorted(missing))}")
    ref = directory / "python"
    evaluate(spec_path, ref)
    reference_model = metadata(ref / "model.json")
    training = training_graph(spec_path, spec, directory, reference_model["whiteBalance"])
    with core.run(training) as (_, outputs):
        native_model = read_record(outputs, "model", "glue.MixModel.v1")
    compare_value(native_model, dict(reference_model["model"], whiteBalance=reference_model["whiteBalance"]),
                  "model")
    cases = []
    for index, entry in enumerate(spec["cases"], 1):
        ref_dir = ref / f"frame-{index:04}"
        measurement = measurement_graph(ref_dir / "original.png", entry, spec["measurement"], native_model)
        with core.run(measurement) as (run_id, outputs):
            cases.append(compare_frame(core, run_id, outputs, ref_dir, f"frame-{index:04}"))
    with dll.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    return {"datasetKind": spec["datasetKind"], "version": core.lib.lyflow_version().decode(),
            "abiVersion": core.lib.lyflow_abi_version_string().decode(), "dllSha256": digest,
            "frames": cases}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("spec", type=Path)
    parser.add_argument("--dll", type=Path, required=True)
    parser.add_argument("--report", type=Path, help="保存通过对照的报告")
    args = parser.parse_args()
    try:
        with tempfile.TemporaryDirectory() as directory:
            report = verify(args.spec.resolve(), args.dll.resolve(), Path(directory))
        result = json.dumps(report, indent=2, ensure_ascii=False)
        if args.report:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(result + "\n", encoding="utf-8")
        print(result)
    except (OSError, KeyError, ValueError, AssertionError) as error:
        parser.exit(1, f"原生对照失败：{error}\n")


if __name__ == "__main__":
    main()
