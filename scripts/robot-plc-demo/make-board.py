"""Known synthetic calibration target: 9x6 inner corners, 40 px per 3.2 mm square."""
from pathlib import Path
import argparse
from simulator_config import DEFAULT_CONFIG, load_config

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
args = parser.parse_args()
root = Path(load_config(args.config)["runtimeDir"])
root.mkdir(parents=True, exist_ok=True)
width, height = 640, 480
pixels = bytearray([235]) * (width * height)
for row in range(7):
    for col in range(10):
        color = 15 if (row + col) % 2 == 0 else 235
        for y in range(100 + row * 40, 100 + (row + 1) * 40):
            start = y * width + 120 + col * 40
            pixels[start:start + 40] = bytes([color]) * 40
path = root / "calibration-board.pgm"
path.write_bytes(f"P5\n{width} {height}\n255\n".encode() + pixels)
print(path)
