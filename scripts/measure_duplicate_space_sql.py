"""#1104 SQL-only measurement; no native cache, Windows or disk-throughput claim.

Extract the production candidate-count SQL and real visibility views. Use the
checked-in schemas and indexes. Compare the previous rescan query on the same
eligibility CTE. First means the first SELECT on a freshly seeded in-memory DB;
warm means repeated SQL, not a hit in the Rust candidate-space cache.
"""
import json
import re
import sqlite3
import statistics
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / 'crates/loomward-catalog/src'
SOURCE = (CATALOG / 'duplicates.rs').read_text()
TEMPLATE = re.search(r'const ELIGIBLE_SIZES: &str = "([^"]+)";', SOURCE).group(1)
BODY = SOURCE.split('fn candidate_space(', 1)[1].split('/// Completed candidate-space', 1)[0]
TAIL = re.search(r'"\{\}\s*(SELECT [^"]+)"', BODY).group(1)
PRODUCTION = TEMPLATE
for old, new in re.findall(r'\.replace\(\s*"([^"]+)",\s*"([^"]+)"\s*\)', BODY):
    PRODUCTION = PRODUCTION.replace(old, new)
PRODUCTION += ' ' + TAIL
assert '{' not in PRODUCTION, 'unresolved production SQL template'
PREVIOUS = (TEMPLATE.replace('{cols}', 'f.logical, f.flags')
            .replace('{size_cols}', 'logical')
            + ' SELECT count(*), coalesce(sum(logical),0) FROM eligible'
              ' WHERE logical IN (SELECT logical FROM sizes)')
VIEWS = re.search(r'execute_batch\("(CREATE TEMP VIEW root_grant[^"]+)"\)',
                  (CATALOG / 'db.rs').read_text()).group(1)
# Same explicit selection exclusions: hardlink_suspected (bit 2) stays eligible.
FLAGS = sum(1 << bit for bit in (0, 1, 3, 4, 5, 6, 9, 10))


def fixture(shape, count=130_000):
    db = sqlite3.connect(':memory:')
    db.executescript((CATALOG / 'catalog.sql').read_text())
    db.execute("ATTACH ':memory:' AS st")
    # Execute the actual state schema in the attached database, then attach the views.
    state = sqlite3.connect(':memory:')
    state.executescript((CATALOG / 'state.sql').read_text())
    for statement in state.iterdump():
        if statement.startswith('CREATE TABLE '):
            db.execute(statement.replace('CREATE TABLE ', 'CREATE TABLE st.', 1))
        elif statement.startswith(('CREATE INDEX ', 'CREATE UNIQUE INDEX ')):
            statement = re.sub(r'^(CREATE (?:UNIQUE )?INDEX )', r'\1st.', statement)
            db.execute(statement)
    state.close()
    db.executescript(VIEWS)
    db.execute("INSERT INTO st.root_grant(id,volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES(1,'synthetic',zeroblob(16),'Synthetic root','fixture','fixture','active',0)")
    db.execute("INSERT INTO root(id,grant_id,state) VALUES(1,1,'complete')")
    db.executemany("INSERT INTO dir(id,root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run,sub_complete) VALUES(?,1,?,?,'path_observation','none',?,16,'complete',1,1,1)",
                   ((i, None if i == 1 else 1, f'd{i}', 0 if i == 1 else 1)
                    for i in range(1, (count + 999) // 1000 + 2)))
    db.executemany("INSERT INTO file(dir_id,name,logical,allocated,attrs,born_run,seen_run) VALUES(?,?,?,4096,32,1,1)",
                   ((2 + i // 1000, f'f{i}', 10 if shape == 'one_size' else 10 + i % 20_000)
                    for i in range(count)))
    db.commit()
    return db


def timed(db, query):
    ticks = 0
    def progress():
        nonlocal ticks
        ticks += 1
        return 0
    db.set_progress_handler(progress, 1000)
    start = time.perf_counter()
    answer = db.execute(query, (1, 1, FLAGS)).fetchone()
    elapsed = (time.perf_counter() - start) * 1000
    db.set_progress_handler(None, 0)
    return {'answer': answer, 'ms': round(elapsed, 2), 'steps_approx': ticks * 1000}


def parity():
    """Small hand-counted cases pin flags, floor, ancestry, grants and overflow."""
    db = fixture('one_size', 12)
    db.execute('DELETE FROM file')
    db.execute("INSERT INTO dir(id,root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run,flags) VALUES(3,1,1,'shield','path_observation','none',1,16,'excluded',1,1,0),(4,1,3,'child','path_observation','none',2,16,'complete',1,1,0),(5,1,1,'gone','path_observation','none',1,16,'absent_pending',1,1,0)")
    rows = [(2, 10, 0), (2, 10, 4), (2, 20, 0), (2, 20, 0), (2, 20, 0),
            (2, 30, 0), (2, 0, 0), (2, 0, 0), (2, -1, 0), (2, -1, 0),
            (4, 10, 0), (4, 10, 0), (5, 10, 0), (5, 10, 0)]
    rows += [(2, 10, 1 << bit) for bit in (0, 1, 3, 4, 5, 6, 9, 10)]
    db.executemany("INSERT INTO file(dir_id,name,logical,allocated,attrs,born_run,seen_run,flags) VALUES(?,?,?,4096,32,1,1,?)",
                   ((directory, f'p{i}', size, flags) for i, (directory, size, flags) in enumerate(rows)))
    cases = 0
    for floor, expected in [(1, (5, 80)), (20, (3, 60)), (21, (0, 0))]:
        for query in (PREVIOUS, PRODUCTION):
            assert db.execute(query, (1, floor, FLAGS)).fetchone() == expected
            cases += 1
    db.execute("UPDATE dir SET flags=8 WHERE id=3")
    db.execute("UPDATE dir SET listing_state='complete' WHERE id=3")
    for query in (PREVIOUS, PRODUCTION):
        assert db.execute(query, (1, 1, FLAGS)).fetchone() == (5, 80)
        cases += 1
    db.execute("UPDATE st.root_grant SET state='revoked'")
    for query in (PREVIOUS, PRODUCTION):
        assert db.execute(query, (1, 1, FLAGS)).fetchone() == (0, 0)
        cases += 1
    db.execute("UPDATE st.root_grant SET state='active'")
    for sizes in [(5_000_000_000_000_000_000,) * 2,
                  (3_000_000_000_000_000_000,) * 2 + (3_000_000_000_000_000_001,) * 2]:
        db.execute('DELETE FROM file')
        db.executemany("INSERT INTO file(dir_id,name,logical,allocated,attrs,born_run,seen_run) VALUES(2,?,?,4096,32,1,1)",
                       ((f'o{i}', size) for i, size in enumerate(sizes)))
        for query in (PREVIOUS, PRODUCTION):
            try:
                db.execute(query, (1, 1, FLAGS)).fetchone()
            except sqlite3.OperationalError as error:
                assert 'integer overflow' in str(error)
            else:
                raise AssertionError('overflow must be an error, never a REAL or unknown')
            cases += 1
    db.close()
    return cases


def measure(shape, query):
    db = fixture(shape)
    first = timed(db, query)
    warm = [timed(db, query) for _ in range(5)]
    expected = (130_000, 1_300_000 if shape == 'one_size' else 1_251_235_000)
    assert tuple(first['answer']) == expected
    assert all(tuple(run['answer']) == expected for run in warm)
    plan = [row[3] for row in db.execute('EXPLAIN QUERY PLAN ' + query, (1, 1, FLAGS))]
    db.close()
    return {'first': first, 'warm_median_ms': statistics.median(run['ms'] for run in warm),
            'warm_steps_approx': [run['steps_approx'] for run in warm], 'plan': plan}


if __name__ == '__main__':
    result = {'scope': __doc__, 'sqlite': sqlite3.sqlite_version, 'parity_checks': parity(),
              'shapes': {shape: {name: measure(shape, query)
                                 for name, query in [('previous', PREVIOUS), ('production', PRODUCTION)]}
                         for shape in ['one_size', 'spread_20k']}}
    print(json.dumps(result, indent=2))
    if '--require-improvement' in sys.argv:
        spread = result['shapes']['spread_20k']
        assert spread['production']['first']['steps_approx'] < spread['previous']['first']['steps_approx'], \
            'the production count must use fewer VM steps on the 20k-size fixture'
