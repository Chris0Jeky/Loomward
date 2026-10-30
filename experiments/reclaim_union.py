"""In-memory, synthetic reclaim what-if. No paths, scans, effects or production imports.

Run: python experiments/reclaim_union.py [--need 16] [--node-budget 50000] [--json]
The CLI only uses the fictional specimen below; it cannot ingest an owner inventory.
"""
from __future__ import annotations

import argparse
import json
import re
from dataclasses import dataclass, replace


@dataclass(frozen=True)
class FileFact:
    volume: str
    id: str
    logical_bytes: int
    allocated_bytes: int | None
    resident: bool | None
    total_links: int | None
    names: tuple[str, ...]
    identity_verified: bool = True


@dataclass(frozen=True)
class Candidate:
    id: str
    volume: str
    names: tuple[str, ...]
    parent: str | None = None
    meaning: str = 'Synthetic candidate; disposability and rebuild cost unknown'
    complete: bool = True
    excluded_reason: str | None = None


@dataclass(frozen=True)
class Scenario:
    files: tuple[FileFact, ...]
    candidates: tuple[Candidate, ...]
    need: dict[str, int]


def _uint(n):
    return type(n) is int and 0 <= n <= 2**64 - 1


def _token(s):
    return isinstance(s, str) and re.fullmatch(r'[a-z][a-z0-9_-]{0,63}', s) is not None


def ancestors(s, c):
    by_id = {c.id: c for c in s.candidates}
    result = set()
    parent = c.parent
    while parent is not None:
        if parent not in by_id or parent == c.id or parent in result:
            raise ValueError('missing parent or cyclic ancestry')
        result.add(parent)
        parent = by_id[parent].parent
    return result


def validate(s):
    """Refuse contradictory facts instead of repairing them into an optimistic model."""
    if len(s.candidates) > 12 or len(s.files) > 128 or not 1 <= len(s.need) <= 4:
        raise ValueError('synthetic model cap: 12 candidates, 128 objects, 1..4 targets')
    if not all(_token(v) and _uint(n) for v, n in s.need.items()):
        raise ValueError('targets require synthetic volume tokens and unsigned integer bytes')
    names = {}
    keys = set()
    for f in s.files:
        key = (f.volume, f.id)
        if not _token(f.volume) or not _token(f.id) or key in keys:
            raise ValueError('invalid or repeated volume/file identity')
        keys.add(key)
        if (not _uint(f.logical_bytes) or
                (f.allocated_bytes is not None and not _uint(f.allocated_bytes)) or
                (f.total_links is not None and (not _uint(f.total_links) or f.total_links < len(f.names))) or
                (f.resident is not None and type(f.resident) is not bool) or
                type(f.identity_verified) is not bool or not f.names):
            raise ValueError('inconsistent object facts')
        if f.resident is False and f.allocated_bytes not in (None, 0):
            raise ValueError('nonresident objects cannot assert positive resident allocation')
        for n in f.names:
            if not _token(n) or n in names:
                raise ValueError('invalid or repeated link name token')
            names[n] = f
    if len(names) > 512:
        raise ValueError('synthetic model cap: 512 names')
    by_id = {c.id: c for c in s.candidates}
    if len(by_id) != len(s.candidates):
        raise ValueError('repeated candidate')
    for c in s.candidates:
        if (not _token(c.id) or not _token(c.volume) or type(c.complete) is not bool or
                len(set(c.names)) != len(c.names) or
                any(n not in names or names[n].volume != c.volume for n in c.names)):
            raise ValueError('invalid candidate names or volume')
        for a in ancestors(s, c):
            if c.volume != by_id[a].volume or not set(c.names) <= set(by_id[a].names):
                raise ValueError('ancestry and membership disagree')
    for i, c in enumerate(s.candidates):
        for d in s.candidates[i + 1:]:
            if set(c.names) & set(d.names) and c.id not in ancestors(s, d) and d.id not in ancestors(s, c):
                raise ValueError('overlapping names without an ancestry relation')


def excluded(s, c):
    if c.excluded_reason:
        return c.excluded_reason
    if not c.complete:
        return 'coverage incomplete or stale; candidate excluded'
    if any(not f.identity_verified for f in s.files if set(c.names).intersection(f.names)):
        return 'identity unverified; candidate excluded'
    return None


def compatible(s, ids, c):
    return all(d.id not in ancestors(s, c) and c.id not in ancestors(s, d)
               for d in s.candidates if d.id in ids)


def _sum_upper(values):
    values = list(values)
    return None if None in values else sum(values)


def relief(s, ids):
    """Union by volume/file identity; ids may be a relaxed, non-antichain bound."""
    removed = {n for c in s.candidates if c.id in ids for n in c.names}
    rows = []
    for f in s.files:
        covered = len(removed.intersection(f.names))
        if not covered:
            continue
        all_observed = covered == len(f.names)
        closed = f.total_links == covered
        could_close = all_observed and (closed or f.total_links is None)
        lo = f.allocated_bytes if closed and f.resident is True and f.allocated_bytes is not None else 0
        hi = f.allocated_bytes if could_close and f.resident is not False else 0
        reasons = []
        if not all_observed:
            reasons.append('observed link retained outside this set')
        if f.total_links is None:
            reasons.append('outside links unknown')
        elif f.total_links > len(f.names):
            reasons.append('known link outside supplied scope')
        if f.allocated_bytes is None:
            reasons.append('allocation unknown; logical size is not an upper bound')
        if f.resident is None:
            reasons.append('residency unknown')
        if f.resident is False:
            reasons.append('nonresident; no resident allocation credited')
        if not f.identity_verified:
            lo, hi = 0, None
            reasons.append('identity unknown; cannot deduplicate')
        rows.append(dict(volume=f.volume, file_id=f.id, selected_names=sorted(removed.intersection(f.names)),
                         observed_names=list(f.names), total_links=f.total_links,
                         logical_bytes=f.logical_bytes, allocated_bytes=f.allocated_bytes,
                         resident=f.resident, guaranteed_bytes=lo, possible_bytes=hi, unknowns=reasons))
    volumes = sorted(set(s.need) | {f.volume for f in s.files})
    return dict(
        logical_bytes={v: sum(r['logical_bytes'] for r in rows if r['volume'] == v) for v in volumes},
        allocated_bytes={v: _sum_upper(r['allocated_bytes'] for r in rows if r['volume'] == v) for v in volumes},
        guaranteed_bytes={v: sum(r['guaranteed_bytes'] for r in rows if r['volume'] == v) for v in volumes},
        possible_bytes={v: _sum_upper(r['possible_bytes'] for r in rows if r['volume'] == v) for v in volumes},
        objects=rows)


def objective(s, ids):
    r = relief(s, ids)
    return (sum(max(0, n - r['guaranteed_bytes'][v]) for v, n in s.need.items()),
            len(ids), sum(r['logical_bytes'].values()), tuple(sorted(ids)))


def _individual(s, c):
    r = relief(s, (c.id,))
    # Match reclaim.rs's all-or-unknown rule, rather than reinterpreting unknown as zero.
    known = all(o['total_links'] == len(o['selected_names']) and o['allocated_bytes'] is not None
                and o['resident'] is True and not o['unknowns'] for o in r['objects'])
    return r['guaranteed_bytes'][c.volume] if known else None


def ranking_prefix(s, eligible):
    """Explicit hypothetical consumer of current within-group ranking, not product code."""
    def key(c):
        verified = _individual(s, c)
        by_name = {n: f.allocated_bytes for f in s.files for n in f.names}
        allocated = _sum_upper(by_name[n] for n in c.names)
        return (verified is None, -(verified or 0), allocated is None, -(allocated or 0), c.id)
    ordered = sorted((c for c in eligible if c.volume in s.need), key=key)
    chosen = []
    for c in ordered:
        if objective(s, chosen)[0] == 0:
            break
        if compatible(s, chosen, c):
            chosen.append(c.id)
    return dict(order=[c.id for c in ordered], selected=sorted(chosen), objective=list(objective(s, chosen)))


def solve(s, node_budget=50000):
    validate(s)
    if type(node_budget) is not int or not 0 <= node_budget <= 200000:
        raise ValueError('node budget must be an integer in 0..200000')
    eligible = sorted((c for c in s.candidates if not excluded(s, c)), key=lambda c: c.id)
    baseline = ranking_prefix(s, eligible)
    incumbent = min(((), tuple(baseline['selected'])), key=lambda ids: objective(s, ids))
    best = objective(s, incumbent)
    frontier = [(0, ())]
    visited = 0

    def relaxed_shortfall(i, chosen):
        # Superset of every descendant, ignoring ancestry. Guaranteed relief is monotone.
        return objective(s, chosen + tuple(c.id for c in eligible[i:]))[0]

    while frontier and visited < node_budget:
        i, chosen = frontier.pop()
        visited += 1
        score = objective(s, chosen)
        if score < best:
            incumbent, best = chosen, score
        if i == len(eligible) or relaxed_shortfall(i, chosen) > best[0]:
            continue
        frontier.append((i + 1, chosen))
        if compatible(s, chosen, eligible[i]):
            frontier.append((i + 1, chosen + (eligible[i].id,)))
    lower = min([best[0]] + [relaxed_shortfall(i, chosen) for i, chosen in frontier])
    ceiling = relief(s, tuple(c.id for c in eligible))['possible_bytes']
    impossible = any(ceiling[v] is not None and ceiling[v] < n for v, n in s.need.items())
    status = ('proved_in_static_model' if best[0] == 0 else 'impossible_even_optimistically' if impossible
              else 'not_guaranteed_by_facts' if lower > 0 else 'search_incomplete')
    rows = []
    for c in s.candidates:
        rejection = excluded(s, c)
        conflicts = [d.id for d in s.candidates if d.id in incumbent and d.id != c.id
                     and (d.id in ancestors(s, c) or c.id in ancestors(s, d))]
        why = (rejection or ('ancestry conflict with ' + ', '.join(conflicts) if conflicts else
                            'no requested relief on this volume' if c.volume not in s.need else
                            'outside the incumbent; search incomplete' if frontier else
                            'not selected by the exact shortfall/count/logical/ID objective'))
        if c.id in incumbent:
            without = objective(s, tuple(i for i in incumbent if i != c.id))[0]
            why = f'included: removing this choice increases guaranteed shortfall by {without - best[0]} bytes'
        rows.append(dict(id=c.id, volume=c.volume, parent=c.parent, names=list(c.names), meaning=c.meaning,
                         selected=c.id in incumbent, eligible=rejection is None, reason=why,
                         individual_verified_bytes=None if rejection else _individual(s, c),
                         facts=relief(s, (c.id,))))
    return dict(schema_version=1, scope='synthetic static model only', executable=False,
                filesystem_changed=False, permission='none; inspection is not removal approval',
                hypothesis='All selected names cease referencing their objects; no retained open handles, snapshots or shared extents. Recycling or renaming alone gives zero immediate relief.',
                need_bytes=dict(s.need), selected=sorted(incumbent), objective=list(best),
                relief=relief(s, incumbent), target_status=status, optimistic_relief_ceiling_bytes=ceiling,
                search=dict(nodes_visited=visited, node_budget=node_budget, budget_exhausted=bool(frontier),
                            optimal_in_model=not frontier, shortfall_lower_bytes=lower, shortfall_upper_bytes=best[0],
                            bound_scope='minimum total per-volume guaranteed shortfall; count/logical tie-breaks unbounded until complete'),
                ranking_baseline=baseline, candidates=rows)


def specimen():
    return Scenario(
        files=(FileFact('fast', 'shared', 120, 12, True, 2, ('build-link', 'cache-link')),
               FileFact('fast', 'extra', 40, 4, True, 1, ('extra-link',)),
               FileFact('fast', 'single', 200, 7, True, 1, ('single-link',)),
               FileFact('fast', 'mystery', 80, 8, True, None, ('mystery-link',)),
               FileFact('fast', 'unmeasured', 1, None, None, 1, ('unknown-link',)),
               FileFact('fast', 'pinned', 90, 9, True, 1, ('pinned-link',)),
               FileFact('archive', 'shared', 500, 50, True, 1, ('archive-link',))),
        candidates=(Candidate('project', 'fast', ('build-link', 'extra-link'), meaning='Fictional build tree; rebuild cost unknown'),
                    Candidate('build', 'fast', ('build-link',), parent='project'),
                    Candidate('cache', 'fast', ('cache-link',), meaning='Fictional cache sharing the build object'),
                    Candidate('standalone', 'fast', ('single-link',)),
                    Candidate('mystery', 'fast', ('mystery-link',)),
                    Candidate('unmeasured', 'fast', ('unknown-link',)),
                    Candidate('pinned', 'fast', ('pinned-link',), excluded_reason='pinned; outside selectable set'),
                    Candidate('other-volume', 'archive', ('archive-link',))),
        need={'fast': 16})


def inspect_plan(r):
    def size(x):
        return 'unknown' if x is None else str(x)
    def upper(x):
        return 'unbounded (unknown)' if x is None else str(x)
    lines = ['# Synthetic reclaim inspection - NOT AN ACTION PLAN', '',
             'All labels and byte quantities are fictional. No scan or file operation occurred.', '',
             '**Permission:** ' + r['permission'] + '.', '', '**Hypothesis:** ' + r['hypothesis'], '',
             '**Target status:** ' + r['target_status'], '',
             '| Volume | Requested relief | Selected logical | Selected allocation | Guaranteed relief | Possible relief |',
             '|---|---:|---:|---:|---:|---:|']
    for v in r['relief']['logical_bytes']:
        q = r['relief']
        lines.append(f"| {v} | {r['need_bytes'].get(v, 0)} | {q['logical_bytes'][v]} | {size(q['allocated_bytes'][v])} | {q['guaranteed_bytes'][v]} | {upper(q['possible_bytes'][v])} |")
    q = r['search']
    lines += ['', f"Selected: {', '.join(r['selected']) or 'do nothing'}.", '',
              f"Search: {q['nodes_visited']}/{q['node_budget']} nodes; budget exhausted: {q['budget_exhausted']}; optimal in this model: {q['optimal_in_model']}.",
              f"Minimum guaranteed-shortfall bound: [{q['shortfall_lower_bytes']}, {q['shortfall_upper_bytes']}] bytes. {q['bound_scope']}.", '',
              '## Choices and exclusions', '', '| Choice | Selected | Individual verified relief | Explanation |', '|---|---|---:|---|']
    for c in r['candidates']:
        lines.append(f"| {c['id']} | {c['selected']} | {size(c['individual_verified_bytes'])} | {c['reason']}; {c['meaning']} |")
    lines += ['', '## Object facts and unknowns', '',
              'Objects below are counted once per (volume, file identity). Observed names may occur in nested choices.', '',
              '| Volume / identity | Observed names | Total links | Logical | Allocation | Resident | Facts / unknowns |',
              '|---|---|---:|---:|---:|---|---|']
    objects = {(o['volume'], o['file_id']): o for c in r['candidates'] for o in c['facts']['objects']}
    selected = {(o['volume'], o['file_id']): o for o in r['relief']['objects']}
    for key, o in sorted(objects.items()):
        o = selected.get(key, o)
        lines.append(f"| {o['volume']} / {o['file_id']} | {', '.join(o['observed_names'])} | {size(o['total_links'])} | {o['logical_bytes']} | {size(o['allocated_bytes'])} | {o['resident']} | {'; '.join(o['unknowns']) or 'input allocation and link count known'} |")
    b = r['ranking_baseline']
    lines += ['', '## Ranking comparison', '',
              'Current reclaim deliberately supplies an individual ranking and does not sum nested candidates. It has no set-selection claim.', '',
              f"A hypothetical antichain-aware prefix of that within-group ranking chooses {', '.join(b['selected']) or 'nothing'}; objective {b['objective'][:3]}.",
              f"This set search chooses {', '.join(r['selected']) or 'nothing'}; objective {r['objective'][:3]} (guaranteed shortfall, choice count, unique logical bytes).", '',
              'Logical size is not relief. Unknowns are not zeros: zero is a conservative lower bound. A possible upper bound is not an achievable promise.', '',
              'No native identity/freshness, actual free-space increase, durability, recoverability, semantic disposability or owner approval is proved.', '']
    return '\n'.join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--need', type=int, default=16, help='fictional fast-volume relief target, bytes')
    parser.add_argument('--node-budget', type=int, default=50000)
    parser.add_argument('--json', action='store_true')
    args = parser.parse_args()
    try:
        result = solve(replace(specimen(), need={'fast': args.need}), args.node_budget)
    except ValueError as error:
        parser.error(str(error))
    print(json.dumps(result, indent=2, sort_keys=True) if args.json else inspect_plan(result))


if __name__ == '__main__':
    main()
