# 45. Daily-use readiness checklist (HUMAN_TODO q-39)

A living checklist. The owner will use Loomward daily only after it is extremely well tested on synthetic but
extreme scenarios that prove its safety (q-39, 2026-10-12). The owner alone judges "UX done"; no row below
speaks for it.

Rules for this file:

- **PASS** needs a recorded run of the proving command, with date, host and commit in the "Run record"
  section. Without one the row is PARTIAL or UNVERIFIED, whatever the code looks like.
- **PARTIAL**: something real exists and ran; the gap column says what is missing.
- **UNVERIFIED**: nothing proves it yet.
- Evidence cited from earlier waves was measured by them; this PR did not rerun it unless the run record says so.
- Synthetic data only. A row is never closed by a real-folder run (invariant 2).
- When a row moves, edit its line and add the run; keep the old run in the record.

| # | Bar item | Status |
|---|---|---|
| 1 | Crash injection at every write | PARTIAL |
| 2 | The 10M-entry lab | PARTIAL |
| 3 | Pathological trees | PARTIAL |
| 4 | Churn during scans | PASS |
| 5 | Injected I/O faults and a full disk | PARTIAL |
| 6 | Fuzzing every parser and contract | UNVERIFIED |
| 7 | 72-hour soak | UNVERIFIED |
| 8 | Power-cut simulation, per effect | UNVERIFIED |
| 9 | Proven restore, per effect | UNVERIFIED |

## 1. Crash injection at every write: PARTIAL

**Exists.** `crates/loomward-catalog/tests/crash_sweep.rs` over the `test-hooks` crash points in
`crates/loomward-catalog/src/crashpoint.rs`. Every durable boundary of the catalogue writer counts: each row
change, each commit before and after it reaches the log, each checkpoint (end of run, forget cut, shutdown),
`VACUUM st` and `VACUUM main`, the forget's marker clear, backup and rename. A clean
run enumerates them; one child process per boundary is ended there (`exit(86)`, no destructors, no graceful
close); the parent reopens and asserts the recovery invariants (integrity and foreign-key checks on both files, no
running run or half-built bulk table, no staged or pending-reference residue, no orphan rows, rollups agree with
rows, forget all-or-nothing in rows and bytes, every acknowledged step durable, a second recovery changes
nothing, the store still takes a write). Three sweeps: the lifecycle (scan, durable reference and labels, bulk
first scan, rescan, revoke, forget; the scanned root is published through staged chunks, as the service does), a schema migration from a v4-shaped store, and a second
crash inside the recovery of the first. The checker can fail: planted damage is caught (`the_verifier_catches_planted_damage`; two of its cases are damage
that opening the store already repairs, so they prove less than the others, #434) and a mutant that skips finishing a
half-done forget failed 13 of 240 (before F1's fix; 10 of 272 for the marker-recovery mutant after it)
crash points (see the run record). Every sweep child must exit with the crash code at its injected boundary, and
a clean run must exit 0: an unreached boundary fails the sweep (`an_unreached_crash_point_fails_the_sweep`
injects `boundaries+1`). Earlier single-point tests stay: `PublicationCrash` in
`src/writer/publication_tests.rs`, `grant_survives_a_crash_between_precious_and_derived_commits` and the
uncommitted-transaction child in `tests/writer.rs`, `tests/open.rs` corruption and interrupted-run cases.

**Gap.** A killed process is not a power cut (row 8). Writers outside the catalogue writer are not swept: the
service's `collections` connection that writes both files, label, learning and job state, backup import. A torn
`VACUUM INTO` backup file cannot be injected mid-copy by a boundary hook. The migration fixture is a synthetic
downgrade of a current store, not a store written by an old binary. Finding **F1** below (fixed).

**Prove.**

```powershell
cargo test -p loomward-catalog --features test-hooks --test crash_sweep   # about 53 s (2026-10-12), samples row boundaries
$env:LOOMWARD_CRASH_SWEEP='full'; cargo test -p loomward-catalog --features test-hooks --test crash_sweep   # every boundary, about 505 s (2026-10-12)
cargo test -p loomward-catalog --features test-hooks --test crash_sweep -- lifecycle_at 123   # one boundary
```

## 2. The 10M-entry lab: PARTIAL

**Exists.** `crates/loomward-lab` generates the 10M tier (E: scale lab only; `crates/loomward-lab/README.md`);
`evidence/v3/scan-l7.md` and `evidence/v3/bench/scan-10m.json`: 20 fresh native runs, p50 546,530 observed
entries/s, every run partial by design at the engine arena cap (48,769 directories observed), peak private commit
37.5 MiB. S and M tiers match their manifests exactly (`scan-s.json`, `scan-m.json`). Catalogue ingest at 100k
files: `evidence/v3/scan-throughput-310.md`; first-scan bulk load: `evidence/v3/first-scan-bulk-load-148.md`.

**Gap.** No complete 10M scan, and no 10M catalogue: the L runs use a sink with no SQLite, and the writer has not
ingested, queried, rolled up, forgotten (`VACUUM main` at that size is unmeasured) or crash-recovered at that size. G: stays capped at 2M by ADR-V3-17.

**Prove.** `loomward-lab bench scan --tier L` (E: lab, private root path in `.loomward/scan-roots.log`), then a
service scan of that root into a real catalogue with `tree.slice` and a forget; record RSS, wall time and WAL size.

## 3. Pathological trees: PARTIAL

| Case | Exists | Gap |
|---|---|---|
| Deep | Deep forget test; lab `pathological::Profile::DepthLimit` creates 512 directory identities including the root; engine regression `writer_depth_limit_512_scanned_end_to_end` | **#414: scanner cap 128 truncates the writer's 512-directory chain.** Reproduced partial coverage with `limit_hit=true`; regression retained `#[ignore]` with the defect named (#483) |
| Very long paths | Lab `LongPath`; engine `past_max_path_scanned_queried_and_forgotten` exercises native scan, catalogue query and forget beyond MAX_PATH | No remaining gap in this case on KRASPYON; see run record |
| Odd Unicode | Lab `UnpairedSurrogate`; engine checks exact UTF-16LE bytes, lossy display/name flag and forget | No remaining gap in this case on KRASPYON; see run record |
| Link loops | Lab `JunctionCycle` and `SymlinkCycle`; separate engine tests require visible/excluded cycle entries, no traversal, then forget | Both ran on KRASPYON; other hosts report symlink privilege error 1314 as UNVERIFIED, never elevate |
| Hard links | Lab `HardLinks`; engine checks 4,096 catalogue names, eight shared identities and eight `multilink` groups, then forget | No remaining gap in this case on KRASPYON; groups respect NTFS's per-file link limit |
| Denied folders | Lab `DeniedDirectory` applies a listing-deny ACE to its own temporary child; engine checks actual OS denial, visible denied coverage/flag, partial root, then forget; lab test verifies ACE restoration | No remaining gap in this case on KRASPYON; no elevated or system ACL change |
| Offline or placeholder files | `directory_becoming_reparse_or_offline_matches_fresh_scan`, `excluded_entries_stay_visible_uncounted_and_complete_a_full_traversal` | no Cloud Files provider; refusal is tested against synthetic attributes only |

**Prove.** `cargo test -p loomward-engine -p loomward-lab --test pathological -- --nocapture`.
`cargo test -p loomward-engine --test pathological writer_depth_limit_512 -- --ignored --nocapture`
explicitly reruns the open depth defect. The tests live in new files under each crate's `tests/`.
The engine integration tests exercise the real engine through the service's existing catalogue sink;
no fixture sink substitutes for durable publication/query/forget. Generation uses isolated temporary
directories under `CARGO_TARGET_DIR` (the system temporary directory when unset), outside the worktree.
Elevated hosts report native cases UNVERIFIED, as do hosts missing symlink privilege; the recorded
KRASPYON run was non-elevated and both link types actually ran. With `LOOMWARD_REQUIRE_NATIVE=1` those
skips fail instead, so a host that cannot run a native case cannot pass it vacuously.

## 4. Churn during scans: PASS

**Exists.** `evidence/v3/scan-robustness-201-184.md` (rename bursts, channel overflow reconciles to a fresh-scan
oracle), `evidence/v3/l16-change-tracking.md` and `change-tracking.json` (watch, USN catch-up),
`evidence/v3/relist-containment-269.md`, `evidence/v3/scan-cancellation-228.md`,
`evidence/v3/pending-publication-356.md` (references bound mid-publication).
`crates/loomward-engine/tests/pathological.rs` adds
`concurrent_create_delete_rename_grow_reconciles_exactly_to_fresh_scan`: a lab-generated tree,
a concurrent thread performing 128 create/delete/rename/grow batches (at least 8 must overlap enumeration:
the job is running and has published no file rows yet), followed by a quiescent refresh/reconciliation. Every stable file observation, relative topology and
directory count/byte/allocation/coverage rollup must equal a fresh scan by the same engine plus a filesystem
walk: the fresh scan goes into an empty catalogue, and the file paths and logical lengths must also equal a
`std::fs` walk of the final tree (reparse points not followed). Catalogue IDs, run/revision counters and enumeration-sensitive access times
are excluded from that comparison. Both stores register the same empty fixture before generation.

**Gap.** No remaining gap for this scenario on KRASPYON: the 100,000-file run passed (see record).
The default 4,000-file fixture keeps routine tests bounded. This proves convergence after an explicit
quiescent refresh, not complete coverage during endless churn or an atomic filesystem snapshot.

**Prove.** `$env:LOOMWARD_CHURN_FILES='100000'; cargo test -p loomward-engine --test pathological concurrent_create_delete_rename_grow -- --nocapture`.
Unset `LOOMWARD_CHURN_FILES` afterwards. The final workspace test uses the smaller default.

## 5. Injected I/O faults and a full disk: PARTIAL

**Exists.** SQL-level fault injection: a `BEFORE INSERT` trigger that aborts leaves the grant intact and the
store usable (`grant_survives_a_crash_between_precious_and_derived_commits`, `invalid_listing_rolls_back_and_...`);
the corrupt-database paths in `tests/open.rs`; `evidence/v3/backups-import.md` refusals.

**Gap.** No read-error, access-denied-mid-scan or short-write injection below SQLite; no full disk. SQLite
returns `SQLITE_FULL` under `PRAGMA max_page_count`, which can drive the full-disk case with no OS support:
write until full, require a clean error, free space, require the store to recover (extend `crash_sweep.rs`).

## 6. Fuzzing every parser and contract: UNVERIFIED

**Exists.** Fixed negative examples (`contracts/v3` valid and invalid examples), the offline mutation check
`crates/loomward-backup/tests/mutations.py`, decoder corruption tests in `loomward-lab`. No fuzzer.

**Gap.** No `cargo-fuzz` or property tests on: the directory-buffer decoder, backup receipt normaliser, protocol
and HTTP envelope parsers, cursors, idempotency keys, the Tauri IPC surface, the snapshot importer.

**Prove.** A fuzz target per parser with a corpus seeded from the fixtures; a recorded bounded run per target.

## 7. 72-hour soak: UNVERIFIED

Nothing runs for hours. `evidence/v3/telemetry*.md` are short samplers, not a soak. Needs a harness that cycles
scan, query, revoke, forget, restart and watch against a lab tree for 72 hours, recording handles, RSS, WAL size
and rows, with a pass rule fixed first.

## 8. Power-cut simulation, per effect: UNVERIFIED

Row 1 kills the process; the operating system's cache survives it, so un-synced writes are never lost. The
catalogue runs `synchronous=NORMAL` (state and publication-with-references use FULL), so a power cut can lose
recent catalogue commits by design. Needs a fault-injecting SQLite VFS or a VM with a dropped write cache,
dropping every write after the Nth fsync, then the row 1 invariants. File effects do not exist yet (invariant
1); each effect's gate ships its own power-cut case.

## 9. Proven restore, per effect: UNVERIFIED

No effect exists to restore from. Backups are evidence import only (`evidence/v3/backup-evidence-lw084.md`);
no restore has run (invariant 9). The store-level analogue, a start-up `VACUUM INTO` copy that opens, is covered
by `tests/open.rs`, not by a recovery drill.

## Findings from the crash sweep

**F1 (fixed in this PR; #391). Forget left staged-listing names in `catalog.db`.** The service publishes every
listing through `StageChunk` and `ListingDone` (`crates/loomward-service/src/sink.rs`). The staged rows are deleted
when the listing publishes, while `secure_delete` is off, so their bytes stay in the free space and unused page
tails of `catalog.db`; the forget only deleted rows and rewrote `state.db`, so the root's file and directory names
stayed in `catalog.db` after revoke and forget, after a graceful close. Fix: the forget's rewrite (`wal_cut`) now
runs `VACUUM main` as well as `VACUUM st` before the log cuts, and the forget's own `state.db` transaction writes a
`scrub_pending` marker (a `meta` row, no schema change) that is cleared only after both rewrites and both cuts. A
crash anywhere in between, including after the catalogue half has already committed, makes the next open (or the
writer at start-up) finish the rewrite; the sweep found that window by killing the process in it (10 of 272 crash
points with the marker recovery removed). Regression test, enabled:
`forget_leaves_no_staged_listing_names`. Cost: `VACUUM main` rewrites the whole catalogue, so a forget takes time
and temporary space proportional to its size (the copy may be built in RAM because the connection sets
`temp_store=MEMORY`, #434; plus WAL space on disk) and holds the writer meanwhile; measure it at 10M entries (row 2). Not
covered by F1: the older `state.start-*`, `state.before-v*` and `catalog.corrupt-*` copies (#391, #402).

The sweep found no recovery defect other than the window F1's marker closes.

## Run record

All 2026-10-17 patho-r34 cargo commands below used
`--manifest-path C:/lw/wt/patho-r34/Cargo.toml`, `CARGO_BUILD_JOBS=4` and
`CARGO_TARGET_DIR=C:\lw\target\patho-r34`. Raw logs stay outside Git in that lane target.

| Date | Commit | Host | Command | Result |
|---|---|---|---|---|
| 2026-10-12 | 00c3d1f + this PR (before merging main) | DESKTOP-IHKOOJS, Windows 11, debug build | `LOOMWARD_CRASH_SWEEP=full cargo test -p loomward-catalog --features test-hooks --test crash_sweep` | 7 passed in 355 s: lifecycle 240 of 240 boundaries, migration 13 of 13, 3,045 double crashes, no violation |
| 2026-10-12 | same | same | default mode of the same command | 7 passed in 29 s: lifecycle 132 of 240, migration 13 of 13, 143 double crashes |
| 2026-10-12 | same, with `purge_root` removed from the open-time forget completion (mutant, reverted) | same | `LOOMWARD_CRASH_SWEEP=full ... -- every_write_of_scan` | failed: 13 of 240 crash points (N=223 and later, the state half of the forget committed) |
| 2026-10-12 | e2833a9 (this PR merged with main 7cf4a54; the merge brought documentation only) | same | default mode of the same command | 7 passed, 1 ignored (F1) in 13 s; lifecycle 132 of 240, migration 13 of 13, 143 double crashes |
| 2026-10-12 | same | same | `cargo test --workspace --no-fail-fast`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `py -3 scripts/verify.py` | 949 passed, 1 ignored (F1), 0 failed; clippy clean; verify.py exit 0 |
| 2026-10-12 | round-1 head (F1 fixed, shutdown checkpoint and `VACUUM main` instrumented, strict exit guard), main a9299b4 | same | `LOOMWARD_CRASH_SWEEP=full cargo test -p loomward-catalog --features test-hooks --test crash_sweep` | 9 passed in 505 s: lifecycle 272 of 272 boundaries, migration 14 of 14, 3,870 double crashes, every child exited with the crash code, no violation |
| 2026-10-12 | same | same | default mode of the same command | 9 passed in 53 s (cold, parallel with a warm build): lifecycle 149 of 272, migration 14 of 14, 191 double crashes |
| 2026-10-12 | same, with the marker recovery removed (mutant, reverted) | same | `LOOMWARD_CRASH_SWEEP=full ... -- every_write_of_scan` | failed: 10 of 272 crash points (N=261 to 267, after the catalogue half of the forget committed), names left in the live store files |
| 2026-10-12 | same | same | `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --no-fail-fast`; `py -3 scripts/verify.py`; `git diff --check` | fmt and clippy clean; 951 passed, 0 ignored, 0 failed; verify.py exit 0; diff check clean |
| 2026-10-17 | 89df386 + uncommitted patho-r34 lane | KRASPYON, Windows 11 Home, non-elevated debug build | `cargo test -p loomward-engine -p loomward-lab --test pathological -- --nocapture --test-threads=1` | Engine 7 passed, 0 failed, 1 ignored (depth-cap defect); lab 4 passed. Both cycle types and real ACL denial/restoration ran. Churn: 4,000 files, 128 batches, 71 overlapped the scan; 4,041 exact observations matched the fresh scan |
| 2026-10-17 | same, initial regression run before depth ignore | same | `cargo test -p loomward-engine -p loomward-lab --test pathological -- --nocapture --test-threads=1` | Depth-512 regression failed: partial, limit_hit=true, 128 indexed child directories, 0 leaf files. Initial engine run 5 passed/3 failed; other two were test assumptions corrected to use public denied coverage and stored flags (slice nodes do not carry flags). No production assertion or existing test was weakened |
| 2026-10-17 | 89df386 + uncommitted patho-r34 lane | KRASPYON, Windows 11 Home, non-elevated debug build | `LOOMWARD_CHURN_FILES=100000 cargo test -p loomward-engine --test pathological concurrent_create_delete_rename_grow -- --nocapture` | 1 passed, 0 failed, 7 filtered out in 496.36 s, including generation and cleanup: 100,000 initial files, all 128 mutation batches overlapped the scan, 101,001 exact observations matched an independent fresh catalogue after refresh; temporary tree removed |
| 2026-10-17 | same | same | `cargo clippy -p loomward-lab -p loomward-engine --all-targets -- -D warnings` | Exit 0; final workspace clippy below covers the subsequent fixture/test edits |
| 2026-10-17 | same, before correcting the slice request depth to 8 | same | `RUST_TEST_THREADS=4 cargo test -p loomward-lab -p loomward-engine --no-fail-fast` | 237 passed, 12 failed, 1 ignored. Four known #292 engine failures; two existing native tests timed out; six new tests rejected the helper's invalid slice depth 64 (request maximum 8), corrected to 8. Lab profiles: all 5 passed, including ACL cleanup and a junction path containing a shell metacharacter |
| 2026-10-17 | same | same | `RUST_TEST_THREADS=1 cargo test -p loomward-engine --lib native_scan_job_reports_exact_final_progress -- --exact scan::tests::native_scan_job_reports_exact_final_progress --nocapture`; same command with filter `concurrent_scan_start_reserves_root_before_touching_watch` and exact name `scan::watch::native_tests::concurrent_scan_start_reserves_root_before_touching_watch` | Both timeout rechecks passed individually, 1 test each, 1.31 s and 1.39 s; assertions/deadlines unchanged |
| 2026-10-17 | 89df386 + final uncommitted patho-r34 lane | KRASPYON, Windows 11 Home | `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings` | Both exit 0; workspace clippy completed in 2m 54s |
| 2026-10-17 | same final lane | same, `RUST_TEST_THREADS=1`, default 4,000-file churn size | `cargo test --workspace --no-fail-fast` | 997 passed, 8 failed, 1 ignored; exit 101. All new cases passed: engine pathological target 7 passed/1 ignored (depth cap), lab pathological target 5 passed. Full lab crate 32 passed; full engine crate 213 passed/4 known #292 failures/1 ignored, including doc tests. The known Windows reopen test also failed (#292). Three additional unchanged service tests failed on timing/deadlines: `wrong_dataset_and_timed_out_providers_never_publish_fallback` (20 ms deadline, provider call count 0), `forgetting_a_revoked_root_purges_its_names_and_keeps_an_anonymised_record` (`roots.revoke` deadline), `placement_refuses_identity_unverified_rows` (read deadline). Both earlier engine timeout cases passed in this run. No failing existing test was edited or skipped |
| 2026-10-17 | same final lane, exact service test binary from the workspace run | same | `C:/lw/target/patho-r34/debug/deps/loomward_service-2cd7cfb8d03ab454.exe --exact inventory::tests::wrong_dataset_and_timed_out_providers_never_publish_fallback --nocapture --test-threads=1`; repeat for exact names `tests::forget_tests::forgetting_a_revoked_root_purges_its_names_and_keeps_an_anonymised_record` and `tests::placement_refuses_identity_unverified_rows` | All three passed separately (1 each), 0.92 s / 2.30 s / 1.47 s, with the same assertions, deadlines and binary. This does not turn the full workspace run green. A package-only cargo recheck was interrupted during compilation because it rebuilt a different feature graph; these checks reuse the workspace binary |
| 2026-10-17 | same final lane | same | `git -C C:/lw/wt/patho-r34 diff --check` | Exit 0. New pathological fixture directory empty after tests. Removed 60 disposable synthetic databases left by existing tests under this worktree's ignored `target/test-roots` (created during this run; absolute scope and absence of reparse entries checked). Raw logs remain under the external lane target, not in Git |
