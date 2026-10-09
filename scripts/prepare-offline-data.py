"""Extract the Hikvision sample archive and prepare lossless replay directories."""
import argparse
import csv
import hashlib
import json
import re
import shutil
import stat
import zipfile
from collections import Counter, defaultdict
from pathlib import Path, PurePosixPath

from PIL import Image, ImageDraw, ImageOps


def sha256(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def prepare(archive, destination):
    destination = destination.resolve()
    destination.mkdir(parents=True, exist_ok=True)
    raw = destination / "raw"
    raw.mkdir(exist_ok=True)
    names = set()
    extracted = []
    with zipfile.ZipFile(archive) as z:
        entries = z.infolist()
        if len(entries) > 10000 or sum(i.file_size for i in entries) > 500_000_000:
            raise ValueError("Archive exceeds the sample-data size limit")
        for entry in entries:
            relative = PurePosixPath(entry.filename)
            if relative.is_absolute() or ".." in relative.parts or "\\" in entry.filename or ":" in entry.filename:
                raise ValueError("Unsafe archive path: " + entry.filename)
            if stat.S_ISLNK(entry.external_attr >> 16):
                raise ValueError("Archive links are unsupported")
            key = entry.filename.casefold()
            if key in names:
                raise ValueError("Duplicate archive path: " + entry.filename)
            names.add(key)
            target = raw.joinpath(*relative.parts).resolve()
            if not target.is_relative_to(raw.resolve()):
                raise ValueError("Archive target is outside the extraction directory")
            if entry.is_dir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            with z.open(entry) as source, target.open("wb") as output:
                shutil.copyfileobj(source, output)
            extracted.append((entry.filename, target))
    rows = []
    references = []
    model_files = []
    for original, source in extracted:
        match = re.fullmatch(r"Frame(\d+)_(\d+)\.jpe?g", source.name, re.I)
        if not match:
            if source.suffix.lower() == ".bin":
                model_files.append(str(source.relative_to(destination)))
            elif source.suffix.lower() in {".jpg", ".jpeg", ".png", ".bmp"}:
                references.append(str(source.relative_to(destination)))
            continue
        sequence, channel = map(int, match.groups())
        group = source.parent.name
        if not 1 <= channel <= 3:
            raise ValueError("Unexpected replay channel: " + original)
        target = destination / "replay" / group / ("cam%d_%06d.jpg" % (channel, sequence))
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
        digest = sha256(source)
        if sha256(target) != digest:
            raise ValueError("Image bytes changed while preparing " + original)
        with Image.open(target) as image:
            width, height = image.size
            if width * height > 40_000_000:
                raise ValueError("Image exceeds the application pixel limit")
            image.load()
            mode = image.mode
        rows.append({"group": group, "channel": channel, "sequence": sequence,
                     "source": original, "replay": str(target.relative_to(destination)),
                     "width": width, "height": height, "mode": mode, "sha256": digest})
    rows.sort(key=lambda r: (r["group"], r["channel"], r["sequence"]))
    if not rows:
        raise ValueError("No named replay images in archive")
    with (destination / "frame-name-map.csv").open("w", newline="", encoding="utf-8-sig") as output:
        writer = csv.DictWriter(output, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)
    groups = []
    for group in sorted({r["group"] for r in rows}):
        frames = [r for r in rows if r["group"] == group]
        channels = []
        by_sequence = defaultdict(list)
        for row in frames:
            by_sequence[row["sequence"]].append(row["sha256"])
        for channel in sorted({r["channel"] for r in frames}):
            selected = [r for r in frames if r["channel"] == channel]
            indices = [r["sequence"] for r in selected]
            if len(set(indices)) != len(indices):
                raise ValueError("Duplicate frame numbers")
            channels.append({"replayChannel": channel, "frames": len(indices), "first": min(indices),
                             "last": max(indices), "missing": sorted(set(range(min(indices), max(indices)+1))-set(indices)),
                             "sizes": sorted({(r["width"], r["height"]) for r in selected})})
        groups.append({"name": group, "replayDir": str((destination / "replay" / group).resolve()),
                       "channels": channels,
                       "identicalThreeChannelTriplets": sum(len(v) == 3 and len(set(v)) == 1 for v in by_sequence.values())})
        contact_sheet(destination, group, [r for r in frames if r["channel"] == 1])
    report = {"archive": str(archive.resolve()), "archiveSha256": sha256(archive),
              "destination": str(destination), "frames": len(rows), "groups": groups,
              "references": references, "proprietaryModels": model_files,
              "formats": dict(Counter(r["mode"] for r in rows)),
              "allJpegImagesDecoded": True, "allRenamedFilesByteIdentical": True,
              "naming": "cam{channel}_{sequence:06d}.jpg; channel and sequence are preserved",
              "timing": "The archive contains no timestamps. Replay FPS is a test setting, not a measured production rate.",
              "calibration": "Physical pixel scale and camera poses are not supplied as application calibration.",
              "modelCompatibility": "The vendor BIN models are retained as reference data; this application does not load them.",
              "limitations": ["All three channels contain identical image bytes at corresponding frame indices.",
                              "The sample groups have no human OK/NG labels or real capture timestamps."]}
    write_json(destination / "dataset.json", report)
    prepare_timed_replay(destination, report)
    print(json.dumps({k: report[k] for k in ["destination", "frames", "groups", "formats"]}, ensure_ascii=False, indent=2))


def prepare_timed_replay(destination, report):
    """Supply an explicitly synthetic, finite timeline for complete-cycle testing."""
    for group in report["groups"]:
        target = destination / "timed-replay" / group["name"]
        target.mkdir(parents=True, exist_ok=True)
        frames = []
        for channel in group["channels"]:
            for sequence in range(channel["first"], channel["last"] + 1):
                name = "cam%d_%06d.jpg" % (channel["replayChannel"], sequence)
                shutil.copy2(Path(group["replayDir"]) / name, target / name)
                frames.append({"file": name, "ts": (sequence - 1) * 100})
        write_json(target / "part.json", {
            "startedTs": 0, "frames": frames, "syntheticTiming": True,
            "note": "100 ms per frame is a test clock, not an original acquisition timestamp."
        })


def contact_sheet(destination, group, rows):
    indices = sorted(set(round(i*(len(rows)-1)/5) for i in range(6)))
    sheet = Image.new("RGB", (3*480, 2*380), "#111111")
    draw = ImageDraw.Draw(sheet)
    for position, index in enumerate(indices):
        row = rows[index]
        with Image.open(destination / row["replay"]) as image:
            thumb = ImageOps.contain(image.convert("RGB"), (464, 340))
        x, y = (position % 3)*480, (position // 3)*380
        sheet.paste(thumb, (x+(480-thumb.width)//2, y+24+(340-thumb.height)//2))
        draw.text((x+10, y+5), group+" / cam1 / frame "+str(row["sequence"]), fill="white")
    path = destination / "preview"
    path.mkdir(exist_ok=True)
    sheet.save(path / (group+"-contact.jpg"), quality=90)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--timed-only", action="store_true")
    args = parser.parse_args()
    if args.timed_only:
        prepare_timed_replay(args.destination, json.loads((args.destination / "dataset.json").read_text(encoding="utf-8")))
    else:
        prepare(args.archive, args.destination)
