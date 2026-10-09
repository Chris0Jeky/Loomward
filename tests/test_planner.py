import copy
import unittest
from loomward.planner import plan_tiers

def scenario():
    return {'source_id':'fast','target_free_bytes':50,'max_transfer_bytes':1000,'cooldown_days':7,
      'volumes':[{'id':'fast','capacity_bytes':100,'free_bytes':20,'reserve_bytes':10,'tier':0,'online':True,'writable':True},
                 {'id':'slow','capacity_bytes':200,'free_bytes':150,'reserve_bytes':20,'tier':2,'online':True,'writable':True}],
      'groups':[{'id':'cold','volume_id':'fast','source_bytes':30,'destination_bytes':40,'transfer_bytes':40,'heat':0.05,
                 'pinned':False,'active':False,'protected':False,'days_since_move':30}]}

class PlannerTests(unittest.TestCase):
    def test_accounts_for_different_source_destination_sizes(self):
        r=plan_tiers(scenario()); self.assertTrue(r['satisfied'])
        self.assertEqual(r['projected_free_bytes'],{'fast':50,'slow':110})
        self.assertEqual(r['transfer_bytes'],40)
    def test_infeasible_capacity_is_reported(self):
        s=scenario(); s['volumes'][1]['free_bytes']=50
        r=plan_tiers(s); self.assertEqual(r['proposals'],[]); self.assertEqual(r['shortfall_bytes'],30)
    def test_pinned_active_and_protected_are_ineligible(self):
        for flag in ('pinned','active','protected'):
            s=scenario(); s['groups'][0][flag]=True
            self.assertEqual(plan_tiers(s)['proposals'],[],flag)
    def test_unknown_heat_is_not_assumed_cold(self):
        s=scenario(); s['groups'][0]['heat']=None
        self.assertEqual(plan_tiers(s)['proposals'],[])
    def test_offline_destination_is_ineligible(self):
        s=scenario(); s['volumes'][1]['online']=False
        self.assertEqual(plan_tiers(s)['proposals'],[])
    def test_recent_move_cooldown(self):
        s=scenario(); s['groups'][0]['days_since_move']=2
        self.assertEqual(plan_tiers(s)['proposals'],[])
    def test_transfer_budget(self):
        s=scenario(); s['max_transfer_bytes']=39
        self.assertEqual(plan_tiers(s)['proposals'],[])
    def test_no_pressure_means_no_move(self):
        s=scenario(); s['target_free_bytes']=10
        self.assertEqual(plan_tiers(s)['proposals'],[])
    def test_duplicate_ids_rejected(self):
        s=scenario(); s['groups'].append(copy.deepcopy(s['groups'][0]))
        with self.assertRaises(ValueError): plan_tiers(s)
    def test_invalid_numbers_rejected(self):
        for bad in (-1,True,1.5):
            s=scenario(); s['volumes'][0]['free_bytes']=bad
            with self.assertRaises(ValueError): plan_tiers(s)
    def test_nan_heat_rejected(self):
        s=scenario(); s['groups'][0]['heat']=float('nan')
        with self.assertRaises(ValueError): plan_tiers(s)
    def test_group_not_split_to_fit(self):
        s=scenario(); s['volumes'][1]['free_bytes']=59
        self.assertEqual(plan_tiers(s)['proposals'],[])
    def test_no_faster_tier_for_demotion(self):
        s=scenario(); s['volumes'][0]['tier']=2; s['volumes'][1]['tier']=0
        self.assertEqual(plan_tiers(s)['proposals'],[])
    def test_hot_group_stays(self):
        s=scenario(); s['groups'][0]['heat']=0.95
        self.assertEqual(plan_tiers(s)['proposals'],[])
    def test_input_is_unchanged(self):
        s=scenario(); before=copy.deepcopy(s); plan_tiers(s); self.assertEqual(s,before)
