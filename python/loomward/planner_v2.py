"""Bounded allocation search over estimates. No file access or executable operations.

The legacy greedy planner remains unchanged. A portfolio supplies a feasible
incumbent; tiny problems additionally receive a node-bounded exhaustive search.
'Optimal' refers only to this static disjoint-group model and its lexicographic
objective, never to real-world safety or future access costs.
"""
from __future__ import annotations

from fractions import Fraction
from typing import Any
from .inventory import integer
from .planner import plan_tiers

MAX_SEARCH_GROUPS = 12
MAX_SEARCH_TARGETS = 4


def plan_tiers_v2(scenario: dict[str, Any], *, node_budget: int = 50_000) -> dict[str, Any]:
    integer(node_budget, 'node_budget', 0, 200_000)
    baseline = plan_tiers(scenario)  # Reuse the complete v1 input/capacity checks.
    source_id = scenario['source_id']
    volumes = {v['id']: v for v in scenario['volumes']}
    source = volumes[source_id]
    goal = scenario['target_free_bytes']
    need = max(0, goal - source['free_bytes'])
    budget = scenario['max_transfer_bytes']
    groups = {g['id']: g for g in scenario['groups']}
    targets = sorted((v for v in volumes.values() if v['id'] != source_id
                      and v.get('online', False) and v.get('writable', False)
                      and v['tier'] >= source['tier']), key=lambda v: v['id'])
    candidates = []
    eligibility_rejected = []
    for g in groups.values():
        reason = None
        if g['volume_id'] != source_id:
            reason = 'not_on_source'
        elif any(g.get(k, True) for k in ('pinned', 'active', 'protected')):
            reason = 'pinned_active_protected_or_unspecified'
        elif g.get('heat') is None:
            reason = 'heat_unknown'
        elif g['heat'] > .25:
            reason = 'not_cold'
        elif g.get('days_since_move') is None or g['days_since_move'] < scenario.get('cooldown_days', 7):
            reason = 'cooldown_or_history_unknown'
        elif not source.get('online', False) or not source.get('writable', False):
            reason = 'source_offline_or_readonly'
        if reason:
            eligibility_rejected.append({'group_id': g['id'], 'reason': reason})
        else:
            candidates.append(g)

    def score(assignments: list[tuple[str, str]]) -> tuple:
        selected = [groups[g] for g, _ in assignments]
        relief = sum(g['source_bytes'] for g in selected)
        transfer = sum(g['transfer_bytes'] for g in selected)
        heat = sum((Fraction(str(g['heat'])) * g['source_bytes'] for g in selected), Fraction(0))
        return (max(0, need - relief), transfer, len(selected), heat, tuple(sorted(assignments)))

    best = [(p['group_id'], p['target_id']) for p in baseline['proposals']]
    best_score = score(best)
    orders = [
        sorted(candidates, key=lambda g: (-Fraction(g['source_bytes'], g['transfer_bytes']), -g['source_bytes'], g['id'])),
        sorted(candidates, key=lambda g: (-g['source_bytes'], g['transfer_bytes'], g['id'])),
        sorted(candidates, key=lambda g: (g['transfer_bytes'], -g['source_bytes'], g['id'])),
    ]
    for ordered in orders:
        free = {v['id']: v['free_bytes'] for v in targets}
        allocation = []; spent = relief = 0
        for g in ordered:
            if relief >= need:
                break
            if spent + g['transfer_bytes'] > budget:
                continue
            fitting = [v for v in targets if free[v['id']] - g['destination_bytes'] >= v['reserve_bytes']]
            if not fitting:
                continue
            # Best fit reduces fragmentation; it is a heuristic, not a proof.
            target = min(fitting, key=lambda v: (free[v['id']] - v['reserve_bytes'] - g['destination_bytes'], -v['tier'], v['id']))
            free[target['id']] -= g['destination_bytes']
            relief += g['source_bytes']; spent += g['transfer_bytes']
            allocation.append((g['id'], target['id']))
        if score(allocation) < best_score:
            best, best_score = allocation, score(allocation)

    ordered = orders[0]
    suffix_relief = [0] * (len(ordered) + 1)
    for i in range(len(ordered) - 1, -1, -1):
        suffix_relief[i] = suffix_relief[i + 1] + ordered[i]['source_bytes']
    visited = 0
    cutoff = False
    too_large = len(ordered) > MAX_SEARCH_GROUPS or len(targets) > MAX_SEARCH_TARGETS
    free = [v['free_bytes'] for v in targets]
    assignment: list[tuple[str, str]] = []

    def visit(i: int, relief: int, spent: int) -> None:
        nonlocal visited, cutoff, best, best_score
        if visited >= node_budget:
            cutoff = True
            return
        visited += 1
        optimistic_shortfall = max(0, need - relief - suffix_relief[i])
        if optimistic_shortfall > best_score[0]:
            return
        if optimistic_shortfall == best_score[0] and spent > best_score[1]:
            return
        if i == len(ordered) or relief >= need:
            current = score(assignment)
            if current < best_score:
                best, best_score = list(assignment), current
            return
        g = ordered[i]
        if spent + g['transfer_bytes'] <= budget:
            for j, v in enumerate(targets):
                if free[j] - g['destination_bytes'] < v['reserve_bytes']:
                    continue
                free[j] -= g['destination_bytes']
                assignment.append((g['id'], v['id']))
                visit(i + 1, relief + g['source_bytes'], spent + g['transfer_bytes'])
                assignment.pop(); free[j] += g['destination_bytes']
                if cutoff:
                    return
        visit(i + 1, relief, spent)

    if not too_large:
        visit(0, 0, 0)
    complete = not too_large and not cutoff
    projected = {v['id']: v['free_bytes'] for v in volumes.values()}
    proposals = []
    for group_id, target_id in sorted(best):
        g = groups[group_id]
        projected[source_id] += g['source_bytes']
        projected[target_id] -= g['destination_bytes']
        proposals.append({'group_id': group_id, 'source_id': source_id, 'target_id': target_id,
                          'source_bytes_relieved': g['source_bytes'],
                          'destination_bytes_required': g['destination_bytes'],
                          'transfer_bytes': g['transfer_bytes'],
                          'reason': 'bounded_capacity_budget_allocation',
                          'requires_consent': True, 'executable': False})
    selected = {p['group_id'] for p in proposals}
    rejected = eligibility_rejected + [{'group_id': g['id'], 'reason': 'not_selected_by_bounded_allocator'}
                                      for g in candidates if g['id'] not in selected]
    shortfall = best_score[0]
    return {'mode': 'simulation', 'algorithm': 'bounded_portfolio_search_v2',
            'optimality_claim': complete,
            'optimality_scope': 'Static supplied estimates; lexicographic shortfall, transfer, group count, heat, IDs only.',
            'shortfall_optimal': complete or shortfall == 0,
            'search': {'complete': complete, 'nodes_visited': visited, 'node_budget': node_budget,
                       'reason': 'problem_size_limit' if too_large else 'node_budget' if cutoff else 'exhausted',
                       'eligible_groups': len(candidates), 'eligible_targets': len(targets),
                       'lower_bound_shortfall_bytes': shortfall if complete else max(0, need - suffix_relief[0])},
            'proposals': proposals, 'rejected': sorted(rejected, key=lambda x: x['group_id']),
            'projected_free_bytes': projected, 'target_free_bytes': goal, 'shortfall_bytes': shortfall,
            'satisfied': shortfall == 0, 'transfer_bytes': best_score[1], 'filesystem_changed': False,
            'baseline_shortfall_bytes': baseline['shortfall_bytes'],
            'shortfall_improvement_bytes': baseline['shortfall_bytes'] - shortfall,
            'assumption': baseline['assumption']}
