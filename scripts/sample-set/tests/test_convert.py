import contextlib
import io
import json
import shutil
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


def fake_png(width: int, height: int) -> bytes:
    ihdr = width.to_bytes(4, "big") + height.to_bytes(4, "big") + b"\x08\x00\x00\x00\x00"
    return b"\x89PNG\r\n\x1a\n" + len(ihdr).to_bytes(4, "big") + b"IHDR" + ihdr + b"\x00" * 4


def fake_bmp(width: int, height: int) -> bytes:
    dib = (40).to_bytes(4, "little") + width.to_bytes(4, "little") + (-height).to_bytes(4, "little", signed=True)
    return b"BM" + b"\x00" * 12 + dib + b"\x00" * 28


def fake_tiff(width: int, height: int) -> bytes:
    entries = (256).to_bytes(2, "big") + (3).to_bytes(2, "big") + (1).to_bytes(4, "big") + width.to_bytes(2, "big") + b"\x00\x00"
    entries += (257).to_bytes(2, "big") + (4).to_bytes(2, "big") + (1).to_bytes(4, "big") + height.to_bytes(4, "big")
    return b"MM\x00*" + (8).to_bytes(4, "big") + (2).to_bytes(2, "big") + entries + b"\x00" * 4


def run(*argv: str) -> int:
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        return convert.main(list(argv))


def load(out: Path) -> dict:
    return json.loads((out / "sample-set.json").read_text(encoding="utf-8"))


def save(out: Path, doc: dict) -> None:
    (out / "sample-set.json").write_text(json.dumps(doc, ensure_ascii=False), encoding="utf-8")


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

    def test_单圈点位相机图_点位按数值排序且缺图时各相机第k个拍照点仍对应点位k(self):
        src = self.root / "round"
        src.mkdir()
        for point in (1, 2, 10):
            for cam in (1, 2):
                if (point, cam) != (2, 2):
                    (src / f"{point}_{cam}.bmp").write_bytes(fake_bmp(1280, 1024))
        out = self.root / "out"
        self.assertEqual(run("round", str(src), str(out)), 0)
        doc = load(out)
        per_camera = {c["id"]: [s["origin"] for s in doc["shots"] if s["cameraId"] == c["id"]] for c in doc["cameras"]}

        self.assertEqual(per_camera, {"cam1": ["1_1.bmp", "2_1.bmp", "10_1.bmp"], "cam2": ["1_2.bmp", "2_2", "10_2.bmp"]})
        self.assertEqual(len(doc["parts"]), 1)
        _, missing = convert.validate_set(out)
        self.assertEqual(missing, ["parts/p01/S004_cam2_v1"])

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

    def test_像素当量参数_全局写入mmPerPx且逐点只写入指定拍照点(self):
        out = self.root / "out"
        self.assertEqual(run("isv", str(self.make_isv()), str(out), "--mm-per-px", "0.1",
                             "--shot-mm-per-px", "S002=0.085"), 0)
        doc = load(out)

        self.assertEqual(doc["mmPerPx"], 0.1)
        self.assertEqual([s.get("mmPerPx") for s in doc["shots"]], [None, 0.085, None])

    def test_像素当量非正非有限或shotId不在清单_拒绝且不生成输出(self):
        cases = [
            ("全局非正", "--mm-per-px", "0"),
            ("逐点非有限", "--shot-mm-per-px", "S001=nan"),
            ("未知shotId", "--shot-mm-per-px", "S004=0.1"),
        ]
        src = self.make_isv()
        for i, (name, flag, value) in enumerate(cases):
            with self.subTest(name):
                out = self.root / f"out{i}"
                self.assertEqual(run("isv", str(src), str(out), flag, value), 1)
                self.assertFalse(out.exists())

    def test_validate_缺图或未知schemaVersion时报告并按严格程度返回退出码(self):
        def drop_image(out: Path):
            (out / "parts/p01/S002_cam1_v1.jpg").unlink()

        def bump_version(out: Path):
            doc = load(out)
            doc["schemaVersion"] = 2
            (out / "sample-set.json").write_text(json.dumps(doc), encoding="utf-8")

        def bad_shot_mm(out: Path):
            doc = load(out)
            doc["shots"][1]["mmPerPx"] = 0
            (out / "sample-set.json").write_text(json.dumps(doc), encoding="utf-8")

        cases = [
            ("缺图", drop_image, False, ["parts/p01/S002_cam1_v1"], 0, 2),
            ("未知版本", bump_version, True, [], 1, 1),
            ("逐点当量非正", bad_shot_mm, True, [], 1, 1),
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

    def convert_isv(self, name: str) -> Path:
        if not (self.root / "isv").exists():
            self.make_isv()
        out = self.root / name
        self.assertEqual(run("isv", str(self.root / "isv"), str(out)), 0)
        return out

    def test_拍照点视角_不等于相机视角时格式错误(self):
        for shot_views, has_errors in (([1], True), ([1, 2], False)):
            with self.subTest(shot_views=shot_views):
                out = self.convert_isv(f"out{len(shot_views)}")
                doc = load(out)
                doc["cameras"][0]["views"] = [1, 2]
                for shot in doc["shots"]:
                    shot["views"] = shot_views
                save(out, doc)

                self.assertEqual(bool(convert.validate_set(out)[0]), has_errors)

    def test_样本图尺寸_各格式与imageSize一致才通过(self):
        cases = [
            ("png", fake_png(1280, 960), False),
            ("bmp", fake_bmp(1280, 960), False),
            ("pgm", b"P5\n# gluesight\n1280 960\n255\n", False),
            ("tif", fake_tiff(1280, 960), False),
            ("jpg", fake_jpeg(640, 480, "small"), True),
            ("png", b"\x89PNG\r\n\x1a\n", True),
        ]
        for i, (ext, data, has_errors) in enumerate(cases):
            with self.subTest(ext=ext, has_errors=has_errors):
                out = self.convert_isv(f"out{i}")
                (out / "parts/p01/S002_cam1_v1.jpg").unlink()
                (out / f"parts/p01/S002_cam1_v1.{ext}").write_bytes(data)
                errors, missing = convert.validate_set(out)

                self.assertEqual(bool(errors), has_errors)
                self.assertEqual(missing, [])

    def test_相机编号partId与sn_按后端规则校验且下划线相机编号可匹配图像(self):
        def rename_camera(camera_id: str):
            def mutate(out: Path, doc: dict):
                doc["cameras"][0]["id"] = camera_id
                for shot in doc["shots"]:
                    shot["cameraId"] = camera_id
                for f in (out / "parts").glob("*/*_cam1_v1.jpg"):
                    f.rename(f.with_name(f.name.replace("_cam1_", f"_{camera_id}_")))
            return mutate

        def part_id_case_duplicate(out: Path, doc: dict):
            doc["parts"][1]["partId"] = "P01"
            shutil.rmtree(out / "parts/p02")

        def blank_sn(out: Path, doc: dict):
            doc["parts"][0]["sn"] = "  "

        cases = [
            ("相机编号含下划线", rename_camera("cam_1"), False),
            ("相机编号超过32字符", rename_camera("c" * 33), True),
            ("partId仅大小写不同", part_id_case_duplicate, True),
            ("sn全空白", blank_sn, True),
        ]
        for i, (name, mutate, has_errors) in enumerate(cases):
            with self.subTest(name):
                out = self.convert_isv(f"out{i}")
                doc = load(out)
                mutate(out, doc)
                save(out, doc)
                errors, missing = convert.validate_set(out)

                self.assertEqual(bool(errors), has_errors)
                if not has_errors:
                    self.assertEqual(missing, [])

    def test_force_只覆盖顶层仅含清单与parts且校验通过的样本集目录(self):
        def extra_entry(out: Path):
            (out / "notes.txt").write_text("keep", encoding="utf-8")
            return out / "notes.txt"

        def broken_manifest(out: Path):
            (out / "sample-set.json").write_text("{}", encoding="utf-8")
            return out / "parts/p01/S001_cam1_v1.jpg"

        def renamed_valid_set(out: Path):
            doc = load(out)
            doc["name"] = "旧样本集"
            save(out, doc)
            return None

        cases = [
            ("顶层多余条目", extra_entry, 1),
            ("清单格式错误", broken_manifest, 1),
            ("合法样本集", renamed_valid_set, 0),
        ]
        for i, (name, mutate, code) in enumerate(cases):
            with self.subTest(name):
                out = self.convert_isv(f"out{i}")
                kept = mutate(out)
                manifest_before = (out / "sample-set.json").read_bytes()

                self.assertEqual(run("isv", str(self.root / "isv"), str(out), "--force"), code)
                if code == 0:
                    self.assertNotEqual(load(out)["name"], "旧样本集")
                else:
                    self.assertTrue(kept.exists())
                    self.assertEqual((out / "sample-set.json").read_bytes(), manifest_before)


if __name__ == "__main__":
    unittest.main()
