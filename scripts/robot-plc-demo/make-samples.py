"""Export selected schema 4 teaching frames and an image-space gap fixture.

Only frozen simulator images are accepted. These samples do not prove field accuracy.
"""
import argparse
import json
import math
import os
from pathlib import Path
import re
import time

from simulator_config import APP_ID, DEFAULT_CONFIG, load_config, load_recipe, recipe_contract, trigger_plan, validate_recipe


def read_pgm(data):
    offset, tokens = 0, []
    for _ in range(4):
        while offset < len(data):
            if data[offset] in b" \t\r\n":
                offset += 1
            elif data[offset] == 35:
                end = data.find(b"\n", offset)
                offset = len(data) if end < 0 else end + 1
            else:
                break
        start = offset
        while offset < len(data) and data[offset] not in b" \t\r\n#":
            offset += 1
        tokens.append(data[start:offset])
    if tokens[0] != b"P5" or tokens[3] != b"255" or not tokens[1].isdigit() or not tokens[2].isdigit():
        raise ValueError("Expected an 8-bit P5 grayscale image")
    width, height = map(int, tokens[1:3])
    if not 0 < width <= 16384 or not 0 < height <= 16384 or width * height > 64_000_000:
        raise ValueError("Unsupported PGM dimensions")
    if data[offset:offset + 2] == b"\r\n":
        offset += 2
    elif data[offset:offset + 1] and data[offset] in b" \t\r\n":
        offset += 1
    else:
        raise ValueError("Missing PGM pixel separator")
    if len(data) - offset != width * height:
        raise ValueError("PGM pixel count differs from its dimensions")
    return width, height, bytearray(data[offset:])


def pgm(width, height, pixels):
    return f"P5\n{width} {height}\n255\n".encode() + pixels


def path_at(path, distance):
    for a, b in zip(path, path[1:]):
        length = math.dist(a, b)
        if distance <= length and length > 0:
            return [a[i] + (b[i] - a[i]) * max(0, distance) / length for i in (0, 1)]
        distance -= length
    return path[-1]


def nearest(path, x, y):
    best, arc, before = float("inf"), 0, 0
    for a, b in zip(path, path[1:]):
        dx, dy = b[0] - a[0], b[1] - a[1]
        length = math.hypot(dx, dy)
        if length == 0:
            continue
        t = min(1, max(0, ((x - a[0]) * dx + (y - a[1]) * dy) / (length * length)))
        distance = math.hypot(x - a[0] - t * dx, y - a[1] - t * dy)
        if distance < best:
            best, arc = distance, before + t * length
        before += length
    return best, arc


def remove_gap(data, shot, recipe):
    width, height, pixels = read_pgm(data)
    if [width, height] != [1280, 1024]:
        raise ValueError("Gap generation requires the current 1280x1024 simulator background")
    mm = shot["mmPerPx"]
    length = sum(math.dist(a, b) for a, b in zip(shot["path"], shot["path"][1:]))
    count = math.floor(length * mm / recipe["spacing"] + 1e-3) + 1
    limits = shot.get("limits") or recipe["limits"]
    stations = math.ceil((limits["maxGapLen"] + 2) / recipe["spacing"])
    if stations >= count - 2:
        raise ValueError("The taught path is too short for a gap beyond its configured limit")
    start = ((count - stations) // 2) * recipe["spacing"] / mm
    end = start + stations * recipe["spacing"] / mm
    span = [path_at(shot["path"], start), path_at(shot["path"], end)]
    before = 0
    for a, b in zip(shot["path"], shot["path"][1:]):
        before += math.dist(a, b)
        if start < before < end:
            span.append(b)
    detect = shot.get("detect") or recipe["detect"]
    radius = max(4, detect["widthRange"][1]) / mm / 2 + 3
    xmin = max(0, math.floor(min(p[0] for p in span) - radius))
    xmax = min(width - 1, math.ceil(max(p[0] for p in span) + radius))
    ymin = max(0, math.floor(min(p[1] for p in span) - radius))
    ymax = min(height - 1, math.ceil(max(p[1] for p in span) + radius))
    changed = 0
    for y in range(ymin, ymax + 1):
        for x in range(xmin, xmax + 1):
            distance, arc = nearest(shot["path"], x, y)
            if distance <= radius and start <= arc < end:
                pixels[y * width + x] = int(150 + 60 * x / width + 25 * y / height + 6 * math.sin(x * 0.05 + y * 0.31))
                changed += 1
    if changed == 0:
        raise ValueError("The gap interval does not overlap the image")
    return pgm(width, height, pixels), {"startMm": start * mm, "lengthMm": (end - start) * mm,
                                     "allowedMm": limits["maxGapLen"], "changedPixels": changed}


def frozen_data(workspace, image, shot):
    if not isinstance(image, dict) or image.get("source") != "Sim":
        raise ValueError("Sample export accepts frozen simulator images only")
    if image.get("camera") != shot["camera"] or image.get("view") not in (1, 2, 3):
        raise ValueError("Frozen image camera/view differs from the shot")
    image_id = image.get("id")
    if not isinstance(image_id, str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", image_id):
        raise ValueError("Invalid frozen image id")
    data = (workspace / "images" / f"{image_id}.pgm").read_bytes()
    width, height, _ = read_pgm(data)
    if image.get("size") != [width, height]:
        raise ValueError("Frozen image size differs from its metadata")
    return data


def export_samples(workspace, output, expected_recipe=None):
    workspace, output = Path(workspace), Path(output)
    state = json.loads((workspace / "workspace.json").read_text(encoding="utf-8-sig"))
    recipe = validate_recipe(state["doc"])
    if expected_recipe is not None and recipe_contract(recipe) != recipe_contract(expected_recipe):
        raise ValueError("Workspace differs from the configured recipe fixture")
    frames = state["frames"]
    if len(frames) != len(recipe["shots"]):
        raise ValueError("Every planned shot requires a frozen image")
    measured = [k for k, shot in enumerate(recipe["shots"]) if not shot.get("skip", False) and shot.get("path")]
    if not measured or any(not s.get("skip", False) and not s.get("path") for s in recipe["shots"]):
        raise ValueError("Every inspected shot must have a taught pixel path")
    gap_k = measured[min(1, len(measured) - 1)]
    plan, counts = trigger_plan(recipe)
    required_views = {camera: (3 if any(s["view"] > 1 for s in recipe["shots"] if s["camera"] == camera) else 1)
                      for camera in counts}
    files, manifest = {}, []
    gap_info = None
    for k, (shot, frame) in enumerate(zip(recipe["shots"], frames)):
        if frame.get("k") != k or not isinstance(frame.get("image"), dict) or frame["image"].get("view") != shot["view"]:
            raise ValueError("Frozen shot index or selected view differs from the recipe")
        data = frozen_data(workspace, frame["image"], shot)
        files[f"good/k{k + 1}.pgm"] = data
        gap_data, info = remove_gap(data, shot, recipe) if k == gap_k else (data, None)
        files[f"gap/k{k + 1}.pgm"] = gap_data
        if info:
            gap_info = {"k": k, "shotId": shot["id"], "camera": shot["camera"], "view": shot["view"], **info}
        views = frame.get("views") or [frame["image"]]
        if not any(image.get("id") == frame["image"]["id"] and image.get("view") == shot["view"] for image in views):
            raise ValueError("Selected image does not belong to the frozen device frame")
        if len({v["view"] for v in views}) != len(views) or {v["view"] for v in views} not in ({1}, {1, 2, 3}):
            raise ValueError("Frozen device frame has duplicate or incomplete views")
        if required_views[shot["camera"]] == 3 and {v["view"] for v in views} != {1, 2, 3}:
            raise ValueError("A tricam trigger requires all three frozen views")
        for image in views:
            pixels = frozen_data(workspace, image, shot)
            files[f"replay/{shot['camera']}_{plan[k]['ordinal']}_v{image['view']}.pgm"] = pixels
            manifest.append({**plan[k], "view": image["view"], "selected": image["view"] == shot["view"],
                             "sourceImageId": image["id"], "size": image["size"]})
    report = {"source": "Frozen GlueSight simulator pixels; gap removal in the selected image-space path",
              "physicalValidation": False, "imageEngineValidated": False, "recipe": recipe,
              "plannedDeviceTriggers": counts, "gap": gap_info, "frames": manifest,
              "files": {name: {"bytes": len(data)} for name, data in files.items()}}
    output.mkdir(parents=True, exist_ok=False)
    for name, data in files.items():
        path = output / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    (output / "provenance.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--workspace", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    cfg = load_config(args.config)
    recipe = load_recipe(cfg["robot"]["recipe"])
    if args.workspace is None and not os.environ.get("APPDATA"):
        parser.error("--workspace is required when APPDATA is unavailable")
    workspace = args.workspace or Path(os.environ["APPDATA"]) / APP_ID / "workspaces" / recipe["id"]
    output = args.output or Path(cfg["runtimeDir"]) / "samples" / f"{recipe['id']}-{time.time_ns()}"
    export_samples(workspace, output, recipe)
    print(output)


if __name__ == "__main__":
    main()
