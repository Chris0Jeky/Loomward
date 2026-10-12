"""Offline package-generator regressions, included in normal unittest discovery."""
import contextlib
import io
import runpy
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
GENERATOR = runpy.run_path(str(ROOT / "packaging/winget/make_manifest.py"))
HASH = "AB" * 32
INSTALLER = "Loomward_0.3.0-preview.1_x64-setup.exe"


class PackageChannels(unittest.TestCase):
    def test_original_generator_suite_is_part_of_discovery(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "packaging/winget/test_make_manifest.py")],
            cwd=ROOT, capture_output=True, text=True, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def assert_cli_refuses_without_output_or_network(self, *arguments):
        with tempfile.TemporaryDirectory() as directory:
            with patch("urllib.request.urlopen") as network:
                with contextlib.redirect_stdout(io.StringIO()):
                    with self.assertRaises(SystemExit):
                        GENERATOR["main"]([*arguments, "--out", directory])
                network.assert_not_called()
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_invalid_calendar_dates_are_refused_before_download_or_output(self):
        for date in ("2026-02-29", "2026-02-30", "2026-13-01", "0000-01-01"):
            with self.subTest(date=date):
                self.assert_cli_refuses_without_output_or_network(
                    "0.3.0-preview.1", "--release-date", date,
                )

    def test_non_ascii_version_digits_are_refused_before_download_or_output(self):
        for version in ("٠.٣.٠", "0.3.１", "0.٣.0-preview.1"):
            with self.subTest(version=version):
                self.assert_cli_refuses_without_output_or_network(version)

    def test_ambiguous_target_checksums_are_refused(self):
        for second_hash in (HASH, "CD" * 32):
            with self.subTest(second_hash=second_hash):
                text = f"{HASH}  {INSTALLER}\n{second_hash} *{INSTALLER}\n"
                with self.assertRaises(SystemExit):
                    GENERATOR["sha_from_sums"](text, INSTALLER)

    def test_one_target_checksum_accepts_text_and_binary_forms(self):
        for separator in ("  ", " *"):
            with self.subTest(separator=separator):
                text = f"{'CD' * 32}  Other_installer.exe\n{HASH.lower()}{separator}{INSTALLER}\n"
                self.assertEqual(GENERATOR["sha_from_sums"](text, INSTALLER), HASH)

    def test_leap_day_remains_supported_offline(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch("urllib.request.urlopen") as network:
                with contextlib.redirect_stdout(io.StringIO()):
                    GENERATOR["main"]([
                        "0.3.0-preview.1", "--release-date", "2028-02-29",
                        "--sha256", HASH, "--out", directory,
                    ])
                network.assert_not_called()
            files = list(Path(directory).rglob("*.yaml"))
            self.assertEqual(len(files), 3)
            installer = next(path for path in files if path.name.endswith(".installer.yaml"))
            self.assertIn("ReleaseDate: 2028-02-29\n", installer.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
