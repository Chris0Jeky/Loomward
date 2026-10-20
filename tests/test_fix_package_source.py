import importlib.util
import contextlib
import hashlib
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
            names_before = sorted(p.name for p in root.iterdir())
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
                                 (hashlib.sha256(b"tracked").hexdigest() + "  a.txt\n").encode())
            self.assertEqual(sorted(p.name for p in root.iterdir()), names_before)
            self.assertFalse((root / "MANIFEST.sha256").exists())
            self.assertEqual(package_source.source_files(root), [root / "a.txt"])

    def test_untracked_root_manifest_is_refused_without_modification(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t) / "repo"
            root.mkdir()
            self.init_repo(root)
            manifest = root / "MANIFEST.sha256"
            original = b"uncommitted notes, not generated output\n"
            manifest.write_bytes(original)
            output = Path(t) / "source.zip"
            with patch.object(package_source, "ROOT", root), \
                 patch("sys.argv", ["package_source.py", "--output", str(output)]), \
                 contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaisesRegex(SystemExit, "Untracked source files"):
                    package_source.main()
            self.assertEqual(manifest.read_bytes(), original)
            self.assertFalse(output.exists())

    def test_existing_source_manifest_is_preserved_and_archive_is_regenerated(self):
        for tracked in (True, False):
            with self.subTest(tracked=tracked), tempfile.TemporaryDirectory() as t:
                root = Path(t) / "repo"
                root.mkdir()
                self.init_repo(root)
                manifest = root / "MANIFEST.sha256"
                manifest.write_bytes(b"old manifest\n")
                if tracked:
                    subprocess.run(["git", "add", "MANIFEST.sha256"], cwd=root,
                                   check=True, capture_output=True)
                else:
                    (root / ".git" / "info" / "exclude").write_text("/MANIFEST.sha256\n")
                original = b"uncommitted manifest edits\n"
                manifest.write_bytes(original)
                output = Path(t) / "source.zip"
                with patch.object(package_source, "ROOT", root), \
                     patch("sys.argv", ["package_source.py", "--output", str(output)]), \
                     contextlib.redirect_stdout(io.StringIO()):
                    package_source.main()
                self.assertEqual(manifest.read_bytes(), original)
                with zipfile.ZipFile(output) as archive:
                    self.assertEqual(archive.namelist(), ["loomward/MANIFEST.sha256", "loomward/a.txt"])
                    self.assertEqual(archive.read("loomward/MANIFEST.sha256"),
                                     (hashlib.sha256(b"tracked").hexdigest() + "  a.txt\n").encode())

    def test_archive_entries_have_portable_posix_order(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t) / "repo"
            root.mkdir()
            self.init_repo(root)
            (root / "Z.txt").write_bytes(b"upper")
            (root / "a").mkdir()
            (root / "a" / "child.txt").write_bytes(b"nested")
            subprocess.run(["git", "add", "Z.txt", "a/child.txt"], cwd=root,
                           check=True, capture_output=True)
            output = Path(t) / "source.zip"
            with patch.object(package_source, "ROOT", root), \
                 patch("sys.argv", ["package_source.py", "--output", str(output)]), \
                 contextlib.redirect_stdout(io.StringIO()):
                package_source.main()
            with zipfile.ZipFile(output) as archive:
                self.assertEqual(archive.namelist(), ["loomward/MANIFEST.sha256", "loomward/Z.txt",
                                                     "loomward/a.txt", "loomward/a/child.txt"])

    def test_existing_output_is_refused_without_modification(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t) / "repo"
            root.mkdir()
            self.init_repo(root)
            output = Path(t) / "source.zip"
            original = b"preserve an existing artifact\n"
            output.write_bytes(original)
            with patch.object(package_source, "ROOT", root), \
                 patch("sys.argv", ["package_source.py", "--output", str(output)]):
                with self.assertRaisesRegex(SystemExit, "Refusing to overwrite"):
                    package_source.main()
            self.assertEqual(output.read_bytes(), original)
            self.assertFalse((root / "MANIFEST.sha256").exists())

    def test_output_inside_source_tree_is_refused(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)
            self.init_repo(root)
            output = root / "source.zip"
            with patch.object(package_source, "ROOT", root), \
                 patch("sys.argv", ["package_source.py", "--output", str(output)]):
                with self.assertRaisesRegex(SystemExit, "outside the source tree"):
                    package_source.main()
            self.assertFalse(output.exists())
            self.assertFalse((root / "MANIFEST.sha256").exists())

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
