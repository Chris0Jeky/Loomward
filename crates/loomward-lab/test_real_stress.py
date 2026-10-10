"""Child entry-point guards; only disposable synthetic roots may be scanned."""
import contextlib
import io
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import real_stress

sys.path.insert(0, str(real_stress.REPO / "python"))


class PythonChildGuards(unittest.TestCase):
    def invoke(self, root, log):
        with patch.object(sys, "argv", ["real_stress.py", "--python-child", str(root),
                                      "--label", "synthetic", "--root-log", str(log)]), \
                patch.object(real_stress, "require_non_elevated"):
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
        with tempfile.TemporaryDirectory(dir=r"G:\loomward-lab\scale") as temp:
            root = Path(temp) / "scanroot"
            root.mkdir()
            for log in [root / "scope.log", Path(str(root).upper()) / "scope.log",
                        root.parent / "scanroot" / ".." / "scanroot" / "scope.log",
                        Path("\\\\?\\" + str(root)) / "scope.log"]:
                with self.subTest(log=log):
                    self.assert_refused(root, log)

    def test_child_refuses_relative_log_and_unresolved_parent(self):
        with tempfile.TemporaryDirectory(dir=r"G:\loomward-lab\scale") as temp:
            root = Path(temp) / "scanroot"
            root.mkdir()
            self.assert_refused(root, Path("relative-scope.log"))
            self.assert_refused(root, root.parent / "missing" / "scope.log")

    def test_child_accepts_external_log_and_scans_synthetic_root(self):
        with tempfile.TemporaryDirectory(dir=r"G:\loomward-lab\scale") as temp:
            root = Path(temp) / "scanroot"
            root.mkdir()
            (root / "synthetic.txt").write_text("fixture", encoding="utf-8")
            log = root.parent / "scope.log"
            output = io.StringIO()
            with contextlib.redirect_stdout(output), patch.object(real_stress, "peak_rss", return_value=0):
                self.invoke(root, log)
            self.assertIn('"files": 1', output.getvalue())
            self.assertIn(str(root), log.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
