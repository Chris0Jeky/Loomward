"""LW-001: live Windows witness for the frozen Python reference, synthetic files only.

Run with ``py -3 scripts/test_reference_windows.py``. No legacy browser UI exists
since q-40; this exercises its retained reference API, not the native engine.
"""
from __future__ import annotations

import ctypes
import json
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'python'))

from loomward.server import App


@unittest.skipUnless(sys.platform == 'win32', 'LW-001 requires an actual Windows host')
class ReferenceWindowsTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if ctypes.windll.shell32.IsUserAnAdmin():
            raise RuntimeError('LW-001 must run without elevation')

    def setUp(self):
        state_dir = ROOT / '.loomward'
        state_dir.mkdir(exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix='lw001-', dir=state_dir)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name) / 'selected-root'
        self.root.mkdir()
        (self.root / 'nested').mkdir()
        self.contents = {'a.txt': b'alpha', 'nested/b.txt': b'alpha', 'c.txt': b'bravo'}
        for name, content in self.contents.items():
            (self.root / name).write_bytes(content)
        self.state = Path(self.temporary.name) / 'state.sqlite'
        # Reading may change access time; names, bytes, identity and write time must stay intact.
        self.before = self.fixture_state()

    def fixture_state(self):
        return {
            path.relative_to(self.root).as_posix(): (
                path.read_bytes(), path.stat().st_ino, path.stat().st_mtime_ns
            )
            for path in self.root.rglob('*') if path.is_file()
        }

    def test_selected_root_duplicates_and_feedback_restart(self):
        app = App(self.state, root=self.root)
        state = app.state()
        self.assertEqual(state['runtime'], 'python_reference')
        self.assertEqual(state['mode'], 'observed')
        self.assertEqual(state['inventory']['root'], str(self.root))
        self.assertEqual(
            {row['relative_path']: row['size_bytes'] for row in state['inventory']['files']},
            {name: len(content) for name, content in self.contents.items()},
        )
        self.assertTrue(state['inventory']['coverage']['unqualified_complete'])
        self.assertIsNone(state['duplicates'])
        self.assertEqual(state['processes']['status'], 'not_requested')
        self.assertFalse(state['capabilities']['file_mutation'])
        self.assertFalse(state['capabilities']['process_mutation'])
        for consent in ({}, {'confirm_content_read': False}):
            with self.assertRaisesRegex(ValueError, 'consent'):
                app.post('/api/duplicates', consent)
        report = app.post('/api/duplicates', {'confirm_content_read': True, 'byte_budget': 4096})
        self.assertFalse(report['budget_exhausted'])
        self.assertEqual(report['skipped_count'], 0)
        self.assertGreater(report['bytes_read'], 0)
        self.assertEqual(len(report['groups']), 1)
        group = report['groups'][0]
        self.assertEqual({row['relative_path'] for row in group['files']}, {'a.txt', 'nested/b.txt'})
        self.assertEqual(group['status'], 'byte_equal_at_observation')
        self.assertFalse(group['deletion_authorised'])
        self.assertEqual(report['logical_duplicate_bytes'], 5)
        self.assertIsNone(report['reclaimable_bytes'])

        # A separate interpreter commits the feedback and exits before the restart reads it.
        child = subprocess.run(
            [sys.executable, '-c',
             "import sys; sys.path.insert(0, 'python'); from loomward.server import App; "
             "app = App(sys.argv[1], root=sys.argv[2]); "
             "item = next(x for x in app.inventory['files'] if x['relative_path'] == 'a.txt'); "
             "app.post('/api/label', {'item_id': item['id'], 'label': 'Documents', "
             "'event_id': 'lw001-feedback'})",
             str(self.state), str(self.root)],
            cwd=ROOT, capture_output=True, text=True, timeout=30, check=False,
        )
        self.assertEqual(child.returncode, 0, child.stderr)
        restarted = App(self.state, root=self.root)
        events = restarted.feedback_export()['events']
        self.assertEqual(len(events), 1)
        self.assertEqual((events[0]['event_id'], events[0]['label'], events[0]['source']),
                         ('lw001-feedback', 'Documents', 'human'))
        self.assertEqual(restarted.state()['learning']['training_count'], 1)
        self.assertTrue(restarted.store.verify_audit())
        self.assertEqual(self.fixture_state(), self.before)

    def test_first_live_cpu_sample_is_unknown(self):
        app = App(self.state, root=self.root, allow_processes=True)
        sample = app.post('/api/processes', {})
        # Missing psutil or an empty observation cannot qualify the first-sample witness.
        self.assertEqual(sample['status'], 'observed', sample.get('note'))
        self.assertIsNone(sample['sample_interval_seconds'])
        self.assertGreater(len(sample['processes']), 0)
        for row in sample['processes']:
            self.assertIsNone(row['cpu_percent_of_machine'])
        self.assertFalse(app.state()['capabilities']['process_mutation'])
        self.assertEqual(self.fixture_state(), self.before)


if __name__ == '__main__':
    if sys.platform != 'win32':
        raise SystemExit('UNVERIFIED: LW-001 requires an actual Windows host')
    print(json.dumps({'os': 'Windows', 'build': sys.getwindowsversion().build,
                      'python': platform.python_version(),
                      'elevated': bool(ctypes.windll.shell32.IsUserAnAdmin()),
                      'selected_root': 'disposable fixture under worktree/.loomward; state is its sibling'}),
          flush=True)
    unittest.main(verbosity=2)
