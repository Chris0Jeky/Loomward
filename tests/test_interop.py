import copy
import json
import unittest
from loomward.catalog import Catalog
from loomward.interop import ToolService, wire_bytes
from test_catalog import snapshot
from test_planner import scenario

class InteropTests(unittest.TestCase):
    def setUp(self):
        self.c = Catalog(snapshot(), disclose_names=True); self.s = ToolService(self.c, origin='synthetic_demo')
    def tearDown(self): self.c.close()
    def test_only_four_narrow_tools_and_no_action_authority(self):
        names = {t['name'] for t in self.s.tool_specs()}
        self.assertEqual(names, {'workspace_summary','catalog_search','evidence_explain','placement_simulate'})
        result = self.s.call('workspace_summary', {})
        self.assertFalse(result['authority']['execute']); self.assertFalse(result['authority']['approve'])
    def test_read_queries_and_provenance(self):
        r = self.s.call('catalog_search', {'query':'invoice', 'limit':2})
        self.assertEqual(len(r['data']['items']), 2)
        self.assertEqual(r['evidence']['origin'], 'synthetic_demo')
        ref = r['data']['items'][0]['item_ref']
        e = self.s.call('evidence_explain', {'item_ref':ref})
        self.assertFalse(e['data']['execution_authority'])
    def test_cannot_expand_scope_or_request_content(self):
        for args in ({'prefix':'/'}, {'query':'x','read_contents':True}, {'sql':'select *'}):
            with self.assertRaises(ValueError): self.s.call('catalog_search',args)
    def test_unknown_and_dangerous_tools_absent(self):
        for name in ('execute','shell','delete','approve','read_file','kill','scan_root'):
            with self.assertRaises(LookupError): self.s.call(name,{})
    def test_no_names_permission_omits_sensitive_tools(self):
        with Catalog(snapshot()) as c:
            s = ToolService(c)
            self.assertEqual({t['name'] for t in s.tool_specs()}, {'workspace_summary','placement_simulate'})
            with self.assertRaises(PermissionError): s.call('catalog_search',{})
    def test_simulation_accepts_exact_decimal_byte_strings(self):
        s = scenario(); r = self.s.call('placement_simulate', {'scenario':wire_bytes(s)})
        self.assertEqual(r['data']['shortfall_bytes'], '0')
        self.assertFalse(r['data']['filesystem_changed'])
        self.assertFalse(r['data']['proposals'][0]['executable'])
    def test_simulation_rejects_unrecognised_fields_and_bad_decimal(self):
        s = wire_bytes(scenario()); s['root'] = 'C:/secret'
        with self.assertRaises(ValueError): self.s.call('placement_simulate', {'scenario':s})
        for bad in ('-1','01','1.0',True):
            s = wire_bytes(scenario()); s['max_transfer_bytes'] = bad
            with self.assertRaises(ValueError): self.s.call('placement_simulate', {'scenario':s})
    def test_integer_byte_values_are_not_silently_round_tripped(self):
        with self.assertRaises(ValueError): self.s.call('placement_simulate', {'scenario':scenario()})
    def test_resources_are_fixed_not_filesystem_uris(self):
        self.assertEqual(len(self.s.resources()),2)
        with self.assertRaises(LookupError): self.s.resource('file:///C:/secrets')
        self.assertFalse(self.s.resource('loomward://reference/policy')['execute'])
    def test_specs_do_not_share_mutable_objects(self):
        specs = self.s.tool_specs(); specs[0]['name'] = 'shell'
        self.assertNotEqual(self.s.tool_specs()[0]['name'], 'shell')
    def test_no_shell_fields_at_any_simulation_level(self):
        for location in ('volume', 'group'):
            s = wire_bytes(scenario()); target = s['volumes'][0] if location=='volume' else s['groups'][0]
            target['command'] = 'whoami'
            with self.assertRaises(ValueError): self.s.call('placement_simulate',{'scenario':s})
    def test_boundary_keeps_source_snapshot_private(self):
        raw=json.dumps(self.s.call('workspace_summary',{}))
        self.assertNotIn('PRIVATE_ROOT',raw)
        self.assertNotIn('Invoice',raw)
