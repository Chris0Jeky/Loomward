import tempfile
import unittest
from pathlib import Path
from loomward.inventory import scan
from loomward.telemetry import Telemetry


class InventoryExcludeCaseTests(unittest.TestCase):
    def test_exclude_names_match_case_insensitively(self):
        with tempfile.TemporaryDirectory() as t:
            p = Path(t)
            (p / 'Secrets').mkdir()
            (p / 'Secrets' / 's.txt').write_bytes(b'x')
            (p / 'keep.txt').write_bytes(b'y')
            s = scan(p, exclude_names=frozenset({'Secrets'}))
            self.assertEqual(sorted(f['relative_path'] for f in s['files']), ['keep.txt'])

    def test_exclude_names_uppercase_matches_lowercase_entry(self):
        with tempfile.TemporaryDirectory() as t:
            p = Path(t)
            (p / 'secrets').mkdir()
            (p / 'secrets' / 's.txt').write_bytes(b'x')
            (p / 'keep.txt').write_bytes(b'y')
            s = scan(p, exclude_names=frozenset({'SECRETS'}))
            self.assertEqual(sorted(f['relative_path'] for f in s['files']), ['keep.txt'])


class TelemetryMaxProcessesTests(unittest.TestCase):
    def test_invalid_max_processes_rejected(self):
        for v in (0, -1, 10001, True, False, '10', 1.5, None):
            with self.subTest(value=v):
                with self.assertRaises(ValueError):
                    Telemetry().snapshot(max_processes=v)

    def test_valid_max_processes_accepted(self):
        for v in (1, 200, 10000):
            with self.subTest(value=v):
                s = Telemetry().snapshot(max_processes=v)
                self.assertIn('processes', s)


if __name__ == '__main__':
    unittest.main()
