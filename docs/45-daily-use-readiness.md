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
| 7 | 72-hour soak | UNVERIFIED |
| 8 | Power-cut simulation, per effect | UNVERIFIED |
| 9 | Proven restore, per effect | UNVERIFIED |

## 1. Crash injection at every write: PARTIAL

**Exists.** `crates/loomward-catalog/tests/crash_sweep.rs` over the `test-hooks` crash points in
`crates/loomward-catalog/src/crashpoint.rs`. Every durable boundary of the catalogue writer counts: each row
change, each commit before and after it reaches the log, each checkpoint, `VACUUM st`, backup and rename. A clean
run enumerates them; one child process per boundary is ended there (`exit(86)`, no destructors, no graceful
close); the parent reopens and asserts the recovery invariants (integrity and foreign-key checks on both files, no
running run or half-built bulk table, no staged or pending-reference residue, no orphan rows, rollups agree with
rows, forget all-or-nothing in rows and bytes, every acknowledged step durable, a second recovery changes
nothing, the store still takes a write). Three sweeps: the lifecycle (scan, durable reference and labels, bulk
first scan with staged chunks, rescan, revoke, forget), a schema migration from a v4-shaped store, and a second
crash inside the recovery of the first. The checker is proven able to fail: planted damage is caught
(`the_verifier_catches_planted_damage`) and a mutant that skips finishing a half-done forget fails 13 of 240
crash points (see the run record). Earlier single-point tests stay: `PublicationCrash` in
`src/writer/publication_tests.rs`, `grant_survives_a_crash_between_precious_and_derived_commits` and the
uncommitted-transaction child in `tests/writer.rs`, `tests/open.rs` corruption and interrupted-run cases.

**Gap.** A killed process is not a power cut (row 8). Writers outside the catalogue writer are not swept: the
service's `collections` connection that writes both files, label, learning and job state, backup import. A torn
`VACUUM INTO` backup file cannot be injected mid-copy by a boundary hook. The migration fixture is a synthetic
downgrade of a current store, not a store written by an old binary. Finding **F1** below.

**Prove.**

```powershell
cargo test -p loomward-catalog --features test-hooks --test crash_sweep   # about 30 s, samples row boundaries
$env:LOOMWARD_CRASH_SWEEP='full'; cargo test -p loomward-catalog --features test-hooks --test crash_sweep   # every boundary, about 6 min
cargo test -p loomward-catalog --features test-hooks --test crash_sweep -- lifecycle_at 123   # one boundary
```

## 2. The 10M-entry lab: PARTIAL

**Exists.** `crates/loomward-lab` generates the 10M tier (E: scale lab only; `crates/loomward-lab/README.md`);
`evidence/v3/scan-l7.md` and `evidence/v3/bench/scan-10m.json`: 20 fresh native runs, p50 546,530 observed
entries/s, every run partial by design at the engine arena cap (48,769 directories observed), peak private commit
37.5 MiB. S and M tiers match their manifests exactly (`scan-s.json`, `scan-m.json`). Catalogue ingest at 100k
files: `evidence/v3/scan-throughput-310.md`; first-scan bulk load: `evidence/v3/first-scan-bulk-load-148.md`.

**Gap.** No complete 10M scan, and no 10M catalogue: the L runs use a sink with no SQLite, and the writer has not
ingested, queried, rolled up, forgotten or crash-recovered at that size. G: stays capped at 2M by ADR-V3-17.

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

## Findings from the crash sweep (not fixed here)

**F1. Forget leaves staged-listing names in `catalog.db`.** A root whose listing was published through
`StageChunk`/`ListingDone` keeps its file and directory names in `catalog.db` pages after revoke and forget, after
a graceful close. The `stage_entry` rows were deleted while `secure_delete` was off, and the forget path only
rewrites `state.db` (`VACUUM st`). No current production caller stages chunks (the engine publishes with
`DirListing`), so there is no reachable leak today, but the first caller that does will leak. Repro:
`cargo test -p loomward-catalog --features test-hooks --test crash_sweep -- --ignored forget_leaves_no_staged_listing_names`
(fails today). Suggested fix for the driver: `VACUUM main` in the forget cut, or secure-delete before staging.

The sweep found no recovery defect in the catalogue or state stores at any crash point of the sequences above.

## Run record

| Date | Commit | Host | Command | Result |
|---|---|---|---|---|
| 2026-10-12 | 00c3d1f + this PR (before merging main) | DESKTOP-IHKOOJS, Windows 11, debug build | `LOOMWARD_CRASH_SWEEP=full cargo test -p loomward-catalog --features test-hooks --test crash_sweep` | 7 passed in 355 s: lifecycle 240 of 240 boundaries, migration 13 of 13, 3,045 double crashes, no violation |
| 2026-10-12 | same | same | default mode of the same command | 7 passed in 29 s: lifecycle 132 of 240, migration 13 of 13, 143 double crashes |
| 2026-10-12 | same, with `purge_root` removed from the open-time forget completion (mutant, reverted) | same | `LOOMWARD_CRASH_SWEEP=full ... -- every_write_of_scan` | failed: 13 of 240 crash points (N=223 and later, the state half of the forget committed) |
| 2026-10-12 | e2833a9 (this PR merged with main 7cf4a54; the merge brought documentation only) | same | default mode of the same command | 7 passed, 1 ignored (F1) in 13 s; lifecycle 132 of 240, migration 13 of 13, 143 double crashes |
| 2026-10-12 | same | same | `cargo test --workspace --no-fail-fast`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `py -3 scripts/verify.py` | 949 passed, 1 ignored (F1), 0 failed; clippy clean; verify.py exit 0 |
