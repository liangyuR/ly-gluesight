import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import convert  # noqa: E402


def fake_jpeg(width: int, height: int, tag: str) -> bytes:
    app0 = b"\xff\xe0" + (2 + 5).to_bytes(2, "big") + b"JFIF\x00"
    sof = b"\xff\xc0" + (11).to_bytes(2, "big") + b"\x08" + height.to_bytes(2, "big") + width.to_bytes(2, "big")
    sof += b"\x01\x01\x11\x00"
    return b"\xff\xd8" + app0 + sof + tag.encode() + b"\xff\xd9"


def run(*argv: str) -> int:
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        return convert.main(list(argv))


def load(out: Path) -> dict:
    return json.loads((out / "sample-set.json").read_text(encoding="utf-8"))


class SampleSetTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def make_isv(self) -> Path:
        src = self.root / "isv"
        for folder in ("20261009192320_B2", "20261009192151_A1"):
            d = src / folder
            d.mkdir(parents=True)
            for frame in ("Frame2000001.jpg", "Frame1000010.jpg", "Frame1000002.jpg"):
                (d / frame).write_bytes(fake_jpeg(1280, 960, f"{folder}/{frame}"))
            (d / "overview.jpg").write_bytes(fake_jpeg(10, 10, "overview"))
            (d / "teachInfo.txt").write_text("{}", encoding="utf-8")
        return src

    def make_mx11(self, labels_by_sn: dict, name: str = "mx11") -> Path:
        src = self.root / name
        for sn, labels in labels_by_sn.items():
            d = src / sn
            d.mkdir(parents=True)
            for i, (point, label) in enumerate(labels.items()):
                stem = f"{sn}_BOL010_20261005_04090{i}_MX11_1_{point}_{label}"
                (d / f"{stem}.jpg").write_bytes(fake_jpeg(640, 480, stem))
                (d / f"{stem}_ret.jpg").write_bytes(fake_jpeg(640, 480, "ret"))
        return src

    def test_isv目录_按d再序号编号拍照点并按时间排件(self):
        out = self.root / "out"
        self.assertEqual(run("isv", str(self.make_isv()), str(out)), 0)
        doc = load(out)

        self.assertEqual(doc["imageSize"], [1280, 960])
        self.assertEqual(
            [(s["shotId"], s["origin"]) for s in doc["shots"]],
            [("S001", "Frame1000002.jpg"), ("S002", "Frame1000010.jpg"), ("S003", "Frame2000001.jpg")],
        )
        p01 = doc["parts"][0]
        self.assertEqual(
            (p01["partId"], p01["sn"], p01["capturedAt"], p01["label"]),
            ("p01", "A1", "2026-10-09T19:21:51+08:00", "unknown"),
        )
        self.assertIn(b"20261009192151_A1/Frame1000010.jpg", (out / "parts/p01/S002_cam1_v1.jpg").read_bytes())
        self.assertEqual(sorted(f.name for f in (out / "parts/p02").iterdir()),
                         ["S001_cam1_v1.jpg", "S002_cam1_v1.jpg", "S003_cam1_v1.jpg"])

    def test_mx11标签_任一点非ok则件为ng且shotLabels按点号数值排序(self):
        cases = [
            ({"2": "Ok", "10": "Ok", "9": "Ok"}, "ok", {"S001": "ok", "S002": "ok", "S003": "ok"}),
            ({"2": "Ok", "10": "Breaking", "9": "Ok"}, "ng", {"S001": "ok", "S002": "ok", "S003": "breaking"}),
        ]
        for i, (labels, expected_label, expected_shot_labels) in enumerate(cases):
            with self.subTest(expected_label=expected_label):
                src = self.make_mx11({"2032720000": labels}, f"mx11-{i}")
                out = self.root / f"out{i}"
                self.assertEqual(run("mx11", str(src), str(out)), 0)
                doc = load(out)
                part = doc["parts"][0]

                self.assertEqual([s["origin"] for s in doc["shots"]], ["MX11_1_2", "MX11_1_9", "MX11_1_10"])
                self.assertEqual((part["sn"], part["label"]), ("2032720000", expected_label))
                self.assertEqual(part["shotLabels"], expected_shot_labels)
                self.assertEqual(len(list((out / "parts/p01").iterdir())), 3)

    def test_shots子集_从S001重新编号并只按所选点汇总标签(self):
        src = self.make_mx11({"2032720000": {"2": "Ok", "3": "Breaking", "5": "Ok", "7": "Ok"}})
        out = self.root / "out"
        self.assertEqual(run("mx11", str(src), str(out), "--shots", "1,3-4"), 0)
        doc = load(out)
        part = doc["parts"][0]

        self.assertEqual([(s["shotId"], s["origin"]) for s in doc["shots"]],
                         [("S001", "MX11_1_2"), ("S002", "MX11_1_5"), ("S003", "MX11_1_7")])
        self.assertEqual((part["label"], part["shotLabels"]), ("ok", {"S001": "ok", "S002": "ok", "S003": "ok"}))
        self.assertIn(b"_MX11_1_5_Ok", (out / "parts/p01/S002_cam1_v1.jpg").read_bytes())

    def test_validate_缺图或未知schemaVersion时报告并按严格程度返回退出码(self):
        def drop_image(out: Path):
            (out / "parts/p01/S002_cam1_v1.jpg").unlink()

        def bump_version(out: Path):
            doc = load(out)
            doc["schemaVersion"] = 2
            (out / "sample-set.json").write_text(json.dumps(doc), encoding="utf-8")

        cases = [
            ("缺图", drop_image, False, ["parts/p01/S002_cam1_v1"], 0, 2),
            ("未知版本", bump_version, True, [], 1, 1),
        ]
        src = self.make_isv()
        for i, (name, mutate, has_errors, expected_missing, code, strict_code) in enumerate(cases):
            with self.subTest(name):
                out = self.root / f"out{i}"
                self.assertEqual(run("isv", str(src), str(out)), 0)
                mutate(out)
                errors, missing = convert.validate_set(out)

                self.assertEqual(bool(errors), has_errors)
                self.assertEqual(missing, expected_missing)
                self.assertEqual(run("validate", str(out)), code)
                self.assertEqual(run("validate", str(out), "--strict"), strict_code)


if __name__ == "__main__":
    unittest.main()
