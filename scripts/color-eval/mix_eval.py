"""从带白平衡回读的 GlueSight Bayer 录制件拟合混合轴，导出逐帧量测与复核图。"""

import argparse
from functools import lru_cache
import json
from pathlib import Path
import tempfile

import cv2
import numpy as np

from glue_color_eval import EvalError, load_image, pgm_bayer_pattern
from mix_model import (DARK, HIGHLIGHT, VALID, compute_mix_map, fit_mix_model, measure_background,
                       usable_sample_mask)
from mix_regions import measure_regions


def metadata(path):
    return json.loads(path.read_text(encoding="utf-8-sig"))


def read_frame(part_path, part, seq, camera, dataset_kind):
    if part.get("datasetKind", "field") != dataset_kind:
        raise ValueError(f"{part_path} 的数据来源与标注的 datasetKind 不一致")
    matches = [f for f in part["frames"] if f["cameraId"] == camera and f["seq"] == seq]
    devices = [d for d in part["devices"] if d["cameraId"] == camera]
    if len(matches) != 1 or len(devices) != 1:
        raise ValueError(f"{part_path} 未找到唯一的 {camera} 帧 {seq} 或相机记录")
    frame, device = matches[0], devices[0]
    if not frame.get("available") or frame.get("imageMode") != "color":
        raise ValueError(f"{part_path} 第 {seq} 帧缺失或不是彩色")
    if frame.get("session") is None or frame["session"] != device.get("deviceSession"):
        raise ValueError(f"{part_path} 第 {seq} 帧和设备回读属于不同的相机会话")
    wb = (device.get("imaging") or {}).get("whiteBalance")
    if (not isinstance(wb, dict) or wb.get("unit") not in ("ratio", "deviceInteger")
            or not all(isinstance(wb.get(c), (int, float)) and np.isfinite(wb[c]) and wb[c] > 0
                       for c in ("red", "green", "blue"))):
        raise ValueError(f"{part_path} 缺少有效的白平衡回读，不可用于混胶评估")
    serial = device["cameraConfig"].get("serial")
    cycle = part.get("cycleId")
    if not serial or not cycle:
        raise ValueError(f"{part_path} 缺少相机序列号或工件 cycleId")
    path = part_path.parent / frame["file"]
    if not pgm_bayer_pattern(path):
        raise ValueError(f"{path} 缺少原始 Bayer 相位")
    rgb, _ = load_image(path, None)
    return rgb.astype(np.uint8), cycle, {"serial": serial, "whiteBalance": wb}


def sample_rectangles(shape, rectangles):
    h, w = shape
    mask = np.zeros(shape, dtype=bool)
    for rect in rectangles:
        if len(rect) != 4 or not all(type(v) is int for v in rect):
            raise ValueError(f"取样矩形应为整数 [x, y, w, h]：{rect}")
        x, y, width, height = rect
        if not (0 <= x < x + width <= w and 0 <= y < y + height <= h):
            raise ValueError(f"取样矩形越界或为空：{rect}")
        mask[y:y+height, x:x+width] = True
    if not mask.any():
        raise ValueError("缺少取样矩形")
    return mask


def save_png(path, rgb):
    ok, data = cv2.imencode(".png", cv2.cvtColor(rgb, cv2.COLOR_RGB2BGR))
    if not ok:
        raise ValueError(f"无法编码 {path}")
    path.write_bytes(data.tobytes())


def save_evidence(directory, rgb, maps, observations, station_threshold):
    directory.mkdir(parents=True, exist_ok=True)
    save_png(directory / "original.png", rgb)
    valid = maps["state"] == VALID
    axial = np.where(valid, maps["axis"], 0)
    strength = np.clip(np.abs(axial), 0, 1)[..., None]
    yellow, blue = [246, 203, 32], [52, 125, 252]
    colors = np.where(axial[..., None] < 0, yellow, blue)
    heat = (rgb * 0.28).astype(np.uint8)
    heat[valid] = ((1 - strength) * [30, 147, 70] + strength * colors)[valid].astype(np.uint8)
    heat[maps["state"] == HIGHLIGHT] = [245, 245, 245]
    heat[maps["state"] == DARK] = [30, 30, 30]
    save_png(directory / "axis.png", heat)
    np.savez_compressed(directory / "maps.npz", axis=maps["axis"], off_axis=maps["off_axis"],
                        state=maps["state"])
    overlay = rgb.copy()
    for region in observations["regions"]:
        color = yellow if region["direction"] == "yellow" else blue
        for contour in region["contours"]:
            points = np.rint(contour).astype(np.int32)
            cv2.polylines(overlay, [points], True, color, thickness=1)
    for station in observations["stations"]:
        median = station["medianAxial"]
        if median is not None and abs(median) >= station_threshold:
            xy = tuple(np.rint(station["center"]).astype(int))
            cv2.circle(overlay, xy, 1, yellow if median < 0 else blue, -1)
    save_png(directory / "overlay.png", overlay)


def write_json(path, value):
    path.write_text(json.dumps(value, allow_nan=False, indent=2, ensure_ascii=False), encoding="utf-8")


def evaluate(spec_path, out):
    out.parent.mkdir(parents=True, exist_ok=True)
    if out.exists() and (not out.is_dir() or any(out.iterdir())):
        raise ValueError(f"输出目录已有内容，请指定新目录或空目录：{out}")
    with tempfile.TemporaryDirectory(dir=out.parent) as staging:
        result = Path(staging) / "result"
        result.mkdir()
        evaluate_into(spec_path, result)
        if out.is_dir():
            out.rmdir()
        result.rename(out)


def evaluate_into(spec_path, out):
    spec = metadata(spec_path.resolve())
    camera = spec["cameraId"]
    kind = spec["datasetKind"]
    if kind not in ("field", "synthetic") or not spec["fit"] or not spec["cases"]:
        raise ValueError("需要 datasetKind（field/synthetic）、拟合帧和评估帧")
    identity = None
    partitions = {}
    read_part = lru_cache(maxsize=32)(metadata)

    def frame_in_split(entry, split):
        nonlocal identity
        part_path = (spec_path.parent / entry["part"]).resolve()
        rgb, cycle, current = read_frame(part_path, read_part(part_path), entry["seq"], camera, kind)
        if identity is None:
            identity = current
        if current != identity:
            raise ValueError("录制件相机序列号或实测白平衡与拟合帧不同")
        if partitions.setdefault(cycle, split) != split:
            raise ValueError(f"同一工件 {cycle} 跨越拟合、调参或验收集")
        return rgb, cycle

    samples = {name: [] for name in ("green", "yellow", "blue", "background")}
    settings = spec["measurement"]
    highlight_radius = settings.get("highlightRadiusPx", 1)
    for entry in spec["fit"]:
        rgb, _ = frame_in_split(entry, "fit")
        usable = usable_sample_mask(rgb, highlight_radius_px=highlight_radius)
        for role, rects in entry["samples"].items():
            samples[role].append(rgb[usable & sample_rectangles(rgb.shape[:2], rects)])
    if not all(samples.values()):
        raise ValueError("拟合需要 green、yellow、blue 和 background 四组取样")
    model = fit_mix_model(**{role: np.concatenate(rows) for role, rows in samples.items()},
                          highlight_radius_px=highlight_radius)
    station_threshold = settings["stationThreshold"]
    if not 0 < station_threshold <= 1:
        raise ValueError("stationThreshold 应在 (0, 1] 范围内")
    summary = []
    for index, entry in enumerate(spec["cases"], 1):
        split = entry["split"]
        if split not in ("tune", "holdout"):
            raise ValueError("评估帧 split 只接受 tune 或 holdout")
        rgb, cycle = frame_in_split(entry, split)
        maps = compute_mix_map(rgb, model, off_axis_limit=settings["offAxisLimit"])
        observations = measure_regions(
            maps["axis"], maps["state"], entry["line"],
            half_width_px=settings["halfWidthPx"], smooth_px=settings["smoothPx"],
            axis_threshold=settings["axisThreshold"], max_highlight_gap_px=settings["maxHighlightGapPx"],
        )
        background_mask = sample_rectangles(rgb.shape[:2], entry["background"])
        background = measure_background(rgb, background_mask, model)
        background["classifiedSurfaceFraction"] = float((maps["state"][background_mask] == VALID).mean())
        coverage = None
        if "surface" in entry:
            surface = maps["state"][sample_rectangles(rgb.shape[:2], entry["surface"])]
            coverage = {"annotatedPixels": int(surface.size),
                        "unmeasurableFraction": float((surface != VALID).mean())}
        case_dir = out / f"frame-{index:04}"
        save_evidence(case_dir, rgb, maps, observations, station_threshold)
        case = {"frameDir": case_dir.name, "cycleId": cycle, "seq": entry["seq"], "split": split,
                "note": entry.get("note", ""), "background": background, "surfaceCoverage": coverage}
        write_json(case_dir / "observation.json", {**case, **observations})
        summary.append(case)
    reference = {"cameraId": camera, **identity, "datasetKind": kind, "model": model}
    if all(case["background"] == spec["cases"][0]["background"] for case in spec["cases"]):
        reference["backgroundRects"] = spec["cases"][0]["background"]
    write_json(out / "model.json", reference)
    write_json(out / "report.json", {"datasetKind": kind, "measurement": settings, "frames": summary})


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("spec", type=Path, help="取样和评估标注 JSON")
    parser.add_argument("--out", type=Path, required=True, help="新的或空的评估结果目录")
    args = parser.parse_args()
    try:
        evaluate(args.spec, args.out)
    except (OSError, KeyError, ValueError, TypeError, EvalError) as exc:
        parser.exit(2, f"评估失败：{exc}\n")
