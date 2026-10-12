from __future__ import annotations

import argparse
import json
import math
import re
import shutil
import sys
from collections import Counter
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path

SCHEMA_VERSION = 1
MANIFEST = "sample-set.json"
IMAGE_EXTS = ("jpg", "jpeg", "png", "bmp", "tif", "tiff", "pgm")
LABELS = ("ok", "ng", "unknown")
TZ = timezone(timedelta(hours=8))

ISV_PART_RE = re.compile(r"^(\d{14})_(.+)$")
ISV_FRAME_RE = re.compile(r"^Frame(\d)(\d{6})\.(" + "|".join(IMAGE_EXTS) + r")$", re.IGNORECASE)
ISV_KNOWN_EXTRA = {"overview.jpg", "teachinfo.txt"}
MX11_FILE_RE = re.compile(
    r"^(?P<sn>[^_]+)_(?P<station>[^_]+)_(?P<date>\d{8})_(?P<time>\d{6})_(?P<model>[^_]+)"
    r"_(?P<cam>\d+)_(?P<point>\d+)_(?P<label>[A-Za-z]+)\.(?P<ext>" + "|".join(IMAGE_EXTS) + r")$",
    re.IGNORECASE,
)
MX11_RET_RE = re.compile(r"_ret\.[A-Za-z]+$", re.IGNORECASE)
SHOT_ID_RE = re.compile(r"^S\d{3}$")
IMAGE_NAME_RE = re.compile(r"^(S[0-9]{3})_(.+)_v([0-9]+)\.([A-Za-z]+)$")
ID_RE = re.compile(r"[A-Za-z0-9_-]{1,32}")


class ConvertError(Exception):
    pass


@dataclass
class SourceShot:
    key: tuple
    origin: str
    camera_id: str


@dataclass
class SourcePart:
    sn: str
    origin: str
    captured_at: datetime
    files: dict = field(default_factory=dict)
    labels: dict = field(default_factory=dict)


@dataclass
class SourceSet:
    kind: str
    shots: list
    parts: list
    source: str
    ignored: list = field(default_factory=list)


def _jpeg_size(f) -> tuple[int, int] | None:
    if f.read(2) != b"\xff\xd8":
        return None
    while True:
        byte = f.read(1)
        if not byte:
            return None
        if byte != b"\xff":
            continue
        marker = f.read(1)
        while marker == b"\xff":
            marker = f.read(1)
        if not marker:
            return None
        code = marker[0]
        if code in (0x01, 0xD8) or 0xD0 <= code <= 0xD7:
            continue
        if code == 0xD9:
            return None
        raw_len = f.read(2)
        if len(raw_len) < 2:
            return None
        length = int.from_bytes(raw_len, "big")
        if code in (0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF):
            data = f.read(5)
            if len(data) < 5:
                return None
            return int.from_bytes(data[3:5], "big"), int.from_bytes(data[1:3], "big")
        f.seek(length - 2, 1)


def _png_size(f) -> tuple[int, int] | None:
    head = f.read(24)
    if len(head) < 24 or head[:8] != b"\x89PNG\r\n\x1a\n" or head[12:16] != b"IHDR":
        return None
    return int.from_bytes(head[16:20], "big"), int.from_bytes(head[20:24], "big")


def _bmp_size(f) -> tuple[int, int] | None:
    head = f.read(26)
    if len(head) < 22 or head[:2] != b"BM":
        return None
    if int.from_bytes(head[14:18], "little") == 12:
        return int.from_bytes(head[18:20], "little"), int.from_bytes(head[20:22], "little")
    if len(head) < 26:
        return None
    width = int.from_bytes(head[18:22], "little", signed=True)
    height = int.from_bytes(head[22:26], "little", signed=True)
    return width, abs(height)


def _pgm_size(f) -> tuple[int, int] | None:
    data = f.read(1024)
    if data[:2] not in (b"P5", b"P2"):
        return None
    tokens: list[bytes] = []
    for line in data[2:].split(b"\n"):
        tokens.extend(line.split(b"#", 1)[0].split())
        if len(tokens) >= 2:
            break
    if len(tokens) < 2 or not all(t.isdigit() for t in tokens[:2]):
        return None
    return int(tokens[0]), int(tokens[1])


def _tiff_size(f) -> tuple[int, int] | None:
    head = f.read(8)
    if len(head) < 8 or head[:4] not in (b"II*\x00", b"MM\x00*"):
        return None
    order = "little" if head[:2] == b"II" else "big"
    f.seek(int.from_bytes(head[4:8], order))
    raw_count = f.read(2)
    if len(raw_count) < 2:
        return None
    found: dict[int, int] = {}
    for _ in range(int.from_bytes(raw_count, order)):
        entry = f.read(12)
        if len(entry) < 12:
            return None
        tag, kind = int.from_bytes(entry[0:2], order), int.from_bytes(entry[2:4], order)
        if tag in (256, 257) and kind in (3, 4):
            found[tag] = int.from_bytes(entry[8:10] if kind == 3 else entry[8:12], order)
    if 256 not in found or 257 not in found:
        return None
    return found[256], found[257]


SIZE_READERS = {
    "jpg": _jpeg_size, "jpeg": _jpeg_size, "png": _png_size, "bmp": _bmp_size,
    "pgm": _pgm_size, "tif": _tiff_size, "tiff": _tiff_size,
}


def read_image_size(path: Path) -> tuple[int, int]:
    reader = SIZE_READERS.get(path.suffix.lower().lstrip("."))
    if reader is None:
        raise ConvertError(f"不支持的图像格式：{path}")
    try:
        with open(path, "rb") as f:
            size = reader(f)
    except OSError as e:
        raise ConvertError(f"读取图像失败：{path}：{e}") from None
    if size is None or size[0] <= 0 or size[1] <= 0:
        raise ConvertError(f"无法解析图像头部尺寸：{path}")
    return size


def parse_shots(spec: str, total: int) -> list[int]:
    selected: set[int] = set()
    for chunk in spec.split(","):
        chunk = chunk.strip()
        m = re.fullmatch(r"(\d+)(?:-(\d+))?", chunk)
        if not m:
            raise ConvertError(f"无法解析拍照点范围：{chunk!r}")
        start = int(m.group(1))
        end = int(m.group(2) or start)
        if start < 1 or end < start or end > total:
            raise ConvertError(f"拍照点范围 {chunk} 超出 1..{total}")
        selected.update(range(start, end + 1))
    return sorted(selected)


def parse_mm_per_px(text: str, what: str) -> float:
    try:
        value = float(text)
    except ValueError:
        raise ConvertError(f"{what} 不是数字：{text!r}") from None
    if not _is_mm_per_px(value):
        raise ConvertError(f"{what} 必须为正的有限数：{text!r}")
    return value


def parse_shot_mm_per_px(spec: str) -> dict[str, float]:
    result: dict[str, float] = {}
    for chunk in spec.split(","):
        sid, sep, value = chunk.strip().partition("=")
        sid = sid.strip()
        if not sep or not SHOT_ID_RE.fullmatch(sid):
            raise ConvertError(f"无法解析逐点像素当量：{chunk!r}（格式 S001=0.085）")
        if sid in result:
            raise ConvertError(f"逐点像素当量重复：{sid}")
        result[sid] = parse_mm_per_px(value.strip(), f"拍照点 {sid} 的像素当量")
    return result


def apply_mm_per_px(manifest: dict, mm_per_px: str | None, shot_mm_per_px: str | None) -> None:
    if mm_per_px is not None:
        manifest["mmPerPx"] = parse_mm_per_px(mm_per_px, "--mm-per-px")
    if shot_mm_per_px is None:
        return
    shots = {s["shotId"]: s for s in manifest["shots"]}
    for sid, value in parse_shot_mm_per_px(shot_mm_per_px).items():
        if sid not in shots:
            raise ConvertError(f"--shot-mm-per-px 中的拍照点 {sid} 不在生成的清单里（共 {len(shots)} 个拍照点）")
        shots[sid]["mmPerPx"] = value


def scan_isv(src: Path) -> SourceSet:
    parts: list[SourcePart] = []
    ignored: list[str] = []
    shots: dict[tuple, SourceShot] = {}
    for entry in sorted(src.iterdir()):
        if not entry.is_dir():
            continue
        m = ISV_PART_RE.match(entry.name)
        if not m:
            ignored.append(entry.name + "/")
            continue
        captured = datetime.strptime(m.group(1), "%Y%m%d%H%M%S").replace(tzinfo=TZ)
        part = SourcePart(sn=m.group(2), origin=entry.name, captured_at=captured)
        for f in entry.iterdir():
            fm = ISV_FRAME_RE.match(f.name) if f.is_file() else None
            if not fm:
                if f.name.lower() not in ISV_KNOWN_EXTRA:
                    ignored.append(f"{entry.name}/{f.name}")
                continue
            key = (int(fm.group(1)), int(fm.group(2)))
            shots.setdefault(key, SourceShot(key=key, origin=f.name, camera_id="cam1"))
            part.files[key] = f
        parts.append(part)
    parts.sort(key=lambda p: (p.captured_at, p.origin))
    return SourceSet("isv", [shots[k] for k in sorted(shots)], parts, src.name, ignored)


def scan_mx11(src: Path) -> SourceSet:
    parts: list[SourcePart] = []
    ignored: list[str] = []
    shots: dict[tuple, SourceShot] = {}
    for entry in sorted(src.iterdir()):
        if not entry.is_dir():
            continue
        part: SourcePart | None = None
        for f in sorted(entry.iterdir()):
            if not f.is_file() or MX11_RET_RE.search(f.name):
                continue
            fm = MX11_FILE_RE.match(f.name)
            if not fm:
                ignored.append(f"{entry.name}/{f.name}")
                continue
            captured = datetime.strptime(fm["date"] + fm["time"], "%Y%m%d%H%M%S").replace(tzinfo=TZ)
            if part is None:
                part = SourcePart(sn=entry.name, origin=entry.name, captured_at=captured)
            part.captured_at = min(part.captured_at, captured)
            key = (int(fm["point"]), int(fm["cam"]))
            if key in part.files:
                raise ConvertError(f"{entry.name} 中点号 {key[0]} 相机 {key[1]} 重复：{f.name}")
            shots.setdefault(
                key,
                SourceShot(key=key, origin=f"{fm['model']}_{fm['cam']}_{fm['point']}", camera_id=f"cam{int(fm['cam'])}"),
            )
            part.files[key] = f
            part.labels[key] = fm["label"].lower()
        if part is None:
            ignored.append(entry.name + "/")
            continue
        parts.append(part)
    parts.sort(key=lambda p: (p.captured_at, p.origin))
    return SourceSet("mx11", [shots[k] for k in sorted(shots)], parts, src.name, ignored)


def part_label(labels: list[str]) -> str:
    if not labels:
        return "unknown"
    return "ok" if all(l == "ok" for l in labels) else "ng"


def build_manifest(source: SourceSet, set_id: str, name: str, selection: list[int] | None) -> tuple[dict, list]:
    indexes = selection or list(range(1, len(source.shots) + 1))
    chosen = [source.shots[i - 1] for i in indexes]
    shot_ids = {s.key: f"S{n:03d}" for n, s in enumerate(chosen, start=1)}
    cameras = sorted({s.camera_id for s in chosen}, key=lambda c: (len(c), c))
    shots_json = [
        {"shotId": shot_ids[s.key], "cameraId": s.camera_id, "views": [1], "origin": s.origin} for s in chosen
    ]
    parts_json = []
    copies = []
    for n, part in enumerate(source.parts, start=1):
        part_id = f"p{n:02d}"
        entry: dict = {"partId": part_id, "sn": part.sn}
        if source.kind == "mx11":
            shot_labels = {shot_ids[s.key]: part.labels[s.key] for s in chosen if s.key in part.labels}
            entry["label"] = part_label(list(shot_labels.values()))
            entry["shotLabels"] = shot_labels
        else:
            entry["label"] = "unknown"
        entry["capturedAt"] = part.captured_at.isoformat()
        entry["origin"] = part.origin
        parts_json.append(entry)
        for s in chosen:
            src_file = part.files.get(s.key)
            ext = src_file.suffix.lower().lstrip(".") if src_file else None
            target = f"parts/{part_id}/{shot_ids[s.key]}_{s.camera_id}_v1.{ext}" if ext else None
            copies.append((part_id, shot_ids[s.key], s.origin, src_file, target))
    first = next((c[3] for c in copies if c[3] is not None), None)
    if first is None:
        raise ConvertError("源目录中没有可用的图像")
    width, height = read_image_size(first)
    manifest = {
        "schemaVersion": SCHEMA_VERSION,
        "id": set_id,
        "name": name,
        "source": source.source,
        "imageSize": [width, height],
        "mmPerPx": None,
        "cameras": [{"id": c, "views": [1]} for c in cameras],
        "shots": shots_json,
        "parts": parts_json,
    }
    return manifest, copies


def default_identity(source: SourceSet, selection: list[int] | None, shots_spec: str | None) -> tuple[str, str]:
    day = min(p.captured_at for p in source.parts).strftime("%Y%m%d")
    prefix = {"isv": "ISV", "mx11": "MX11"}[source.kind]
    set_id = f"{source.kind}-{day}"
    name = f"{prefix} 现场原图（{len(source.parts)} 件）"
    if selection:
        set_id += "-shots-" + re.sub(r"[^0-9]+", "_", shots_spec).strip("_")
        name += f"，拍照点 {shots_spec}"
    return set_id, name


def ensure_replaceable_set(out: Path) -> None:
    allowed = {MANIFEST: Path.is_file, "parts": Path.is_dir}
    for entry in out.iterdir():
        check = allowed.get(entry.name)
        if check is None or entry.is_symlink() or not check(entry):
            raise ConvertError(f"输出目录顶层只能有 {MANIFEST} 与 parts/，发现 {entry.name}，拒绝覆盖：{out}")
    if not (out / MANIFEST).is_file():
        raise ConvertError(f"输出目录不是样本集（缺少 {MANIFEST}），拒绝覆盖：{out}")
    errors, _ = validate_set(out)
    if errors:
        raise ConvertError(f"输出目录不是有效样本集（格式错误 {len(errors)} 项，首项：{errors[0]}），拒绝覆盖：{out}")


def prepare_output(src: Path, out: Path, force: bool) -> None:
    src_r, out_r = src.resolve(), out.resolve()
    if out_r == src_r or src_r in out_r.parents or out_r in src_r.parents:
        raise ConvertError("输出目录不能与源目录相同或互相包含")
    if out.exists():
        if not out.is_dir():
            raise ConvertError(f"输出路径不是目录：{out}")
        if any(out.iterdir()):
            if not force:
                raise ConvertError(f"输出目录已存在且非空：{out}（使用 --force 覆盖）")
            ensure_replaceable_set(out)
            shutil.rmtree(out)
    out.mkdir(parents=True, exist_ok=True)


def convert(kind: str, src: Path, out: Path, shots_spec: str | None, set_id: str | None,
            name: str | None, force: bool, mm_per_px: str | None = None,
            shot_mm_per_px: str | None = None) -> dict:
    if not src.is_dir():
        raise ConvertError(f"源目录不存在：{src}")
    source = scan_isv(src) if kind == "isv" else scan_mx11(src)
    if not source.parts:
        raise ConvertError(f"源目录中没有可识别的件：{src}")
    selection = parse_shots(shots_spec, len(source.shots)) if shots_spec else None
    default_id, default_name = default_identity(source, selection, shots_spec)
    manifest, copies = build_manifest(source, set_id or default_id, name or default_name, selection)
    apply_mm_per_px(manifest, mm_per_px, shot_mm_per_px)

    per_part = Counter(c[0] for c in copies if c[3] is not None)
    missing = [f"{c[0]}/{c[1]}（{c[2]}）" for c in copies if c[3] is None]
    print(f"[自检] {kind} 件数 {len(manifest['parts'])}，拍照点 {len(manifest['shots'])}，"
          f"应有图像 {len(copies)}，可复制 {len(copies) - len(missing)}")
    for p in manifest["parts"]:
        print(f"  {p['partId']} {p['sn']:<14} {per_part[p['partId']]:>4} 张  label={p['label']}")
    if missing:
        print(f"[自检] 缺图 {len(missing)} 张：")
        for m in missing:
            print(f"  {m}")
    if source.ignored:
        print(f"[自检] 忽略无法识别的条目 {len(source.ignored)} 个：")
        for m in source.ignored[:20]:
            print(f"  {m}")

    prepare_output(src, out, force)
    total_bytes = 0
    for part_id, _, _, src_file, target in copies:
        if src_file is None:
            continue
        dest = out / target
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(src_file, dest)
        total_bytes += dest.stat().st_size
    (out / MANIFEST).write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    labels = Counter(p["label"] for p in manifest["parts"])
    print(f"[完成] {out}")
    shot_mm = {s["shotId"]: s["mmPerPx"] for s in manifest["shots"] if "mmPerPx" in s}
    print(f"  id={manifest['id']} name={manifest['name']} imageSize={manifest['imageSize']} "
          f"mmPerPx={manifest['mmPerPx']}" + (f" 逐点 {shot_mm}" if shot_mm else ""))
    print(f"  件数 {len(manifest['parts'])}，拍照点 {len(manifest['shots'])}，复制文件 {len(copies) - len(missing)}，"
          f"大小 {total_bytes / 1024 / 1024:.1f} MiB，标签 {dict(labels)}")
    return manifest


def _is_pos_int(v) -> bool:
    return isinstance(v, int) and not isinstance(v, bool) and v > 0


def _is_mm_per_px(v) -> bool:
    return isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) and v > 0


def validate_set(root: Path) -> tuple[list[str], list[str]]:
    errors: list[str] = []
    missing: list[str] = []
    path = root / MANIFEST
    if not path.is_file():
        return [f"缺少 {MANIFEST}"], missing
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, UnicodeDecodeError) as e:
        return [f"{MANIFEST} 解析失败：{e}"], missing
    if not isinstance(doc, dict):
        return [f"{MANIFEST} 顶层必须是对象"], missing
    if doc.get("schemaVersion") != SCHEMA_VERSION or isinstance(doc.get("schemaVersion"), bool):
        return [f"不支持的 schemaVersion：{doc.get('schemaVersion')!r}"], missing

    for key in ("id", "name"):
        if not isinstance(doc.get(key), str) or not doc[key].strip():
            errors.append(f"{key} 必须是非空字符串")
    if "source" in doc and not isinstance(doc["source"], str):
        errors.append("source 必须是字符串")
    size = doc.get("imageSize")
    image_size = size if isinstance(size, list) and len(size) == 2 and all(_is_pos_int(v) for v in size) else None
    if image_size is None:
        errors.append("imageSize 必须是 [宽, 高] 正整数")
    mm = doc.get("mmPerPx", None)
    if mm is not None and not _is_mm_per_px(mm):
        errors.append("mmPerPx 必须是正的有限数或 null")

    cameras: dict[str, set] = {}
    raw_cameras = doc.get("cameras")
    if not isinstance(raw_cameras, list) or not raw_cameras:
        errors.append("cameras 必须是非空数组")
        raw_cameras = []
    for i, cam in enumerate(raw_cameras):
        cid = cam.get("id") if isinstance(cam, dict) else None
        views = cam.get("views") if isinstance(cam, dict) else None
        if not isinstance(cid, str) or not ID_RE.fullmatch(cid):
            errors.append(f"cameras[{i}].id 非法：{cid!r}")
            continue
        if cid in cameras:
            errors.append(f"cameras 中 id 重复：{cid}")
        if not (isinstance(views, list) and views and all(_is_pos_int(v) and v <= 3 for v in views)
                and len(set(views)) == len(views)):
            errors.append(f"cameras[{i}].views 必须是 1..3 且不重复")
            views = []
        cameras[cid] = set(views)

    shots: list[tuple[str, str, list]] = []
    raw_shots = doc.get("shots")
    if not isinstance(raw_shots, list) or not raw_shots:
        errors.append("shots 必须是非空数组")
        raw_shots = []
    for i, shot in enumerate(raw_shots, start=1):
        if not isinstance(shot, dict):
            errors.append(f"shots[{i - 1}] 必须是对象")
            continue
        sid, cid, views = shot.get("shotId"), shot.get("cameraId"), shot.get("views")
        if sid != f"S{i:03d}":
            errors.append(f"shots[{i - 1}].shotId 应为 S{i:03d}，实际 {sid!r}")
        if shot.get("mmPerPx") is not None and not _is_mm_per_px(shot["mmPerPx"]):
            errors.append(f"shots[{i - 1}]（{sid}）.mmPerPx 必须是正的有限数或 null")
        if cid not in cameras:
            errors.append(f"shots[{i - 1}].cameraId 未在 cameras 中声明：{cid!r}")
            continue
        if not (isinstance(views, list) and views and all(_is_pos_int(v) for v in views)
                and len(set(views)) == len(views) and set(views) == cameras[cid]):
            errors.append(f"shots[{i - 1}].views 必须等于相机 {cid} 声明的视角 {sorted(cameras[cid])}")
            continue
        shots.append((sid, cid, views))
    shot_ids = {s[0] for s in shots}

    raw_parts = doc.get("parts")
    if not isinstance(raw_parts, list) or not raw_parts:
        errors.append("parts 必须是非空数组")
        raw_parts = []
    part_ids: set[str] = set()
    for i, part in enumerate(raw_parts):
        if not isinstance(part, dict):
            errors.append(f"parts[{i}] 必须是对象")
            continue
        pid = part.get("partId")
        if not isinstance(pid, str) or not ID_RE.fullmatch(pid):
            errors.append(f"parts[{i}].partId 非法：{pid!r}")
            continue
        if pid.lower() in part_ids:
            errors.append(f"partId 重复（不区分大小写）：{pid}")
        part_ids.add(pid.lower())
        if not isinstance(part.get("sn"), str) or not part["sn"].strip():
            errors.append(f"{pid}.sn 必须是非空字符串")
        if part.get("label") not in LABELS:
            errors.append(f"{pid}.label 必须是 ok/ng/unknown：{part.get('label')!r}")
        shot_labels = part.get("shotLabels", {})
        if not isinstance(shot_labels, dict) or any(
                k not in shot_ids or not isinstance(v, str) or not v for k, v in shot_labels.items()):
            errors.append(f"{pid}.shotLabels 的键必须是已声明的 shotId，值为非空字符串")
        if "capturedAt" in part:
            try:
                datetime.fromisoformat(part["capturedAt"])
            except (TypeError, ValueError):
                errors.append(f"{pid}.capturedAt 不是 ISO 8601 时间：{part['capturedAt']!r}")

        part_dir = root / "parts" / pid
        present: dict[tuple, list[str]] = {}
        if part_dir.is_dir():
            for f in part_dir.iterdir():
                m = IMAGE_NAME_RE.match(f.name)
                if not f.is_file() or not m or m.group(4).lower() not in IMAGE_EXTS:
                    errors.append(f"parts/{pid}/{f.name} 不符合命名规则")
                    continue
                present.setdefault((m.group(1), m.group(2), int(m.group(3))), []).append(f.name)
        expected = {(sid, cid, v) for sid, cid, views in shots for v in views}
        for key, names in present.items():
            if key not in expected:
                errors.append(f"parts/{pid}/{names[0]} 未在 shots 中声明")
            elif len(names) > 1:
                errors.append(f"parts/{pid} 中 {key[0]}_{key[1]}_v{key[2]} 存在多个文件：{sorted(names)}")
            elif image_size is not None:
                try:
                    actual = read_image_size(part_dir / names[0])
                except ConvertError as e:
                    errors.append(f"parts/{pid}/{names[0]} {e}")
                    continue
                if list(actual) != image_size:
                    errors.append(f"parts/{pid}/{names[0]} 尺寸 {actual[0]}×{actual[1]} "
                                  f"与 imageSize {image_size[0]}×{image_size[1]} 不一致")
        for sid, cid, views in shots:
            for v in views:
                if (sid, cid, v) not in present:
                    missing.append(f"parts/{pid}/{sid}_{cid}_v{v}")

    parts_root = root / "parts"
    if parts_root.is_dir():
        for d in parts_root.iterdir():
            if d.name.lower() not in part_ids:
                errors.append(f"parts/{d.name} 未在 parts 中声明")
    return errors, missing


def run_validate(root: Path, strict: bool) -> int:
    errors, missing = validate_set(root)
    for e in errors:
        print(f"[错误] {e}")
    if missing:
        print(f"[缺图] {len(missing)} 张：")
        for m in missing:
            print(f"  {m}")
    if errors:
        print(f"[失败] {root}：格式错误 {len(errors)} 项")
        return 1
    if missing and strict:
        print(f"[失败] {root}：缺图 {len(missing)} 张（--strict）")
        return 2
    print(f"[通过] {root}" + (f"（缺图 {len(missing)} 张）" if missing else ""))
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="把现场原图转换为离线样本集 v1，并校验样本集格式")
    sub = parser.add_subparsers(dest="command", required=True)
    for kind, help_text in (("isv", "转换 ISV 现场原图"), ("mx11", "转换 MX11 现场原图")):
        p = sub.add_parser(kind, help=help_text)
        p.add_argument("source", type=Path)
        p.add_argument("output", type=Path)
        p.add_argument("--shots", help="按排序后的拍照点序号选子集，如 1-8 或 1,3,5-7")
        p.add_argument("--id", dest="set_id")
        p.add_argument("--name")
        p.add_argument("--force", action="store_true", help="允许覆盖已存在的样本集输出目录")
        p.add_argument("--mm-per-px", dest="mm_per_px", help="全局像素当量（mm/px），写入 mmPerPx")
        p.add_argument("--shot-mm-per-px", dest="shot_mm_per_px",
                       help="逐拍照点像素当量，覆盖全局，如 S001=0.085,S002=0.09（shotId 为生成后的编号）")
    v = sub.add_parser("validate", help="按契约校验样本集目录")
    v.add_argument("root", type=Path)
    v.add_argument("--strict", action="store_true", help="缺图也视为失败（退出码 2）")
    args = parser.parse_args(argv)

    try:
        if args.command == "validate":
            return run_validate(args.root, args.strict)
        convert(args.command, args.source, args.output, args.shots, args.set_id, args.name, args.force,
                args.mm_per_px, args.shot_mm_per_px)
        return run_validate(args.output, False)
    except ConvertError as e:
        print(f"[错误] {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
