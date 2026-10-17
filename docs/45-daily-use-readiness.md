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
| 6 | Fuzzing every parser and contract | PARTIAL |
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

## 6. Fuzzing every parser and contract: PARTIAL

**Exists.** Fixed negative examples (`contracts/v3` valid and invalid examples), the offline mutation check
`crates/loomward-backup/tests/mutations.py`, decoder corruption tests in `loomward-lab`, and (#414) one
property-fuzz target per parser below.

**Substitution (stated).** The pinned toolchain is stable MSVC Rust 1.97.1 with no nightly, so these are
**property tests on `proptest` (a dev-dependency only), not `cargo-fuzz`/libFuzzer**: no coverage guidance and no
persistent corpus (regression files are off; a failing input is printed in full and shrunk). Each strategy is seeded
from the existing fixtures (the `contracts/v3/examples` requests, the synthetic restic documents of `receipts.rs`,
the synthetic links of `shell.rs`) and from record builders for the binary decoders. Default case counts keep every
target under about 10 s in a debug build; `LOOMWARD_FUZZ_CASES=100000` (any positive integer) runs every property
that many cases. Shared helpers: `fixtures/fuzz/support.rs` (`#[path]`-included by each test, not a crate).

Properties, per target: never panics on arbitrary bytes or JSON; a valid input round-trips; a mutated valid input is
refused with a typed error or accepted with exactly the semantics an independent reading gives it (no silent
coercion, invariant 4). The binary decoders are also compared with a second, independently written reference
decoder on field-edited chains, and overlapping records are refused.

| Parser | Target | Seeds |
|---|---|---|
| Directory-buffer decoder `enumerate::decode_buffer` (both native classes) and change-notification decoder `watch::decode` | `crates/loomward-windows/tests/fuzz_decoders.rs` | record builders |
| Lab directory-buffer decoder `decode_records` | `crates/loomward-lab/tests/fuzz_records.rs` | record builders |
| Backup receipt normaliser `normalise_backup_receipt` (the parse core of `backups.import`, the snapshot importer): snapshots JSON, check summary, supplied source | `crates/loomward-backup/tests/fuzz_receipt.rs` | `receipts.rs` documents |
| Protocol ingress `RequestEnvelope::from_slice` (the path HTTP and the Tauri `lw_call` both decode through), cursor, `client_event_id`, request id, epoch, snapshot id, digest and decimal-counter wire grammars | `crates/loomward-protocol/tests/fuzz_envelope.rs` | every `contracts/v3/examples/commands/*.request.json`, `envelopes/`, `invalid/` |
| HTTP `dispatch`, `parse_last_event_id`, `static_relative` | `crates/loomward-http/tests/fuzz_http.rs` | the same command examples through `FixtureService` |
| Service cursor table `Cursors` (handles are random tokens; no edit resumes another entry) | `crates/loomward-service/src/cursors_fuzz.rs` | issued handles |
| MS-SHLLINK `shortcuts::parse` (beyond the row's list; adds to the fixed-seed loop in `shell.rs`) | `crates/loomward-shell/tests/fuzz_shortcut.rs` | the `shell.rs` links |

Mutation check of the targets themselves (the property must fail when the parser is weakened; each reverted):
the first versions of the lab, protocol and backup targets did **not** catch a weakened bound, a disabled
canonical-shape check and an accepted `.` segment, so the overlap, positional-array and hostile-path properties
and the reference decoders were added; after that all five mutants failed their targets (see the run record).

**Gap (why PARTIAL).**
- The Tauri IPC surface is not fuzzed directly: `Desktop::dispatch_bytes` (`native/src/lib.rs`) needs the Tauri crate
  built and a live service, and `native/` is outside the workspace. Its parsing entry, `RequestEnvelope::from_slice`,
  is covered above; its size gate, the `Adapter::Tauri` availability filter and the `lw_events` arguments are not.
- The HTTP header and body gate (`call_header_refusal`, `route`, `origin_check`, `token_ok`) is private and async; it
  stays pinned by `security_pins.rs` and `serve.rs`, not fuzzed.
- The snapshot importer's file side, `read_receipt` (picker bytes, `pub(crate)`) and `summary_document` (private), is
  not fuzzed; durable `client_event_id` replay (`feedback.record`) is store behaviour, covered only by its wire grammar.
- Not covered: protocol result and event decoding, other DTO payload types beyond what the command request examples
  exercise, the Python reference's parsers, and any coverage-guided search (`cargo-fuzz` on a nightly host).

**Prove.** Row 6 reaches PASS when the Tauri entry and the two private HTTP/importer seams are fuzzed (or the owner
accepts them as covered by the pins) and a coverage-guided run has been recorded on a nightly host.

Finding: none in this wave. No target found a defect at the default counts, at 100,000 cases per property, or at
1,000,000 cases for the two decoders. One behaviour noted, not a defect: `parse_last_event_id` accepts leading zeros
in the sequence (`id.007` resumes at 7); the value means the same number and the server only ever emits canonical ids.

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
| 2026-10-17 | 912a491 (row 6 targets) | KRASPYON, Windows 11, debug build, stable MSVC 1.97.1 | default mode: `cargo test -p loomward-windows --test fuzz_decoders`, `-p loomward-lab --test fuzz_records`, `-p loomward-backup --test fuzz_receipt`, `-p loomward-protocol --test fuzz_envelope`, `-p loomward-http --test fuzz_http`, `-p loomward-shell --test fuzz_shortcut`, `-p loomward-service --lib cursors` | 8, 6, 9, 10, 7, 4 and 8 (3 new) passed in 2.4, 1.7, 7.7, 4.5, 6.5, 1.9 and 7.3 s on a loaded host |
| 2026-10-17 | 912a491 | same | `LOOMWARD_FUZZ_CASES=100000 cargo test -p loomward-windows --test fuzz_decoders` (`enumerate::decode_buffer` Extended and Both, `watch::decode`) | 8 properties x 100,000 cases passed in 39 s; no panic, no reference disagreement |
| 2026-10-17 | 912a491 | same | `LOOMWARD_FUZZ_CASES=1000000 cargo test -p loomward-windows --test fuzz_decoders` | 8 properties x 1,000,000 cases passed in 538 s |
| 2026-10-17 | 912a491 | same | `LOOMWARD_FUZZ_CASES=100000 cargo test -p loomward-lab --test fuzz_records` (`decode_records`) | 6 x 100,000 passed in 39 s |
| 2026-10-17 | 912a491 | same | `LOOMWARD_FUZZ_CASES=1000000 cargo test -p loomward-lab --test fuzz_records` | 6 x 1,000,000 passed in 373 s |
| 2026-10-17 | 912a491 | same | `LOOMWARD_FUZZ_CASES=100000 cargo test -p loomward-backup --test fuzz_receipt` (`normalise_backup_receipt`) | 9 x 100,000 passed in 187 s |
| 2026-10-17 | 912a491 | same | `LOOMWARD_FUZZ_CASES=100000 cargo test -p loomward-protocol --test fuzz_envelope` (`RequestEnvelope::from_slice`, cursor, `client_event_id` and the other wire grammars) | 10 x 100,000 passed in 62 s |
| 2026-10-17 | 912a491 | same | `LOOMWARD_FUZZ_CASES=100000 cargo test -p loomward-http --test fuzz_http` (`dispatch`, `parse_last_event_id`, `static_relative`) | 7 x 100,000 passed in 276 s |
| 2026-10-17 | 912a491 | same | `LOOMWARD_FUZZ_CASES=100000 cargo test -p loomward-service --lib cursors::fuzz` (`Cursors`) | 3 x 100,000 passed in 88 s |
| 2026-10-17 | 912a491 | same | `LOOMWARD_FUZZ_CASES=100000 cargo test -p loomward-shell --test fuzz_shortcut` (`shortcuts::parse`) | 4 x 100,000 passed in 68 s |
| 2026-10-17 | 912a491, each with one parser check weakened (mutants, reverted) | same | `cargo test -p <crate> --test <target>` after: windows `name_len > record_len - header` to `name_len > record_len`; windows `next < header ||` removed; lab `next < end` removed; protocol `same_json(&canonical, &value)` to `true`; backup `part != "."` removed from `provider_path` | all five failed their target (panic at `enumerate/mod.rs`, overlap property, reference disagreement, `positional_arrays_are_refused`, `hostile_provider_paths_are_refused_on_the_bound_snapshot_only`). Earlier drafts of the lab, protocol and backup targets passed the same three mutants; the directed properties above were added in response |
| 2026-10-17 | 912a491 + the docs commit of this PR | same | `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast` | fmt and clippy clean; 1,033 passed, 7 failed: the 5 known host failures of #292 (4 `loomward-engine` native relist tests, 1 `loomward-windows` reopen test) plus 2 in `loomward-service` that pass or fail with host load (`inventory::tests::wrong_dataset_and_timed_out_providers_never_publish_fallback`, a 20 ms deadline assertion, failed again alone; `meaning_tests::meaning_is_bounded_read_only_advice_with_explicit_unknowns` passed alone); neither touches this PR's code and neither was rerun on main |
