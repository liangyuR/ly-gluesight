import argparse
import hashlib
import json
import os
from pathlib import Path
import re


REPOSITORY = Path(__file__).resolve().parents[1]
OUTPUT_ROOT = REPOSITORY / "output"
ROUND_ID = re.compile(r"capture-[A-Za-z0-9-]+\Z")


def checked_path(path):
    absolute = path.absolute()
    for component in [absolute, *absolute.parents]:
        if component.exists() and (component.is_symlink() or component.is_junction()):
            raise ValueError(f"拒绝符号链接或目录联接：{component}")
    return absolute.resolve()


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def prepare(source, destination, clone_id):
    source = checked_path(source)
    destination = checked_path(destination)
    output = checked_path(OUTPUT_ROOT)
    if not destination.is_relative_to(output) or destination == output:
        raise ValueError("输出必须位于仓库 output/ 的独立子目录中")
    if destination.is_relative_to(source) or source.is_relative_to(destination):
        raise ValueError("源轮次与输出目录不得互相包含")
    if destination.exists():
        raise ValueError(f"输出目录已存在，拒绝覆盖：{destination}")

    manifest_path = destination.parent / (destination.name + "-dataset.json")
    if manifest_path.exists():
        raise ValueError(f"清单已存在，拒绝覆盖：{manifest_path}")
    raw_round = checked_path(source / "round.json").read_bytes()
    raw_frames = checked_path(source / "frames.jsonl").read_bytes()
    metadata = json.loads(raw_round)
    round_id = metadata["roundId"]
    if source.name != round_id or not ROUND_ID.fullmatch(round_id):
        raise ValueError("源目录名与合法 roundId 不一致")
    if metadata.get("state") != "complete" or metadata.get("groupCount", 0) <= 0:
        raise ValueError("只接受已完成且有帧的采集轮次")
    clone_id = clone_id or round_id + "-benchmark"
    if clone_id == round_id or not ROUND_ID.fullmatch(clone_id):
        raise ValueError("副本 roundId 必须合法且不同于源轮次")

    frames = [json.loads(line) for line in raw_frames.splitlines() if line.strip()]
    if not frames:
        raise ValueError("帧清单为空")
    sources = {}
    for frame in frames:
        relative = Path(frame["path"])
        if relative.is_absolute() or ".." in relative.parts or ":" in str(relative):
            raise ValueError(f"帧路径越界：{relative}")
        image = checked_path(source / relative)
        if not image.is_relative_to(source) or not image.is_file():
            raise ValueError(f"帧文件不在源轮次内：{relative}")
        if str(relative) in sources:
            raise ValueError(f"帧清单存在重复路径：{relative}")
        sources[str(relative)] = image

    destination.mkdir(parents=True)
    for identity in [round_id, clone_id]:
        target = destination / identity
        target.mkdir()
        document = {**metadata, "roundId": identity}
        (target / "round.json").write_bytes(raw_round if identity == round_id else json.dumps(document, ensure_ascii=False).encode("utf-8"))
        (target / "frames.jsonl").write_bytes(raw_frames)
        for relative, image in sources.items():
            linked = target / relative
            linked.parent.mkdir(parents=True, exist_ok=True)
            os.link(image, linked)

    hashes = {"round.json": sha256(raw_round), "frames.jsonl": sha256(raw_frames)}
    final_hashes = {name: sha256((source / name).read_bytes()) for name in hashes}
    manifest = {
        "source": str(source), "captureRoot": str(destination),
        "copiedRounds": [round_id, clone_id], "groups": metadata["groupCount"],
        "cameraIds": [camera["cameraId"] for camera in metadata["cameras"]],
        "cameras": len(metadata["cameras"]), "imageCount": len(frames),
        "sourceImageLogicalBytes": sum(image.stat().st_size for image in sources.values()),
        "width": frames[0]["width"], "height": frames[0]["height"],
        "hashesAtBenchmarkStart": hashes, "hashesAfterPreparation": final_hashes,
        "sourceMetadataUnchangedAfterPreparation": hashes == final_hashes,
        "sameVolumeHardLinks": True,
        "note": "Second round is an isolated metadata copy with hard links to the same image files; OS disk cache is not purged.",
    }
    manifest_path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    if hashes != final_hashes:
        raise ValueError("准备期间源元数据发生变化，请使用静止的已完成轮次重新准备")
    return manifest_path


def main():
    parser = argparse.ArgumentParser(description="复制已完成轮次元数据并硬链接原图，准备隔离缩略图性能基准；输出目录须不存在。")
    parser.add_argument("--source", type=Path, required=True, help="源轮次目录；指定 --round 时为 recipe-capture 根目录")
    parser.add_argument("--round", help="源根目录下的轮次 ID")
    parser.add_argument("--destination", type=Path, default=OUTPUT_ROOT / "perf-thumbnails" / "capture")
    parser.add_argument("--clone-id", help="第二件的轮次 ID，默认在源 roundId 后追加 -benchmark")
    args = parser.parse_args()
    if args.round and not ROUND_ID.fullmatch(args.round):
        parser.error("--round 不是合法的采集轮次 ID")
    source = args.source / args.round if args.round else args.source
    try:
        manifest = prepare(source, args.destination, args.clone_id)
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"准备失败：{error}\n")
    print(manifest)


if __name__ == "__main__":
    main()
