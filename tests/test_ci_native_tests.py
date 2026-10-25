"""Exercise the CI reporter through real child stdout/stderr and exit codes."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
REPORTER = ROOT / "scripts/ci_native_tests.py"


class NativeCoverageReporting(unittest.TestCase):
    def report(self, source):
        with tempfile.TemporaryDirectory() as temp:
            summary = Path(temp) / "summary.md"
            summary.write_text("existing summary\n", encoding="utf-8")
            result = subprocess.run(
                [sys.executable, str(REPORTER), sys.executable, "-c", source],
                env=dict(os.environ, GITHUB_STEP_SUMMARY=str(summary)),
                capture_output=True, text=True, encoding="utf-8", check=False,
            )
            return result, summary.read_text(encoding="utf-8")

    def test_success_counts_markers_and_keeps_logs(self):
        result, summary = self.report(
            "import sys; print('native output'); "
            "print('SKIPPED (elevated): crate::first', file=sys.stderr); "
            "print('SKIPPED (elevated): crate::first'); "
            "print('SKIPPED (elevated): crate::second')"
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("native output", result.stdout)
        self.assertIn("SKIPPED (elevated): crate::first", result.stdout)
        self.assertTrue(summary.startswith("existing summary\n"))
        self.assertIn("Elevated-skip markers observed: **3**", summary)
        self.assertIn("repeated checks count separately", summary)
        self.assertIn("UNVERIFIED", summary)
        self.assertIn("Cargo exit status: 0", summary)

    def test_failure_status_survives_reporting(self):
        result, summary = self.report(
            "import sys; print('SKIPPED (elevated): crate::failed'); sys.exit(17)"
        )
        self.assertEqual(result.returncode, 17, result.stderr)
        self.assertIn("Cargo exit status: 17", summary)
        self.assertIn("Elevated-skip markers observed: **1**", summary)

    def test_no_markers_is_not_a_coverage_claim(self):
        result, summary = self.report("print('test result: ok')")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Elevated-skip markers observed: **0**", summary)
        self.assertIn("does not prove complete native coverage", summary)

    def test_windows_suites_expose_success_output_to_reporter(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn(
            "python scripts/ci_native_tests.py cargo +stable test --workspace -- --show-output",
            workflow,
        )
        self.assertIn(
            "python scripts/ci_native_tests.py cargo +stable test --locked -p loomward-windows --all-features -- --show-output",
            workflow,
        )


if __name__ == "__main__":
    unittest.main()
