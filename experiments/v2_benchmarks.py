"""Reproduce the v1 counterexamples and measure the snapshot-only query path.

No real disks, Windows API, model, filesystem operation or competitor is benchmarked.
All timing results are environment-specific; output is JSON for rerunning locally.
"""
from __future__ import annotations
import copy
import json
import platform
import random
import statistics
import sys
import time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'python'))
sys.path.insert(0,str(ROOT/'experiments'))
from loomward.catalog import Catalog
from loomward.planner import plan_tiers
from loomward.planner_v2 import plan_tiers_v2
from reference_smoke import group,oracle_shortfall


def cases():
    rng=random.Random(20261009)
    result=[{'source_id':'fast','target_free_bytes':30,'max_transfer_bytes':10,'volumes':[dict(id='fast',capacity_bytes=100,free_bytes=10,reserve_bytes=5,tier=0,online=True,writable=True),dict(id='slow',capacity_bytes=100,free_bytes=80,reserve_bytes=10,tier=1,online=True,writable=True)],'groups':[group('colder-small',10,10,.01),group('large',20,10,.1)]}]
    for i in range(50):
        result.append({'source_id':'fast','target_free_bytes':10+rng.randint(50,140),'max_transfer_bytes':rng.randint(40,180),'volumes':[dict(id='fast',capacity_bytes=1000,free_bytes=10,reserve_bytes=5,tier=0,online=True,writable=True)]+[dict(id=k,capacity_bytes=1000,free_bytes=rng.randint(40,160),reserve_bytes=20,tier=n,online=True,writable=True) for n,k in enumerate(['warm','slow'],1)],'groups':[group(j,rng.randint(8,50),rng.randint(8,60),rng.choice([.01,.1,.2]),rng.randint(8,60)) for j in range(6)]})
    return result


def p95(values):return sorted(values)[max(0,int(.95*len(values))-1)]

def measure(fn,n=30):
    times=[]
    for _ in range(n):
        start=time.perf_counter();value=fn();times.append((time.perf_counter()-start)*1000)
    return {'repetitions':n,'median_ms':statistics.median(times),'p95_ms':p95(times)},value


def run():
    rows=[]
    for i,s in enumerate(cases()):
        old=plan_tiers(s);start=time.perf_counter();new=plan_tiers_v2(s,node_budget=5000);elapsed=(time.perf_counter()-start)*1000;best=oracle_shortfall(s)
        assert new['shortfall_bytes']==best,(i,new['shortfall_bytes'],best)
        assert new['transfer_bytes']<=s['max_transfer_bytes']
        assert all(new['projected_free_bytes'][v['id']]>=v['reserve_bytes'] for v in s['volumes'] if v['id']!=s['source_id'])
        assert all(not p['executable'] for p in new['proposals'])
        rows.append({'case':i,'v1_shortfall':old['shortfall_bytes'],'v2_shortfall':new['shortfall_bytes'],'oracle_shortfall':best,'v2_search_complete':new['search']['complete'],'v2_nodes':new['search']['nodes_visited'],'v2_ms':elapsed})
    count=50000
    snap={'schema_version':1,'files':[{'id':f'f{i}','name':f'asset-{i:06}.txt','relative_path':f'project-{i%40:02}/asset-{i:06}.txt','extension':'.txt','size_bytes':100+i%5000,'flags':[]} for i in range(count)]}
    serialized=len(json.dumps(snap,separators=(',',':')).encode())
    start=time.perf_counter()
    with Catalog(snap,disclose_names=True,query_step_budget=10000000) as catalog:
        build_ms=(time.perf_counter()-start)*1000
        first_time,first=measure(lambda:catalog.search(limit=50))
        # Cursor acquisition is deliberately outside timing. Deep-offset comparison
        # times equivalent SQL rows against a position already held by the caller.
        after=catalog._db.execute('SELECT sort_size,item_ref FROM catalog ORDER BY sort_size,item_ref LIMIT 1 OFFSET 39999').fetchone()
        key_time,key_rows=measure(lambda:catalog._db.execute('SELECT item_ref,size_bytes FROM catalog WHERE (sort_size,item_ref)>(?,?) ORDER BY sort_size,item_ref LIMIT 50',tuple(after)).fetchall(),100)
        offset_time,offset_rows=measure(lambda:catalog._db.execute('SELECT item_ref,size_bytes FROM catalog ORDER BY sort_size,item_ref LIMIT 50 OFFSET 40000').fetchall(),100)
        assert [tuple(r) for r in key_rows]==[tuple(r) for r in offset_rows]
        index_plan=catalog.query_plan()
        catalog_result={'rows':count,'build_ms':build_ms,'first_page_including_projection_and_cursor':first_time,'page_rows':len(first['items']),'page_json_bytes':len(json.dumps(first,separators=(',',':')).encode()),'full_input_json_bytes':serialized,'sql_deep_page_keyset':key_time,'sql_deep_page_offset':offset_time,'sql_rows_identical':True,'query_plan':index_plan,'limits':'Warm in-memory SQLite; no filesystem I/O. SQL microbenchmark excludes API serialization/cursor validation. Cursor position known in advance. Not a cold/native/end-to-end benchmark.'}
    return {'schema_version':2,'platform':platform.platform(),'python':platform.python_version(),'seed':20261009,'scope':'Synthetic reference algorithms only','tier':{'cases':len(rows),'v1_missed_feasible':sum(r['oracle_shortfall']==0 and r['v1_shortfall']>0 for r in rows),'v2_missed_feasible':sum(r['oracle_shortfall']==0 and r['v2_shortfall']>0 for r in rows),'v1_positive_regret_cases':sum(r['v1_shortfall']>r['oracle_shortfall'] for r in rows),'v2_positive_regret_cases':sum(r['v2_shortfall']>r['oracle_shortfall'] for r in rows),'all_v2_searches_complete':all(r['v2_search_complete'] for r in rows),'constraint_violations_in_checked_cases':0,'median_v2_ms':statistics.median(r['v2_ms'] for r in rows),'maximum_v2_nodes':max(r['v2_nodes'] for r in rows),'rows':rows},'catalog':catalog_result}

if __name__=='__main__':print(json.dumps(run(),indent=2))
