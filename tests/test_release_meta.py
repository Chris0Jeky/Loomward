"""scripts/release_meta.py: nightly versions, notes rendering and the updater manifest."""

import importlib.util
import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location("release_meta", ROOT / "scripts" / "release_meta.py")
meta = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(meta)


class NightlyVersion(unittest.TestCase):
    def test_shape_and_order(self):
        self.assertEqual(meta.nightly_version("0.3.0", "20261012", 7), "0.3.0-nightly.20261012.7")
        key = lambda v: [int(x) for x in v.split("-nightly.")[1].split(".")]
        a, b, c = (meta.nightly_version("0.3.0", d, r) for d, r in [("20261012", 9), ("20261012", 10), ("20261013", 1)])
        self.assertLess(key(a), key(b))
        self.assertLess(key(b), key(c))

    def test_rejects_bad_input(self):
        for core, date, run in [("0.3.0-preview.1", "20261012", 1), ("0.3", "20261012", 1),
                                ("0.3.0", "2026-10-12", 1), ("0.3.0", "20261332", 1), ("0.3.0", "20261012", 0)]:
            with self.assertRaises(ValueError, msg=(core, date, run)):
                meta.nightly_version(core, date, run)

    def test_app_version_is_a_plain_core(self):
        self.assertRegex(meta.app_version(), meta.CORE)


class Notes(unittest.TestCase):
    def test_nightly_banner_comes_first_and_every_placeholder_is_filled(self):
        rel = ROOT / ".github" / "release"
        out = meta.render_notes((rel / "preview-notes.md").read_text(encoding="utf-8"), "0.3.0-nightly.20261012.7",
                                "abc123", "abc123", (rel / "nightly-banner.md").read_text(encoding="utf-8"))
        self.assertTrue(out.startswith("> [!WARNING]\n> **NIGHTLY: an untested build.**"))
        self.assertNotIn("{{", out)
        self.assertIn("0.3.0-nightly.20261012.7", out)
        self.assertIn("blob/abc123/", out)

    def test_preview_has_no_banner(self):
        self.assertNotIn("NIGHTLY", meta.render_notes("v{{VERSION}} {{TAG}} {{SHA}}", "1", "t", "s", None))


class Manifest(unittest.TestCase):
    def test_latest_json_and_retag(self):
        m = meta.latest_json("0.3.0-nightly.20261012.7", "nightly", "x/Loomward_0.3.0-nightly.20261012.7_x64-setup.exe",
                             "c2ln\n", "notes", "2026-10-12T00:00:00Z")
        self.assertEqual(set(m["platforms"]), {"windows-x86_64-nsis", "windows-x86_64"})
        entry = m["platforms"]["windows-x86_64"]
        self.assertEqual(entry["signature"], "c2ln")
        self.assertEqual(entry["url"], "https://github.com/Chris0Jeky/Loomward/releases/download/nightly/"
                                       "Loomward_0.3.0-nightly.20261012.7_x64-setup.exe")
        prev = meta.retag(m, "nightly-previous")
        self.assertIn("/download/nightly-previous/Loomward_", prev["platforms"]["windows-x86_64-nsis"]["url"])
        self.assertIn("/download/nightly/", m["platforms"]["windows-x86_64"]["url"], "retag copies")
        json.dumps(prev)


if __name__ == "__main__":
    unittest.main()
