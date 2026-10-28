"""Synthetic set planning: independent concrete-world oracle, no filesystem fixture."""
import itertools
import unittest
from dataclasses import replace
from pathlib import Path

from experiments.reclaim_union import Candidate, FileFact, Scenario, inspect_plan, relief, solve, specimen, validate


def oracle(s):
    """Enumerate selections and concrete worlds, not the planner's relief function."""
    choices = []
    for f in s.files:
        # None admits an unseen outside link. One witness suffices to prevent relief.
        counts = [f.total_links] if f.total_links is not None else [len(f.names), len(f.names) + 1]
        sizes = [f.allocated_bytes] if f.allocated_bytes is not None else [0, 37]
        resident = [f.resident] if f.resident is not None else [False, True]
        choices.append(list(itertools.product(counts, sizes, resident)))
    best = None
    best_ids = None
    intervals = {}
    for bits in itertools.product([False, True], repeat=len(s.candidates)):
        chosen = [c for c, b in zip(s.candidates, bits) if b]
        ids = {c.id for c in chosen}
        valid = True
        for c in chosen:
            if c.excluded_reason or not c.complete:
                valid = False
            parent = c.parent
            while parent is not None:
                if parent in ids:
                    valid = False
                parent = next(p.parent for p in s.candidates if p.id == parent)
            if any(not f.identity_verified for f in s.files if set(f.names) & set(c.names)):
                valid = False
        if not valid:
            continue
        removed = {n for c in chosen for n in c.names}
        worlds = []
        for world in itertools.product(*choices):
            freed = dict.fromkeys(s.need, 0)
            for f, (links, allocation, resident) in zip(s.files, world):
                remaining = links - sum(n in removed for n in f.names)
                if remaining == 0 and resident and f.volume in freed:
                    freed[f.volume] += allocation
            worlds.append(freed)
        lower = {v: min(w[v] for w in worlds) for v in s.need}
        upper = {v: max(w[v] for w in worlds) for v in s.need}
        logical = sum(f.logical_bytes for f in s.files if removed.intersection(f.names))
        key = (sum(max(0, n - lower[v]) for v, n in s.need.items()), len(chosen), logical, tuple(sorted(ids)))
        intervals[tuple(sorted(ids))] = (lower, upper)
        if best is None or key < best:
            best, best_ids = key, tuple(sorted(ids))
    return best, best_ids, intervals


def tiny():
    return Scenario(
        files=(FileFact('fast', 'shared', 100, 12, True, 2, ('left-link', 'right-link')),
               FileFact('fast', 'own', 4, 4, True, 1, ('own-link',))),
        candidates=(Candidate('parent', 'fast', ('left-link', 'own-link'), meaning='Synthetic build tree'),
                    Candidate('child', 'fast', ('left-link',), parent='parent'),
                    Candidate('peer', 'fast', ('right-link',))),
        need={'fast': 16})


class ReclaimUnionTests(unittest.TestCase):
    def test_joint_links_and_nesting(self):
        r = solve(tiny())
        self.assertEqual(r['selected'], ['parent', 'peer'])
        self.assertEqual(r['relief']['guaranteed_bytes'], {'fast': 16})
        self.assertEqual(r['relief']['logical_bytes'], {'fast': 104})
        self.assertTrue(r['search']['optimal_in_model'])
        self.assertFalse(r['executable'])

    def test_nested_summation_is_wrong(self):
        s = Scenario((FileFact('fast', 'a', 10, 10, True, 1, ('a-link',)),
                      FileFact('fast', 'b', 2, 2, True, 1, ('b-link',))),
                     (Candidate('parent', 'fast', ('a-link', 'b-link')),
                      Candidate('child', 'fast', ('a-link',), parent='parent')), {'fast': 20})
        r = solve(s)
        self.assertEqual(sum(c['individual_verified_bytes'] for c in r['candidates']), 22)
        self.assertEqual(r['relief']['guaranteed_bytes']['fast'], 12)
        self.assertEqual(r['target_status'], 'impossible_even_optimistically')
        self.assertEqual(r['search']['shortfall_lower_bytes'], 8)

    def test_unknown_outside_links_cannot_prove_target(self):
        s = tiny()
        s = replace(s, files=(replace(s.files[0], total_links=None), s.files[1]))
        r = solve(s)
        self.assertEqual(r['target_status'], 'not_guaranteed_by_facts')
        self.assertEqual(r['search']['shortfall_lower_bytes'], 12)
        self.assertEqual(r['optimistic_relief_ceiling_bytes'], {'fast': 16})
        self.assertIn('outside links unknown', inspect_plan(r))

    def test_known_external_link_cannot_free_allocation(self):
        s = tiny()
        r = solve(replace(s, files=(replace(s.files[0], total_links=3), s.files[1])))
        self.assertEqual(r['relief']['guaranteed_bytes'], {'fast': 4})
        self.assertEqual(r['optimistic_relief_ceiling_bytes'], {'fast': 4})

    def test_oracle_agreement_and_budget_bounds(self):
        # 48 tiny models, all admissible selections, allocation/residency/link worlds.
        for links, allocation, resident, target in itertools.product([2, 3, None], [0, 12], [True, None], [0, 4, 12, 17]):
            s = tiny()
            s = replace(s, files=(replace(s.files[0], total_links=links, allocated_bytes=allocation,
                                          resident=resident), s.files[1]), need={'fast': target})
            expected, ids, intervals = oracle(s)
            exact = solve(s)
            self.assertEqual(tuple(exact['selected']), ids)
            self.assertEqual(exact['objective'][:3], list(expected[:3]))
            for selection, (lower, upper) in intervals.items():
                got = relief(s, selection)
                self.assertEqual(got['guaranteed_bytes']['fast'], lower['fast'])
                self.assertEqual(got['possible_bytes']['fast'], upper['fast'])
            for budget in [0, 1, 2, 4, 8]:
                r = solve(s, budget)
                self.assertLessEqual(r['search']['shortfall_lower_bytes'], expected[0])
                self.assertGreaterEqual(r['search']['shortfall_upper_bytes'], expected[0])
                self.assertLessEqual(r['search']['nodes_visited'], budget)
                if r['search']['optimal_in_model']:
                    self.assertEqual(tuple(r['selected']), ids)

    def test_budget_exhaustion_never_claims_optimal(self):
        r = solve(tiny(), 0)
        self.assertTrue(r['search']['budget_exhausted'])
        self.assertFalse(r['search']['optimal_in_model'])
        self.assertLessEqual(r['search']['shortfall_lower_bytes'], r['search']['shortfall_upper_bytes'])

    def test_volumes_do_not_cancel_and_file_id_is_volume_scoped(self):
        s = Scenario((FileFact('fast', 'same', 8, 8, True, 1, ('a',)),
                      FileFact('slow', 'same', 50, 50, True, 1, ('b',))),
                     (Candidate('a', 'fast', ('a',)), Candidate('b', 'slow', ('b',))),
                     {'fast': 10, 'slow': 20})
        r = solve(s)
        self.assertEqual(r['search']['shortfall_lower_bytes'], 2)
        self.assertEqual(r['relief']['guaranteed_bytes'], {'fast': 8, 'slow': 50})

    def test_unknown_allocation_is_unbounded_not_logical_size(self):
        s = Scenario((FileFact('fast', 'unknown', 1, None, None, 1, ('x',)),),
                     (Candidate('x', 'fast', ('x',)),), {'fast': 100})
        r = solve(s)
        self.assertIsNone(r['optimistic_relief_ceiling_bytes']['fast'])
        self.assertEqual(r['target_status'], 'not_guaranteed_by_facts')
        self.assertEqual(r['selected'], [])

    def test_observed_unselectable_name_prevents_upper_relief(self):
        s = Scenario((FileFact('fast', 'shared', 10, 12, True, None, ('inside', 'outside')),),
                     (Candidate('inside', 'fast', ('inside',)),), {'fast': 1})
        r = solve(s)
        self.assertEqual(r['optimistic_relief_ceiling_bytes'], {'fast': 0})
        self.assertEqual(r['target_status'], 'impossible_even_optimistically')

    def test_nonresident_and_partial_residency_are_not_logical_relief(self):
        s = tiny()
        for resident, allocation in [(False, 0), (None, 12), (True, None)]:
            changed = replace(s, files=(replace(s.files[0], resident=resident, allocated_bytes=allocation), s.files[1]))
            r = solve(changed)
            self.assertEqual(r['relief']['guaranteed_bytes'], {'fast': 4})
            expected, ids, _ = oracle(changed)
            self.assertEqual(tuple(r['selected']), ids)
            self.assertEqual(r['objective'][:3], list(expected[:3]))

    def test_exact_bytes_beyond_u64_totals_and_input_order(self):
        large = 2**64 - 1
        s = Scenario((FileFact('fast', 'a', large, large, True, 1, ('a',)),
                      FileFact('fast', 'b', large, large, True, 1, ('b',))),
                     (Candidate('a', 'fast', ('a',)), Candidate('b', 'fast', ('b',))), {'fast': large})
        self.assertEqual(relief(s, ('a', 'b'))['guaranteed_bytes']['fast'], 2 * large)
        self.assertEqual(solve(s)['selected'], ['a'])
        self.assertEqual(solve(replace(s, candidates=s.candidates[::-1], files=s.files[::-1]))['selected'], ['a'])

    def test_refusals_and_exclusions(self):
        s = tiny()
        invalid = [replace(s, files=s.files + (s.files[0],)),
                   replace(s, files=(replace(s.files[0], total_links=1), s.files[1])),
                   replace(s, files=(replace(s.files[0], allocated_bytes=-1), s.files[1])),
                   replace(s, need={'fast': True}),
                   replace(s, candidates=(replace(s.candidates[0], parent='child'),) + s.candidates[1:]),
                   replace(s, candidates=(replace(s.candidates[0], names=('own-link',)),) + s.candidates[1:]),
                   replace(s, candidates=(replace(s.candidates[0], names=('missing',)),)),
                   replace(s, candidates=(replace(s.candidates[0], volume='other'),)),
                   replace(s, candidates=(replace(s.candidates[1], parent=None), s.candidates[0])),
                   replace(s, files=(replace(s.files[0], resident=False), s.files[1])),
                   replace(s, files=(replace(s.files[0], id='C:/personal'), s.files[1])),
                   replace(s, candidates=s.candidates * 5)]
        for bad in invalid:
            with self.assertRaises(ValueError):
                validate(bad)
        for budget in [-1, True, 200001]:
            with self.assertRaises(ValueError):
                solve(s, budget)
        for change in [dict(complete=False), dict(excluded_reason='pinned')]:
            r = solve(replace(s, candidates=(replace(s.candidates[0], **change),) + s.candidates[1:]))
            self.assertNotIn('parent', r['selected'])
        r = solve(replace(s, files=(replace(s.files[0], identity_verified=False), s.files[1])))
        self.assertEqual(r['selected'], [])

    def test_specimen_beats_ranking_prefix_and_is_inert(self):
        s = specimen()
        before = repr(s)
        r = solve(s)
        self.assertEqual(repr(s), before)
        self.assertEqual(r['selected'], ['cache', 'project'])
        self.assertGreater(r['ranking_baseline']['objective'][1], r['objective'][1])
        self.assertFalse(r['filesystem_changed'])
        self.assertEqual(r['permission'], 'none; inspection is not removal approval')
        self.assertIn('NOT AN ACTION PLAN', inspect_plan(r))

    def test_committed_specimen_reproduces(self):
        path = Path(__file__).resolve().parents[1] / 'evidence/v3/reclaim-union-specimen.md'
        self.assertEqual(path.read_text(encoding='utf-8'), inspect_plan(solve(specimen())))


if __name__ == '__main__':
    unittest.main()
