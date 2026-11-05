"""比较几种“彩色转单通道”方式下，有色胶条在不同板材上的对比度，用来决定 Bayer 转单通道采用哪种公式。

现行取图把 8 位 Bayer 按 (R + 2G + B) / 4 转灰度（`frame.rs::bayer8_gray_into`），胶在两种材质上的明暗方向可能相反。
本脚本读彩色样图（或相机原始 Bayer），在人工标注的探针上量每种通道的胶对比度、极性、材质间响应比和干扰物响应。
标注格式与命令见 docs/testing/TESTING.md“胶条颜色通道评估”。
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import sys
from pathlib import Path

import cv2
import matplotlib
import numpy as np

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

plt.rcParams["font.sans-serif"] = ["Microsoft YaHei", "SimHei", "Noto Sans CJK SC", "DejaVu Sans"]
plt.rcParams["axes.unicode_minus"] = False

REPO = Path(__file__).resolve().parents[2]
IMAGE_SUFFIXES = {".png", ".jpg", ".jpeg", ".bmp", ".tif", ".tiff", ".raw"}
# 软件录制件里另存的原始 Bayer（检测设置“同时保存原始 Bayer”）；同目录的灰度 .pgm 没有颜色，不收
BAYER_PGM = "_bayer.pgm"

# GenICam 的 Bayer 命名比 OpenCV 错一格：BayerBG8 要用 COLOR_BayerRG2RGB。
BAYER_TO_CV = {
    "BG": cv2.COLOR_BayerRG2RGB,
    "RG": cv2.COLOR_BayerBG2RGB,
    "GB": cv2.COLOR_BayerGR2RGB,
    "GR": cv2.COLOR_BayerGB2RGB,
}

# 判定门槛：0.3 取自 D16 的对比度比（contrastRatio 默认值）；其余是经验值，只用于给出“可用 / 不可用”的提示。
MIN_DPRIME = 3.0
MIN_MATERIAL_RATIO = 0.3
MAX_DISTRACTOR_RATIO = 0.5

GAP_PX = 3.0
FLANK_PX = 15.0
STEP_PX = 0.5
BAND_HALF_PX = 2


class EvalError(Exception):
    pass


def current_gray(rgb: np.ndarray, raw: np.ndarray | None) -> np.ndarray:
    """现行取图的灰度：原始 Bayer 时与 bayer8_gray_into 逐像素一致，彩色图时等价于 (R + 2G + B) / 4。"""
    if raw is None:
        r, g, b = rgb[..., 0], rgb[..., 1], rgb[..., 2]
        return (r + 2 * g + b) / 4
    h, w = raw.shape
    ys = np.minimum(np.arange(h), h - 2)
    xs = np.minimum(np.arange(w), w - 2)
    a = raw.astype(np.uint16)
    s = a[ys][:, xs] + a[ys][:, xs + 1] + a[ys + 1][:, xs] + a[ys + 1][:, xs + 1]
    return ((s + 2) >> 2).astype(np.float32)


def lab_a(rgb: np.ndarray, raw: np.ndarray | None) -> np.ndarray:
    return cv2.cvtColor(rgb / 255.0, cv2.COLOR_RGB2Lab)[..., 1]


def saturation(rgb: np.ndarray, raw: np.ndarray | None) -> np.ndarray:
    return cv2.cvtColor(rgb / 255.0, cv2.COLOR_RGB2HSV)[..., 1] * 255.0


CHANNELS = {
    "灰度(现行)": current_gray,
    "G": lambda rgb, raw: rgb[..., 1].copy(),
    "R-G": lambda rgb, raw: rgb[..., 0] - rgb[..., 1],
    "a*": lab_a,
    "饱和度": saturation,
}


def load_image(path: Path, bayer: dict | None) -> tuple[np.ndarray, np.ndarray | None]:
    """返回 (RGB float32, 原始 Bayer 或 None)。"""
    is_raw, is_bayer_pgm = path.suffix.lower() == ".raw", path.name.lower().endswith(BAYER_PGM)
    if is_raw or is_bayer_pgm:
        if not bayer:
            raise EvalError(f"{path.name} 是原始 Bayer，标注文件需要 bayer: {{pattern}}（.raw 还要 width、height）")
        pattern = str(bayer.get("pattern", "")).upper()
        if pattern not in BAYER_TO_CV:
            raise EvalError(f"bayer.pattern 只能是 {'/'.join(BAYER_TO_CV)}，实际 {pattern!r}")
        if is_bayer_pgm:
            raw = cv2.imdecode(np.fromfile(path, dtype=np.uint8), cv2.IMREAD_UNCHANGED)
            if raw is None or raw.ndim != 2 or raw.dtype != np.uint8:
                raise EvalError(f"读不出 8 位单通道的原始 Bayer {path}")
        else:
            w, h = int(bayer["width"]), int(bayer["height"])
            data = np.fromfile(path, dtype=np.uint8)
            if data.size < w * h:
                raise EvalError(f"{path.name} 只有 {data.size} 字节，{w}×{h} 需要 {w * h}")
            raw = data[: w * h].reshape(h, w)
        return cv2.cvtColor(raw, BAYER_TO_CV[pattern]).astype(np.float32), raw
    img = cv2.imdecode(np.fromfile(path, dtype=np.uint8), cv2.IMREAD_COLOR)
    if img is None:
        raise EvalError(f"读不出图像 {path}")
    return cv2.cvtColor(img, cv2.COLOR_BGR2RGB).astype(np.float32), None


def sample_profile(channel: np.ndarray, p0, p1) -> tuple[np.ndarray, np.ndarray]:
    """沿探针等距取样，并在探针法向 ±BAND_HALF_PX 内平均，返回 (t, 值)，t 以探针中点为 0。"""
    p0, p1 = np.asarray(p0, np.float32), np.asarray(p1, np.float32)
    length = float(np.linalg.norm(p1 - p0))
    u = (p1 - p0) / length
    n = np.array([-u[1], u[0]], np.float32)
    t = np.arange(-length / 2, length / 2 + 1e-6, STEP_PX, dtype=np.float32)
    mid = (p0 + p1) / 2
    rows = []
    for k in range(-BAND_HALF_PX, BAND_HALF_PX + 1):
        pts = mid + np.outer(t, u) + k * n
        mx = pts[:, 0].reshape(1, -1).astype(np.float32)
        my = pts[:, 1].reshape(1, -1).astype(np.float32)
        rows.append(cv2.remap(channel, mx, my, cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE)[0])
    return t, np.mean(rows, axis=0)


def response(channel: np.ndarray, bright: bool, diameter: int) -> np.ndarray:
    """与 glue.response 同思路：胶比背景亮用白顶帽，暗用黑顶帽；结构元直径 = 检测胶宽上限 + 1。"""
    kernel = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (diameter, diameter))
    op = cv2.MORPH_TOPHAT if bright else cv2.MORPH_BLACKHAT
    return cv2.morphologyEx(channel, op, kernel)


def check_probe(probe: dict, shape, where: str) -> None:
    h, w = shape
    for key in ("p0", "p1"):
        p = probe.get(key)
        if not (isinstance(p, list) and len(p) == 2 and all(isinstance(v, (int, float)) for v in p)):
            raise EvalError(f"{where}：{key} 需要 [x, y]")
        if not (0 <= p[0] < w and 0 <= p[1] < h):
            raise EvalError(f"{where}：{key} {p} 超出图像 {w}×{h}")
    if probe["p0"] == probe["p1"]:
        raise EvalError(f"{where}：p0 与 p1 相同")
    kind = probe.get("kind")
    if kind not in ("glue", "distractor"):
        raise EvalError(f"{where}：kind 只能是 glue 或 distractor")
    if kind == "glue":
        if not probe.get("material"):
            raise EvalError(f"{where}：胶探针需要 material（如“镀锌”“热成型”）")
        hw = probe.get("halfWidthPx")
        if not (isinstance(hw, (int, float)) and hw > 0):
            raise EvalError(f"{where}：胶探针需要正的 halfWidthPx（胶宽的一半，px）")
        need = 2 * (hw + GAP_PX + FLANK_PX)
        length = float(np.hypot(probe["p1"][0] - probe["p0"][0], probe["p1"][1] - probe["p0"][1]))
        if length < need:
            raise EvalError(f"{where}：探针长 {length:.0f} px，至少要 {need:.0f} px 才能在两侧各取 {FLANK_PX:.0f} px 背景")


def glue_metrics(t: np.ndarray, v: np.ndarray, hw: float) -> dict:
    inner = v[np.abs(t) <= 0.6 * hw]
    left = v[(t <= -(hw + GAP_PX)) & (t >= -(hw + GAP_PX + FLANK_PX))]
    right = v[(t >= hw + GAP_PX) & (t <= hw + GAP_PX + FLANK_PX)]
    bg = np.concatenate([left, right])
    glue, level = float(np.median(inner)), float(np.median(bg))
    sigma = max(1.4826 * float(np.median(np.abs(bg - level))), 1.0)
    dl, dr = glue - float(np.median(left)), glue - float(np.median(right))
    return {
        "glue": glue,
        "background": level,
        "delta": glue - level,
        "sigma": sigma,
        "dprime": (glue - level) / sigma,
        "sidesAgree": bool(np.sign(dl) == np.sign(dr)),
    }


def run(spec_path: Path, out: Path) -> int:
    spec = json.loads(spec_path.read_text(encoding="utf-8-sig"))
    width_max = spec.get("widthMaxPx")
    if not (isinstance(width_max, (int, float)) and width_max > 0):
        raise EvalError("标注文件需要正的 widthMaxPx（检测胶宽上限，px）")
    diameter = int(np.ceil(width_max)) + 1
    base = spec_path.parent
    if spec.get("imageDir"):
        image_dir = Path(spec["imageDir"])
        base = image_dir if image_dir.is_absolute() else spec_path.parent / image_dir
    images = spec.get("images") or []
    if not any(p.get("kind") == "glue" for im in images for p in im.get("probes", [])):
        raise EvalError("标注文件里没有胶探针（kind: glue）")
    out.mkdir(parents=True, exist_ok=True)

    loaded = []
    for i, im in enumerate(images, 1):
        path = base / im["file"]
        rgb, raw = load_image(path, spec.get("bayer"))
        for j, probe in enumerate(im.get("probes", []), 1):
            check_probe(probe, rgb.shape[:2], f"第 {i} 张图 {im['file']} 第 {j} 个探针")
        loaded.append((im, rgb, {name: fn(rgb, raw).astype(np.float32) for name, fn in CHANNELS.items()}))

    rows = []
    for im, rgb, chans in loaded:
        for j, probe in enumerate(im.get("probes", []), 1):
            if probe["kind"] != "glue":
                continue
            for name, ch in chans.items():
                t, v = sample_profile(ch, probe["p0"], probe["p1"])
                m = glue_metrics(t, v, probe["halfWidthPx"])
                rows.append({"file": im["file"], "probe": j, "material": probe["material"], "channel": name, **m})

    bright = {}
    for name in CHANNELS:
        deltas = [r["delta"] for r in rows if r["channel"] == name]
        bright[name] = sum(d > 0 for d in deltas) >= len(deltas) / 2

    resp_rows = []
    for im, rgb, chans in loaded:
        responses = {name: response(ch, bright[name], diameter) for name, ch in chans.items()}
        for j, probe in enumerate(im.get("probes", []), 1):
            for name, r in responses.items():
                t, v = sample_profile(r, probe["p0"], probe["p1"])
                if probe["kind"] == "glue":
                    peak = float(np.max(v[np.abs(t) <= probe["halfWidthPx"]]))
                else:
                    peak = float(np.max(v))
                resp_rows.append({"file": im["file"], "probe": j, "kind": probe["kind"],
                                  "label": probe.get("material") or probe.get("label") or "干扰物",
                                  "channel": name, "peak": peak})
        plot_image(im, chans, responses, bright, out)

    summary = summarize(rows, resp_rows, bright)
    write_outputs(out, rows, resp_rows, summary, diameter)
    print_summary(summary, diameter, out)
    return 0


def summarize(rows: list, resp_rows: list, bright: dict) -> list:
    out = []
    for name in CHANNELS:
        mine = [r for r in rows if r["channel"] == name]
        materials = sorted({r["material"] for r in mine})
        signs = {np.sign(r["delta"]) for r in mine if abs(r["dprime"]) >= 1}
        per_mat = {}
        for mat in materials:
            m = [r for r in mine if r["material"] == mat]
            glue_resp = [r["peak"] for r in resp_rows if r["channel"] == name and r["kind"] == "glue" and r["label"] == mat]
            per_mat[mat] = {
                "n": len(m),
                "deltaMedian": float(np.median([r["delta"] for r in m])),
                "dprimeMedian": float(np.median([r["dprime"] for r in m])),
                "dprimeMin": float(min(r["dprime"] for r in m)),
                "responseMedian": float(np.median(glue_resp)),
                "sidesDisagree": sum(not r["sidesAgree"] for r in m),
            }
        resp = [v["responseMedian"] for v in per_mat.values()]
        ratio = min(resp) / max(resp) if max(resp) > 0 else 0.0
        glue_all = float(np.median([r["peak"] for r in resp_rows if r["channel"] == name and r["kind"] == "glue"]))
        distractors = [r for r in resp_rows if r["channel"] == name and r["kind"] == "distractor"]
        worst = max(distractors, key=lambda r: r["peak"], default=None)
        worst_ratio = worst["peak"] / glue_all if worst and glue_all > 0 else None
        split = sum(not r["sidesAgree"] for r in mine)
        consistent = len(signs) == 1 and not split
        dmin = min(abs(r["dprime"]) for r in mine)
        problems = []
        if len(signs) > 1:
            problems.append("胶的明暗方向随探针变化，单一极性测不全")
        if split:
            problems.append(f"{split} 个探针上胶比一侧背景亮、比另一侧暗，单一极性测不全")
        if dmin < MIN_DPRIME:
            problems.append(f"最弱处对比度只有背景噪声的 {dmin:.1f} 倍（< {MIN_DPRIME:g}）")
        if len(per_mat) > 1 and ratio < MIN_MATERIAL_RATIO:
            problems.append(f"材质间响应比 {ratio:.2f} < {MIN_MATERIAL_RATIO}：一帧跨两种材质时弱的一侧会被 D16 判无胶")
        if worst_ratio is not None and worst_ratio > MAX_DISTRACTOR_RATIO:
            problems.append(f"干扰物“{worst['label']}”响应达胶的 {worst_ratio:.2f} 倍")
        out.append({
            "channel": name,
            "polarity": "亮" if bright[name] else "暗",
            "consistent": consistent,
            "dprimeMinAbs": dmin,
            "materialRatio": ratio,
            "materials": per_mat,
            "worstDistractor": worst["label"] if worst else None,
            "worstDistractorRatio": worst_ratio,
            "usable": not problems,
            "problems": problems,
        })
    return out


def plot_image(im: dict, chans: dict, responses: dict, bright: dict, out: Path) -> None:
    names = list(chans)
    fig, axes = plt.subplots(len(names), 2, figsize=(11, 3.2 * len(names)), squeeze=False)
    for row, name in enumerate(names):
        for col, (img, title) in enumerate([(chans[name], f"{name}"),
                                            (responses[name], f"{name} 响应（{'白' if bright[name] else '黑'}顶帽）")]):
            ax = axes[row][col]
            lo, hi = np.percentile(img, [1, 99])
            ax.imshow(img, cmap="gray", vmin=lo, vmax=hi if hi > lo else lo + 1)
            ax.set_title(title, fontsize=9)
            ax.axis("off")
            for j, p in enumerate(im.get("probes", []), 1):
                color = "#f0a43c" if p["kind"] == "glue" else "#4fc3f7"
                ax.plot([p["p0"][0], p["p1"][0]], [p["p0"][1], p["p1"][1]], color=color, linewidth=1.2)
                ax.text(p["p1"][0], p["p1"][1], str(j), color=color, fontsize=7)
    fig.tight_layout()
    fig.savefig(out / f"{Path(im['file']).stem}_channels.png", dpi=110)
    plt.close(fig)


def write_outputs(out: Path, rows: list, resp_rows: list, summary: list, diameter: int) -> None:
    (out / "report.json").write_text(json.dumps({"structuringElementPx": diameter, "summary": summary,
                                                 "glueProbes": rows, "responses": resp_rows},
                                                ensure_ascii=False, indent=2), encoding="utf-8")
    with (out / "probes.csv").open("w", newline="", encoding="utf-8-sig") as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0]))
        w.writeheader()
        w.writerows(rows)


def print_summary(summary: list, diameter: int, out: Path) -> None:
    print(f"结构元直径 {diameter} px；d' = |胶 − 背景| / 背景噪声（σ 下限 1）")
    print("| 通道 | 胶极性 | 极性一致 | 最弱 d' | 材质间响应比 | 最强干扰 / 胶 | 结论 |")
    print("|---|---|---|---|---|---|---|")
    for s in summary:
        dist = f"{s['worstDistractorRatio']:.2f}（{s['worstDistractor']}）" if s["worstDistractorRatio"] is not None else "—"
        print(f"| {s['channel']} | {s['polarity']} | {'是' if s['consistent'] else '否'} | {s['dprimeMinAbs']:.1f} | "
              f"{s['materialRatio']:.2f} | {dist} | {'可用' if s['usable'] else '不可用'} |")
    print()
    for s in summary:
        mats = "；".join(f"{m}: Δ中位 {v['deltaMedian']:+.1f}，d' 中位 {v['dprimeMedian']:+.1f}（{v['n']} 个探针"
                        + (f"，{v['sidesDisagree']} 个两侧反号" if v["sidesDisagree"] else "") + "）"
                        for m, v in s["materials"].items())
        print(f"{s['channel']}：{mats}")
        for p in s["problems"]:
            print(f"  - {p}")
    print(f"\n明细：{out / 'report.json'}、{out / 'probes.csv'}；各图通道与响应：{out}\\*_channels.png")


def image_dir_for(spec_path: Path, image_dir: Path) -> str:
    """标注文件里的 imageDir 按标注文件所在目录解析（与 run 一致）；跨盘符时写绝对路径。"""
    target = image_dir.resolve()
    try:
        return Path(os.path.relpath(target, spec_path.resolve().parent)).as_posix()
    except ValueError:
        return str(target)


def recorded_pattern(image_dir: Path) -> str | None:
    """软件录制件的 part.json 里各帧原始 Bayer 的相位；不是录制目录或没存 Bayer 时为 None。"""
    part = image_dir / "part.json"
    if not part.is_file():
        return None
    frames = json.loads(part.read_text(encoding="utf-8-sig")).get("frames") or []
    patterns = {f["bayer"]["pattern"] for f in frames if f.get("bayer")}
    if len(patterns) > 1:
        raise EvalError(f"{part} 里各帧 Bayer 相位不一致：{sorted(patterns)}，请按相机分开标注")
    return next(iter(patterns), None)


def init_spec(image_dir: Path, spec_path: Path) -> int:
    files = sorted(p.name for p in image_dir.iterdir()
                   if p.suffix.lower() in IMAGE_SUFFIXES or p.name.lower().endswith(BAYER_PGM))
    if not files:
        raise EvalError(f"{image_dir} 里没有图像（{', '.join(sorted(IMAGE_SUFFIXES))} 或录制件的 *{BAYER_PGM}）")
    if spec_path.exists():
        raise EvalError(f"{spec_path} 已存在，不覆盖")
    pattern = recorded_pattern(image_dir)
    if any(f.lower().endswith(BAYER_PGM) for f in files):
        bayer = {"pattern": pattern or "BG"}
    elif any(f.lower().endswith(".raw") for f in files):
        bayer = {"pattern": "BG", "width": 1280, "height": 1024}
    else:
        bayer = None
    spec = {
        "_说明": "每张图标若干探针：胶探针垂直横穿胶条、中点落在胶中心，两侧各留 ≥ 18 px 背景；"
                 "干扰物探针横穿焊缝、高光等不该被当成胶的窄结构。坐标为原图像素 [x, y]。",
        "_示例": [
            {"kind": "glue", "material": "镀锌", "p0": [600, 300], "p1": [660, 300], "halfWidthPx": 12},
            {"kind": "distractor", "label": "焊缝", "p0": [500, 520], "p1": [560, 520]},
        ],
        "imageDir": image_dir_for(spec_path, image_dir),
        "widthMaxPx": 40,
        "bayer": bayer,
        "images": [{"file": f, "probes": []} for f in files],
    }
    spec_path.write_text(json.dumps(spec, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"已写标注模板 {spec_path}（{len(files)} 张图），填好探针后运行：python {Path(__file__).relative_to(REPO)} {spec_path}")
    return 0


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        stream.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description="比较有色胶条在不同单通道转换下的对比度")
    ap.add_argument("spec", type=Path, help="标注文件（JSON）")
    ap.add_argument("--init", type=Path, metavar="图像目录", help="按图像目录生成标注模板写到 spec，不运行评估")
    ap.add_argument("--out", type=Path, default=REPO / "output" / "color-eval", help="输出目录（默认 output/color-eval）")
    args = ap.parse_args(argv)
    try:
        if args.init:
            return init_spec(args.init, args.spec)
        return run(args.spec, args.out)
    except EvalError as e:
        print(f"错误：{e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
