import argparse
import json
import re
from pathlib import Path

import cv2
import numpy as np


def bayer_pattern(raw):
    if raw[:2] not in (b"P5", b"P6"):
        return None
    cursor = 2
    tokens = 0
    pattern = None
    while cursor < len(raw) and tokens < 3:
        if raw[cursor:cursor + 1].isspace():
            cursor += 1
        elif raw[cursor] == ord("#"):
            end = raw.find(b"\n", cursor)
            if end < 0:
                raise ValueError("PNM 头不完整")
            match = re.fullmatch(rb"#\s*bayer\s+(RG|BG|GR|GB)", raw[cursor:end].strip(), re.I)
            if match:
                pattern = match[1].decode().upper()
            cursor = end + 1
        else:
            while cursor < len(raw) and not raw[cursor:cursor + 1].isspace():
                cursor += 1
            tokens += 1
    return pattern


def prepare(record, output, product_code, frame_rate, pass_frames):
    root = record.resolve(strict=True)
    metadata = json.loads((root / "part.json").read_text(encoding="utf-8-sig"))
    if not metadata.get("available") or metadata.get("missingFrames") or metadata.get("rawDropped"):
        raise ValueError("仅接受原图完整可用、没有缺帧的录制件")
    groups = {}
    for frame in metadata["frames"]:
        if not frame.get("available") or frame.get("manual"):
            raise ValueError("录制件含不可用帧或手动触发帧")
        camera = frame["cameraId"]
        if not re.fullmatch(r"[A-Za-z0-9_-]+", camera):
            raise ValueError("相机 ID 无效")
        seq = frame["seq"]
        if type(seq) is not int or seq < 1 or seq in groups.setdefault(camera, {}):
            raise ValueError("相机帧号重复或无效")
        relative_source = Path(frame["file"])
        if relative_source.is_absolute() or relative_source.drive:
            raise ValueError("原图路径必须相对于录制目录")
        source = (root / relative_source).resolve(strict=True)
        if not source.is_relative_to(root):
            raise ValueError("原图路径越过录制目录")
        groups[camera][seq] = source
    if not 1 <= len(groups) <= 8:
        raise ValueError("需要 1–8 台相机的完整录制帧")
    count = len(next(iter(groups.values())))
    if any(sorted(frames) != list(range(1, count + 1)) for frames in groups.values()):
        raise ValueError("各相机必须具有从 1 开始、连续且相同的帧序列")
    passes = [int(n) for n in pass_frames.split(",")] if pass_frames else [count]
    if any(n <= 0 for n in passes) or sum(passes) != count:
        raise ValueError("每笔帧数须大于 0，合计等于录制帧组数")
    if not 1 <= product_code <= 32767 or not 1 <= frame_rate <= 60:
        raise ValueError("型号须在 1–32767，帧率须在 1–60 Hz")
    output.mkdir(parents=True, exist_ok=False)
    cameras = []
    formats = {}
    patterns = {"RG": cv2.COLOR_BayerRGGB2BGR, "BG": cv2.COLOR_BayerBGGR2BGR,
                "GR": cv2.COLOR_BayerGRBG2BGR, "GB": cv2.COLOR_BayerGBRG2BGR}
    for index, (camera, frames) in enumerate(sorted(groups.items())):
        (output / camera).mkdir()
        paths = []
        mode = None
        shape = None
        for seq, source in sorted(frames.items()):
            raw = np.fromfile(source, dtype=np.uint8)
            image = cv2.imdecode(raw, cv2.IMREAD_UNCHANGED)
            if image is None or image.dtype != np.uint8:
                raise ValueError(f"无法读取 8 位图像：{source}")
            bayer = bayer_pattern(raw.tobytes())
            if bayer:
                if image.ndim != 2:
                    raise ValueError(f"Bayer 图像不是单通道：{source}")
                image = cv2.cvtColor(image, patterns[bayer])
            if image.ndim != 2 and (image.ndim != 3 or image.shape[2] != 3):
                raise ValueError(f"仅支持灰度或三通道彩色图像：{source}")
            if shape is not None and shape != image.shape:
                raise ValueError(f"相机 {camera} 的图像尺寸或通道不一致")
            shape = image.shape
            current = "gray" if image.ndim == 2 else "color"
            if mode is not None and mode != current:
                raise ValueError(f"相机 {camera} 的图像模式不一致")
            mode = current
            relative = f"{camera}/{seq:06}.png"
            ok, encoded = cv2.imencode(".png", image)
            if not ok:
                raise ValueError(f"图像编码失败：{source}")
            encoded.tofile(output / relative)
            paths.append(relative)
        cameras.append({"id": camera, "port": 12001 + index, "frames": paths})
        formats[camera] = mode
    config = {"plcPort": 1102, "plcDb": 4, "pcDb": 2, "productCode": product_code,
              "snPrefix": "SIM", "frameRate": frame_rate, "captureLeadMs": 2000,
              "intervalMs": 2000, "resultTimeoutMs": 10000, "passFrames": passes,
              "passGapMs": 500, "cameras": cameras}
    (output / "simulator.json").write_text(json.dumps(config, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"已生成 {output / 'simulator.json'}；每件 {count} 帧组，笔帧数 {passes}")
    for camera in cameras:
        print(f"{camera['id']}: 127.0.0.1:{camera['port']}, imageMode={formats[camera['id']]}")
    if not pass_frames:
        print("未指定 --pass-frames，按单笔回放；多笔录制请按原件笔边界填写。")


def main():
    parser = argparse.ArgumentParser(description="把完整录制件复制并转换为独立生产模拟器输入，不修改源资料或设备配置。")
    parser.add_argument("--record", type=Path, required=True, help="包含 part.json 的录制目录")
    parser.add_argument("--output", type=Path, required=True, help="尚不存在的新输出目录")
    parser.add_argument("--product-code", type=int, required=True)
    parser.add_argument("--frame-rate", type=float, default=10)
    parser.add_argument("--pass-frames", help="按原件填写每笔帧数，例如 30,20,40")
    args = parser.parse_args()
    prepare(args.record, args.output, args.product_code, args.frame_rate, args.pass_frames)


if __name__ == "__main__":
    main()
