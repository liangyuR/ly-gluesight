"""把现场导出的一件录制（`*_cycle_<cycleId>` 目录或其 zip）导入开发项目的历史记录，供在历史件选择器里选示教件、标定件调试。

原图转成单件样本集后交给 `capture_fixture` 按样本集模式写仿历史录制件：记录归到 `--recipe` 配方，设备配置取项目当前
`config/cameras.json`（选择器要求与当前一致），SN 前缀 `FIXTURE-`，选择器标“手动配置”。命令与注意事项见
docs/testing/TESTING.md“导入现场录制件”。
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DEFAULT_DATA_ROOT = Path("C:/ProgramData/ly/GlueSight-dev")
VISION_ROOT = Path.home() / "gs_logs" / "vision"


class ImportFailure(Exception):
    pass


def read_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8-sig"))


def unpack(source: Path, work: Path) -> Path:
    if source.is_dir():
        return source
    if not zipfile.is_zipfile(source):
        raise ImportFailure(f"{source} 不是目录也不是 zip")
    raw = work / "raw"
    shutil.rmtree(raw, ignore_errors=True)
    with zipfile.ZipFile(source) as z:
        z.extractall(raw)
    found = [p.parent for p in raw.rglob("part.json")]
    if len(found) != 1:
        raise ImportFailure(f"zip 里应恰有一个 part.json，找到 {len(found)} 个")
    return found[0]


def build_sample_set(part_dir: Path, part: dict, out: Path) -> dict[str, int]:
    """每台相机按 seq 1..groupCount 排帧序列；缺的帧不放文件，按缺帧处理，不错位。"""
    cameras = part["cameras"]
    groups = int(part["groupCount"])
    shutil.rmtree(out, ignore_errors=True)
    images = out / "parts" / "p01"
    images.mkdir(parents=True)
    shots, counts, n = [], {}, 0
    for camera in cameras:
        counts[camera] = 0
        for seq in range(1, groups + 1):
            n += 1
            shot = f"S{n:03}"
            name = f"{camera}_s{seq:06}.pgm"
            shots.append({"shotId": shot, "cameraId": camera, "views": [1], "origin": name})
            if (part_dir / name).is_file():
                shutil.copyfile(part_dir / name, images / f"{shot}_{camera}_v1.pgm")
                counts[camera] += 1
    if n > 999:
        raise ImportFailure(f"帧数 {n} 超过样本集 S001–S999 的上限")
    sizes = {(f["width"], f["height"]) for f in part.get("frames", []) if f.get("width")}
    if len(sizes) != 1:
        raise ImportFailure(f"part.json 的帧尺寸不唯一：{sizes}")
    manifest = {
        "schemaVersion": 1,
        "id": "history-import",
        "name": part_dir.name,
        "source": part_dir.name,
        "imageSize": list(sizes.pop()),
        "mmPerPx": None,
        "cameras": [{"id": c, "views": [1]} for c in cameras],
        "shots": shots,
        "parts": [{"partId": "p01", "sn": part.get("sn") or part_dir.name, "label": "unknown", "origin": part_dir.name}],
    }
    (out / "sample-set.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    return counts


def recipe_doc(project: Path, recipe_id: str, part: dict, version: int | None) -> tuple[dict, str]:
    """优先用项目已发布的同编号配方（录制时生效的配方）；没有时用候选文档，版本、阶段取现场 part.json 的配方。"""
    published = project / "recipes" / f"{recipe_id}.json"
    workspace = project / "workspaces" / recipe_id / "workspace.json"
    field = part.get("recipe") or {}
    if published.is_file():
        doc, source = read_json(published), f"已发布配方 {published.name}"
    elif workspace.is_file():
        doc, source = read_json(workspace)["doc"], f"候选 {recipe_id} 的文档"
        doc["version"] = field.get("version") or 1
        doc["stage"] = field.get("stage") or "collect"
    else:
        raise ImportFailure(f"项目里没有配方或候选 {recipe_id}")
    if version is not None:
        doc["version"] = version
    return doc, f"{source}，记为 {recipe_id}-v{doc['version']}"


def records_root(project_id: str, given: str | None) -> Path:
    if given:
        return Path(given)
    found = sorted(VISION_ROOT.glob(f"*/*/{project_id}/records"))
    if len(found) != 1:
        listing = "、".join(str(p) for p in found) or "无"
        raise ImportFailure(f"找到 {len(found)} 个真机录制根（{listing}）：先用软件打开一次该项目，或用 --records 指定")
    return found[0]


def config_notes(part: dict, cameras: list[dict]) -> list[str]:
    current = {c["id"]: c for c in cameras}
    notes = []
    for device in part.get("devices") or []:
        mine = current.get(device["cameraId"])
        field = device.get("cameraConfig") or {}
        if mine is None:
            notes.append(f"项目没有相机 {device['cameraId']}")
            continue
        diff = [k for k in ("serial", "frameRate", "triggerSource", "triggerActivation") if field.get(k) != mine.get(k)]
        if diff:
            notes.append(f"{device['cameraId']} 的 " + "、".join(f"{k} 现场 {field.get(k)} / 项目 {mine.get(k)}" for k in diff))
    return notes


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="导入现场录制件到开发项目历史记录")
    ap.add_argument("source", type=Path, help="*_cycle_<cycleId> 录制目录或其 zip")
    ap.add_argument("--project", required=True, help="项目编号（projects/ 下的目录名）")
    ap.add_argument("--recipe", help="记录归属的配方编号，缺省取 part.json 的配方")
    ap.add_argument("--version", type=int, help="记录用的配方版本；同修订号已有不同快照被拒时换一个")
    ap.add_argument("--label", help="来源标签，缺省为录制目录名")
    ap.add_argument("--data-root", type=Path, default=DEFAULT_DATA_ROOT)
    ap.add_argument("--records", help="真机录制根，缺省在 ~/gs_logs/vision 下按项目编号查找")
    ap.add_argument("--work", type=Path, help="中间文件目录，缺省 output/history-import/<录制目录名>")
    ap.add_argument("--dry-run", action="store_true", help="只准备输入并打印命令")
    args = ap.parse_args(argv)
    try:
        project = args.data_root / "projects" / args.project
        db = project / "inspection.db"
        if not db.is_file():
            raise ImportFailure(f"{db} 不存在：先用软件打开一次该项目")
        work = args.work or REPO / "output" / "history-import" / args.source.name.removesuffix(".zip")
        work.mkdir(parents=True, exist_ok=True)
        part_dir = unpack(args.source.resolve(), work)
        part = read_json(part_dir / "part.json")
        recipe_id = args.recipe or (part.get("recipe") or {}).get("id")
        if not recipe_id:
            raise ImportFailure("part.json 没有配方编号：用 --recipe 指定")
        doc, doc_source = recipe_doc(project, recipe_id, part, args.version)
        cameras = read_json(project / "config" / "cameras.json")["cameras"]
        counts = build_sample_set(part_dir, part, work / "sample-set")
        (work / "doc.json").write_text(json.dumps(doc, ensure_ascii=False, indent=2), encoding="utf-8")
        (work / "cameras.json").write_text(json.dumps(cameras, ensure_ascii=False, indent=2), encoding="utf-8")
        field_code = (part.get("plc") or {}).get("productCode")
        print(f"录制 {part_dir.name}：{part.get('groupCount')} 个帧组，" + "，".join(f"{c} {n} 帧" for c, n in counts.items()))
        print(f"配方：{doc_source}，型号 {doc.get('productCode')}" + (f"（现场型号 {field_code}）" if field_code != doc.get("productCode") else ""))
        for note in config_notes(part, cameras):
            print(f"设备配置按项目当前值记录，与现场不同：{note}")
        command = [
            "cargo", "run", "--manifest-path", str(REPO / "src-tauri" / "Cargo.toml"), "--offline", "--locked",
            "--example", "capture_fixture", "--",
            "--doc", str(work / "doc.json"), "--cameras", str(work / "cameras.json"),
            "--db", str(db), "--records", str(records_root(args.project, args.records)),
            "--parts", "1", "--label", args.label or part_dir.name,
            "--sample-set", str(work / "sample-set"), "--part", "p01",
        ]
        if args.dry_run:
            print(subprocess.list2cmdline(command))
            return 0
        result = subprocess.run(command, cwd=REPO, capture_output=True, text=True, encoding="utf-8")
        if result.returncode != 0:
            lines = result.stderr.strip().splitlines()
            start = next((i + 1 for i, line in enumerate(lines) if "Running `" in line), 0)
            detail = [line for line in lines[start:] if not line.startswith("用法：")]
            raise ImportFailure("\n".join(detail) or "capture_fixture 失败")
        print(result.stdout.strip())
        return 0
    except ImportFailure as error:
        print(f"导入失败：{error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
