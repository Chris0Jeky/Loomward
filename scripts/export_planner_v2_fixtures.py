"""Write fixtures/v2/planner-v2-parity.json: Python plan_tiers_v2 outputs for the Rust port.

Suites: experiment-51 (the original seed-20261009 corpus, via experiments/v2_benchmarks.cases),
edge (hand-built boundaries), tie-fuzz and fuzz (seeded random), reject (inputs Python must
refuse). Every case records the exact input and node_budget; accepted cases record the full
Python output, rejected ones omit `output`. Rerun: py -3 scripts/export_planner_v2_fixtures.py
"""
from __future__ import annotations
import copy
import json
import random
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT / 'python'), str(ROOT / 'experiments')]
from loomward.planner_v2 import plan_tiers_v2  # noqa: E402
from v2_benchmarks import cases as experiment_cases  # noqa: E402
from reference_smoke import oracle_shortfall  # noqa: E402

OUT = ROOT / 'fixtures' / 'v2' / 'planner-v2-parity.json'
BIG = 2**53 - 1


def vol(i, cap=1000, free=100, reserve=20, tier=1, online=True, writable=True):
    return dict(id=i, capacity_bytes=cap, free_bytes=free, reserve_bytes=reserve, tier=tier,
                online=online, writable=writable)


def grp(i, src, xfer, heat=.1, dst=None, volume='fast', days=90, pinned=False, active=False, protected=False):
    return dict(id=i, volume_id=volume, source_bytes=src, destination_bytes=dst or src, transfer_bytes=xfer,
                heat=heat, pinned=pinned, active=active, protected=protected, days_since_move=days)


def scen(groups, goal=70, budget=100, vols=None, **extra):
    s = {'source_id': 'fast', 'target_free_bytes': goal, 'max_transfer_bytes': budget,
         'volumes': vols or [vol('fast', free=10, reserve=5, tier=0), vol('warm'), vol('slow', tier=2)],
         'groups': groups}
    s.update(extra)
    return s


def run(s, budget):
    """Python output, or the exception class name when Python refuses the input."""
    try:
        return plan_tiers_v2(copy.deepcopy(s), node_budget=budget), None
    except (ValueError, KeyError) as e:
        return None, type(e).__name__


def edge():
    c = {}
    c['empty-groups'] = scen([])
    c['missing-groups'] = {k: v for k, v in scen([]).items() if k != 'groups'}
    c['already-satisfied'] = scen([grp('a', 30, 30)], goal=5)
    c['goal-equals-free'] = scen([grp('a', 30, 30)], goal=10)
    c['goal-zero'] = scen([grp('a', 30, 30)], goal=0)
    c['pinned-active-protected'] = scen(
        [grp('pin', 40, 10, pinned=True), grp('act', 40, 10, active=True), grp('pro', 40, 10, protected=True),
         grp('ok', 40, 10), {k: v for k, v in grp('unspecified', 40, 10).items() if k not in ('pinned', 'active', 'protected')},
         grp('other-volume', 5, 5, volume='warm')], goal=50)
    c['short-circuit-flag-after-true'] = scen(  # Python never evaluates `active` once pinned is true
        [dict(grp('a', 40, 10, pinned=True), active=None), grp('b', 40, 10)], goal=50)
    c['eligibility-boundaries'] = scen(
        [grp('heat-025', 20, 5, heat=.25), grp('heat-just-over', 20, 5, heat=.2500001), grp('hot', 20, 5, heat=.9),
         grp('heat-unknown', 20, 5, heat=None), grp('days-none', 20, 5, days=None), grp('days-6', 20, 5, days=6),
         grp('days-7', 20, 5, days=7), grp('heat-int-zero', 20, 5, heat=0), grp('heat-1e-5', 20, 5, heat=1e-05)], goal=80)
    c['cooldown-zero'] = scen([grp('d0', 30, 10, days=0), grp('d1', 30, 10, days=1)], goal=40, cooldown_days=0)
    c['cooldown-custom'] = scen([grp('d9', 30, 10, days=9), grp('d10', 30, 10, days=10)], goal=40, cooldown_days=10)
    c['dest-offline'] = scen([grp('a', 30, 10)], goal=40, vols=[vol('fast', free=10, reserve=5, tier=0), vol('warm', online=False), vol('slow', tier=2)])
    c['dest-readonly'] = scen([grp('a', 30, 10)], goal=40, vols=[vol('fast', free=10, reserve=5, tier=0), vol('warm', writable=False), vol('slow', tier=2, writable=False)])
    c['dest-lower-tier'] = scen([grp('a', 30, 10)], goal=40, vols=[vol('fast', free=10, reserve=5, tier=1), vol('warm', tier=0), vol('slow', tier=1)])
    c['no-targets'] = scen([grp('a', 30, 10)], goal=40, vols=[vol('fast', free=10, reserve=5, tier=0)])
    c['source-offline'] = scen([grp('a', 30, 10)], goal=40, vols=[vol('fast', free=10, reserve=5, tier=0, online=False), vol('warm')])
    c['source-readonly'] = scen([grp('a', 30, 10)], goal=40, vols=[vol('fast', free=10, reserve=5, tier=0, writable=False), vol('warm')])
    two = [grp('x', 30, 40), grp('y', 30, 20)]
    c['budget-exactly-at-limit'] = scen(two, goal=70, budget=60)
    c['budget-one-short'] = scen(two, goal=70, budget=59)
    c['budget-zero'] = scen(two, goal=70, budget=0)
    c['budget-huge'] = scen(two, goal=70, budget=BIG)
    wv = lambda free, reserve: [vol('fast', free=10, reserve=5, tier=0), vol('warm', free=free, reserve=reserve), vol('slow', free=500, reserve=20, tier=2)]
    c['reserve-exactly-at-limit'] = scen([grp('a', 80, 10, dst=80)], goal=90, vols=wv(100, 20))
    c['reserve-one-over'] = scen([grp('a', 81, 10, dst=81)], goal=91, vols=wv(100, 20))
    c['reserve-cumulative'] = scen([grp('a', 40, 10, dst=40), grp('b', 40, 10, dst=41)], goal=90, vols=wv(101, 20))
    c['reserve-zero'] = scen([grp('a', 60, 10, dst=100)], goal=70, vols=wv(100, 0))
    c['independent-costs'] = scen([grp('a', 100, 5, dst=10), grp('b', 5, 100, dst=100), grp('c', 50, 50, dst=50)], goal=120, budget=60)
    many = [grp(f'g{i:02}', 8 + (i * 7) % 23, 5 + (i * 11) % 17, heat=[.01, .1, .2][i % 3], dst=6 + (i * 5) % 19) for i in range(12)]
    c['cutoff-node-budget'] = scen(many, goal=100, budget=120)
    c['cutoff-zero-budget'] = scen(many[:4], goal=30)
    c['too-large-13-groups'] = scen(many + [grp('g12', 9, 9)], goal=100, budget=120)
    c['too-large-5-targets'] = scen(many[:5], goal=40, vols=[vol('fast', free=10, reserve=5, tier=0)] + [vol(f't{i}', tier=1 + i % 3) for i in range(5)])
    c['too-large-heuristic-reserve-exact'] = scen(  # only the portfolio can find it: v1 is budget-blocked, search is skipped
        [grp('big', 60, 50, heat=.2, dst=80)] + [grp(f's{i:02}', 1, 1, heat=.01) for i in range(12)], goal=70, budget=60,
        vols=[vol('fast', free=10, reserve=5, tier=0), vol('warm', free=100, reserve=20)])
    c['exactly-12-groups'] = scen(many, goal=100, budget=120)
    c['ties-by-id'] = scen([grp(i, 20, 10, heat=.1) for i in 'cab'], goal=40, vols=[vol('fast', free=10, reserve=5, tier=0), vol('warm', tier=1), vol('also', tier=1)])
    c['unicode-ids'] = scen([grp('é', 20, 10), grp('z', 20, 10), grp('\U0001f600', 20, 10), grp('ａ', 20, 10)], goal=50)
    c['id-128-chars'] = scen([grp('a' * 128, 30, 10)], goal=40)
    c['large-bytes'] = scen(
        [grp('big', BIG // 4, BIG // 4, dst=BIG // 4), grp('bigger', BIG // 3, BIG // 3, dst=BIG // 3)], goal=BIG // 2, budget=BIG,
        vols=[vol('fast', cap=BIG, free=1000, reserve=0, tier=0), vol('warm', cap=BIG, free=BIG - 5, reserve=BIG // 10)])
    c['many-targets-4'] = scen(many[:6], goal=60, vols=[vol('fast', free=10, reserve=5, tier=0)] + [vol(f't{i}', free=40 + 3 * i, tier=1 + i % 2) for i in range(4)])
    c['heat-decimal-tie'] = scen([grp('a', 3, 1, heat=.1), grp('b', 3, 1, heat=.2), grp('c', 3, 1, heat=.3), grp('d', 3, 1, heat=0)], goal=16)
    c['source-in-middle'] = scen([grp('a', 30, 10)], goal=40, vols=[vol('warm'), vol('fast', free=10, reserve=5, tier=0), vol('slow', tier=2)])
    c['float-heat-1e-7'] = scen([grp('a', 10, 5, heat=1e-07), grp('b', 10, 5, heat=2e-07)], goal=20)
    return c


def reject():
    ok = lambda: scen([grp('a', 30, 10), grp('b', 30, 10)], goal=40)

    def mod(f):
        s = ok(); f(s); return s
    c = {
        'duplicate-group-id': mod(lambda s: s['groups'].append(grp('a', 5, 5))),
        'duplicate-volume-id': mod(lambda s: s['volumes'].append(vol('warm'))),
        'unknown-source': mod(lambda s: s.update(source_id='nope')),
        'group-unknown-volume': mod(lambda s: s['groups'].append(grp('z', 5, 5, volume='nope'))),
        'heat-above-one': mod(lambda s: s['groups'][0].update(heat=1.5)),
        'heat-negative': mod(lambda s: s['groups'][0].update(heat=-.1)),
        'heat-bool': mod(lambda s: s['groups'][0].update(heat=True)),
        'zero-source-bytes': mod(lambda s: s['groups'][0].update(source_bytes=0)),
        'float-byte-count': mod(lambda s: s['groups'][0].update(source_bytes=10.0)),
        'negative-transfer': mod(lambda s: s['groups'][0].update(transfer_bytes=-1)),
        'allocation-exceeds-used-capacity': mod(lambda s: s['groups'][0].update(source_bytes=991)),
        'free-above-capacity': mod(lambda s: s['volumes'][1].update(free_bytes=1001)),
        'tier-ten': mod(lambda s: s['volumes'][1].update(tier=10)),
        'goal-above-capacity': mod(lambda s: s.update(target_free_bytes=1001)),
        'budget-above-safe-integer': mod(lambda s: s.update(max_transfer_bytes=2**53)),
        'cooldown-too-large': mod(lambda s: s.update(cooldown_days=36501)),
        'days-negative': mod(lambda s: s['groups'][0].update(days_since_move=-1)),
        'volume-flag-null': mod(lambda s: s['volumes'][1].update(online=None)),
        'volume-flag-string': mod(lambda s: s['volumes'][1].update(writable='yes')),
        'group-flag-null': mod(lambda s: s['groups'][0].update(pinned=None)),
        'group-flag-string': mod(lambda s: s['groups'][0].update(active='no')),
        'empty-group-id': mod(lambda s: s['groups'][0].update(id='')),
        'group-id-129-chars': mod(lambda s: s['groups'][0].update(id='a' * 129)),
        'no-volumes': mod(lambda s: s.update(volumes=[])),
        'groups-null': mod(lambda s: s.update(groups=None)),
        'missing-goal': {k: v for k, v in ok().items() if k != 'target_free_bytes'},
    }
    return c


def fuzz(seed, count, tie):
    rng = random.Random(seed)
    heats = [.01, .02, .03, .05, .07, .1, .2, .25] if tie else [0, .01, .1, .2, .25, .3, .9, None]
    for n in range(count):
        n_groups = rng.randint(3, 8) if tie else rng.randint(0, 14)
        n_targets = rng.randint(1, 3) if tie else rng.randint(1, 5)
        vols = [vol('fast', free=rng.randint(0, 30), reserve=rng.randint(0, 10), tier=0,
                    online=rng.random() > .05, writable=rng.random() > .05)]
        vols += [vol(f't{i}', free=rng.randint(10, 200), reserve=rng.randint(0, 40), tier=rng.randint(0, 2),
                     online=rng.random() > .1, writable=rng.random() > .1) for i in range(n_targets)]
        groups = []
        for j in range(n_groups):
            g = grp(f'g{j:02}', rng.randint(3, 9) if tie else rng.randint(1, 60), 1 if tie else rng.randint(1, 60),
                    heat=rng.choice(heats), dst=rng.randint(1, 9) if tie else rng.randint(1, 60),
                    volume='fast' if rng.random() > .1 else 't0', days=rng.choice([0, 3, 7, 30, None]),
                    pinned=rng.random() < .05, active=rng.random() < .05, protected=rng.random() < .05)
            groups.append(g)
        yield f"{'tie-' if tie else ''}fuzz-{n:02}", scen(
            groups, goal=rng.randint(0, 120), budget=rng.randint(0, 200) if not tie else rng.randint(2, 12), vols=vols,
            cooldown_days=rng.choice([0, 7, 7, 14])), rng.choice([0, 1, 10, 100, 5000, 50000])


def main():
    out = []

    def add(suite, name, s, budget, note=None):
        output, err = run(s, budget)
        case = {'suite': suite, 'name': name, 'node_budget': budget, 'input': s}
        if note:
            case['note'] = note
        if err and suite != 'reject':
            raise SystemExit(f'{name}: unexpected {err}')
        elif suite == 'reject':
            assert err == 'ValueError', (name, err)
            case['python_error'] = err
        if output is not None:
            case['output'] = output
        out.append(case)

    for i, s in enumerate(experiment_cases()):
        add('experiment-51', f'experiment-{i:02}' + ('-explicit-counterexample' if i == 0 else ''), s, 5000)
        if i == 0:
            assert oracle_shortfall(s) == 0 and plan_tiers_v2(s, node_budget=5000)['shortfall_bytes'] == 0
    for name, s in edge().items():
        budget = {'cutoff-node-budget': 50, 'cutoff-zero-budget': 0}.get(name, 50000)
        add('edge', f'edge-{name}', s, budget)
    # Budget boundary: exactly the nodes a complete search needs, then one fewer.
    full, _ = run(edge()['cutoff-node-budget'], 200000)
    nodes = full['search']['nodes_visited']
    assert full['search']['complete']
    add('edge', 'edge-budget-equals-nodes-needed', edge()['cutoff-node-budget'], nodes)
    add('edge', 'edge-budget-one-below-nodes-needed', edge()['cutoff-node-budget'], nodes - 1)
    add('edge', 'edge-default-node-budget', edge()['pinned-active-protected'], 50000)
    for name, s in reject().items():
        add('reject', f'reject-{name}', s, 50000)
    for bad in (200001, -1, True):
        add('reject', f'reject-node-budget-{bad}', scen([grp('a', 30, 10)], goal=40), bad)
    for tie, seed, count in ((True, 7, 40), (False, 11, 60)):
        for name, s, budget in fuzz(seed, count, tie):
            add('tie-fuzz' if tie else 'fuzz', name, s, budget)
    # One compact case per line keeps the file small and its diffs reviewable.
    with OUT.open('w', encoding='utf-8', newline='\n') as f:
        f.write('[\n' + ',\n'.join(json.dumps(c, separators=(',', ':')) for c in out) + '\n]\n')
    kinds = {}
    for c in out:
        kinds[c['suite']] = kinds.get(c['suite'], 0) + 1
    print(f'{OUT.relative_to(ROOT)}: {len(out)} cases', kinds,
          'incomplete:', sum('output' in c and not c['output']['search']['complete'] for c in out))


if __name__ == '__main__':
    main()
