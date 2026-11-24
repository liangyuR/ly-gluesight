"""生成混胶色差离线评估使用的合成 Bayer 工件与取样标注。"""

import argparse
import json
from pathlib import Path

import numpy as np


WIDTH, HEIGHT = 192, 80
GREEN = np.array([61, 160, 46], dtype=float)
YELLOW = np.array([196, 155, 28], dtype=float)
BLUE = np.array([40, 86, 194], dtype=float)
BACKGROUND = np.array([82, 84, 89], dtype=float)
BEAD = slice(28, 53)
BACKGROUND_PATCHES = [[20, 10, 152, 8], [20, 62, 152, 8]]
CAMERA = "cam1"
FRAME = f"{CAMERA}_s000001.pgm"


def shifted_green(endpoint, fraction):
    """用已知对数色度位置构造图像，fraction 是合成坐标。"""
    log_rgb = np.log1p(GREEN) + fraction * (np.log1p(endpoint) - np.log1p(GREEN))
    return np.expm1(log_rgb)


def make_bayer(scene, seed):
    image = np.empty((HEIGHT, WIDTH, 3))
    image[:] = BACKGROUND
    image[BEAD] = GREEN
    if scene in ("yellow", "blue"):
        image[BEAD] = YELLOW if scene == "yellow" else BLUE
    elif scene == "edge-pair":
        image[28:36, 34:151] = YELLOW
        image[36:44, 34:151] = BLUE
    elif scene == "highlight":
        image[34:47, 34:151] = YELLOW
    elif scene == "slight-blue":
        image[BEAD] = shifted_green(BLUE, 0.25)
    elif scene == "lighting":
        image *= (shifted_green(YELLOW, 0.25) + 1) / (GREEN + 1)
    elif scene not in ("green", "gloss"):
        raise ValueError(f"未知合成场景 {scene}")
    rng = np.random.default_rng(seed)
    image *= rng.uniform(0.98, 1.02)
    image += rng.normal(0, 0.7, image.shape)
    if scene == "highlight":
        image[27:54, 88:92] = 255
    elif scene == "gloss":
        image[32:46, 76:113] = 255
    image = np.rint(image).clip(0, 255).astype(np.uint8)
    raw = image[..., 1].copy()
    raw[::2, ::2] = image[::2, ::2, 2]
    raw[1::2, 1::2] = image[1::2, 1::2, 0]
    return raw


def write_json(path, doc):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(doc, stream, ensure_ascii=False, indent=2, allow_nan=False)
        stream.write("\n")


def record(out, cycle, scene, seed):
    directory = out / "parts" / cycle
    directory.mkdir(parents=True)
    raw = make_bayer(scene, seed)
    with (directory / FRAME).open("xb") as stream:
        stream.write(f"P5\n# bayer BG\n{WIDTH} {HEIGHT}\n255\n".encode("ascii"))
        stream.write(raw.tobytes())
    wb = {"unit": "ratio", "red": 1.0, "green": 1.0, "blue": 1.0}
    write_json(directory / "part.json", {
        "datasetKind": "synthetic",
        "cycleId": cycle, "sn": cycle, "cameras": [CAMERA], "groupCount": 1,
        "available": True, "missingFrames": [], "rawDropped": [], "errors": [],
        "devices": [{
            "cameraId": CAMERA, "deviceSession": 1,
            "cameraConfig": {"serial": "SYNTHETIC-MIX-CAM1", "imageMode": "color"},
            "imaging": {"whiteBalance": wb},
        }],
        "frames": [{
            "camera": 0, "cam": 0, "cameraId": CAMERA, "session": 1, "seq": 1, "file": FRAME,
            "counter": "synthetic", "frameCounter": 1, "triggerCounter": 1,
            "width": WIDTH, "height": HEIGHT, "lostPackets": 0,
            "queued": True, "available": True, "error": None, "imageMode": "color",
        }],
    })
    return {"part": f"parts/{cycle}/part.json", "seq": 1}


def generate(out):
    out.mkdir(parents=True, exist_ok=True)
    if any(out.iterdir()):
        raise ValueError(f"目录已有内容，请指定新的或空的目录：{out}")
    fit = []
    cases = []
    for index, role in enumerate(("green", "green", "yellow", "blue")):
        entry = record(out, f"mix-fit-{index+1}-{role}", role, seed=1200+index)
        entry["samples"] = {role: [[20, 32, 152, 16]]}
        if role == "green":
            entry["samples"]["background"] = BACKGROUND_PATCHES
        fit.append(entry)
    conditions = [
        ("green", "tune", "合格绿胶，局部区域为空，测站中位数接近 0。"),
        ("edge-pair", "tune", "横向紧邻的黄蓝纹，黄纹贴着上胶边。胶路内应各有一个长区域。"),
        ("gloss", "holdout", "正常绿胶中部饱和反光，反光不可测，其余胶面正常。"),
        ("highlight", "holdout", "同一黄纹中间有 4 px 饱和空缺。检查 Bayer 彩边是否阻断合并，空缺应从确认长度中扣除。"),
        ("slight-blue", "holdout", "整段沿混合轴轻微偏蓝，测站中位数应越过测站门限而低于局部门限。"),
        ("yellow", "holdout", "整段纯黄，黄胶面可测且连续，测站中位数接近 −1。"),
        ("blue", "holdout", "整段纯蓝，蓝胶面可测且连续，测站中位数接近 +1。"),
        ("lighting", "holdout", "胶面与底材共同向黄色方向漂移，背景色度位移可观测。"),
    ]
    for index, (scene, split, note) in enumerate(conditions):
        entry = record(out, f"mix-{split}-{scene}", scene, seed=3400+index)
        entry.update(split=split, note=note, line=[[18, 40], [173, 40]],
                     background=BACKGROUND_PATCHES, surface=[[18, 28, 156, 25]])
        cases.append(entry)
    spec = out / "spec.json"
    write_json(spec, {
        "datasetKind": "synthetic", "cameraId": CAMERA,
        "measurement": {"offAxisLimit": 3.5, "highlightRadiusPx": 1, "halfWidthPx": 14, "smoothPx": 3,
                        "axisThreshold": 0.45, "stationThreshold": 0.16,
                        "maxHighlightGapPx": 8},
        "fit": fit, "cases": cases,
    })
    return spec


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True, help="新的或空的合成录制件目录")
    args = parser.parse_args()
    try:
        print(generate(args.out))
    except (OSError, ValueError) as exc:
        parser.exit(2, f"生成失败：{exc}\n")
