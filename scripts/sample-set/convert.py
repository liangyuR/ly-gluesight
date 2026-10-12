from __future__ import annotations

import argparse
import json
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
IMAGE_NAME_RE = re.compile(r"^(S\d{3})_([^_]+)_v(\d+)\.([A-Za-z]+)$")


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


def read_image_size(path: Path) -> tuple[int, int]:
    with open(path, "rb") as f:
        if f.read(2) != b"\xff\xd8":
            raise ConvertError(f"不是 JPEG 文件：{path}")
        while True:
            byte = f.read(1)
            if not byte:
                break
            if byte != b"\xff":
                continue
            marker = f.read(1)
            while marker == b"\xff":
                marker = f.read(1)
            if not marker:
                break
            code = marker[0]
            if code in (0x01, 0xD8) or 0xD0 <= code <= 0xD7:
                continue
            if code == 0xD9:
                break
            raw_len = f.read(2)
            if len(raw_len) < 2:
                break
            length = int.from_bytes(raw_len, "big")
            if code in (0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF):
                data = f.read(5)
                if len(data) < 5:
                    break
                height = int.from_bytes(data[1:3], "big")
                width = int.from_bytes(data[3:5], "big")
                return width, height
            f.seek(length - 2, 1)
    raise ConvertError(f"未找到 JPEG SOF 头：{path}")


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
            if not (out / MANIFEST).is_file():
                raise ConvertError(f"输出目录不是样本集（缺少 {MANIFEST}），拒绝覆盖：{out}")
            shutil.rmtree(out)
    out.mkdir(parents=True, exist_ok=True)


def convert(kind: str, src: Path, out: Path, shots_spec: str | None, set_id: str | None,
            name: str | None, force: bool) -> dict:
    if not src.is_dir():
        raise ConvertError(f"源目录不存在：{src}")
    source = scan_isv(src) if kind == "isv" else scan_mx11(src)
    if not source.parts:
        raise ConvertError(f"源目录中没有可识别的件：{src}")
    selection = parse_shots(shots_spec, len(source.shots)) if shots_spec else None
    default_id, default_name = default_identity(source, selection, shots_spec)
    manifest, copies = build_manifest(source, set_id or default_id, name or default_name, selection)

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
    print(f"  id={manifest['id']} name={manifest['name']} imageSize={manifest['imageSize']}")
    print(f"  件数 {len(manifest['parts'])}，拍照点 {len(manifest['shots'])}，复制文件 {len(copies) - len(missing)}，"
          f"大小 {total_bytes / 1024 / 1024:.1f} MiB，标签 {dict(labels)}")
    return manifest


def _is_pos_int(v) -> bool:
    return isinstance(v, int) and not isinstance(v, bool) and v > 0


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
    if not (isinstance(size, list) and len(size) == 2 and all(_is_pos_int(v) for v in size)):
        errors.append("imageSize 必须是 [宽, 高] 正整数")
    mm = doc.get("mmPerPx", None)
    if mm is not None and (isinstance(mm, bool) or not isinstance(mm, (int, float)) or mm <= 0):
        errors.append("mmPerPx 必须是正数或 null")

    cameras: dict[str, set] = {}
    raw_cameras = doc.get("cameras")
    if not isinstance(raw_cameras, list) or not raw_cameras:
        errors.append("cameras 必须是非空数组")
        raw_cameras = []
    for i, cam in enumerate(raw_cameras):
        cid = cam.get("id") if isinstance(cam, dict) else None
        views = cam.get("views") if isinstance(cam, dict) else None
        if not isinstance(cid, str) or not re.fullmatch(r"[A-Za-z0-9-]+", cid):
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
        if cid not in cameras:
            errors.append(f"shots[{i - 1}].cameraId 未在 cameras 中声明：{cid!r}")
            continue
        if not (isinstance(views, list) and views and all(v in cameras[cid] for v in views)
                and len(set(views)) == len(views)):
            errors.append(f"shots[{i - 1}].views 必须是相机 {cid} 已声明视角的非空子集")
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
        if not isinstance(pid, str) or not re.fullmatch(r"[A-Za-z0-9_-]+", pid):
            errors.append(f"parts[{i}].partId 非法：{pid!r}")
            continue
        if pid in part_ids:
            errors.append(f"partId 重复：{pid}")
        part_ids.add(pid)
        if not isinstance(part.get("sn"), str):
            errors.append(f"{pid}.sn 必须是字符串")
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
        for sid, cid, views in shots:
            for v in views:
                if (sid, cid, v) not in present:
                    missing.append(f"parts/{pid}/{sid}_{cid}_v{v}")

    parts_root = root / "parts"
    if parts_root.is_dir():
        for d in parts_root.iterdir():
            if d.name not in part_ids:
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
    v = sub.add_parser("validate", help="按契约校验样本集目录")
    v.add_argument("root", type=Path)
    v.add_argument("--strict", action="store_true", help="缺图也视为失败（退出码 2）")
    args = parser.parse_args(argv)

    try:
        if args.command == "validate":
            return run_validate(args.root, args.strict)
        convert(args.command, args.source, args.output, args.shots, args.set_id, args.name, args.force)
        return run_validate(args.output, False)
    except ConvertError as e:
        print(f"[错误] {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
