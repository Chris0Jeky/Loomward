"""Local issue IDs are durable importer keys, never remote issue numbers."""
import json
import unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]

class BacklogIntegrityTests(unittest.TestCase):
    def setUp(self): self.items=json.loads((ROOT/'backlog/issues.json').read_text())
    def test_unique_local_ids_and_original_critical_path(self):
        self.assertEqual(len(self.items),len({x['id'] for x in self.items}))
        self.assertEqual([x['id'] for x in self.items[:3]],['LW-001','LW-002','LW-064'])
        self.assertTrue(all(x['status']=='planned' for x in self.items))
    def test_each_body_has_one_exact_durable_import_marker(self):
        for item in self.items:
            path=ROOT/item['body_file']
            self.assertTrue(path.is_file(),item['id'])
            self.assertEqual(path.read_text().count('<!-- loomward:'+item['id']+' -->'),1,item['id'])
    def test_all_dependencies_exist_and_graph_is_acyclic(self):
        by_id={i['id']:i for i in self.items};active=set();done=set()
        def visit(ident):
            self.assertIn(ident,by_id);self.assertNotIn(ident,active)
            if ident in done:return
            active.add(ident)
            for dep in by_id[ident]['depends_on']:visit(dep)
            active.remove(ident);done.add(ident)
        for ident in by_id:visit(ident)
