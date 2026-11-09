import gzip
import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest

import numpy as np
import zstandard

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))
from image_bytes import decode_image_bytes, MAX_IMAGE_BYTES


def module(name, relative):
    spec = importlib.util.spec_from_file_location(name, REPO / relative)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


history = module("history_import", "scripts/history-import/import_part.py")
color = module("color_eval", "scripts/color-eval/glue_color_eval.py")


class CompressedImageToolsTests(unittest.TestCase):
    def test_history_export_restores_original_pnm_and_color_eval_keeps_bayer(self):
        mosaic = np.tile(np.array([[200, 100], [100, 20]], dtype=np.uint8), (4, 4))
        raw = b"P5\n# bayer RG\n8 8\n255\n" + mosaic.tobytes()
        for encoded in [raw, gzip.compress(raw),
                        zstandard.ZstdCompressor(write_checksum=True).compress(raw),
                        zstandard.ZstdCompressor(write_checksum=True, write_content_size=False).compress(raw)]:
            with self.subTest(magic=encoded[:4]), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                source = root / "cam1_s000001.pgm"
                source.write_bytes(encoded)
                counts = history.build_sample_set(root, {
                    "cameras": ["cam1"], "groupCount": 1, "frames": [{"width": 8, "height": 8}],
                }, root / "samples")
                self.assertEqual(counts, {"cam1": 1})
                self.assertEqual((root / "samples/parts/p01/S001_cam1_v1.pgm").read_bytes(), raw)
                rgb, bayer = color.load_image(source, None)
                np.testing.assert_array_equal(bayer, mosaic)
                np.testing.assert_array_equal(rgb, np.full((8, 8, 3), [200, 100, 20], dtype=np.float32))

    def test_damaged_or_oversized_frames_are_rejected_before_image_decode(self):
        raw = b"P5\n8 8\n255\n" + bytes(64)
        zstd = zstandard.ZstdCompressor(write_checksum=True).compress(raw)
        damaged = bytearray(zstd)
        damaged[-1] ^= 1
        gzip_data = gzip.compress(raw)
        oversized = f"P5\n{MAX_IMAGE_BYTES} 2\n255\n".encode()
        for encoded in [bytes(damaged), zstd[:-1], zstd + zstd,
                        zstandard.ZstdCompressor(write_checksum=False).compress(raw),
                        gzip_data[:-1], gzip_data + b"extra", gzip.compress(raw[:-1]),
                        gzip.compress(oversized), zstandard.ZstdCompressor(write_checksum=True).compress(oversized)]:
            with self.subTest(magic=encoded[:4], size=len(encoded)), self.assertRaises(ValueError):
                decode_image_bytes(encoded)


if __name__ == "__main__":
    unittest.main()
