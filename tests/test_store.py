import sqlite3
import tempfile
import unittest
from pathlib import Path
from loomward.store import Store

class StoreTests(unittest.TestCase):
    def test_feedback_persists_and_revisions_increase(self):
        with tempfile.TemporaryDirectory() as t:
            s=Store(Path(t)/'state.sqlite'); f={'name':'invoice'}
            a=s.feedback('scope','a',f,'Finance',event_id='one'); b=s.feedback('scope','a',f,'Media',event_id='two')
            self.assertEqual((a['revision'],b['revision']),(1,2))
            self.assertEqual(len(Store(Path(t)/'state.sqlite').events('scope')),2)
            self.assertTrue(s.verify_audit())
    def test_idempotent_event(self):
        with tempfile.TemporaryDirectory() as t:
            s=Store(Path(t)/'x.db'); args=('x','a',{'name':'x'},'Finance')
            self.assertEqual(s.feedback(*args,event_id='one'),s.feedback(*args,event_id='one'))
            self.assertEqual(len(s.events('x')),1)
    def test_conflicting_event_id_rejected(self):
        with tempfile.TemporaryDirectory() as t:
            s=Store(Path(t)/'x.db'); s.feedback('x','a',{'name':'x'},'Finance',event_id='one')
            with self.assertRaises(ValueError): s.feedback('x','a',{'name':'x'},'Media',event_id='one')
    def test_scopes_isolated(self):
        with tempfile.TemporaryDirectory() as t:
            s=Store(Path(t)/'x.db'); s.feedback('demo','a',{'name':'x'},'Finance')
            self.assertEqual(s.events('personal'),[])
    def test_tamper_detected(self):
        with tempfile.TemporaryDirectory() as t:
            p=Path(t)/'x.db'; s=Store(p); s.feedback('x','a',{'name':'x'},'Finance')
            with sqlite3.connect(p) as c: c.execute("UPDATE audit SET payload='{}' WHERE seq=1")
            self.assertFalse(s.verify_audit())
    def test_connection_is_closed_after_context(self):
        import sqlite3
        with tempfile.TemporaryDirectory() as d:
            store=Store(Path(d)/'state.db')
            with store._connect() as connection:
                self.assertEqual(connection.execute('SELECT 1').fetchone()[0],1)
            with self.assertRaises(sqlite3.ProgrammingError):
                connection.execute('SELECT 1')
    def test_training_validation_precedes_feedback_commit(self):
        with tempfile.TemporaryDirectory() as d:
            store=Store(Path(d)/'state.db')
            def reject(events):raise ValueError('training budget exhausted')
            with self.assertRaises(ValueError):
                store.feedback('scope','item',{'name':'invoice.pdf'},'Finance',validate_batch=reject)
            self.assertEqual(store.events('scope'),[]);self.assertEqual(store.audit()['total'],0)
    def test_batch_validator_receives_proposed_complete_scope(self):
        with tempfile.TemporaryDirectory() as d:
            store=Store(Path(d)/'state.db');store.feedback('scope','one',{'name':'invoice.pdf'},'Finance')
            seen=[]
            store.feedback('scope','two',{'name':'document.pdf'},'Documents',validate_batch=lambda events:seen.extend(events))
            self.assertEqual(len(seen),2);self.assertEqual(len(store.events('scope')),2)
