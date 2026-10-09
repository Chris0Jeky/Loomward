import copy
import json
import tempfile
import unittest
from pathlib import Path
from loomward.server import App, ASSETS
from loomward.scheduler import admit_workloads

class ExpansionApiTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory()
        self.app=App(Path(self.temp.name)/'state.db',demo=True)
    def tearDown(self):
        self.temp.cleanup()
    def test_plan_v2_is_a_separate_non_executing_endpoint(self):
        before=copy.deepcopy(self.app.inventory)
        plan=self.app.post('/api/plan-v2',self.app.scenario)
        self.assertEqual(plan['algorithm'],'bounded_portfolio_search_v2')
        self.assertTrue(all(not p['executable'] for p in plan['proposals']))
        self.assertEqual(self.app.inventory,before)
    def test_admission_matches_reference_without_state_mutation(self):
        request={'capacity':{'cpu_slots':1},'jobs':[{'id':'index','demand':{'cpu_slots':1}}]}
        self.assertEqual(self.app.post('/api/schedule',request),admit_workloads(request))
        self.assertEqual(self.app.store.events(self.app.scope),[])
    def test_admission_rejects_action_fields(self):
        with self.assertRaises(ValueError): self.app.post('/api/schedule',{'capacity':{},'jobs':[],'execute':True})
    def test_lease_and_approval_endpoints_are_absent(self):
        for route in ('/api/lease','/api/approve','/api/apply','/api/connect'):
            with self.assertRaises(LookupError): self.app.post(route,{})
    def test_v1_plan_contract_is_preserved(self):
        plan=self.app.post('/api/plan',self.app.scenario)
        self.assertNotEqual(plan.get('algorithm'),'bounded_portfolio_search_v2')
    def test_expansion_asset_is_explicit(self):
        self.assertEqual(ASSETS['/expansion.js'][0],'expansion.js')
