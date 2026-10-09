import importlib.util
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

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

    def test_untracked_and_ignored_files_excluded(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)
            self.init_repo(root)
            (root / "Resources").mkdir()
            (root / "Resources" / "secret.txt").write_text("private")
            (root / ".gitignore").write_text("ignored.txt\n")
            (root / "ignored.txt").write_text("ignored")
            self.assertEqual(package_source.source_files(root), [root / "a.txt"])

    def test_non_git_dir_raises(self):
        with tempfile.TemporaryDirectory() as t:
            with self.assertRaises(SystemExit):
                package_source.source_files(Path(t))
