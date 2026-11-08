import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest

import cv2
import numpy as np

spec = importlib.util.spec_from_file_location("prepare", Path(__file__).parents[2] / "scripts/Prepare-ProductionSimulation.py")
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class ProductionInputTests(unittest.TestCase):
    def write_record(self, root, images):
        root.mkdir()
        frames = []
        for camera, data in images.items():
            for seq, raw in enumerate(data, 1):
                name = f"{camera}_s{seq:06}.pgm"
                (root / name).write_bytes(raw)
                frames.append({"cameraId": camera, "seq": seq, "file": name, "available": True})
        metadata = {"available": True, "frames": frames}
        (root / "part.json").write_text(json.dumps(metadata), encoding="utf-8")
        return metadata

    def run_prepare(self, root, output, passes="1,1"):
        with contextlib.redirect_stdout(io.StringIO()):
            prepare.prepare(root, output, 7, 10, passes)

    def test_complete_record_preserves_passes_frame_order_and_color(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            images = {}
            for phase, tile in {"RG": [[200,100],[100,20]], "BG": [[20,100],[100,200]],
                                "GR": [[100,200],[20,100]], "GB": [[100,20],[200,100]]}.items():
                mosaic = np.tile(np.array(tile, dtype=np.uint8), (4, 4))
                raw = f"P5\n# bayer {phase}\n8 8\n255\n".encode() + mosaic.tobytes()
                images[phase] = [raw, raw]
            gray = np.arange(64, dtype=np.uint8).reshape(8, 8)
            gray.flat[:11] = np.frombuffer(b"# bayer RG\n", dtype=np.uint8)
            images["gray"] = [b"P5\n8 8\n255\n" + gray.tobytes(), b"P5\n8 8\n255\n" + (gray // 2).tobytes()]
            rgb = np.full((8, 8, 3), [200,100,20], dtype=np.uint8)
            images["rgb"] = [b"P6\n8 8\n255\n" + rgb.tobytes()] * 2
            self.write_record(base / "record", images)
            self.run_prepare(base / "record", base / "out")
            config = json.loads((base / "out/simulator.json").read_text())
            self.assertEqual(config["passFrames"], [1, 1])
            for camera in config["cameras"]:
                for index, filename in enumerate(camera["frames"]):
                    image = cv2.imread(str(base / "out" / filename), cv2.IMREAD_UNCHANGED)
                    expected = gray // (index + 1) if camera["id"] == "gray" else np.full((8,8,3), [20,100,200], dtype=np.uint8)
                    np.testing.assert_array_equal(image, expected)

    def test_incomplete_or_unsafe_record_cannot_produce_runnable_config(self):
        for case in ["escape", "missing", "duplicate", "passes", "alpha", "size"]:
            with self.subTest(case=case), tempfile.TemporaryDirectory() as temp:
                base = Path(temp)
                raw = b"P5\n8 8\n255\n" + bytes(64)
                root, output = base / "record", base / "out"
                metadata = self.write_record(root, {"cam1": [raw, raw], "cam2": [raw, raw]})
                if case == "escape":
                    (base / "outside.pgm").write_bytes(raw)
                    metadata["frames"][0]["file"] = "../outside.pgm"
                elif case == "missing":
                    metadata["frames"].pop()
                elif case == "duplicate":
                    metadata["frames"][1]["seq"] = 1
                elif case in ("alpha", "size"):
                    image = np.zeros((8,8,4) if case == "alpha" else (9,8), dtype=np.uint8)
                    (root / metadata["frames"][1]["file"]).write_bytes(cv2.imencode(".png", image)[1].tobytes())
                (root / "part.json").write_text(json.dumps(metadata), encoding="utf-8")
                with self.assertRaises(ValueError):
                    self.run_prepare(root, output, "1" if case == "passes" else "1,1")
                self.assertFalse((output / "simulator.json").exists())


if __name__ == "__main__":
    unittest.main()
