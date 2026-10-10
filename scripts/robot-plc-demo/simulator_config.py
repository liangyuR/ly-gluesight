"""Versioned simulator settings. Relative paths are relative to the config file."""
import argparse
import json
import math
import os
import re
from pathlib import Path

DEFAULT_CONFIG = Path(__file__).with_name("demo.config.json")
APP_ID = "com.xyzrobotics.gluesight.robot-plc-demo"
SCENARIOS = {
    "normal": (1, 0),
    "gap": (13, 0),
    "lostFrame": (90, 91),
    "locateFail": (90, 99),
    "countMismatch": (90, 94),
}


def read_json(path):
    return json.loads(Path(path).read_text(encoding="utf-8-sig"))


def integer(value, low, high, name):
    if type(value) is not int or not low <= value <= high:
        raise ValueError(f"{name} must be an integer in {low}..{high}")
    return value


def duration(value, name):
    if type(value) not in (int, float) or not math.isfinite(value) or not 0 < value <= 300:
        raise ValueError(f"{name} must be a finite number in (0, 300]")
    return value


def resolve_path(base, value):
    path = Path(os.path.expandvars(value)).expanduser()
    return (base / path).resolve() if not path.is_absolute() else path.resolve()


def finite(value, name, low=None, high=None):
    if type(value) not in (int, float) or not math.isfinite(value) or abs(value) > 3.4028234663852886e38:
        raise ValueError(f"{name} must be finite")
    if (low is not None and value < low) or (high is not None and value > high):
        raise ValueError(f"{name} is outside the supported range")
    return value


def identifier(value, name):
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,32}", value):
        raise ValueError(f"{name} requires 1..32 letters, digits, underscores or hyphens")


def label(value, name):
    if not isinstance(value, str) or not value.strip() or len(value) > 32 or any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in value):
        raise ValueError(f"{name} requires a nonempty label up to 32 characters without control characters")


def detect_params(value, name):
    if not isinstance(value, dict) or value.get("polarity") not in ("dark", "light"):
        raise ValueError(f"{name} requires detection parameters and dark/light polarity")
    search = finite(value.get("searchMm"), f"{name}.searchMm")
    width = value.get("widthRange")
    if not isinstance(width, list) or len(width) != 2:
        raise ValueError(f"{name}.widthRange requires [min, max]")
    lo, hi = [finite(v, f"{name}.widthRange") for v in width]
    if not 0 < lo < hi < 2 * search:
        raise ValueError(f"{name} requires 0 < width min < width max < 2 * searchMm")


def shot_limits(value, name):
    if not isinstance(value, dict):
        raise ValueError(f"{name} requires shot limits")
    finite(value.get("maxGapLen"), f"{name}.maxGapLen", 0)
    for kind in ("position", "width"):
        params = value.get(kind)
        if params is None:
            continue
        if not isinstance(params, dict):
            raise ValueError(f"{name}.{kind} requires limits or null")
        for field in ("nominal", "tolUpper", "tolLower", "absMin", "absMax", "maxExcursionLen"):
            finite(params.get(field), f"{name}.{kind}.{field}", 0 if field in ("tolUpper", "tolLower", "maxExcursionLen") else None)
        if not params["absMin"] <= params["nominal"] <= params["absMax"]:
            raise ValueError(f"{name}.{kind} nominal must be within absolute limits")


def validate_recipe(recipe):
    if not isinstance(recipe, dict):
        raise ValueError("Recipe must be an object")
    if type(recipe.get("schemaVersion")) is not int or recipe["schemaVersion"] != 4:
        raise ValueError("Unsupported recipe schemaVersion (expected 4); rebuild legacy recipes")
    identifier(recipe.get("id"), "recipe.id")
    if not isinstance(recipe.get("name"), str) or not recipe["name"].strip():
        raise ValueError("Recipe requires a name")
    integer(recipe.get("version", 0), 0, 0xFFFFFFFF, "recipe.version")
    integer(recipe.get("productCode"), 1, 65535, "recipe.productCode")
    if recipe.get("triggerMode") not in ("fly", "stop"):
        raise ValueError("Robot model supports fly/stop-trigger recipes")
    finite(recipe.get("spacing"), "recipe.spacing", 0.1, 10)
    window = integer(recipe.get("filterWindow"), 1, 31, "recipe.filterWindow")
    if window % 2 != 1:
        raise ValueError("recipe.filterWindow must be odd")
    detect_params(recipe.get("detect"), "recipe.detect")
    shot_limits(recipe.get("limits"), "recipe.limits")
    shots = recipe.get("shots")
    if not isinstance(shots, list) or not 1 <= len(shots) <= 64:
        raise ValueError("Robot model requires 1..64 shot objects")
    ids, points = set(), 0
    for shot in shots:
        if not isinstance(shot, dict):
            raise ValueError("Recipe schema 4 requires shot objects, not robot [x, y] coordinates")
        identifier(shot.get("id"), "shot.id")
        if shot["id"] in ids:
            raise ValueError(f"Duplicate shot id: {shot['id']}")
        ids.add(shot["id"])
        label(shot.get("poseId"), "shot.poseId")
        label(shot.get("bead"), "shot.bead")
        identifier(shot.get("camera"), "shot.camera")
        integer(shot.get("view"), 1, 3, "shot.view")
        if shot.get("calib") is not None:
            identifier(shot["calib"], "shot.calib")
        if type(shot.get("skip", False)) is not bool:
            raise ValueError("shot.skip must be boolean")
        path = shot.get("path", [])
        if not isinstance(path, list) or any(not isinstance(p, list) or len(p) != 2 for p in path):
            raise ValueError("shot.path must contain pixel [x, y] pairs")
        for p in path:
            for v in p:
                finite(v, "shot.path coordinate")
        length = sum(math.dist(a, b) for a, b in zip(path, path[1:]))
        if len(path) == 1 or (path and length < 1):
            raise ValueError("shot.path requires two points and at least one pixel of length")
        mm = shot.get("mmPerPx")
        if mm is not None and not 0 < finite(mm, "shot.mmPerPx") <= 10:
            raise ValueError("shot.mmPerPx must be in (0, 10]")
        if path and mm is None:
            raise ValueError("shot.path requires mmPerPx")
        if path and not shot.get("skip", False):
            count = math.floor(length * mm / recipe["spacing"] + 1e-3) + 1
            if count < 3:
                raise ValueError("shot.path must cover at least three measurement stations")
            points += count
        if shot.get("detect") is not None:
            detect_params(shot["detect"], "shot.detect")
        if shot.get("limits") is not None:
            shot_limits(shot["limits"], "shot.limits")
    if points > 200_000:
        raise ValueError("Recipe exceeds 200000 measurement stations")
    return recipe


def load_recipe(path):
    return validate_recipe(read_json(path))


def trigger_plan(recipe):
    counts, plan = {}, []
    for k, shot in enumerate(recipe["shots"]):
        camera = shot["camera"]
        counts[camera] = counts.get(camera, 0) + 1
        plan.append({"k": k, "shotId": shot["id"], "poseId": shot["poseId"], "camera": camera,
                     "view": shot["view"], "ordinal": counts[camera]})
    return plan, counts


def recipe_contract(recipe):
    contract = {key: recipe[key] for key in ("id", "productCode", "schemaVersion", "triggerMode", "spacing", "filterWindow", "detect", "limits")}
    contract["version"] = max(1, integer(recipe.get("version", 0), 0, 0xFFFFFFFF, "recipe.version"))
    contract["shots"] = [{"id": shot["id"], "poseId": shot["poseId"], "camera": shot["camera"], "view": shot["view"],
                          "bead": shot["bead"], "calib": shot.get("calib") or shot["camera"], "skip": shot.get("skip", False),
                          "path": shot.get("path", []), "mmPerPx": shot.get("mmPerPx"),
                          "detect": shot.get("detect") or recipe["detect"], "limits": shot.get("limits") or recipe["limits"]}
                         for shot in recipe["shots"]]
    return contract


def load_config(path=DEFAULT_CONFIG):
    path = Path(path).resolve()
    cfg = read_json(path)
    if cfg.get("schemaVersion") != 1:
        raise ValueError("Unsupported simulator schemaVersion (expected 1)")
    ports = [integer(cfg["plc"]["port"], 1, 65535, "plc.port"),
             integer(cfg["robot"]["port"], 1, 65535, "robot.port"),
             integer(cfg["bridge"]["debugPort"], 1, 65535, "bridge.debugPort")]
    if len(set(ports)) != 3:
        raise ValueError("PLC, Robot and WebView ports must be different")
    integer(cfg["plc"]["unitId"], 1, 247, "plc.unitId")
    for name in ("motionSeconds", "settleSeconds", "releaseSeconds", "intervalSeconds"):
        duration(cfg["robot"][name], f"robot.{name}")
    for name in ("ready", "armed", "trigger", "result", "release"):
        duration(cfg["robot"]["timeouts"][name], f"robot.timeouts.{name}")
    cfg["runtimeDir"] = str(resolve_path(path.parent, cfg["runtimeDir"]))
    cfg["robot"]["recipe"] = str(resolve_path(path.parent, cfg["robot"]["recipe"]))
    cfg["configPath"] = str(path)
    cfg["appId"] = APP_ID
    cfg["consoleUrl"] = f"http://127.0.0.1:{ports[1]}"
    return cfg


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    args = parser.parse_args()
    print(json.dumps(load_config(args.config), ensure_ascii=True))
