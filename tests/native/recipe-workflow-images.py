from pathlib import Path
import sys

from PIL import Image

destination = Path(sys.argv[1]).resolve()
destination.mkdir(parents=True, exist_ok=True)
view = Image.new("L", (300, 300), 220)
view.paste(35, (0, 130, 300, 170))
view.save(destination / "view.png")
original = Image.new("L", (900, 300))
for index in range(3):
    original.paste(view, (index * 300, 0))
original.save(destination / "original.png")
