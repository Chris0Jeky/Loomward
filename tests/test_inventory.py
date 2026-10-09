import os
import tempfile
import unittest
from pathlib import Path
from loomward.inventory import scan, signature

class InventoryTests(unittest.TestCase):
    def test_metadata_counts(self):
        with tempfile.TemporaryDirectory() as t:
            p=Path(t); (p/'a.txt').write_bytes(b'abc'); (p/'d').mkdir(); (p/'d'/'b.bin').write_bytes(b'12345')
            s=scan(p)
            self.assertEqual(s['summary']['logical_bytes'],8)
            self.assertEqual(s['summary']['file_count'],2)
            self.assertTrue(s['coverage']['complete_under_policy'])
            self.assertEqual(sorted(x['relative_path'] for x in s['files']),['a.txt','d/b.bin'])
    def test_identity_matches_fresh_stat_and_open_handle(self):
        # Windows DirEntry.stat() reports st_ino/st_dev/st_nlink as 0 and fstat's st_ctime differs from stat's
        with tempfile.TemporaryDirectory() as t:
            p=Path(t)/'a.bin'; p.write_bytes(b'abc')
            r=scan(t)['files'][0]
            self.assertGreaterEqual(r['nlink'],1)
            recorded=tuple(r[k] for k in ('device','inode','size_bytes','mtime_ns','ctime_ns'))
            self.assertEqual(signature(os.lstat(p)),recorded)
            fd=os.open(p,os.O_RDONLY|getattr(os,'O_BINARY',0))
            try: self.assertEqual(signature(os.fstat(fd)),recorded)
            finally: os.close(fd)
    def test_missing_root_rejected(self):
        with self.assertRaises(ValueError): scan('/this/path/does/not/exist/loomward')
    def test_regular_file_root_rejected(self):
        with tempfile.NamedTemporaryFile() as f:
            with self.assertRaises(ValueError): scan(f.name)
    def test_entry_limit_is_visible(self):
        with tempfile.TemporaryDirectory() as t:
            for i in range(6): (Path(t)/str(i)).write_text('x')
            s=scan(t,max_entries=2)
            self.assertEqual(s['summary']['file_count'],2)
            self.assertTrue(s['coverage']['limit_hit'])
            self.assertFalse(s['coverage']['complete_under_policy'])
    def test_depth_limit_is_visible(self):
        with tempfile.TemporaryDirectory() as t:
            p=Path(t)/'a'; p.mkdir(); (p/'file').write_text('x')
            s=scan(t,max_depth=0)
            self.assertEqual(s['summary']['file_count'],0)
            self.assertGreater(s['coverage']['skipped_count'],0)
    @unittest.skipUnless(hasattr(os,'symlink'), 'symlinks unsupported')
    def test_symlink_not_followed(self):
        with tempfile.TemporaryDirectory() as t, tempfile.TemporaryDirectory() as outside:
            (Path(outside)/'secret').write_text('private')
            try: os.symlink(outside,Path(t)/'link',target_is_directory=True)
            except OSError: self.skipTest('symlink privilege unavailable')
            self.assertEqual(scan(t)['summary']['file_count'],0)
            with self.assertRaises(ValueError): scan(Path(t)/'link')
    def test_empty_root_is_complete(self):
        with tempfile.TemporaryDirectory() as t:
            s=scan(t); self.assertEqual(s['summary']['logical_bytes'],0); self.assertTrue(s['coverage']['complete_under_policy'])
    def test_limits_validate(self):
        with tempfile.TemporaryDirectory() as t:
            for v in (0,-1,True):
                with self.assertRaises(ValueError): scan(t,max_entries=v)
    def test_sensitive_metadata_flagged(self):
        with tempfile.TemporaryDirectory() as t:
            (Path(t)/'.env').write_text('SECRET=x')
            self.assertIn('sensitive',scan(t)['files'][0]['flags'])
