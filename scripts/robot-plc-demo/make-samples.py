"""Export newly taught synthetic frames; remove 6 mm of bead from the first image.

These are software simulation fixtures, not physical camera validation samples.
"""
import json
import argparse
import math
import os
import re
from pathlib import Path
from simulator_config import APP_ID, DEFAULT_CONFIG, load_config

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
args = parser.parse_args()
cfg = load_config(args.config)
workspace = Path(os.environ["APPDATA"]) / APP_ID / "workspaces/ROBOT-DEMO"
state = json.loads((workspace / "workspace.json").read_text(encoding="utf-8"))
if state["doc"]["shots"] != [[95,50],[285,50],[285,150],[95,150]] or state["doc"]["fov"] != [216,145]:
    raise ValueError("This image fixture generator requires the default four-point geometry")
out = Path(cfg["runtimeDir"]) / "samples"
out.mkdir(parents=True, exist_ok=True)
for k, frame in enumerate(state["frames"]):
    data = (workspace / "images" / (frame["image"]["id"] + ".pgm")).read_bytes()
    (out / f"k{k + 1}.pgm").write_bytes(data)
    if k == 0:
        match = re.match(rb"P5\s+(\d+)\s+(\d+)\s+255\s", data)
        assert match, "Expected generated P5 grayscale image"
        width, height = map(int, match.groups())
        pixels = bytearray(data[match.end():])
        assert len(pixels) == width * height
        # Same 0.08 mm/pixel and camera center used by GlueSight's simulator.
        cx, cy = state["doc"]["shots"][0]
        fovx, fovy = state["doc"]["fov"]
        for iy in range(height):
            y = cy - fovy / 2 + (iy + 0.5) * 0.08
            if abs(y) > 0.57:
                continue
            for ix in range(width):
                x = cx - fovx / 2 + (ix + 0.5) * 0.08
                if 57 <= x <= 63:
                    pixels[iy * width + ix] = round(186 + 6 * math.sin(x / 37) + 4 * math.cos(y / 23))
        (out / "k1-gap.pgm").write_bytes(data[:match.end()] + pixels)
(out / "provenance.json").write_text(json.dumps({
    "source": "Synthetic frames captured and taught through GlueSight UI in this demo",
    "recipe": state["doc"], "gap": "6 mm removal at top edge x=57..63 mm in frame k1",
    "physical_validation": False
}, ensure_ascii=False, indent=2), encoding="utf-8")
print(out)
