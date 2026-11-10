import sys
import tempfile
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).parents[2] / "scripts"))
import gsr_raw  # noqa: E402

# 与 src-tauri/src/image_file/gsr.rs 的 file_stored_in_format_version_1_still_restores 同一份样本：
# Rust 编码器写出的 8 × 6 彩色录制原图（带 `# bayer RG` 注释）与它的原始 PGM 字节
ORIGINAL = bytes.fromhex(
    "50350a232062617965722052470a3820360a3235350a"
    "002f5e1544732a595d1443722958873e42712857863d6c9b2756853c6b9a5180843b6a99507fae6569984f7ead6493c2"
)
STORED = bytes.fromhex(
    "4753521a010216003000000088815a34"
    "50350a232062617965722052470a3820360a3235350a"
    "28b52ffd2430810100005ee6e642e65ee684e6e65e2fe65ee671e6e65e3b5ee6e65de6e65e275ee6e669e65ee6145ee6e656e65ee698e6e65e34e7b1a9"
)


class GsrRawTests(unittest.TestCase):
    def test_compressed_frame_reads_as_the_same_array_as_its_pgm_and_damage_is_rejected(self):
        # 防的回归：录制目录里的原图空闲压缩成 .gsr 后，脚本按 part.json 里的 .pgm 文件名读不到图、读出的像素与原图不同，
        # 或把损坏的压缩文件读成了图
        with tempfile.TemporaryDirectory() as temp:
            plain = Path(temp) / "plain" / "cam1_s000001.pgm"
            compressed = Path(temp) / "compressed" / "cam1_s000001.pgm"
            plain.parent.mkdir()
            compressed.parent.mkdir()
            plain.write_bytes(ORIGINAL)
            compressed.with_suffix(".gsr").write_bytes(STORED)

            self.assertEqual(gsr_raw.stored_path(compressed), compressed.with_suffix(".gsr"))
            self.assertEqual(gsr_raw.read_original(compressed), ORIGINAL)
            header = len(ORIGINAL) - 48
            pixels = lambda path: np.frombuffer(gsr_raw.read_original(path), dtype=np.uint8)[header:].reshape(6, 8)
            self.assertTrue(np.array_equal(pixels(compressed), pixels(plain)))

            for name, damaged in {
                "截断": STORED[:-3],
                "负载被改": STORED[:-1] + bytes([STORED[-1] ^ 1]),
                "文件头被改": STORED.replace(b"bayer RG", b"bayer GR"),
                "尾随数据": STORED + b"\0",
            }.items():
                with self.assertRaises(gsr_raw.GsrError, msg=name):
                    gsr_raw.decode(damaged)


if __name__ == "__main__":
    unittest.main()
