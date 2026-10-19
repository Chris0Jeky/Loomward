import importlib.util
import contextlib
import io
import os
import shutil
import subprocess
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location(
    "package_source", ROOT / "scripts" / "package_source.py")
package_source = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(package_source)


def git_available():
    return shutil.which("git") is not None


@unittest.skipUnless(git_available(), "git is not on PATH")
class PackageSourceTests(unittest.TestCase):
    def init_repo(self, root):
        runs = [
            ["git", "init"],
            ["git", "config", "user.email", "test@example.com"],
            ["git", "config", "user.name", "Test"],
            ["git", "config", "commit.gpgsign", "false"],
        ]
        for args in runs:
            subprocess.run(args, cwd=root, check=True, capture_output=True)
        (root / "a.txt").write_text("tracked")
        subprocess.run(["git", "add", "a.txt"], cwd=root,
                       check=True, capture_output=True)
        subprocess.run(["git", "commit", "-m", "init"], cwd=root,
                       check=True, capture_output=True)

    def test_ignored_files_excluded(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)
            self.init_repo(root)
            (root / "Resources").mkdir()
            (root / "Resources" / "secret.txt").write_text("private")
            (root / ".git" / "info" / "exclude").write_text("/Resources\nignored.txt\n")
            (root / "ignored.txt").write_text("ignored")
            self.assertEqual(package_source.source_files(root), [root / "a.txt"])

    def test_untracked_unignored_file_is_refused_not_dropped(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)
            self.init_repo(root)
            (root / "new_module.py").write_text("x = 1\n")
            with self.assertRaises(SystemExit):
                package_source.source_files(root)

    def test_subdirectory_of_a_checkout_is_refused(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)
            self.init_repo(root)
            (root / "sub").mkdir()
            with self.assertRaises(SystemExit):
                package_source.source_files(root / "sub")

    def test_non_git_dir_raises(self):
        with tempfile.TemporaryDirectory() as t:
            with self.assertRaises(SystemExit):
                package_source.source_files(Path(t))

    def test_generated_manifest_allows_repeated_source_packages(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t) / "repo"
            root.mkdir()
            self.init_repo(root)
            outputs = [Path(t) / "first.zip", Path(t) / "second.zip"]
            for output in outputs:
                with patch.object(package_source, "ROOT", root), \
                     patch("sys.argv", ["package_source.py", "--output", str(output)]), \
                     contextlib.redirect_stdout(io.StringIO()):
                    package_source.main()
            self.assertEqual(outputs[0].read_bytes(), outputs[1].read_bytes())
            with zipfile.ZipFile(outputs[1]) as archive:
                self.assertEqual(archive.namelist(), ["loomward/MANIFEST.sha256", "loomward/a.txt"])
                self.assertEqual(archive.read("loomward/a.txt"), b"tracked")
                self.assertEqual(archive.read("loomward/MANIFEST.sha256"),
                                 (root / "MANIFEST.sha256").read_bytes())
            self.assertEqual(package_source.source_files(root), [root / "a.txt"])

    def test_manifest_exception_does_not_hide_other_untracked_files(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)
            self.init_repo(root)
            (root / "MANIFEST.sha256").write_text("old generated manifest\n")
            (root / "new_module.py").write_text("x = 1\n")
            with self.assertRaisesRegex(SystemExit, "Untracked source files"):
                package_source.source_files(root)

    def test_nested_untracked_manifest_remains_refused(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)
            self.init_repo(root)
            (root / "sub").mkdir()
            (root / "sub" / "MANIFEST.sha256").write_text("not the generated root manifest\n")
            with self.assertRaisesRegex(SystemExit, "Untracked source files"):
                package_source.source_files(root)

    def test_untracked_manifest_links_remain_refused(self):
        for kind in ("hard", "symbolic"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as t:
                root = Path(t) / "repo"
                root.mkdir()
                self.init_repo(root)
                outside = Path(t) / "outside.txt"
                outside.write_text("preserve this fixture\n")
                manifest = root / "MANIFEST.sha256"
                if kind == "hard":
                    os.link(outside, manifest)
                else:
                    try:
                        manifest.symlink_to(outside)
                    except OSError as exc:
                        if os.name != "nt" or getattr(exc, "winerror", None) != 1314:
                            raise
                        # Exercise the rejection decision even on hosts without symlink privilege.
                        manifest.write_text("synthetic symlink decision\n")
                        with patch.object(Path, "is_symlink", return_value=True):
                            with self.assertRaisesRegex(SystemExit, "Untracked source files"):
                                package_source.source_files(root)
                        continue
                with self.assertRaisesRegex(SystemExit, "Untracked source files"):
                    package_source.source_files(root)
                self.assertEqual(outside.read_text(), "preserve this fixture\n")
