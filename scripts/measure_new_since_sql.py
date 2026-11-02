"""Bounded synthetic SQL-only #1098 measurement, not native or end-to-end timing.

Runs the three production tree queries extracted from new_since.rs against catalog.sql.
No migrations, filesystem inventory, extra indexes, or per-run history. VM steps include
only these queries; native row conversion, ranking heaps, snapshot setup and paths do not.
"""
import json
import re
import sqlite3
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = (ROOT / 'crates/loomward-catalog/src/new_since.rs').read_text()
TREE = re.search(r'const TREE: &str = "([^"]+)";', SOURCE).group(1)
QUERIES = [s.replace('{TREE}', TREE) for s in re.findall(r'"(\{TREE\} [^"]+)"', SOURCE)]
assert len(QUERIES) == 3


def measure(files, directories):
    db = sqlite3.connect(':memory:')
    db.executescript((ROOT / 'crates/loomward-catalog/src/catalog.sql').read_text())
    db.execute("INSERT INTO root(id,grant_id,state) VALUES(1,1,'complete')")
    db.executemany("INSERT INTO dir(id,root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run,sub_complete) VALUES(?,1,?,?,'path_observation','none',1,16,'complete',?,2,1)",
                   ((i, None if i == 1 else max(1, i // 10), f'd{i}', 0 if i == 1 else 2) for i in range(1, directories + 2)))
    db.executemany("INSERT INTO file(dir_id,name,logical,allocated,attrs,born_run,seen_run) VALUES(?,?,1,1,32,2,2)",
                   ((2 + i % directories, f'f{i}') for i in range(files)))
    db.commit()
    results = []
    for query in QUERIES:
        ticks = 0
        def progress():
            nonlocal ticks
            ticks += 1
            return 0
        db.set_progress_handler(progress, 1000)
        args = (1, 1, 56, 5) if '?4' in query else (1, 1, 56)
        start = time.perf_counter()
        rows = 0
        value = None
        for row in db.execute(query, args):
            rows += 1
            value = row[0]
        elapsed = (time.perf_counter() - start) * 1000
        db.set_progress_handler(None, 0)
        results.append({'rows': rows, 'count': value if 'count(*)' in query else None,
                        'vm_steps_approx': ticks * 1000, 'elapsed_ms': round(elapsed, 2),
                        'plan': [r[3] for r in db.execute('EXPLAIN QUERY PLAN ' + query, args)]})
    assert results[0]['rows'] == files and results[1]['count'] == directories
    steps = sum(r['vm_steps_approx'] for r in results)
    db.close()
    return {'files': files, 'directories_excluding_root': directories,
            'three_query_vm_steps_approx': steps, 'budget': 32_000_000,
            'within_query_budget': steps < 32_000_000, 'queries': results}


if __name__ == '__main__':
    print(json.dumps({'sqlite': sqlite3.sqlite_version, 'scope': __doc__,
                      'measurements': [measure(100_000, 10_000), measure(500_000, 50_000)]}, indent=2))
