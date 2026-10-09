"""v2 regression tests; the initial red run used the unchanged greedy baseline."""
import copy
import itertools
import random
import unittest
from loomward.planner import plan_tiers
from loomward.planner_v2 import plan_tiers_v2
from test_planner import scenario


def counterexample():
    s = scenario()
    s['target_free_bytes'] = 40
    s['max_transfer_bytes'] = 10
    base = s['groups'][0]
    s['groups'] = [dict(base, id='colder-small', source_bytes=10, destination_bytes=10,
                        transfer_bytes=10, heat=.01),
                   dict(base, id='larger', source_bytes=20, destination_bytes=20,
                        transfer_bytes=10, heat=.1)]
    return s


def exhaustive(s):
    src = s['volumes'][0]
    targets = s['volumes'][1:]
    best = max(0, s['target_free_bytes'] - src['free_bytes'])
    for choices in itertools.product(range(len(targets) + 1), repeat=len(s['groups'])):
        used = [0] * len(targets); cost = relief = 0
        for g, c in zip(s['groups'], choices):
            if c:
                used[c - 1] += g['destination_bytes']; cost += g['transfer_bytes']; relief += g['source_bytes']
        if cost <= s['max_transfer_bytes'] and all(v['free_bytes'] - u >= v['reserve_bytes'] for v, u in zip(targets, used)):
            best = min(best, max(0, s['target_free_bytes'] - src['free_bytes'] - relief))
    return best


class PlacementV2Tests(unittest.TestCase):
    def test_preserved_greedy_counterexample(self):
        s = counterexample()
        self.assertEqual(plan_tiers(s)['shortfall_bytes'], 10)
        self.assertEqual(plan_tiers_v2(s)['shortfall_bytes'], 0)

    def test_no_effects_and_different_byte_quantities(self):
        r = plan_tiers_v2(scenario())
        self.assertEqual(r['projected_free_bytes'], {'fast': 50, 'slow': 110})
        self.assertEqual(r['transfer_bytes'], 40)
        self.assertFalse(r['filesystem_changed'])
        self.assertTrue(all(not p['executable'] for p in r['proposals']))

    def test_exact_search_matches_independent_tiny_oracle(self):
        rng = random.Random(491)
        for _ in range(30):
            s = scenario(); s['volumes'][0]['capacity_bytes'] = 500
            s['target_free_bytes'] = 80; s['max_transfer_bytes'] = rng.randrange(10, 80)
            s['volumes'].append(dict(s['volumes'][1], id='warm', free_bytes=50, tier=1))
            g = s['groups'][0]
            s['groups'] = [dict(g, id=str(i), source_bytes=rng.randrange(5, 25),
                                transfer_bytes=rng.randrange(5, 30), destination_bytes=rng.randrange(5, 40)) for i in range(5)]
            result = plan_tiers_v2(s)
            self.assertEqual(result['shortfall_bytes'], exhaustive(s))
            self.assertTrue(result['search']['complete'])
            self.assertTrue(result['optimality_claim'])

    def test_cutoff_is_explicit_and_incumbent_not_worse(self):
        s = counterexample()
        r = plan_tiers_v2(s, node_budget=0)
        self.assertFalse(r['search']['complete'])
        self.assertFalse(r['optimality_claim'])
        self.assertEqual(r['search']['nodes_visited'], 0)
        self.assertLessEqual(r['shortfall_bytes'], plan_tiers(s)['shortfall_bytes'])

    def test_positive_node_budget_is_respected(self):
        r = plan_tiers_v2(counterexample(), node_budget=1)
        self.assertLessEqual(r['search']['nodes_visited'], 1)
        self.assertFalse(r['search']['complete'])

    def test_validation_and_input_preservation(self):
        for invalid in (True, -1, 1.2, 200001):
            with self.assertRaises(ValueError):
                plan_tiers_v2(scenario(), node_budget=invalid)
        s = counterexample(); before = copy.deepcopy(s)
        plan_tiers_v2(s); self.assertEqual(s, before)
        s['groups'].append(copy.deepcopy(s['groups'][0]))
        with self.assertRaises(ValueError): plan_tiers_v2(s)

    def test_no_pressure_and_offline_source(self):
        s = scenario(); s['target_free_bytes'] = 10
        self.assertEqual(plan_tiers_v2(s)['proposals'], [])
        s = scenario(); s['volumes'][0]['online'] = False
        self.assertEqual(plan_tiers_v2(s)['proposals'], [])

    def test_protected_active_pinned_unknown_hot_recent_excluded(self):
        for field, value in [('pinned', True), ('active', True), ('protected', True),
                             ('heat', None), ('heat', .5), ('days_since_move', None), ('days_since_move', 1)]:
            s = scenario(); s['groups'][0][field] = value
            self.assertEqual(plan_tiers_v2(s)['proposals'], [], field)

    def test_budget_capacity_and_atomic_groups(self):
        s = scenario(); s['volumes'][1]['free_bytes'] = 59
        self.assertEqual(plan_tiers_v2(s)['shortfall_bytes'], 30)
        s = scenario(); s['max_transfer_bytes'] = 39
        self.assertEqual(plan_tiers_v2(s)['proposals'], [])

    def test_minimise_transfer_after_satisfying_goal(self):
        s = counterexample(); s['max_transfer_bytes'] = 100
        s['groups'].append(dict(s['groups'][1], id='cheaper', source_bytes=20, transfer_bytes=5, heat=.2))
        r = plan_tiers_v2(s)
        self.assertEqual(r['transfer_bytes'], 5)
        self.assertEqual(r['proposals'][0]['group_id'], 'cheaper')

    def test_bounded_large_problem_uses_portfolio(self):
        s = scenario(); s['volumes'][0]['capacity_bytes'] = 10000
        g = s['groups'][0]; s['groups'] = [dict(g, id=str(i)) for i in range(13)]
        r = plan_tiers_v2(s)
        self.assertEqual(r['search']['reason'], 'problem_size_limit')
        self.assertFalse(r['optimality_claim'])

    def test_order_and_ties_deterministic(self):
        s = counterexample()
        a = plan_tiers_v2(s)
        s['groups'].reverse()
        b = plan_tiers_v2(s)
        self.assertEqual(a['proposals'], b['proposals'])

if __name__ == '__main__': unittest.main()
