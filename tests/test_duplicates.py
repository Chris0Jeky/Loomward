import os
import tempfile
import unittest
from pathlib import Path
from loomward.inventory import scan
from loomward.duplicates import find_duplicates

class DuplicateTests(unittest.TestCase):
    def test_exact_duplicates_not_same_size_only(self):
        with tempfile.TemporaryDirectory() as t:
            p=Path(t)
            for n,b in [('a',b'abcd'),('b',b'abcd'),('c',b'wxyz')]: (p/n).write_bytes(b)
            r=find_duplicates(t,scan(t),byte_budget=1000)
            self.assertEqual(len(r['groups']),1)
            self.assertEqual(len(r['groups'][0]['files']),2)
            self.assertEqual(r['groups'][0]['logical_duplicate_bytes'],4)
            self.assertIsNone(r['reclaimable_bytes'])
    def test_byte_budget_never_exceeded(self):
        with tempfile.TemporaryDirectory() as t:
            for n in ('a','b'): (Path(t)/n).write_bytes(b'x'*100)
            r=find_duplicates(t,scan(t),byte_budget=50)
            self.assertLessEqual(r['bytes_read'],50); self.assertEqual(r['groups'],[])
            self.assertTrue(r['budget_exhausted'])
    def test_changed_file_is_rejected(self):
        with tempfile.TemporaryDirectory() as t:
            for n in ('a','b'): (Path(t)/n).write_text('same')
            s=scan(t); (Path(t)/'b').write_text('changed')
            r=find_duplicates(t,s,byte_budget=1000)
            self.assertEqual(r['groups'],[]); self.assertGreater(len(r['skipped']),0)
    def test_wrong_root_rejected(self):
        with tempfile.TemporaryDirectory() as a,tempfile.TemporaryDirectory() as b:
            with self.assertRaises(ValueError): find_duplicates(b,scan(a),byte_budget=100)
    def test_hard_links_not_duplicates(self):
        with tempfile.TemporaryDirectory() as t:
            p=Path(t); (p/'a').write_text('same')
            try: os.link(p/'a',p/'b')
            except OSError: self.skipTest('hard links unsupported')
            self.assertEqual(find_duplicates(t,scan(t),byte_budget=100)['groups'],[])
    def test_path_traversal_rejected(self):
        with tempfile.TemporaryDirectory() as t:
            for n in ('a','b'): (Path(t)/n).write_text('same')
            s=scan(t); s['files'][0]['relative_path']='../escape'
            r=find_duplicates(t,s,byte_budget=1000)
            self.assertEqual(r['groups'],[])
    def test_empty_files_do_not_claim_savings(self):
        with tempfile.TemporaryDirectory() as t:
            for n in ('a','b'): (Path(t)/n).touch()
            self.assertEqual(find_duplicates(t,scan(t),byte_budget=100)['groups'],[])
