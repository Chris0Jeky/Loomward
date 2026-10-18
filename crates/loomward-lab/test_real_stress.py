"""Child entry-point guards; only disposable synthetic roots may be scanned."""
import contextlib
import io
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import real_stress

sys.path.insert(0, str(real_stress.REPO / "python"))


class PythonChildGuards(unittest.TestCase):
    def setUp(self):
        if os.name != "nt":
            self.skipTest("Windows-only child scope and log guards")
        try:
            real_stress.require_non_elevated()
        except RuntimeError as error:
            self.skipTest(str(error))

    def fixture(self):
        # The checkout and default temp may both sit under a refused user profile.
        return tempfile.TemporaryDirectory(prefix="loomward-real-stress-",
                                           dir=Path(real_stress.REPO.anchor))

    def test_child_refuses_hard_link_log_without_writing_or_scanning(self):
        with tempfile.TemporaryDirectory() as checkout:
            protected_repo = Path(checkout) / "Users" / "fixture" / "checkout"
            for repo in (real_stress.REPO, protected_repo):
                with self.subTest(repo=repo), patch.object(real_stress, "REPO", repo):
                    self.assert_hard_link_refused()

    def assert_hard_link_refused(self):
        with self.fixture() as temp:
            root = Path(temp) / "scanroot"
            root.mkdir()
            data = root / "data.bin"
            data.write_bytes(b"synthetic fixture")
            log = root.parent / "scope.log"
            os.link(data, log)
            with patch("loomward.inventory.scan", side_effect=AssertionError("scan reached")) as scan, \
                    patch.object(real_stress, "open_root_log", wraps=real_stress.open_root_log) as open_log:
                with self.assertRaisesRegex(ValueError, "ordinary single-link file"):
                    self.invoke(root, log)
                open_log.assert_called_once_with(log)
                scan.assert_not_called()
            self.assertEqual(data.read_bytes(), b"synthetic fixture")

    def invoke(self, root, log):
        with patch.object(sys, "argv", ["real_stress.py", "--python-child", str(root),
                                      "--label", "synthetic", "--root-log", str(log)]):
            real_stress.main()

    def assert_refused(self, root, log):
        with patch("loomward.inventory.scan", side_effect=AssertionError("scan reached")) as scan:
            with self.assertRaises(ValueError):
                self.invoke(root, log)
            scan.assert_not_called()
        self.assertFalse(log.exists(), "refusal must precede logging")

    def test_child_refuses_broad_protected_and_ambiguous_roots(self):
        with tempfile.TemporaryDirectory() as temp:
            log = Path(temp) / "scope.log"
            for root in ["C:\\", r"C:\Users\fixture", r"E:\Browser", r"E:\keys",
                         r"E:\fixtures\AppData", r"E:\fixtures\..\keys",
                         r"\\?\E:\fixtures", r"E:\fixtures:stream"]:
                with self.subTest(root=root):
                    self.assert_refused(root, log)

    def test_child_refuses_internal_log_before_write_or_scan(self):
        with self.fixture() as temp:
            root = Path(temp) / "scanroot"
            root.mkdir()
            for log in [root / "scope.log", Path(str(root).upper()) / "scope.log",
                        root.parent / "scanroot" / ".." / "scanroot" / "scope.log",
                        Path("\\\\?\\" + str(root)) / "scope.log"]:
                with self.subTest(log=log):
                    self.assert_refused(root, log)

    def test_child_refuses_relative_log_and_unresolved_parent(self):
        with self.fixture() as temp:
            root = Path(temp) / "scanroot"
            root.mkdir()
            self.assert_refused(root, Path("relative-scope.log"))
            self.assert_refused(root, root.parent / "missing" / "scope.log")

    def test_child_accepts_external_log_and_scans_synthetic_root(self):
        with self.fixture() as temp:
            root = Path(temp) / "scanroot"
            root.mkdir()
            (root / "synthetic.txt").write_text("fixture", encoding="utf-8")
            log = root.parent / "scope.log"
            output = io.StringIO()
            with contextlib.redirect_stdout(output), patch.object(real_stress, "peak_rss", return_value=0), \
                    patch.object(real_stress, "open_root_log", wraps=real_stress.open_root_log) as open_log:
                self.invoke(root, log)
                open_log.assert_called_once_with(log)
            self.assertIn('"files": 1', output.getvalue())
            self.assertIn(str(root), log.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
