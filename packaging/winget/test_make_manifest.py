"""py -3 packaging/winget/test_make_manifest.py : the generator's output, its hash lookup and its refusals."""
import tempfile
import unittest
from pathlib import Path

import make_manifest as mm

H1, H2 = "ab" * 32, "CD" * 32
SUMS = f"{H1}  Loomward_0.3.0-preview.2_x64-setup.exe\n{H2} *Loomward_0.3.0-preview.1_x64-setup.exe\n"


class Generator(unittest.TestCase):
    def run_gen(self, *args):
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        mm.main([*args, "--out", d.name])
        return Path(d.name) / "manifests/c/Chris0Jeky/Loomward"

    def test_three_files_with_the_release_hash_in_the_right_place(self):
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        sums = Path(d.name, "SHA256SUMS.txt")
        sums.write_text(SUMS)
        root = self.run_gen("0.3.0-preview.1", "--sums-file", str(sums))
        folder = root / "0.3.0-preview.1"
        self.assertEqual(
            sorted(p.name for p in folder.iterdir()),
            ["Chris0Jeky.Loomward.installer.yaml", "Chris0Jeky.Loomward.locale.en-US.yaml", "Chris0Jeky.Loomward.yaml"],
        )
        inst = (folder / "Chris0Jeky.Loomward.installer.yaml").read_text()
        self.assertIn(f"InstallerSha256: {H2.upper()}\n", inst)  # the .1 line, not the .2 line
        self.assertNotIn(H1.upper(), inst)
        self.assertIn("releases/download/v0.3.0-preview.1/Loomward_0.3.0-preview.1_x64-setup.exe\n", inst)
        for line in ("InstallerType: nullsoft", "Scope: user", "  Silent: /S", "ManifestVersion: 1.12.0"):
            self.assertIn(line, inst)
        loc = (folder / "Chris0Jeky.Loomward.locale.en-US.yaml").read_text()
        for line in ("License: AGPL-3.0-only", "ReleaseNotesUrl: https://github.com/Chris0Jeky/Loomward/releases/tag/v0.3.0-preview.1"):
            self.assertIn(line, loc)

    def test_explicit_hash_and_stable_version(self):
        root = self.run_gen("0.4.0", "--sha256", H1, "--release-date", "2026-11-01")
        inst = (root / "0.4.0/Chris0Jeky.Loomward.installer.yaml").read_text()
        self.assertIn("ReleaseDate: 2026-11-01\n", inst)
        self.assertIn(f"InstallerSha256: {H1.upper()}\n", inst)

    def test_refusals(self):
        for bad in (["v0.3.0", "--sha256", H1], ["0.3", "--sha256", H1], ["0.3.0", "--sha256", "nothex"],
                    ["0.3.0", "--sha256", H1, "--release-date", "yesterday"]):
            with self.subTest(bad=bad), self.assertRaises(SystemExit):
                self.run_gen(*bad)
        with self.assertRaises(SystemExit):  # a sums file without this version's installer
            mm.sha_from_sums(SUMS, "Loomward_9.9.9_x64-setup.exe")


if __name__ == "__main__":
    unittest.main()
