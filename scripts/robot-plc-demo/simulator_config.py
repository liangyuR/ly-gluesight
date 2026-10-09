"""Versioned simulator settings. Relative paths are relative to the config file."""
import argparse
import json
import math
import os
from pathlib import Path

DEFAULT_CONFIG = Path(__file__).with_name("demo.config.json")
APP_ID = "com.xyzrobotics.gluesight.robot-plc-demo"
SCENARIOS = {
    "normal": (1, 0),
    "gap": (13, 0),
    "lostFrame": (90, 91),
    "locateFail": (90, 92),
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


def load_recipe(path):
    recipe = read_json(path)
    if not isinstance(recipe, dict) or not isinstance(recipe.get("id"), str) or not recipe["id"]:
        raise ValueError("Recipe requires an id")
    integer(recipe.get("productCode"), 1, 65535, "recipe.productCode")
    shots = recipe.get("shots")
    if not isinstance(shots, list) or not 2 <= len(shots) <= 64:
        raise ValueError("Robot model requires 2..64 shot positions")
    if any(not isinstance(p, list) or len(p) != 2 or any(type(v) not in (int, float) or not math.isfinite(v) for v in p) for p in shots):
        raise ValueError("Shot positions must be finite [x, y] pairs")
    if recipe.get("mode") != "flyShot" or recipe.get("triggerMode") != "fly":
        raise ValueError("Robot model supports flyShot / fly recipes")
    return recipe


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
