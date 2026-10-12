"""The current design roadmap must agree with the manifest's shared-search gate."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class FindAskRoadmap(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = (ROOT / "docs/46-find-and-ask.md").read_text(encoding="utf-8")

    def test_common_query_surface_requires_the_adapter_not_only_grants(self):
        surface = self.text.split("## 4. One query surface", 1)[1].split("### 4.1", 1)[0]
        self.assertIn("LW-069", surface)
        self.assertIn("LW-067", surface)

    def test_roadmap_names_the_adapter_and_consumers(self):
        roadmap = self.text.split("## 10. Roadmap", 1)[1].split("## 11.", 1)[0]
        rows = [line.split("|") for line in roadmap.splitlines() if line.startswith("| F")]
        by_lane = {row[1].strip(): row for row in rows}
        self.assertTrue(any("(LW-069)" in row[2] and "LW-113" in row[3]
                            and "LW-067" in row[3] for row in rows))
        for lane in ("F4", "F5", "F12"):
            with self.subTest(lane=lane):
                self.assertIn("LW-069", by_lane[lane][3])
        self.assertIn("LW-066", by_lane["F12"][2])


if __name__ == "__main__":
    unittest.main()
