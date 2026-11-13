"""Worktree cleanup preserves ignored lane state."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "scripts/lanes/rmwt.sh"
BASH = shutil.which("bash")
GIT = shutil.which("git")


@unittest.skipUnless(BASH and GIT, "bash and git required")
class RemoveWorktreeTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.home = self.root / "lane home"
        self.lane = "example"
        self.worktree = self.home / "wt" / self.lane
        self.script = self.repo / "scripts/lanes/rmwt.sh"
        self.script.parent.mkdir(parents=True)
        # Git Bash requires LF even if the source checkout uses CRLF.
        self.script.write_bytes(SCRIPT.read_bytes().replace(b"\r\n", b"\n"))
        (self.repo / ".gitignore").write_text(
            ".loomward/\ntarget/\napp/node_modules/\napp/dist/\n", encoding="utf-8")
        self.git("init")
        self.git("add", ".")
        self.git("-c", "user.name=Lane Test", "-c", "user.email=lane@example.invalid",
                 "-c", "commit.gpgsign=false", "commit", "-m", "Add cleanup fixture")
        self.worktree.parent.mkdir(parents=True)
        self.git("worktree", "add", "--detach", self.worktree.as_posix(), "HEAD")
        self.state = self.worktree / ".loomward"
        self.state.mkdir()

    def git(self, *arguments):
        return subprocess.run(
            [GIT, "-C", str(self.repo), *arguments], cwd=self.root,
            capture_output=True, text=True, check=True)

    def remove_worktree(self, **environment):
        return subprocess.run(
            [BASH, self.script.as_posix(), self.lane], cwd=self.root,
            env={**os.environ, "LW_HOME": self.home.as_posix(), **environment},
            capture_output=True, text=True)

    def test_nonempty_state_is_kept_before_worktree_removal(self):
        record = self.state / ".root-record"
        record.write_text("synthetic stress-test root record\n", encoding="utf-8")

        result = self.remove_worktree()

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(self.worktree.exists())
        kept = list((self.home / "kept" / self.lane).glob(".loomward-*"))
        self.assertEqual(len(kept), 1)
        self.assertRegex(kept[0].name, r"^\.loomward-\d{8}T\d{6}Z$")
        self.assertEqual((kept[0] / record.name).read_text(encoding="utf-8"),
                         "synthetic stress-test root record\n")
        self.assertIn(kept[0].as_posix(), result.stdout)

    def test_empty_state_does_not_create_kept_directory(self):
        result = self.remove_worktree()

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(self.worktree.exists())
        self.assertFalse((self.home / "kept").exists())

    def test_preservation_failure_leaves_state_and_build_outputs(self):
        record = self.state / "record"
        record.write_text("keep this state", encoding="utf-8")
        outputs = [self.worktree / path for path in ("target", "app/node_modules", "app/dist")]
        for output in outputs:
            output.mkdir(parents=True)
            (output / "artifact").write_text("keep on failure", encoding="utf-8")
        # A file blocks creation of the kept-state parent on every platform.
        (self.home / "kept").write_text("blocked", encoding="utf-8")

        result = self.remove_worktree()

        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(record.read_text(encoding="utf-8"), "keep this state")
        self.assertTrue((self.worktree / ".git").is_file())
        for output in outputs:
            self.assertEqual((output / "artifact").read_text(encoding="utf-8"),
                             "keep on failure")

    def test_move_failure_aborts_before_any_removal(self):
        record = self.state / "record"
        record.write_text("keep this state", encoding="utf-8")
        outputs = [self.worktree / path for path in ("target", "app/node_modules", "app/dist")]
        for output in outputs:
            output.mkdir(parents=True)
            (output / "artifact").write_text("keep on failure", encoding="utf-8")
        startup = self.root / "fail-move.sh"
        startup.write_bytes(b"mv() { return 42; }\n")

        result = self.remove_worktree(BASH_ENV=startup.as_posix())

        self.assertEqual(result.returncode, 42, result.stdout + result.stderr)
        self.assertEqual(record.read_text(encoding="utf-8"), "keep this state")
        self.assertTrue((self.worktree / ".git").is_file())
        for output in outputs:
            self.assertEqual((output / "artifact").read_text(encoding="utf-8"),
                             "keep on failure")


if __name__ == "__main__":
    unittest.main()
