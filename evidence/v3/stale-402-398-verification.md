# Older follow-ups #402 and #398: reconciliation, 17 October 2026

Lane: `stale-402`, branch `chore/402-398-verify-close`, Windows 11 on KRASPYON.
The initially clean worktree HEAD and freshly fetched `origin/main` both resolved to
`00bb610b4a50c8ab5449cd69efd507ff0ad03437`. Changes are uncommitted for the driver.
No issue, PR, release, upstream forum or owner-decision record was modified.

## Every issue item

| Item | Status and evidence |
|---|---|
| #402.1: misleading content-read tag | Fixed here. `session.hello` adds `capabilities.observation.duplicate_content_read` (Windows: true; other platforms and mock transport: false). It describes budgeted, local, read-only duplicate inspection on explicit request within an active grant. Grants & health labels it “Content read: duplicates only, on request,” separately from the ten file/process effects. The legacy `effects.content_read=false` field remains compatible and is documented as no file bytes returned. DTO, schema, examples, generated types, mock, diagnostic fixture size/digest, browser fixtures and preview-note caveat follow the same change. No capability is opened. |
| #402.2: catalogue archives, including migration reachability | Already fixed by `5ec41d6`, merged in #562 (`e4981a3`). `forget_backups` removes timestamped `catalog.corrupt-*` and `catalog.before-v1-*` databases and sidecars. Existing `preserved_catalogue_copies_are_swept_with_their_sidecars` and `v1_migration_forget_preserves_the_other_grant_and_clears_every_copy` cover archives and real v1 migration respectively. New supplemental test `forget_removes_the_archive_created_by_corrupt_catalogue_recovery` drives actual recovery, verifies the produced archive holds synthetic bytes, then calls public Forget and checks no holder remains. |
| #402 comment: ignored removal errors | Already fixed by #562 (`5ec41d6`); strengthened by #576 (`2bce89d`, merge `ab4ff58`) for WAL/SHM sidecars and catalogue scrub health. Failed cleanup returns a visible pending/error disposition and durable retry marker; it is retried automatically and at startup. Existing `failed_backup_removal_reports_and_restart_retries_every_owned_copy` and busy migration tests exercise failure and preservation. |
| #402 comment: whole-file allocation and UTF-8/UTF-16 split matches | Already fixed by #562 (`5ec41d6`). `holds_reader` buffers 64 KiB plus overlap. Existing `streaming_backup_check_preserves_split_matches_and_read_failures` checks both encodings across chunk boundaries, a bounded reader, and a mid-stream failure. |
| #402 comment: read failure treated as proof of retained bytes | Superseded by #562's explicit fail-closed policy (`dd0cbb4`) and #576's error test (`2bce89d`). `holds` intentionally returns true on an open/read error for an unknown legacy copy: that owned, unrecognised copy is removed, rather than certified clean. A failed removal stays visibly pending. Known migration copies are scrubbed by exact forgotten grant IDs; a failed scrub preserves the rollback copy and reports pending. The suggested `Result<bool>` was not needed to implement this disposition. |
| #402 comment: directory-open/per-entry errors disappear | Already fixed by #562 (`5ec41d6`): `read_dir` failure propagates; individual entry errors record failure while the sweep continues. New supplemental `backup_directory_open_failure_is_not_an_empty_success` executes the open-error branch. Per-entry iterator failure was source-verified; no OS enumeration fault was injected. |
| #398: non-ASCII cutoff | Already fixed by #428 (`d25ae27`, merge `f2affd4`). `sqlite_dir` counts UTF-16 units. Existing `sqlite_path_length_budget_counts_utf16_not_utf8` and exact-boundary tests pin it. |
| #398: verbatim-only components | Already fixed by #428 (`d25ae27`, plus zero-port fix `2e313ae`). Trailing dot/space, device aliases and other Win32-normalised names retain verbatim identity. Existing path/zero-port regressions and the actual Windows leading-space SQLite-VFS test distinguish unsafe conversion from ordinary paths. |
| #398: empty rest and lossy conversion | Already fixed by #428 (`d25ae27`): `.first()` handles the empty prefix and `to_str()` refuses lossy conversion. Existing incomplete-prefix tests cover the panic; the non-Unicode Unix fixture is not a Windows qualification. |
| #398: reader test hang | Already bounded by `f675865` in #574 (`772922d`): a reusable condition-variable barrier times out after 60 seconds instead of waiting forever if a peer dies. `03ad8a2` additionally supplies watchdog thread diagnostics. The eight-reader/60-round lock-race test remains concurrent. No new rendezvous implementation or intentional 60-second dead-peer run was added here. |
| #398: one-shot SQLite opens | Consistency cleanup here: both service preflight opens use a separate `sqlite_destination`; canonical paths still drive grant/static/profile checks and read-only flags stay read-only. `53e52c6` (#397) normalised only the live-state attachment used during backup cleanup; the copy itself was still opened verbatim. That outstanding SQLite open now uses `sqlite_dir(copy)` too. This changes only SQLite's path spelling, not grants, retention, SQL, effect gates or access modes. Existing state-path, startup/preflight, migration and scrub tests exercise the seam. |
| #398: upstream report | Reproduced here and prepared below with a standalone attachment, exact source ID, compile options and a successful plain-path control. Still open for external submission and maintainer disposition; neither is claimed. |

## Regression-first evidence

- Before production edits, `cargo test -p loomward-service hello_reports_duplicate_content_reads_separately_from_effects`
  failed: 0 passed, 1 failed, missing observation was `null`, expected `true` on Windows.
  After the change: 1 passed, 0 failed. It checks synthetic and personal sessions and retains the all-effects-false assertion.
- Before the mock edit, `npm.cmd --prefix app test -- tests/unit/mock.test.ts`
  failed: 12 passed, 1 failed, missing observation property. Full app suite after the fix: 488 passed.
- The corrupt-recovery and directory-open tests supplement already-fixed behaviour; no new Forget production policy was added.
- One-shot normalisation is the issue's consistency tidy, not a newly claimed lock-race repair. Existing safety and SQLite-path tests remain intact.
- The first app suite exposed the changed diagnostic fixture's old byte count/digest; both were recomputed from canonical bytes and the unchanged integrity test passed on rerun.
- The first browser attempt caught the second, outdated 11-effect assertion. Both browser assertions now require all ten file/process effects off and separately check duplicate content reads. No effect assertion was dropped without replacement.

## SQLite upstream handoff (not a submitted report)

Dependency pins from this checkout: `rusqlite 0.38.0`, `libsqlite3-sys 0.36.0`, bundled
SQLite **3.51.1**, `SQLITE_SOURCE_ID`:
`2025-11-28 17:28:25 281fc0e9afc38674b9b0991943b9e9d1e64c6cbdb133d35f6f5c87ff6af38a88`.
The version/source ID were read directly from the installed pinned amalgamation header.

Historical local evidence: #397 (`53e52c6`) records the unnormalised eight-reader test failing
at round 0–3, then passing with ordinary drive paths. This lane independently reproduced the
symptom with the attached [standalone source](sqlite-verbatim-wal-repro.rs), using only std
and the pinned rusqlite dependency, without linking any Loomward implementation.

Platform: Microsoft Windows 11 Home, version `10.0.26100`, build `26100`.
Both processes printed the version and source ID above. Each uses eight reader connections,
100 rounds and three rendezvous points per round: start, all snapshots open, all commits done.
Each round writes one new frame, then checks `PRAGMA wal_checkpoint(TRUNCATE)` after every
reader has committed. Each mode creates a distinct, exclusively new synthetic directory.

Observed output:

```text
mode=plain
PASS: 100 checkpoints after eight simultaneous reader commits
exit=0

mode=verbatim
retained_lock round=1 busy=1 frames=1 checkpointed=1
exit=3
```

The program exits on worker panic and has a 45-second watchdog, so a failed rendezvous cannot
hang indefinitely. It returns 3 for the reproduced symptom, 2 for a panic, and 124 for timeout.
It refuses to reuse an existing fixture directory. The result establishes the path-dependent
checkpoint symptom; the proposed in-process shared-lock accounting race is still a diagnosis
for upstream to investigate, not an upstream-confirmed fact.

Exact `PRAGMA compile_options` output from both modes:

```text
ATOMIC_INTRINSICS=0,COMPILER=msvc-1929,DEFAULT_AUTOVACUUM,DEFAULT_CACHE_SIZE=-2000,DEFAULT_FILE_FORMAT=4,DEFAULT_FOREIGN_KEYS,DEFAULT_JOURNAL_SIZE_LIMIT=-1,DEFAULT_MMAP_SIZE=0,DEFAULT_PAGE_SIZE=4096,DEFAULT_PCACHE_INITSZ=20,DEFAULT_RECURSIVE_TRIGGERS,DEFAULT_SECTOR_SIZE=4096,DEFAULT_SYNCHRONOUS=2,DEFAULT_WAL_AUTOCHECKPOINT=1000,DEFAULT_WAL_SYNCHRONOUS=2,DEFAULT_WORKER_THREADS=0,DIRECT_OVERFLOW_READ,ENABLE_API_ARMOR,ENABLE_COLUMN_METADATA,ENABLE_DBSTAT_VTAB,ENABLE_FTS3,ENABLE_FTS3_PARENTHESIS,ENABLE_FTS5,ENABLE_LOAD_EXTENSION,ENABLE_MEMORY_MANAGEMENT,ENABLE_RTREE,ENABLE_STAT4,HAVE_ISNAN,MALLOC_SOFT_LIMIT=1024,MAX_ATTACHED=10,MAX_COLUMN=2000,MAX_COMPOUND_SELECT=500,MAX_DEFAULT_PAGE_SIZE=8192,MAX_EXPR_DEPTH=1000,MAX_FUNCTION_ARG=1000,MAX_LENGTH=1000000000,MAX_LIKE_PATTERN_LENGTH=50000,MAX_MMAP_SIZE=0x7fff0000,MAX_PAGE_COUNT=0xfffffffe,MAX_PAGE_SIZE=65536,MAX_SQL_LENGTH=1000000000,MAX_TRIGGER_DEPTH=1000,MAX_VARIABLE_NUMBER=32766,MAX_VDBE_OP=250000000,MAX_WORKER_THREADS=8,MUTEX_W32,SOUNDEX,SYSTEM_MALLOC,TEMP_STORE=1,THREADSAFE=1,USE_URI
```

For upstream reproduction, use a standalone Cargo binary with the attached file as `src/main.rs`
and pin both dependency levels (a future compatible sys-crate patch must not change SQLite):

```toml
[dependencies]
rusqlite = { version = "=0.38.0", features = ["bundled"] }
libsqlite3-sys = "=0.36.0"
```

Run the two modes as separate processes, supplying new ordinary local fixture directories;
the source canonicalises each and selects its verbatim or plain spelling. No special VFS is
selected, no application code is linked, and no file-content inventory is involved.
This lane compiled directly against the already-built pinned library to avoid another
shared-pool Cargo build; the exact command is in the final verification table.

External next action: submit this result and the attached source through the upstream report
channel using an authorised upstream identity. No such submission was performed in this lane.
Do not present the current passing catalogue test (which applies `sqlite_dir`) as the raw reproduction.
The official [WAL documentation](https://www.sqlite.org/wal.html) describes WAL readers and
checkpointing; the [SQLite-owned Windows VFS source](https://github.com/sqlite/sqlite/blob/version-3.51.1/src/os_win.c)
is the version-pinned implementation to investigate. Those sources alone do not establish a race.

## Verification and limits

All commands run inside `C:/lw/wt/stale-402`; cargo uses `CARGO_BUILD_JOBS=4` and the
inherited shared `CARGO_TARGET_DIR=C:/lw/target/pool-b`. No clean/delete was run against that pool.

Checks already completed:

| Command | Result |
|---|---|
| `npm.cmd --prefix C:/lw/wt/stale-402/app ci --no-audit --no-fund` | 66 packages installed locally; no package/lockfile change. |
| `npm.cmd --prefix C:/lw/wt/stale-402/app run check` | 416 files, 0 errors, 0 warnings; visual boundary checks passed. |
| `npm.cmd --prefix C:/lw/wt/stale-402/app test` | 47 files, 488 tests passed, 0 failed after updating the diagnostic digest. |
| `npm.cmd --prefix C:/lw/wt/stale-402/app run build` | PASS, production assets built. |
| `py -3 C:/lw/wt/stale-402/scripts/gen_contracts_ts.py --check` | PASS, generated types current. |
| `cargo fmt --manifest-path C:/lw/wt/stale-402/Cargo.toml --all --check` | PASS. |
| `rustfmt --edition 2021 --check C:/lw/wt/stale-402/evidence/v3/sqlite-verbatim-wal-repro.rs` | PASS. |
| `git -C C:/lw/wt/stale-402 diff --check` | PASS. |
| `py -3 C:/lw/wt/stale-402/.loomward/stale-402-browser.py` | PASS: existing `check_health` and `check_add_folder` scenarios; 1360/375 px capability screenshots, no page errors. Runner only orchestrates existing browser fixtures and tests; ignored local artifact. |
| `impeccable.cmd detect --json C:/lw/wt/stale-402/app/src/views/health/View.svelte` | PASS, empty findings array. |

Raw SQLite probe build/run (PowerShell, actual invocations):

```powershell
$taskRusqlite = Get-ChildItem -LiteralPath C:/lw/target/pool-b/debug/deps -Filter 'librusqlite-*.rlib' | Sort-Object LastWriteTime -Descending | Select-Object -First 1
rustc --edition 2021 C:/lw/wt/stale-402/evidence/v3/sqlite-verbatim-wal-repro.rs --extern "rusqlite=$($taskRusqlite.FullName)" -L dependency=C:/lw/target/pool-b/debug/deps -o C:/lw/wt/stale-402/.loomward/sqlite-verbatim-wal-repro.exe
& C:/lw/wt/stale-402/.loomward/sqlite-verbatim-wal-repro.exe plain C:/lw/wt/stale-402/.loomward/sqlite-probe-plain-1
& C:/lw/wt/stale-402/.loomward/sqlite-verbatim-wal-repro.exe verbatim C:/lw/wt/stale-402/.loomward/sqlite-probe-verbatim-1
```

The compile succeeded without diagnostics; the plain control returned 0, the verbatim probe
returned the expected symptom code 3 at round 1. This is a defect reproduction, not a failed
Loomward proving test.

Full browser gate **did not pass**:
`py -3 C:/lw/wt/stale-402/scripts/test_app.py --view-shots C:/lw/wt/stale-402/.loomward/stale-402-view-shots`
passed the capability checks, then stopped at Atlas hover median **30 fps**, below **50 fps**.
The one isolated remeasure,
`py -3 C:/lw/wt/stale-402/.loomward/stale-402-browser.py --atlas`, also measured 30 fps.
Its initial-layout gate passed (47.1 ms in the full attempt, 38.5 ms isolated, budget 50 ms).
`git diff --exit-code -- app/src/views/atlas app/tests/e2e/test_atlas.py` was clean. No Atlas test,
threshold or source was changed. A clean-main performance baseline was not independently built,
so this is recorded as an unchanged-seam failure, not asserted to be a proven pre-existing flake.
The rest of the full browser suite was not reached; the changed health flows were exercised
directly instead.

Ignored local artifacts: `.loomward/` contains only synthetic probe stores, executable, browser
runner and screenshots; `app/node_modules/` and `app/dist/` are local dependencies/builds.
Final ignored status also includes `scripts/__pycache__/` and a worktree-local `target/`;
both are preserved, and the latter's contents were not inventoried or removed. All explicit
Cargo invocations used the inherited shared pool; no target directory was manually deleted.
Keep the screenshots if needed before a coordinator removes this worktree; the source probe
and curated results are tracked candidates under `evidence/v3/`. The worktree is not removal-ready
until the driver commits/pushes its uncommitted changes.

`HUMAN_TODO.md` is unchanged. Its answered q-10 already authorises the scoped duplicate reads;
this change neither opens a safety gate nor needs a new product decision. Status/checkpoint,
AGENTS and harness declarations were deliberately left to the driver as instructed.

NOT verified: hosted CI, native Tauri shell, Linux, physical filesystem faults, per-entry
directory-enumeration fault injection, and an upstream submission. No whole-workspace suite
was run (the driver owns it). Residual risk: new engine/app/contract versions must travel
together; blocked Forget cleanup remains visibly pending until retry can complete; upstream
SQLite's underlying verbatim-path symptom remains, with Loomward's existing workaround retained.

Scoped Rust result:

```powershell
$env:CARGO_BUILD_JOBS='4'
cargo test --manifest-path C:/lw/wt/stale-402/Cargo.toml -p loomward-service -p loomward-protocol -- --test-threads=4
```

- Protocol: **54 passed**, 0 failed (12 contract, 10 envelope fuzz, 29 L1a, 3 grammar).
- Service unit suite: **315 passed, 4 failed, 1 existing ignored** (manual 1 GiB memory probe).
  The new hello/corrupt-recovery/directory-error tests, all Forget cases, startup/preflight
  and plain-state-path tests, the canonical diagnostic digest test, and personal duplicate
  inspection/revocation tests passed.
- One strict protocol test initially rejected adding descriptive metadata to an effect's
  schema node. The metadata was removed, preserving the original exact `{"const":false}`
  nodes and the unchanged test. The 54-test result is the subsequent passing run.
- Cargo stopped after the failed service unit suite. Its already-built integration executable
  was identified by its embedded `stale-402` source path and run directly:
  `C:/lw/target/pool-b/debug/deps/root_coverage-affb18b824c7edf8.exe --test-threads=1`:
  **1 passed**, 0 failed.

Each failed service case was then run alone against the same just-built unit executable:

```powershell
C:/lw/target/pool-b/debug/deps/loomward_service-b941c2d5cb106a6f.exe --exact <name> --test-threads=1 --nocapture
```

| Exact test name | Full run failure | Isolated result |
|---|---|---|
| `tests::a_junction_replacing_a_directory_stays_visible_as_excluded` | `PartialCoverage: aggregates are being repaired` | 1 passed, exit 0 |
| `tests::a_scan_started_past_its_deadline_reports_the_started_job` | Expected deadline error, received queued-job receipt | 1 passed, exit 0 |
| `tests::copy_tests::lab_copy::sources_from_two_granted_roots_are_refused` | Freshness never caught up | 1 passed, exit 0 |
| `tests::a_duplicates_job_started_past_its_deadline_reports_the_started_job` | Expected deadline error, received queued-job receipt | **Still fails**, exit 101 |

The first three failed in the parallel run and passed alone, in unchanged test paths,
consistent with the host's documented load-dependent failures. The fourth remains an unresolved unchanged deadline-test/receipt behavior;
do not call it a flake or claim a green service suite. The relevant test and duplicate handler
are unchanged in this lane; a clean-main executable was not independently rebuilt, so no
claim of executed clean-main equivalence is made. Next action for the driver: reconcile that
deadline expectation with the #565 started-job receipt semantics in its existing service lane.
It is outside the #402/#398 capability/Forget/path work and was not weakened or silently dropped.

The original catalogue path regression and rendezvous tests were source-verified against main,
not rerun as another crate suite; this lane ran only the two crates it changed. The standalone
SQLite probe separately exercised the Windows VFS. Service/protocol doc tests were not reached
after Cargo stopped on unit failures.

Final scoped clippy:

```powershell
$env:CARGO_BUILD_JOBS='4'
cargo clippy --manifest-path C:/lw/wt/stale-402/Cargo.toml -p loomward-service -p loomward-protocol --all-targets -- -D warnings
```

**PASS**, exit 0, no diagnostics. Elapsed 22m 19s includes the shared target-directory lock wait.
No new code changes followed these checks; the final edit only completes this evidence record.

Driver disposition: #402's requested repairs/confirmations are accounted for; #398's local
items are accounted for but its upstream submission remains open. Review/commit the candidate
changes together, preserve the two wider gate failures above, and keep #398 open for the
external report. No owner/product choice is needed for the local fixes.
