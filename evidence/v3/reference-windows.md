# LW-001: reference Windows witness

Executed 25 October 2026 from the `loomward-4` worktree, based on
`17aa6108a01dc972024659c43cb0ed1dee4d70fb`. Windows 11, OS version 10.0.26300
(build 26300), Python 3.14.3, psutil 7.2.2, standard non-elevated token
(`IsUserAnAdmin() == False`). No elevation was requested.

## What was missing

The scanner identity fix already exists in `a4b47b8f` and is pinned by
`tests/test_inventory.py::InventoryTests.test_identity_matches_fresh_stat_and_open_handle`.
The [20 October review](backlog-review-2026-10-20.md) retained #4 because its original
selected-root and first-sample CPU witness was missing. This change supplies that witness;
it makes no changes to the frozen `python/loomward/` oracle.

The browser reference was retired in q-40. The new check exercises its retained Python
API through `App`, real Windows filesystem reads, SQLite and live read-only telemetry.
The separately executed browser checks exercise the current Svelte app with its test
transports; they do not re-establish the retired browser UI or prove native engine behavior.

## Disposable scope and observed results

`py -3 scripts/test_reference_windows.py`: **2 tests passed, 0 skipped**.
The script prints only an anonymized platform/scope summary, never process names or IDs.
It refuses elevation; a non-Windows direct invocation reports UNVERIFIED and exits nonzero.

Each case creates a fresh temporary directory under the worktree's ignored `.loomward/`.
The explicitly selected root is its `selected-root/` child; the SQLite state is a sibling,
outside the scanned root. Only invented files are used: `a.txt` and `nested/b.txt` each hold
five bytes (`alpha`), while same-sized `c.txt` holds different bytes (`bravo`). Temporary
fixtures are removed by the test cleanup. No owner-selected personal folder was scanned.

| Case | Executed observation |
|---|---|
| Metadata scan | Observed mode, exact three paths and sizes, complete coverage without errors/skips; no demo inventory |
| Content consent | Missing and false consent both rejected; initial duplicate result is unknown (`None`) |
| Explicit duplicate request | 4096-byte budget, no exhaustion/skips, content bytes read, exactly the two equal files grouped; same-sized different content excluded; 5 logical duplicate bytes, reclaimable bytes unknown, deletion unauthorized |
| Feedback restart | A separate Python interpreter creates an `App`, commits one human Documents label and exits; a newly initialized `App` reads exactly that event, trains on one example and verifies the audit chain |
| First CPU sample | Actual observed process rows are nonempty; sample interval and every first-sample machine CPU percentage are `None`, never zero; no individual process metadata retained in this receipt |
| Mutation boundary | Fixture file names, bytes, inode identities and write times unchanged after each case; file/process mutation capabilities remain false; only read-only reference operations and disposable feedback-state writes invoked |

Reading may change access times; this witness does not claim access-time preservation.
Creating and removing the disposable fixture and writing its isolated SQLite feedback are
test setup/state effects, not changes to an owner file. No process-control operation is called.

## Regression sensitivity (fails-without-fix control)

There was no remaining production-code defect to repair. To establish that the new fixture
detects the historical defect rather than merely accepting any result, the following controls
were run against the real Windows fixture with in-memory fault injection. They never edit
the frozen oracle or replace platform execution with a synthetic pass.

Save this control as `.loomward/lw001-negative-controls.py`, then run
`py -3 .loomward/lw001-negative-controls.py`:

```python
import io
from pathlib import Path
import sys
import unittest
from unittest.mock import patch
sys.path.insert(0, str(Path.cwd() / 'scripts'))
import test_reference_windows as witness
from loomward.inventory import scan
from loomward.telemetry import Telemetry

real_snapshot = Telemetry.snapshot

def zero_identity(*args, **kwargs):
    result = scan(*args, **kwargs)
    for row in result['files']:
        row.update(device=0, inode=0, nlink=0)
    return result

def zero_cpu(self, **kwargs):
    result = real_snapshot(self, **kwargs)
    for row in result['processes']:
        row['cpu_percent_of_machine'] = 0
    return result

controls = [
    ('historical Windows zero-identity scanner defect',
     'test_selected_root_duplicates_and_feedback_restart',
     'loomward.server.scan', zero_identity),
    ('first-sample CPU incorrectly zero',
     'test_first_live_cpu_sample_is_unknown',
     'loomward.telemetry.Telemetry.snapshot', zero_cpu),
]
for label, test, target, replacement in controls:
    output = io.StringIO()
    with patch(target, replacement):
        suite = unittest.TestSuite([witness.ReferenceWindowsTests(test)])
        result = unittest.TextTestRunner(stream=output, verbosity=2).run(suite)
    print(label)
    print(output.getvalue())
    if len(result.failures) != 1 or result.errors:
        raise SystemExit('FAIL: the regression control did not produce one assertion failure')
print('PASS: both regressions caused the new Windows witness to fail as required')
```

Results: each control ran one case and produced exactly one **expected assertion failure**,
with no setup/import errors. The zero-identity control failed at duplicate `skipped_count`
(`3 != 0`); the zero-CPU control failed at `assertIsNone` (`0 is not None`). The unmodified
reference passes both cases. These are fault-injection controls, not claims of an unfixed
production defect or of a newly applied scanner/telemetry fix.

## Complete applicable checks

All commands below ran in this worktree. `CARGO_TARGET_DIR` remained the inherited shared
cache throughout; no target directory was overridden or cleaned.

| Command | Result |
|---|---|
| `py -3 scripts/verify.py` | Python: 299 run, 297 passed, 2 skipped; Rust format passed. Rust tests/lint initially failed because shared-cache artifacts disappeared during execution; see retry detail below |
| `cargo clippy --workspace --all-targets -j 2 -- -D warnings` | Passed with per-process `CARGO_INCREMENTAL=0` |
| `cargo test --workspace --no-fail-fast -j 2` | Exit 0: 2007 passed, 0 failed, 5 ignored across 87 targets, including doc tests; per-process `CARGO_INCREMENTAL=0` |
| `npm.cmd --prefix app ci --no-audit --no-fund` | Passed; dependencies installed only in ignored `app/node_modules` |
| `npm.cmd --prefix app run check` | Passed, 0 errors, 0 warnings |
| `npm.cmd --prefix app run test` | 53 test files, 677 tests passed |
| `npm.cmd --prefix app run build` | Passed |
| `py -3 scripts/test_app.py` | Exit 0, `ALL PASS`, no console/page errors or dialogs; every request stayed on loopback |
| `git diff --check` | Passed |

The full Python suite also ran verbosely to name every skip:

```powershell
py -3 -c "import sys,unittest; sys.path.insert(0,'python'); suite=unittest.defaultTestLoader.discover('tests'); log=open('.loomward/lw001-python.log','w',encoding='utf-8'); result=unittest.TextTestRunner(stream=log,verbosity=2).run(suite); log.close(); print('Ran',result.testsRun,'failures',len(result.failures),'errors',len(result.errors),'skips',len(result.skipped)); [print('SKIP',test.id(),reason) for test,reason in result.skipped]; sys.exit(not result.wasSuccessful())"
```

Result: 299 run, 0 failures, 0 errors, 2 skips:

- `test_build_book.BuildBookInventory.test_html_cover_and_navigation_use_the_same_inventory`:
  optional HTML renderer `mistune` is not installed.
- `test_inventory.InventoryTests.test_symlink_not_followed`: symlink privilege unavailable.

The initial Rust run lost test executables, dependency `.rlib` files and incremental cache
files in the shared cache. Crash-sweep children failed to spawn with Windows error 2; later
targets could not execute at all. Clippy failed moving its incremental dependency graph with
Windows error 3. These are recorded failures, not dismissed as flaky. A retry disables only
incremental compilation and limits build jobs, leaving source and the shared target unchanged:

```powershell
py -3 -c "import os,subprocess,sys; log=open('.loomward/lw001-clippy-retry.log','w',encoding='utf-8'); result=subprocess.run(['cargo','clippy','--workspace','--all-targets','-j','2','--','-D','warnings'],env=dict(os.environ,CARGO_INCREMENTAL='0'),stdout=log,stderr=subprocess.STDOUT); log.close(); sys.exit(result.returncode)"
py -3 -c "import os,subprocess,sys; log=open('.loomward/lw001-rust-retry.log','w',encoding='utf-8'); result=subprocess.run(['cargo','test','--workspace','--no-fail-fast','-j','2'],env=dict(os.environ,CARGO_INCREMENTAL='0'),stdout=log,stderr=subprocess.STDOUT); log.close(); sys.exit(result.returncode)"
```

Both retries exited 0. Thus every executed `verify.py` leg is green after the scoped
Rust retries; the full wrapper was not repeated after those successful checks. Rust's five
ignored cases are `million_row_output_interiors_do_not_increase_detector_work` (1M-row
benchmark), `freshness::targeted_tests::f3_issue782_measure_full_versus_targeted`
(measurement), `handlers::scrub_copy_tests::large_sparse_backup_memory_probe` (manual
1 GiB probe), `effects::exec::tests::durable_stop_20k_lab`, and
`effects::exec::tests::throughput_lab`. None was enabled or claimed passed.

Local raw logs and the control script are in ignored `.loomward/lw001-*`; this public receipt
retains only anonymized results. No raw host inventory, process rows, paths or credentials are
added to Git. No build outputs were intentionally created outside normal ignored output/cache
directories. No Git-writing command was run.

## NOT verified / residual risk

This is functional Windows reference evidence, not a native identity/effect safety gate,
performance comparison, WebView2 test or manual accessibility/field acceptance (#24/#101).
The two named Python skips and five ignored Rust cases remain unverified. Shared-cache artifact disappearance was observed;
the external cause of that disappearance was not established. No personal file/process mutation,
cloud transfer, packaging or publication is authorized by this receipt. `HUMAN_TODO.md` remains
unchanged; this issue adds no owner action and does not resolve its existing manual gates.
