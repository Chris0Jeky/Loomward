import tempfile
import unittest
from pathlib import Path
from loomward.store import Store


class FixStoreTests(unittest.TestCase):
    def test_explicit_empty_event_id_rejected_without_commit(self):
        with tempfile.TemporaryDirectory() as t:
            s = Store(Path(t) / 'state.db')
            with self.assertRaisesRegex(ValueError, 'Invalid event ID'):
                s.feedback('scope', 'a', {'name': 'x'}, 'Finance', event_id='')
            with self.assertRaisesRegex(ValueError, 'Invalid event ID'):
                s.feedback('scope', 'a', {'name': 'x'}, 'Finance', event_id='')
            self.assertEqual(s.events('scope'), [])
            self.assertEqual(s.audit()['total'], 0)

    def test_non_string_event_id_rejected(self):
        with tempfile.TemporaryDirectory() as t:
            s = Store(Path(t) / 'state.db')
            with self.assertRaisesRegex(ValueError, 'Invalid event ID'):
                s.feedback('scope', 'a', {'name': 'x'}, 'Finance', event_id=123)
            self.assertEqual(s.events('scope'), [])

    def test_none_event_id_still_generates_uuid(self):
        with tempfile.TemporaryDirectory() as t:
            s = Store(Path(t) / 'state.db')
            value = s.feedback('scope', 'a', {'name': 'x'}, 'Finance')
            self.assertTrue(isinstance(value['event_id'], str) and 1 <= len(value['event_id']) <= 128)
            self.assertEqual(len(s.events('scope')), 1)

    def test_audit_limit_validated(self):
        with tempfile.TemporaryDirectory() as t:
            s = Store(Path(t) / 'state.db')
            s.feedback('scope', 'a', {'name': 'x'}, 'Finance', event_id='one')
            for bad in (0, -1, -100, 10001, True, False, '10', 1.5, None):
                with self.assertRaises(ValueError, msg=f'limit={bad!r}'):
                    s.audit(limit=bad)
            self.assertEqual(s.audit(limit=1)['total'], 1)


if __name__ == '__main__':
    unittest.main()
