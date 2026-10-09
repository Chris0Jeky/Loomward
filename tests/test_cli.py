import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from loomward.__main__ import main

class CliTests(unittest.TestCase):
    def run_cli(self,*args):
        out,err=io.StringIO(),io.StringIO()
        with contextlib.redirect_stdout(out),contextlib.redirect_stderr(err):code=main(list(args))
        return code,out.getvalue(),err.getvalue()
    def test_doctor_reports_no_mutations(self):
        code,out,_=self.run_cli('doctor');self.assertEqual(code,0);self.assertFalse(json.loads(out)['capabilities']['file_mutation'])
    def test_scan_creates_valid_snapshot(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)/'files';root.mkdir();(root/'hello.txt').write_text('hello');dest=Path(d)/'scan.json'
            code,_,_=self.run_cli('scan',str(root),'--output',str(dest));self.assertEqual(code,0)
            self.assertEqual(json.loads(dest.read_text())['summary']['file_count'],1)
    def test_output_is_not_overwritten(self):
        with tempfile.TemporaryDirectory() as d:
            dest=Path(d)/'existing.json';dest.write_text('keep')
            code,_,_=self.run_cli('doctor','--output',str(dest));self.assertEqual(code,2);self.assertEqual(dest.read_text(),'keep')
    def test_teacher_request_is_offline(self):
        with tempfile.TemporaryDirectory() as d:
            f=Path(d)/'features.json';f.write_text(json.dumps({'name':'invoice.pdf'}))
            code,out,_=self.run_cli('teacher-request',str(f),'--model','local-test','--item-id','x')
            self.assertEqual(code,0);self.assertIn('json_schema',json.loads(out)['response_format'])
    def test_synthetic_training_requires_explicit_flag(self):
        with tempfile.TemporaryDirectory() as d:
            f=Path(d)/'feedback.json';f.write_text(json.dumps({'mode':'demo','events':[]}))
            code,_,_=self.run_cli('train',str(f));self.assertEqual(code,2)
            code,out,_=self.run_cli('train',str(f),'--allow-synthetic');self.assertEqual(code,0)
            self.assertTrue(json.loads(out)['synthetic_training'])
    def test_duplicate_content_access_requires_consent(self):
        with tempfile.TemporaryDirectory() as d:
            f=Path(d)/'snapshot.json';f.write_text('{}')
            code,_,_=self.run_cli('duplicates',d,'--snapshot',str(f));self.assertEqual(code,2)
    def test_scan_and_duplicate_inspection_preserve_source_contents(self):
        import hashlib
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)/'files';root.mkdir()
            for name,data in [('a.txt',b'fixture'),('b.txt',b'fixture'),('c.txt',b'differ!')]:
                (root/name).write_bytes(data)
            before={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir()}
            snap=Path(d)/'snapshot.json'
            self.assertEqual(self.run_cli('scan',str(root),'--output',str(snap))[0],0)
            code,out,_=self.run_cli('duplicates',str(root),'--snapshot',str(snap),'--consent-content')
            self.assertEqual(code,0)
            self.assertEqual(before,{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir()})
            self.assertEqual(len(json.loads(out)['groups']),1)
            self.assertFalse(json.loads(out)['groups'][0]['deletion_authorised'])
