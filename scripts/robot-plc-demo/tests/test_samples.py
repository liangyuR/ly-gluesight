"""Image-space sample export and provenance tests; no DLL or desktop app is used."""
import copy
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from simulator_config import load_config, load_recipe, trigger_plan

scripts = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("make_samples", scripts / "make-samples.py")
samples = importlib.util.module_from_spec(spec)
spec.loader.exec_module(samples)
spec = importlib.util.spec_from_file_location("run_regression", scripts / "run-regression.py")
regression = importlib.util.module_from_spec(spec)
spec.loader.exec_module(regression)


class SampleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.workspace = self.root / "workspace"
        (self.workspace / "images").mkdir(parents=True)
        self.recipe = load_recipe(load_config()["robot"]["recipe"])
        for shot in self.recipe["shots"]:
            shot["path"] = [[980, 480], [300, 480]]
        self.state = {"doc": self.recipe, "frames": []}
        for k, shot in enumerate(self.recipe["shots"]):
            views = []
            for view in (1, 2, 3):
                image = {"id": f"sample-{k}-v{view}", "view": view, "source": "Sim",
                         "size": [1280, 1024], "camera": shot["camera"]}
                pixels = bytearray([210]) * (1280 * 1024)
                if view == shot["view"]:
                    for y in range(463, 498):
                        pixels[y * 1280 + 300:y * 1280 + 981] = bytes([38]) * 681
                (self.workspace / "images" / f"{image['id']}.pgm").write_bytes(samples.pgm(1280, 1024, pixels))
                views.append(image)
            self.state["frames"].append({"k": k, "image": views[shot["view"] - 1], "views": views})
        self.save()

    def tearDown(self):
        self.temp.cleanup()

    def save(self):
        (self.workspace / "workspace.json").write_text(json.dumps(self.state), encoding="utf-8")

    def test_sample_export_matches_missing_or_zero_fixture_to_backend_version_one(self):
        self.state["doc"]["version"] = 1
        self.save()
        for name, version in [("missing", None), ("zero", 0)]:
            with self.subTest(version=name):
                fixture = copy.deepcopy(self.recipe)
                if version is None:
                    fixture.pop("version", None)
                else:
                    fixture["version"] = version
                output = self.root / ("version-" + name)
                report = samples.export_samples(self.workspace, output, fixture)
                self.assertEqual(len(report["frames"]), 12)
                self.assertEqual(len(list((output / "good").glob("*.pgm"))), 4)
                self.assertEqual(len(list((output / "replay").glob("*.pgm"))), 12)
    def test_tricam_exports_four_selected_images_and_twelve_views_with_device_counts_and_dimensions(self):
        output = self.root / "samples"
        report = samples.export_samples(self.workspace, output, self.recipe)
        self.assertEqual(len(list((output / "good").glob("*.pgm"))), 4)
        self.assertEqual(len(list((output / "gap").glob("*.pgm"))), 4)
        self.assertEqual(len(list((output / "replay").glob("*.pgm"))), 12)
        self.assertEqual(report["plannedDeviceTriggers"], {"cam1": 4})
        self.assertEqual([f["view"] for f in report["frames"] if f["selected"]], [1, 2, 3, 1])
        self.assertFalse(report["physicalValidation"])
        self.assertFalse(report["imageEngineValidated"])
        for name, metadata in report["files"].items():
            width, height, pixels = samples.read_pgm((output / name).read_bytes())
            self.assertEqual((width, height, len(pixels)), (1280, 1024, 1280 * 1024))
            self.assertEqual((output / name).stat().st_size, metadata["bytes"])
        self.assertEqual(report, json.loads((output / "provenance.json").read_text(encoding="utf-8")))

    def test_gap_is_cut_in_the_selected_pixel_path_and_keeps_other_shots_unchanged(self):
        output = self.root / "samples"
        report = samples.export_samples(self.workspace, output, self.recipe)
        info = report["gap"]
        self.assertEqual((info["k"], info["shotId"], info["view"]), (1, "P2", 2))
        self.assertGreater(info["lengthMm"], info["allowedMm"])
        width, _, pixels = samples.read_pgm((output / "gap/k2.pgm").read_bytes())
        center = samples.path_at(self.recipe["shots"][1]["path"],
                                 (info["startMm"] + info["lengthMm"] / 2) / self.recipe["shots"][1]["mmPerPx"])
        self.assertGreater(pixels[480 * width + round(center[0])], 120)
        self.assertEqual(pixels[480 * width + 940], 38)
        for k in (1, 3, 4):
            self.assertEqual((output / f"good/k{k}.pgm").read_bytes(), (output / f"gap/k{k}.pgm").read_bytes())

    def test_wrong_or_incomplete_frozen_identity_is_refused_before_writing_any_output(self):
        edits = [
            lambda s: s["frames"][0]["image"].update(view=2),
            lambda s: s["frames"][0]["image"].update(camera="cam2"),
            lambda s: s["frames"][0]["image"].update(source="Mvs"),
            lambda s: s["frames"][0]["image"].update(id="../outside"),
            lambda s: s["frames"][0]["image"].update(size=[64, 32]),
            lambda s: s["frames"][0].update(k=3),
            lambda s: s["frames"][0].update(image=None),
            lambda s: s["frames"][0].update(views=s["frames"][0]["views"][:1]),
            lambda s: s["frames"][0]["views"].append(copy.deepcopy(s["frames"][0]["views"][0])),
        ]
        original = copy.deepcopy(self.state)
        for edit in edits:
            self.state = copy.deepcopy(original)
            edit(self.state)
            self.save()
            with self.subTest(state=self.state["frames"][0]), self.assertRaises(ValueError):
                samples.export_samples(self.workspace, self.root / "rejected")
            self.assertFalse((self.root / "rejected").exists())

    def test_changed_fixture_and_old_rectangular_schema_are_refused(self):
        expected = copy.deepcopy(self.recipe)
        expected["spacing"] = 2
        with self.assertRaisesRegex(ValueError, "differs"):
            samples.export_samples(self.workspace, self.root / "rejected", expected)
        self.state["doc"]["schemaVersion"] = 3
        self.save()
        with self.assertRaisesRegex(ValueError, "schemaVersion"):
            samples.export_samples(self.workspace, self.root / "legacy")

    def test_pgm_parser_preserves_first_binary_whitespace_pixel_and_rejects_truncation(self):
        self.assertEqual(samples.read_pgm(b"P5\n# comment\n2 1\n255\n\x0a\x20"), (2, 1, bytearray([10, 32])))
        self.assertEqual(samples.read_pgm(b"P5\r\n2 1\r\n255\r\n\x00\xff"), (2, 1, bytearray([0, 255])))
        for invalid in (b"P5\n2 1\n255\n\x00", b"P2\n2 1\n255\n00", b"P5\n2 1\n65535\n\x00\x01"):
            with self.assertRaises(ValueError):
                samples.read_pgm(invalid)


class EvidenceTests(unittest.TestCase):
    def test_scripted_peer_is_not_accepted_as_a_real_image_engine_regression(self):
        recipe = load_recipe(load_config()["robot"]["recipe"])
        result = {"scenario": "normal", "deviceTriggers": {"cam1": 4}, "triggerAcks": []}
        with self.assertRaisesRegex(RuntimeError, "evidence"):
            regression.verify_evidence(result, recipe)
        plan, _ = trigger_plan(recipe)
        result.update(sn=123, cycleId="scripted-cycle", recipeRevision="scripted-recipe", bundleId="scripted-bundle")
        result["triggerAcks"] = [{"ok": True, "identity": {**point, "sn": 123, "recipeId": recipe["id"],
                                                        "productCode": recipe["productCode"], "shotCount": 4,
                                                        "cycleId": result["cycleId"], "recipeRevision": result["recipeRevision"],
                                                        "bundleId": result["bundleId"]},
                                  "evidence": {"scope": "scripted-test-peer"}} for point in plan]
        with self.assertRaisesRegex(RuntimeError, "actual demo app"):
            regression.verify_evidence(result, recipe)

    def test_count_mismatch_has_no_camera_pulses_or_fabricated_cycle_evidence(self):
        recipe = load_recipe(load_config()["robot"]["recipe"])
        result = {"scenario": "countMismatch", "deviceTriggers": {"cam1": 0}, "triggerAcks": []}
        self.assertEqual(regression.verify_evidence(result, recipe), {})
        result["deviceTriggers"]["cam1"] = 12
        with self.assertRaisesRegex(RuntimeError, "counts"):
            regression.verify_evidence(result, recipe)

class RegressionRecipeVersionTests(unittest.TestCase):
    def test_live_regression_preflight_matches_missing_or_zero_fixture_to_backend_version_one(self):
        backend = load_recipe(load_config()["robot"]["recipe"])
        backend["version"] = 1
        state = {"contractVersion": 2, "recipe": backend}
        for version in [None, 0, 1]:
            fixture = copy.deepcopy(backend)
            if version is None:
                fixture.pop("version", None)
            else:
                fixture["version"] = version
            regression.validate_running_recipe(state, fixture)
        fixture = copy.deepcopy(backend)
        fixture["version"] = 7
        with self.assertRaisesRegex(RuntimeError, "differs"):
            regression.validate_running_recipe(state, fixture)
        fixture["version"] = None
        with self.assertRaises(ValueError):
            regression.validate_running_recipe(state, fixture)


if __name__ == "__main__":
    unittest.main()
