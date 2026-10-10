"""Hermetic orchestration test for scripts/verify.py --app (#154, #186).

Tool discovery, the subprocess runner and the Playwright probe are all stubbed, so no npm,
node, cargo or browser runs here.
"""
import contextlib
import importlib.util
import io
import sys
import unittest
from pathlib import Path
from subprocess import CompletedProcess

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location("verify", ROOT / "scripts" / "verify.py")
verify = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(verify)

NPM = "/fake/bin/npm"
E2E = [sys.executable, "scripts/test_app.py"]
CHECK = [NPM, "--prefix", "app", "run", "check"]
UNIT = [NPM, "--prefix", "app", "run", "test"]
BUILD = [NPM, "--prefix", "app", "run", "build"]


def tools(node=True, npm=True):
    """A which() stub: node and npm are found or not; cargo never is."""
    def which(name):
        if name in ("node",) and node:
            return "/fake/bin/node"
        if name in ("npm", "npm.cmd") and npm:
            return NPM
        return None
    return which


class Runner:
    """Records every command and fails the first one whose text contains a marker in fail_on."""

    def __init__(self, fail_on=()):
        self.calls = []
        self.fail_on = tuple(fail_on)

    def __call__(self, command, **kwargs):
        self.calls.append(list(command))
        text = " ".join(command)
        rc = 1 if any(marker in text for marker in self.fail_on) else 0
        return CompletedProcess(command, rc)

    def app_calls(self):
        return [c for c in self.calls if c[0] == NPM or c == E2E]


def e2e_ok():
    return None


class VerifyAppOrchestrationTests(unittest.TestCase):
    def run_main(self, run, which=None, isdir=lambda path: True, e2e_gap=e2e_ok):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = verify.main(["--app"], which=which or tools(), run=run, isdir=isdir, e2e_gap=e2e_gap)
        return code, out.getvalue()

    def test_node_or_npm_missing_is_unverified_without_npm_call(self):
        for node, npm in ((False, False), (True, False), (False, True)):
            with self.subTest(node=node, npm=npm):
                run = Runner()
                code, out = self.run_main(run, which=tools(node=node, npm=npm))
                self.assertEqual(code, 0)
                self.assertEqual(run.app_calls(), [])
                self.assertIn("UNVERIFIED Svelte app: Node or npm is missing; no app success is claimed", out)
                self.assertNotIn("RUN Svelte app", out)

    def test_node_modules_missing_is_unverified(self):
        run = Runner()
        code, out = self.run_main(run, isdir=lambda path: False)
        self.assertEqual(code, 0)
        self.assertEqual(run.app_calls(), [])
        self.assertIn("UNVERIFIED Svelte app: app/node_modules is missing", out)
        self.assertNotIn("RUN Svelte app", out)

    def test_success_path_runs_check_test_build_e2e_in_order(self):
        run = Runner()
        code, out = self.run_main(run)
        self.assertEqual(code, 0)
        self.assertEqual(run.app_calls(), [CHECK, UNIT, BUILD, E2E])
        self.assertIn("PASS Svelte app Playwright e2e", out)
        self.assertIn("RUN Python suite", out)  # the default legs still run before the app legs

    def test_first_failure_stops_remaining_legs_and_fails_the_run(self):
        cases = (
            ("check", "run check", [CHECK]),
            ("unit tests", "run test", [CHECK, UNIT]),
            ("build", "run build", [CHECK, UNIT, BUILD]),
            ("e2e", "scripts/test_app.py", [CHECK, UNIT, BUILD, E2E]),
        )
        for label, marker, expected in cases:
            with self.subTest(leg=label):
                run = Runner(fail_on=[marker])
                code, out = self.run_main(run)
                self.assertEqual(code, 1)
                self.assertEqual(run.app_calls(), expected)
                self.assertIn("STOP Svelte app legs after the first failure", out)

    def test_playwright_missing_runs_npm_legs_and_reports_e2e_unverified(self):
        reason = "the Python playwright package is not installed (py -3 -m pip install playwright)"
        run = Runner()
        code, out = self.run_main(run, e2e_gap=lambda: reason)
        self.assertEqual(code, 0)
        self.assertEqual(run.app_calls(), [CHECK, UNIT, BUILD])
        self.assertNotIn(E2E, run.calls)
        self.assertIn(f"UNVERIFIED Svelte app e2e: {reason}; no e2e success is claimed", out)
        self.assertNotIn("RUN Svelte app Playwright e2e", out)


if __name__ == "__main__":
    unittest.main()
