import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts" / "color-eval"))
from mix_eval import evaluate
from mix_fixture import generate


class MixBayerTests(unittest.TestCase):
    def test_highlight_in_yellow_stripe_preserves_gap_without_spurious_blue(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            spec_path = generate(root / "recordings")
            spec = json.loads(spec_path.read_text(encoding="utf-8"))
            spec["cases"] = [case for case in spec["cases"] if "holdout-highlight/" in case["part"]]
            spec_path.write_text(json.dumps(spec), encoding="utf-8")
            evaluate(spec_path, root / "result")
            observation = root / "result" / "frame-0001" / "observation.json"
            regions = json.loads(observation.read_text(encoding="utf-8"))["regions"]
            self.assertEqual([region["direction"] for region in regions], ["yellow"])
            self.assertGreater(regions[0]["confirmedLengthPx"], 0)
            self.assertLess(regions[0]["confirmedLengthPx"], regions[0]["spanPx"])


if __name__ == "__main__":
    unittest.main()
