"""Target-directory resolution of native/tests/webview2_probe.py (#504).

cargo metadata is stubbed and main() stops at the shell launch, so no cargo build, shell or
WebView2 window runs here.
"""
import contextlib
import importlib.util
import io
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]

if importlib.util.find_spec('playwright') is None:  # the probe imports Playwright when it loads
    raise unittest.SkipTest('playwright is not installed')

_spec = importlib.util.spec_from_file_location('webview2_probe', ROOT / 'native/tests/webview2_probe.py')
probe = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(probe)


class Launched(Exception):
    """Raised in place of the shell launch, so main() stops after resolving its paths."""


class CargoTargetDir(unittest.TestCase):
    def test_reports_the_target_directory_cargo_metadata_names(self):
        out = json.dumps({'target_directory': '/synthetic/cargo-target', 'packages': []})
        with mock.patch.object(probe.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, out, '')) as run:
            self.assertEqual(probe.cargo_target_dir(), Path('/synthetic/cargo-target'))
        cmd = run.call_args.args[0]
        self.assertEqual(cmd[:2], ['cargo', 'metadata'])
        self.assertIn('--no-deps', cmd)
        self.assertEqual(cmd[cmd.index('--manifest-path') + 1], 'native/Cargo.toml')
        self.assertEqual(run.call_args.kwargs['cwd'], probe.ROOT)

    def test_falls_back_to_native_target_when_metadata_cannot_be_read(self):
        cases = [
            {'side_effect': subprocess.CalledProcessError(101, ['cargo', 'metadata'])},
            {'side_effect': FileNotFoundError('cargo')},
            {'return_value': subprocess.CompletedProcess([], 0, 'not json', '')},
            {'return_value': subprocess.CompletedProcess([], 0, json.dumps({'packages': []}), '')},
        ]
        self.assertEqual(probe.FALLBACK_TARGET, probe.ROOT / 'native/target')
        for i, case in enumerate(cases):
            with self.subTest(case=i), contextlib.redirect_stderr(io.StringIO()) as err:
                with mock.patch.object(probe.subprocess, 'run', **case):
                    self.assertEqual(probe.cargo_target_dir(), probe.FALLBACK_TARGET)
            self.assertIn('assuming', err.getvalue())


class DefaultPaths(unittest.TestCase):
    def launch(self, argv, target, profiles):
        """Runs main() up to the shell launch; returns the executable it would start and what it printed."""
        printed = io.StringIO()
        with mock.patch.object(probe, 'cargo_target_dir', return_value=target), \
                mock.patch.object(probe, 'PROFILES', profiles), \
                mock.patch.object(probe.subprocess, 'Popen', side_effect=Launched) as popen, \
                mock.patch.object(sys, 'argv', ['webview2_probe.py', *argv]), \
                contextlib.redirect_stdout(printed):
            with self.assertRaises(Launched):
                probe.main()
        return popen.call_args.args[0][0], printed.getvalue()

    def test_defaults_sit_under_the_target_dir_and_an_explicit_exe_wins(self):
        with tempfile.TemporaryDirectory() as tmp:
            target, profiles = Path(tmp) / 'target', Path(tmp) / 'profiles'
            exe, printed = self.launch([], target, profiles)
            self.assertEqual(exe, str(target / 'debug/loomward-desktop.exe'))
            self.assertTrue((target / 'webview2-probe').is_dir())
            self.assertIn(str(target / 'webview2-probe'), printed)
            exe, _ = self.launch(['--exe', 'explicit.exe'], target, profiles)
            self.assertEqual(exe, 'explicit.exe')


if __name__ == '__main__':
    unittest.main()
