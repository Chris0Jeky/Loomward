"""Reproducible synthetic smoke probes, not Windows or competitor benchmarks."""
from __future__ import annotations
import itertools
import json
import platform
import random
import statistics
import sys
import tempfile
import time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'python'))
from loomward.inventory import scan
from loomward.planner import plan_tiers

def oracle_shortfall(s):
    """Exhaustive tiny-case oracle for eligible source groups and capacity/budget only."""
    source=next(v for v in s['volumes'] if v['id']==s['source_id'])
    targets=[v for v in s['volumes'] if v['id']!=source['id'] and v['online'] and v['writable'] and v['tier']>=source['tier']]
    groups=[g for g in s['groups'] if g['volume_id']==source['id'] and not(g['pinned'] or g['active'] or g['protected']) and g['heat'] is not None and g['heat']<=.25 and g['days_since_move']>=s.get('cooldown_days',7)]
    best=max(0,s['target_free_bytes']-source['free_bytes'])
    for choices in itertools.product(range(len(targets)+1),repeat=len(groups)):
        used=[0]*len(targets);cost=relief=0
        for g,c in zip(groups,choices):
            if c:used[c-1]+=g['destination_bytes'];cost+=g['transfer_bytes'];relief+=g['source_bytes']
        if cost<=s['max_transfer_bytes'] and all(v['free_bytes']-u>=v['reserve_bytes'] for v,u in zip(targets,used)):
            best=min(best,max(0,s['target_free_bytes']-source['free_bytes']-relief))
    return best

def group(i,relief,cost,heat,dest=None):
    return dict(id=str(i),volume_id='fast',source_bytes=relief,destination_bytes=dest or relief,transfer_bytes=cost,heat=heat,pinned=False,active=False,protected=False,days_since_move=90)
def main():
    rng=random.Random(20261009)
    cases=[]
    explicit={'source_id':'fast','target_free_bytes':30,'max_transfer_bytes':10,'volumes':[dict(id='fast',capacity_bytes=100,free_bytes=10,reserve_bytes=5,tier=0,online=True,writable=True),dict(id='slow',capacity_bytes=100,free_bytes=80,reserve_bytes=10,tier=1,online=True,writable=True)],'groups':[group('colder-small',10,10,.01),group('large',20,10,.1)]}
    cases.append(explicit)
    for i in range(50):
        cases.append({'source_id':'fast','target_free_bytes':10+rng.randint(50,140),'max_transfer_bytes':rng.randint(40,180),'volumes':[dict(id='fast',capacity_bytes=1000,free_bytes=10,reserve_bytes=5,tier=0,online=True,writable=True)]+[dict(id=k,capacity_bytes=1000,free_bytes=rng.randint(40,160),reserve_bytes=20,tier=n,online=True,writable=True) for n,k in enumerate(['warm','slow'],1)],'groups':[group(j,rng.randint(8,50),rng.randint(8,60),rng.choice([.01,.1,.2]),rng.randint(8,60)) for j in range(6)]})
    regrets=[];missed=0
    for case in cases:
        plan=plan_tiers(case);best=oracle_shortfall(case)
        assert plan['shortfall_bytes']>=best
        assert plan['transfer_bytes']<=case['max_transfer_bytes']
        assert all(plan['projected_free_bytes'][v['id']]>=v['reserve_bytes'] for v in case['volumes'] if v['id']!=case['source_id'])
        assert all(not p['executable'] for p in plan['proposals'])
        regrets.append(plan['shortfall_bytes']-best)
        missed+=int(best==0 and plan['shortfall_bytes']>0)
    with tempfile.TemporaryDirectory(prefix='loomward-smoke-') as d:
        root=Path(d)
        for n in range(5000):(root/f'fixture-{n:05}.txt').write_bytes(b'synthetic smoke fixture\n')
        times=[]
        for _ in range(3):
            start=time.perf_counter();snapshot=scan(root,max_entries=10000);times.append(time.perf_counter()-start)
            assert snapshot['summary']['file_count']==5000 and snapshot['coverage']['unqualified_complete']
        observed=snapshot['summary']['logical_bytes']
    result={'mode':'synthetic_experiment','platform':platform.system(),'python':platform.python_version(),'not_a_windows_or_competitor_benchmark':True,'scan':{'files':5000,'logical_bytes':observed,'repetitions':3,'seconds':times,'median_seconds':statistics.median(times),'cache_conditions':'Immediately created tiny files on ephemeral container storage; no cold-cache control. No UI/index persistence/NTFS scan.'},'tier_oracle':{'seed':20261009,'cases':len(cases),'case_units':'abstract byte units, not physical disk measurements','constraint_violations':0,'greedy_missed_feasible_target_cases':missed,'cases_with_positive_shortfall_regret':sum(r>0 for r in regrets),'maximum_shortfall_regret':max(regrets),'explicit_counterexample':{'greedy_shortfall':plan_tiers(explicit)['shortfall_bytes'],'oracle_shortfall':oracle_shortfall(explicit)},'interpretation':'The greedy baseline preserves these tested constraints but is not a feasibility-complete or optimal allocator. This experiment does not model crash safety, groups changing, filesystem allocation or real usage heat.'}}
    print(json.dumps(result,indent=2))
if __name__=='__main__':main()
