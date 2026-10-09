"""Constrained greedy tier simulator. This module never touches a filesystem."""
from __future__ import annotations
import math
from typing import Any
from .inventory import integer


def _flag(obj: dict, name: str, default: bool) -> bool:
    value = obj.get(name, default)
    if type(value) is not bool:
        raise ValueError(f'{name} must be boolean')
    return value


def _ident(value: Any) -> str:
    if not isinstance(value, str) or not 1 <= len(value) <= 128:
        raise ValueError('IDs must be nonempty strings of at most 128 characters')
    return value


def plan_tiers(scenario: dict[str, Any]) -> dict[str, Any]:
    if not isinstance(scenario, dict):
        raise ValueError('Scenario must be an object')
    volumes = scenario.get('volumes', [])
    groups = scenario.get('groups', [])
    if not isinstance(volumes, list) or not 1 <= len(volumes) <= 32 or not isinstance(groups, list) or len(groups) > 10_000:
        raise ValueError('Scenario requires 1..32 volumes and at most 10000 groups')
    validated: dict[str, dict[str, Any]] = {}
    for v in volumes:
        if not isinstance(v, dict):
            raise ValueError('Volume must be an object')
        ident = _ident(v.get('id'))
        if ident in validated:
            raise ValueError('Duplicate volume ID')
        capacity = integer(v.get('capacity_bytes'), 'capacity_bytes', 1)
        free = integer(v.get('free_bytes'), 'free_bytes', 0, capacity)
        reserve = integer(v.get('reserve_bytes'), 'reserve_bytes', 0, capacity)
        tier = integer(v.get('tier'), 'tier', 0, 9)
        validated[ident] = {'id': ident, 'capacity_bytes': capacity, 'free_bytes': free,
                            'reserve_bytes': reserve, 'tier': tier,
                            'online': _flag(v, 'online', False), 'writable': _flag(v, 'writable', False)}
    source_id = _ident(scenario.get('source_id'))
    if source_id not in validated:
        raise ValueError('Unknown source volume')
    source = validated[source_id]
    goal = integer(scenario.get('target_free_bytes'), 'target_free_bytes', 0, source['capacity_bytes'])
    budget = integer(scenario.get('max_transfer_bytes'), 'max_transfer_bytes')
    cooldown = integer(scenario.get('cooldown_days', 7), 'cooldown_days', 0, 36500)
    ids: set[str] = set()
    candidates = []
    rejected = []
    accounted = {i: 0 for i in validated}
    for g in groups:
        if not isinstance(g, dict):
            raise ValueError('Group must be an object')
        ident = _ident(g.get('id'))
        if ident in ids:
            raise ValueError('Duplicate group ID')
        ids.add(ident)
        volume_id = _ident(g.get('volume_id'))
        if volume_id not in validated:
            raise ValueError('Group references unknown volume')
        src_bytes = integer(g.get('source_bytes'), 'source_bytes', 1)
        dst_bytes = integer(g.get('destination_bytes'), 'destination_bytes', 1)
        xfer_bytes = integer(g.get('transfer_bytes'), 'transfer_bytes', 1)
        accounted[volume_id] += src_bytes
        heat = g.get('heat')
        if heat is not None and (type(heat) not in (int, float) or not math.isfinite(heat) or not 0 <= heat <= 1):
            raise ValueError('Heat must be null or a finite value in [0,1]')
        days = g.get('days_since_move')
        if days is not None:
            integer(days, 'days_since_move', 0, 36500)
        reason = None
        # Safety and eligibility constraints are deterministic by design; heat may later be learned.
        if volume_id != source_id:
            reason = 'not_on_source'
        elif any(_flag(g, key, True) for key in ('pinned', 'active', 'protected')):
            reason = 'pinned_active_protected_or_unspecified'
        elif heat is None:
            reason = 'heat_unknown'
        elif heat > 0.25:
            reason = 'not_cold'
        elif days is None or days < cooldown:
            reason = 'cooldown_or_history_unknown'
        if reason:
            rejected.append({'group_id': ident, 'reason': reason})
        else:
            candidates.append({'id': ident, 'source_bytes': src_bytes, 'destination_bytes': dst_bytes,
                               'transfer_bytes': xfer_bytes, 'heat': heat})
    for volume_id, count in accounted.items():
        v = validated[volume_id]
        if count > v['capacity_bytes'] - v['free_bytes']:
            raise ValueError('Group source allocations exceed used volume capacity; groups must be disjoint')
    free = {i: v['free_bytes'] for i, v in validated.items()}
    proposals = []
    transferred = 0
    if not source['online'] or not source['writable']:
        rejected.extend({'group_id': g['id'], 'reason': 'source_offline_or_readonly'} for g in candidates)
        candidates = []
    for g in sorted(candidates, key=lambda x: (x['heat'], -x['source_bytes'], x['id'])):
        if free[source_id] >= goal:
            break
        if g['transfer_bytes'] > budget - transferred:
            rejected.append({'group_id': g['id'], 'reason': 'transfer_budget'})
            continue
        targets = [v for i, v in validated.items() if i != source_id and v['online'] and v['writable']
                   and v['tier'] >= source['tier'] and free[i] - g['destination_bytes'] >= v['reserve_bytes']]
        if not targets:
            rejected.append({'group_id': g['id'], 'reason': 'no_eligible_destination_with_reserve'})
            continue
        target = sorted(targets, key=lambda v: (-v['tier'], -free[v['id']], v['id']))[0]
        free[source_id] += g['source_bytes']
        free[target['id']] -= g['destination_bytes']
        transferred += g['transfer_bytes']
        proposals.append({'group_id': g['id'], 'source_id': source_id, 'target_id': target['id'],
                          'source_bytes_relieved': g['source_bytes'], 'destination_bytes_required': g['destination_bytes'],
                          'transfer_bytes': g['transfer_bytes'], 'reason': 'cold_disjoint_group_within_capacity_and_budget',
                          'requires_consent': True, 'executable': False})
    shortfall = max(0, goal - free[source_id])
    return {'mode': 'simulation', 'algorithm': 'constrained_greedy_v1', 'optimality_claim': False,
            'proposals': proposals, 'rejected': rejected, 'projected_free_bytes': free,
            'target_free_bytes': goal, 'shortfall_bytes': shortfall, 'satisfied': shortfall == 0,
            'transfer_bytes': transferred, 'filesystem_changed': False,
            'assumption': 'Group allocations are user-supplied estimates; source, destination and transfer sizes are independent.'}
