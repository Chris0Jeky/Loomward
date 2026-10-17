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
| 4 | Churn during scans | PARTIAL |
| 5 | Injected I/O faults and a full disk | PARTIAL |
| 6 | Fuzzing every parser and contract | UNVERIFIED |
| 7 | 72-hour soak | PARTIAL |
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
| Deep | `forget.rs` `a_very_deep_tree_is_deleted_without_a_recursion_limit`; writer depth limit 512 (`tree depth limit`); lab 40-component paths | no engine scan of a tree at the depth limit |
| Very long paths | `loomward-lab` long-path profiles; `real_root_guard_sees_the_resolved_long_name`; `sqlite_dir` keeps `\\?\` handling | no scan past MAX_PATH end to end |
| Odd Unicode | lab UTF-16 names and case variants; `name_utf16`/`name_lossy` rows; `buffer_decoder_accepts_unicode_and_rejects_corruption` | no unpaired-surrogate tree through scan, query and forget |
| Link loops | `a_junction_replacing_a_directory_stays_visible_as_excluded` (service); reparse refusal | no junction or symlink cycle tree |
| Hard links | `surviving_hardlink_keeps_the_durable_reference_and_membership`, `unknown_allocation_hardlinks_and_failed_run_rollup_are_distinct` | no multi-thousand link fixture |
| Denied folders | `access_denied` listing state and flag | no scan test with a real ACL-denied directory |
| Offline or placeholder files | `directory_becoming_reparse_or_offline_matches_fresh_scan`, `excluded_entries_stay_visible_uncounted_and_complete_a_full_traversal` | no Cloud Files provider; refusal is tested against synthetic attributes only |

**Prove.** `cargo test --workspace --no-fail-fast`; the missing cases need a generator profile in
`crates/loomward-lab` and one engine test per case.

## 4. Churn during scans: PARTIAL

**Exists.** `evidence/v3/scan-robustness-201-184.md` (rename bursts, channel overflow reconciles to a fresh-scan
oracle), `evidence/v3/l16-change-tracking.md` and `change-tracking.json` (watch, USN catch-up),
`evidence/v3/relist-containment-269.md`, `evidence/v3/scan-cancellation-228.md`,
`evidence/v3/pending-publication-356.md` (references bound mid-publication).

**Gap.** No sustained writer thread mutating a tree while a long scan runs, compared with a final fresh scan.

**Prove.** Engine test: mutate (create, delete, rename, grow) concurrently with a scan of a 100k-file lab tree,
then require the scan plus reconciliation to equal a fresh scan exactly.

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

## 7. 72-hour soak: PARTIAL

**Pass rule (fixed before the run, 2026-10-17, #414).** A continuous run must last at least 72 hours,
with no unexpected error or missing lifecycle operation. Each full scan and watch-triggered refresh must
match the generated manifest's file, directory and logical-byte totals, including SQLite row counts;
each forget must leave zero root, directory, file, scan-run and staged-listing rows and no identifiable
root grants. The existing fully anonymised, revoked grant audit (zero identity, no lab binding) is allowed.
The sum of both WAL files must stay <=64 MiB at every sample, and both live database files together
<=64 MiB for this fixed 128-file lab. At the same post-restart phase of each cycle, compare medians
of the first hour after a five-minute warm-up with the last hour: handles may grow by at most
max(32, 10%), private bytes by max(32 MiB, 10%), and RSS by max(64 MiB, 20%). Both windows must be
non-overlapping and contain at least 12 cycles. A sample gap over 60 seconds fails continuity.
These are shared-host bounds, not a claim of zero allocation or cold-cache performance.

A 15-minute smoke uses the identical correctness/size bounds and resource-growth limits, with a
one-minute warm-up and first/last three-minute windows (at least 12 cycles each). It can establish
SMOKE PASS only; row 7 stays PARTIAL until a qualifying 72-hour run. Interrupted or short input
cannot pass. JSONL and generated state stay outside Git. The service catalogue sink currently does
not support live watch reconciliation: exercise the real native watcher and a service refresh after
the hint; this does not prove automatic service watch integration.

**Exists.** `loomward-lab soak --hours N --root <new-lab-child> --out <private-JSONL>` drives
the real in-process service and native Windows watcher against the deterministic 128-file dev plan
(seed 42, 41 descendant directories, 1,078,433 logical bytes). Every operation records current
handles, RSS/private bytes, both DB/WAL sizes, catalogue rows, logical bytes, latency and error.
The process stays alive across graceful service restarts. Existing roots/outputs, reparse/offline
ancestors, elevation, profile/credential roots and output inside Git are refused. No existing tree
is scanned. The CLI and detached 72-hour command are in `crates/loomward-lab/README.md`.

**Prove.** `cargo test -p loomward-lab`; `py -3 -m unittest discover -s crates/loomward-lab -p test_soak_summary.py`;
`loomward-lab soak --hours 72 --root <new-lab-child> --out <private-JSONL>`, then
`py -3 crates/loomward-lab/soak_summary.py <private-JSONL>`. Add `--smoke` only to evaluate a 15-minute smoke.
The evaluator checks completeness, all operation stages, continuity, manifest rows/bytes, forget,
DB/WAL limits and resource medians, and returns nonzero on failure. Tests plant defects in receipts;
a synthetic full-duration receipt tests the evaluator only, never qualifies as a real soak.

**Gap.** No measured 72-hour run. Native watch plus explicit refresh does not establish automatic
service watch reconciliation; graceful teardown/reopen does not establish process crash recovery.

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

The 2026-10-17 soak-r7 checks use `CARGO_BUILD_JOBS=4`,
`CARGO_TARGET_DIR=C:\lw\target\soak-r7` and `RUST_TEST_THREADS=4`. The smoke binary's SHA-256 is
`355af2271d682e871f42b250b4f877399ca4abe0085d70e40e3cdad5c4ef6347`.

| Date | Commit | Host | Command | Result |
|---|---|---|---|---|
| 2026-10-17 | 89df386 + soak-r7 working diff | KRASPYON, Windows 11 Home, shared host | `cargo test --manifest-path C:\lw\wt\soak-r7\Cargo.toml --workspace --no-fail-fast`; retry with `-j 1` | Final completed run: exit 101, **989 passed, 6 failed, 0 ignored**. Five are the known #292 host failures: three `scan::tests::followups::native_relist_containment_*`, `scan::tests::native_dirty_epoch_relists_by_verified_identity`, and `enumerate::tests::reopen_follows_identity_after_rename_and_rejects_changed_volume_or_root`. The sixth is the unchanged service test below. Initial builds failed with MSVC LNK1201, then LNK1285 identifying a corrupt `root_coverage` PDB; only that generated PDB in this lane's target directory was removed, and the serial rebuild completed. No tests disabled or weakened. |
| 2026-10-17 | same | same | `C:\lw\target\soak-r7\debug\deps\loomward_service-2cd7cfb8d03ab454.exe --exact inventory::tests::wrong_dataset_and_timed_out_providers_never_publish_fallback --nocapture --test-threads=1` | Exit 101: 0 passed, 1 failed, 179 filtered. Reproduces the full-run failure at `inventory/tests.rs:166`: the API returned the expected `deadline_exceeded`, but the test expected the provider to have started within its 20 ms deadline (observed calls 0, expected 1). Service sources are unchanged; this additional timing assertion remains unresolved, not dismissed as flaky. No repair in the soak lane. |
| 2026-10-17 | 89df386 + soak-r7 working diff | KRASPYON, Windows 11 Home, shared host | `cargo fmt --manifest-path C:\lw\wt\soak-r7\Cargo.toml --all --check`; `cargo clippy --manifest-path C:\lw\wt\soak-r7\Cargo.toml -p loomward-lab --all-targets -- -D warnings`; `cargo clippy --manifest-path C:\lw\wt\soak-r7\Cargo.toml --workspace --all-targets -- -D warnings`; `git -C C:\lw\wt\soak-r7 diff --check` | All exit 0; formatting and both clippy scopes clean. |
| 2026-10-17 | 89df386 + soak-r7 working diff | KRASPYON, Windows 11 Home, debug build, shared host | `cargo test --manifest-path C:\lw\wt\soak-r7\Cargo.toml -p loomward-lab`; `py -3 -m unittest discover -s C:\lw\wt\soak-r7\crates\loomward-lab -p test_soak_summary.py` | 29 Rust tests passed (includes three service lifecycle cycles in 17.76 s); 3 evaluator tests passed, including planted failures and a synthetic 72-hour receipt. No 72-hour runtime claim. |
| 2026-10-17 | same | same, 02:29-02:44 BST | `C:\lw\target\soak-r7\debug\loomward-lab.exe soak --hours 0.25 --root C:\lw\lab\soak --out C:\lw\soak\smoke.jsonl`; `py -3 C:\lw\wt\soak-r7\crates\loomward-lab\soak_summary.py C:\lw\soak\smoke.jsonl --smoke` | **SMOKE PASS**, evaluator exit 0: 900.039 s, 114 cycles, 1,026 operation samples, zero errors; all 228 scans/refreshes match the manifest, all 114 forgets leave zero root data/identifiable grants. Comparison windows: 27 and 19 cycles; median handles 202 to 202, private growth 3,457,024 bytes (bound 33,554,432), RSS growth 1,802,240 bytes (bound 67,108,864). Maximum sampled combined WAL 168,984 bytes, live DBs 438,272 bytes. Row stays PARTIAL; 72 hours unverified. Private raw JSONL and stdout/stderr remain outside Git under the given output directory. |
| 2026-10-12 | 00c3d1f + this PR (before merging main) | DESKTOP-IHKOOJS, Windows 11, debug build | `LOOMWARD_CRASH_SWEEP=full cargo test -p loomward-catalog --features test-hooks --test crash_sweep` | 7 passed in 355 s: lifecycle 240 of 240 boundaries, migration 13 of 13, 3,045 double crashes, no violation |
| 2026-10-12 | same | same | default mode of the same command | 7 passed in 29 s: lifecycle 132 of 240, migration 13 of 13, 143 double crashes |
| 2026-10-12 | same, with `purge_root` removed from the open-time forget completion (mutant, reverted) | same | `LOOMWARD_CRASH_SWEEP=full ... -- every_write_of_scan` | failed: 13 of 240 crash points (N=223 and later, the state half of the forget committed) |
| 2026-10-12 | e2833a9 (this PR merged with main 7cf4a54; the merge brought documentation only) | same | default mode of the same command | 7 passed, 1 ignored (F1) in 13 s; lifecycle 132 of 240, migration 13 of 13, 143 double crashes |
| 2026-10-12 | same | same | `cargo test --workspace --no-fail-fast`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `py -3 scripts/verify.py` | 949 passed, 1 ignored (F1), 0 failed; clippy clean; verify.py exit 0 |
| 2026-10-12 | round-1 head (F1 fixed, shutdown checkpoint and `VACUUM main` instrumented, strict exit guard), main a9299b4 | same | `LOOMWARD_CRASH_SWEEP=full cargo test -p loomward-catalog --features test-hooks --test crash_sweep` | 9 passed in 505 s: lifecycle 272 of 272 boundaries, migration 14 of 14, 3,870 double crashes, every child exited with the crash code, no violation |
| 2026-10-12 | same | same | default mode of the same command | 9 passed in 53 s (cold, parallel with a warm build): lifecycle 149 of 272, migration 14 of 14, 191 double crashes |
| 2026-10-12 | same, with the marker recovery removed (mutant, reverted) | same | `LOOMWARD_CRASH_SWEEP=full ... -- every_write_of_scan` | failed: 10 of 272 crash points (N=261 to 267, after the catalogue half of the forget committed), names left in the live store files |
| 2026-10-12 | same | same | `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --no-fail-fast`; `py -3 scripts/verify.py`; `git diff --check` | fmt and clippy clean; 951 passed, 0 ignored, 0 failed; verify.py exit 0; diff check clean |
