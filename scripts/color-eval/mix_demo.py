"""生成三相机、八帧一件的混胶色差模拟器输入及可导入的合成模型。"""

import argparse
import json
from pathlib import Path
import runpy
import shutil
import tempfile

from mix_eval import evaluate
from mix_fixture import generate


def prepare_demo(output):
    if output.exists():
        raise ValueError("输出目录已存在，请指定新目录")
    output.parent.mkdir(parents=True, exist_ok=True)
    prepare = runpy.run_path(str(Path(__file__).resolve().parents[1] / "Prepare-ProductionSimulation.py"))["prepare"]
    with tempfile.TemporaryDirectory(dir=output.parent) as temp:
        work = Path(temp)
        spec_path = generate(work / "fit")
        evaluate(spec_path, work / "reference")
        spec = json.loads(spec_path.read_text("utf-8"))
        cases = {Path(c["part"]).parent.name: c for c in spec["cases"]}
        names = ["mix-tune-green", "mix-tune-green", "mix-tune-edge-pair", "mix-holdout-slight-blue",
                 "mix-holdout-highlight", "mix-holdout-lighting", "mix-holdout-yellow", "mix-holdout-blue"]
        record = work / "part"
        record.mkdir()
        devices, frames = [], []
        for index, camera in enumerate(("cam1", "cam2", "cam3")):
            for seq, case_name in enumerate(names, 1):
                case = cases[case_name]
                part_path = spec_path.parent / case["part"]
                part = json.loads(part_path.read_text("utf-8"))
                source = part["frames"][0]
                filename = f"{camera}_s{seq:06d}.pgm"
                shutil.copyfile(part_path.parent / source["file"], record / filename)
                frames.append({"camera": index, "cameraId": camera, "seq": seq, "file": filename, "available": True})
                if seq == 1:
                    device = part["devices"][0]
                    devices.append({**device, "cameraId": camera,
                                    "cameraConfig": {**device["cameraConfig"], "serial": f"SYNTHETIC-MIX-{camera.upper()}"}})
        (record / "part.json").write_text(json.dumps({"datasetKind": "synthetic", "available": True,
                                                    "frames": frames, "devices": devices}), encoding="utf-8")
        bundle = work / "demo"
        prepare(record, bundle, 7701, 5, "8", plc_port=14192, camera_port=14193)
        model = json.loads((work / "reference/model.json").read_text("utf-8"))
        model["backgroundRects"] = cases["mix-tune-green"]["background"]
        (bundle / "model.json").write_text(json.dumps(model, indent=2) + "\n", encoding="utf-8")
        bundle.rename(output)
    print(f"联调输入与模型已写入 {output}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    try:
        prepare_demo(args.out)
    except (OSError, ValueError) as error:
        parser.exit(2, f"生成失败：{error}\n")
