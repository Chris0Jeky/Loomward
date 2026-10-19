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
| 6 | Fuzzing every parser and contract | PARTIAL |
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

The service-side writers are swept too: `crates/loomward-service/src/crash_sweep_tests.rs` arms the service
connection (`catalog.db` with `state.db` attached) and the learning store's connection through
`loomward_catalog::crash_arm` (live only in a process started with a crash or trace variable; a no-op without
`test-hooks`), and drives real service commands in the child. Scenario `meaning` (synthetic session, 59 boundaries
on 2026-10-19): each open's own recovery writes, `collections.create`, `collections.update_members` add (new
references and memberships in one transaction) and remove, `feedback.record` on a node with no reference yet (the
catalogue writer's reference, then the label, its provenance, node binding and replay receipt in one learning
transaction), and a `learning.refit` model commit. Scenario `backup` (personal session, 27 boundaries):
`backups.import`, a replacing import under the same label, `backups.forget`. Invariants: integrity and
foreign-key checks on both files, no membership, label, node binding or provenance without its parent row, no label
without its replay receipt or receipt without its label, no pending reference intent, no resolved reference bound
to a missing catalogue row or another catalogue's instance (the cross-file check), at most one candidate model,
every acknowledged step durable (a step that began but was not acknowledged may land either way, never half), a
second recovery changes nothing (catalogue revision excepted: every open publishes one by design), and each
writer still takes a write. The checker can fail: `the_service_verifier_catches_planted_damage` (five cases) and,
by hand on 2026-10-19, a mutant that commits the label's replay receipt in its own transaction failed 3 of its 61
crash points. `an_unreached_service_crash_point_fails_the_sweep` guards the boundary count. No defect found.
Job state is in memory (`loomward-engine/src/jobs`): its only durable effect is the refit commit swept above.

**Gap.** A killed process is not a power cut (row 8). Service writers not yet in a scenario: tier declarations,
alert settings, client grants, placement saves, teacher requests, the copy journal and diagnostics export; the
learning redaction after a revoke is armed but only its empty pass is swept. A torn `VACUUM INTO` backup file
cannot be injected mid-copy by a boundary hook. The migration fixture is a synthetic
downgrade of a current store, not a store written by an old binary. Finding **F1** below (fixed).

**Prove.**

```powershell
cargo test -p loomward-catalog --features test-hooks --test crash_sweep   # about 53 s (2026-10-12), samples row boundaries
$env:LOOMWARD_CRASH_SWEEP='full'; cargo test -p loomward-catalog --features test-hooks --test crash_sweep   # every boundary, about 505 s (2026-10-12)
cargo test -p loomward-catalog --features test-hooks --test crash_sweep -- lifecycle_at 123   # one boundary
cargo test -p loomward-service --lib crash_sweep   # service writers, about 18 s sampled (2026-10-19)
$env:LOOMWARD_CRASH_SWEEP='full'; cargo test -p loomward-service --lib crash_sweep_tests::every   # all 86 boundaries, about 28 s
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
| Deep | Scanner and writer share `MAX_DIRECTORY_CHAIN_LENGTH=512`, including the root; unignored `writer_depth_limit_512_scanned_end_to_end` scans, queries and forgets the full chain. A 513-directory regression requires partial coverage, `limit_hit=true`, `limit_reason="depth_limit"`, preserved boundary files and no over-depth catalogue rows | Run and resource measurements for #483 below; deeper directories remain unscanned and totals stay lower bounds |
| Very long paths | Lab `LongPath`; engine `past_max_path_scanned_queried_and_forgotten` exercises native scan, catalogue query and forget beyond MAX_PATH | No remaining gap in this case on KRASPYON; see run record |
| Odd Unicode | Lab `UnpairedSurrogate`; engine checks exact UTF-16LE bytes, lossy display/name flag and forget | No remaining gap in this case on KRASPYON; see run record |
| Link loops | Lab `JunctionCycle` and `SymlinkCycle`; separate engine tests require visible/excluded cycle entries, no traversal, then forget | Both ran on KRASPYON; other hosts report symlink privilege error 1314 as UNVERIFIED, never elevate |
| Hard links | Lab `HardLinks`; engine checks 4,096 catalogue names, eight shared identities and eight `multilink` groups, then forget | No remaining gap in this case on KRASPYON; groups respect NTFS's per-file link limit |
| Denied folders | Lab `DeniedDirectory` applies a listing-deny ACE to its own temporary child; engine checks actual OS denial, visible denied coverage/flag, partial root, then forget; lab test verifies ACE restoration | No remaining gap in this case on KRASPYON; no elevated or system ACL change |
| Offline or placeholder files | `directory_becoming_reparse_or_offline_matches_fresh_scan`, `excluded_entries_stay_visible_uncounted_and_complete_a_full_traversal` | no Cloud Files provider; refusal is tested against synthetic attributes only |

**Prove.** `cargo test -p loomward-engine -p loomward-lab --test pathological -- --nocapture`.
`cargo test -p loomward-engine --test pathological depth_limit -- --nocapture`
explicitly reruns both depth boundaries. The tests live under each crate's `tests/`.
The engine integration tests exercise the real engine through the service's existing catalogue sink;
no fixture sink substitutes for durable publication/query/forget. Generation uses isolated temporary
directories under `CARGO_TARGET_DIR` (the system temporary directory when unset), outside the worktree.
Elevated hosts report native cases UNVERIFIED, as do hosts missing symlink privilege; the recorded
KRASPYON run was non-elevated and both link types actually ran. With `LOOMWARD_REQUIRE_NATIVE=1` those
skips fail instead, so a host that cannot run a native case cannot pass it vacuously.

**Depth resource bounds (#483).** `pipeline.rs` visits a `VecDeque` in worker loops and aggregates
an arena bottom-up; neither traversal nor rollup recurses. `source.rs` delegates to `DirSource`.
Extended/Both native listings retain the root anchor and current directory, releasing listed
directories when identity reopening is available. Other sources can retain at most the arena's
directory/byte caps (512 on one accepted chain). Find additionally pins the current absolute path
with an iterative `root_handles` loop, dropping those pins after each listing: 512 granted-tree
components plus the grant's ancestors per active worker, with 1..=32 workers and the Windows path
length bound. It does not retain one set of pins for every depth. Portable-strategy fixtures use
the same iterative queue and bounded arena. The strategy-matrix regression checks both boundaries,
boundary files, sink/report totals and release of the byte reservation for all four strategies;
this is fixture evidence, not a native Find performance measurement.

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
Whole-store full disk (#414): `crates/loomward-catalog/src/writer/full_disk_tests.rs`. Twelve roots of scan-shaped
writes (registration, runs, listings, a staged chunk, references, run ends) drive the real writer until
`PRAGMA max_page_count` refuses growth, first on catalog.db, then on state.db (`st.`). The first `SQLITE_FULL` is a
typed `Error::Sql` with `DiskFull`, and a reader's snapshot of both files is unchanged by it. The cap is lifted by a
reopen, the remaining roots are written, and the closed and reopened store equals a fresh store given the same
successful writes (every table row for row with its rowid, and the public `Reader` children pages).
The test lives in the writer's test module rather than `crash_sweep.rs`.

**Gap.** No read-error, access-denied-mid-scan or short-write injection below SQLite. A disk that fills below SQLite
(an OS write failure, not the page limit) is not exercised; the page limit drives only SQLite's own `SQLITE_FULL`.

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
that many cases. The seed is fixed by default so CI never meets a new input on an unrelated change;
`LOOMWARD_FUZZ_SEED=random` (or a number) explores further. Shared helpers: `fixtures/fuzz/support.rs` (`#[path]`-included by each test, not a crate).

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
All 2026-10-17 patho-r34 cargo commands below used
`--manifest-path C:/lw/wt/patho-r34/Cargo.toml`, `CARGO_BUILD_JOBS=4` and
`CARGO_TARGET_DIR=C:\lw\target\patho-r34`. Raw logs stay outside Git in that lane target.
The depth-483 commands use `--manifest-path C:/lw/wt/depth-483/Cargo.toml`,
`CARGO_BUILD_JOBS=4`, `CARGO_TARGET_DIR=C:\lw\target\depth-483` and
`RUST_TEST_THREADS=1`; app tests use `--maxWorkers=2`.

| Date | Commit | Host | Command | Result |
|---|---|---|---|---|
| 2026-10-17 | 7bac459 + final depth-483 working diff (#483) | KRASPYON, Windows 11 Home, shared host | `cargo test --workspace --no-fail-fast` | Exit 0: **1,131 passed, 0 failed, 1 ignored** across 70 test/doc-test targets. Only ignore is the existing #190 Both-strategy wire-progress test. All five named #292 host-failure cases passed here. Pathological engine target 9/9 passed again; no failing test disabled, skipped or weakened. Fresh workspace test build took 14m 02s; timing is shared-host evidence, not a performance gate |
| 2026-10-17 | 7bac459 + depth-483 working diff (#483) | KRASPYON, Windows 11 Home, shared host | `cargo test -p loomward-engine -p loomward-catalog -p loomward-protocol -p loomward-lab --no-fail-fast` | Exit 0: **533 passed, 0 failed, 1 ignored** across ordinary and doc tests: engine 233 passed/1 existing #190 ignore, catalogue 207 passed, protocol 53 passed, lab 40 passed. Engine pathological target **9/9 passed**, including the unignored 512 regression and new partial 513 case; strategy matrix passed. The four named #292 engine cases passed in this run; no tests disabled or weakened |
| 2026-10-17 | same | same | `cargo clippy --workspace --all-targets -- -D warnings`; `PYTHONPATH=C:/lw/wt/depth-483/python py -3 -m unittest discover -s tests` | Both exit 0. Workspace clippy completed in 1m 24s. Python: 276 run, 275 passed, 1 skipped (optional `mistune` HTML renderer unavailable); JSON Schema validator available. Initial standalone Python invocation omitted the wrapper's `PYTHONPATH` and failed imports; corrected process-scoped environment, no source change |
| 2026-10-17 | 7bac459 + depth-483 working diff (#483) | KRASPYON, Windows 11 Home, shared host | `cargo fmt --all --check`; `cargo clippy -p loomward-engine -p loomward-catalog -p loomward-protocol -p loomward-lab --all-targets -- -D warnings`; `py -3 scripts/gen_contracts_ts.py --check`; `git diff --check` | All exit 0; touched-crate clippy completed in 2m 39s; generated TypeScript matches the schema |
| 2026-10-17 | same | same | `npm.cmd --prefix app run check`; `npm.cmd --prefix app run test -- --maxWorkers=2`; `npm.cmd --prefix app run build` | All exit 0: 0 check errors/warnings, 437 unit tests passed (including two new depth-reason cases), production build succeeded. Initial new cases failed 2/37 against the old wording/legacy merge before the fix; no desktop/browser inspection in this lane |
| 2026-10-17 | 7bac459 + depth-483 working diff (#483) | KRASPYON, Windows 11 Home, non-elevated debug build, shared host | `pathological-824a8787e307113d.exe --exact writer_depth_limit_512_scanned_end_to_end --nocapture --test-threads=1`, monitored by the lane's temporary PowerShell helper | 1 passed, exit 0. Scan **14.003 s**, 512 directory identities including root (max stored depth 511), one leaf file; query and forget passed. **212 peak sampled process handles**, 583 samples from process start through scan completion (includes setup/generation; excludes query, forget and fixture cleanup), requested sleep 1 ms; shared-host scheduling makes this sampled evidence, not an exact transient maximum. Test wall time 26.177 s. Receipt and logs remain in `C:/lw/target/depth-483/`; no personal metadata recorded |
| 2026-10-17 | 7bac459 + regression tests before the cap fix | same | `cargo test -p loomward-engine --test pathological depth_limit -- --nocapture` | 0 passed, 2 failed as expected: the 512-chain scan was partial at 128 children with no leaf file; the 513 case had no `depth_limit` reason |
| 2026-10-17 | 7bac459 + cap fix, before matching the fixture sink's guard | same | `cargo test -p loomward-engine --lib iterative_depth_cap_preserves_boundary_files_for_every_strategy -- --nocapture` | 0 passed, 1 failed as expected: the pipeline reported 512 files/511 children but the in-memory sink's separate 128-depth rollup guard returned 130 files/129 children. That guard now uses the same shared constant |
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
| 2026-10-17 | 89df386 (docs-only branch; code at this commit) | KRASPYON, Windows 11 Home, debug build | `LOOMWARD_CRASH_SWEEP=full cargo test -p loomward-catalog --features test-hooks --test crash_sweep` | 9 passed in 3690 s, exit 0, no violation. The harness prints per-scenario counts only on failure, so they were recovered by replaying the sweep's child runs with the same env vars: lifecycle 334 of 334 boundaries (all 334 first crashes exited 86), 6,652 double crashes |
| 2026-10-17 | same | same | `LOOMWARD_CRASH_SWEEP=full cargo test -p loomward-catalog --features test-hooks --test crash_sweep -- every_write_of_a_schema_migration_recovers --exact --nocapture` | 1 passed in 6.4 s: migrate 14 boundaries, killing at 14 of them |
| 2026-10-17 | a5a4d59 (origin/main) + the #414 branch | KRASPYON, Windows 11 Home, debug build | `cargo test -p loomward-catalog --all-features --lib full_disk` | 2 passed in 48 s: catalogue cap first hit SQLITE_FULL at a staged `ListingDone` after 73 successful writes; state cap at an `ObjectReference` after 146; each reader snapshot unchanged, each store equal to its fresh replay |
| 2026-10-17 | same, with `Writer::call` mapping `DiskFull` to `Ok(WriteReply::Done)` (mutant, reverted; run before the boxing and format-only edits) | same | `cargo test -p loomward-catalog --all-features --lib full_disk` | failed: catalogue `unexpected Closed for ObjectReference`; state `the page limit was never reached` |
| 2026-10-17 | same | same | `cargo fmt --all --check`; `cargo clippy -p loomward-catalog --all-targets --all-features -- -D warnings`; `cargo test -p loomward-catalog --all-features`; `git diff --check` | fmt and clippy clean; 216 passed (lib 55 including the 2 new, crash_sweep 9), 0 failed; diff check clean |
| 2026-10-17 | 80eabd0 (test code 12ef08a) | KRASPYON, Windows 11 Home, non-elevated debug build | `cargo test -p loomward-engine --test pathological` | 7 passed, 0 failed, 1 ignored (depth-cap defect #483), exit 0. Both cycle types and real ACL denial/restoration ran. Includes the default 4,000-file churn test (next three rows) |
| 2026-10-17 | 12ef08a (same tree as 80eabd0) | same | `cargo test -p loomward-engine --test pathological concurrent_create_delete_rename_grow` run three times, default 4,000 files | All three passed, exit 0. Batches overlapping enumeration: 45, 21 and 20 of 128 (floor 8). 4,041 exact observations matched the fresh scan; 4,000 files equal to the `std::fs` walk each time |
| 2026-10-17 | 12ef08a with the walk's first file length +1 (mutant, reverted before commit) | same | the default churn command | Failed at the walk comparison with "catalogue (path, logical bytes) differs from the filesystem walk" (32.03 s). An earlier mutant run, before the writer reused one catalogue connection, stopped at the overlap floor (5 of 128) and never reached the walk |
| 2026-10-17 | same, initial regression run before depth ignore | same | `cargo test -p loomward-engine -p loomward-lab --test pathological -- --nocapture --test-threads=1` | Depth-512 regression failed: partial, limit_hit=true, 128 indexed child directories, 0 leaf files. Initial engine run 5 passed/3 failed; other two were test assumptions corrected to use public denied coverage and stored flags (slice nodes do not carry flags). No production assertion or existing test was weakened |
| 2026-10-17 | 80eabd0 (test code 12ef08a) | KRASPYON, Windows 11 Home, non-elevated debug build | `LOOMWARD_CHURN_FILES=100000 cargo test -p loomward-engine --test pathological concurrent_create_delete_rename_grow -- --nocapture` | 1 passed, 0 failed, 7 filtered out in 572.08 s, exit 0, including generation and cleanup: 100,000 initial files, all 128 mutation batches overlapped enumeration, 101,001 exact observations matched a fresh scan by the same engine after refresh, and all 100,000 files (path and logical bytes) matched a `std::fs` walk; fixture directory empty afterwards |
| 2026-10-17 | 80eabd0 (test code 12ef08a) | same | `cargo fmt --all --check`; `cargo clippy -p loomward-engine -p loomward-lab --all-targets -- -D warnings`; `cargo test -p loomward-lab`; `git diff --check` | fmt exit 0; clippy exit 0; lab crate 17 + 10 + 5 passed, 0 failed, exit 0; diff check exit 0. Full workspace not rerun at this head |
| 2026-10-17 | same | same | `cargo clippy -p loomward-lab -p loomward-engine --all-targets -- -D warnings` | Exit 0; final workspace clippy below covers the subsequent fixture/test edits |
| 2026-10-17 | same, before correcting the slice request depth to 8 | same | `RUST_TEST_THREADS=4 cargo test -p loomward-lab -p loomward-engine --no-fail-fast` | 237 passed, 12 failed, 1 ignored. Four known #292 engine failures; two existing native tests timed out; six new tests rejected the helper's invalid slice depth 64 (request maximum 8), corrected to 8. Lab profiles: all 5 passed, including ACL cleanup and a junction path containing a shell metacharacter |
| 2026-10-17 | same | same | `RUST_TEST_THREADS=1 cargo test -p loomward-engine --lib native_scan_job_reports_exact_final_progress -- --exact scan::tests::native_scan_job_reports_exact_final_progress --nocapture`; same command with filter `concurrent_scan_start_reserves_root_before_touching_watch` and exact name `scan::watch::native_tests::concurrent_scan_start_reserves_root_before_touching_watch` | Both timeout rechecks passed individually, 1 test each, 1.31 s and 1.39 s; assertions/deadlines unchanged |
| 2026-10-17 | 89df386 + final uncommitted patho-r34 lane | KRASPYON, Windows 11 Home | `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings` | Both exit 0; workspace clippy completed in 2m 54s |
| 2026-10-17 | same final lane | same, `RUST_TEST_THREADS=1`, default 4,000-file churn size | `cargo test --workspace --no-fail-fast` | 997 passed, 8 failed, 1 ignored; exit 101. All new cases passed: engine pathological target 7 passed/1 ignored (depth cap), lab pathological target 5 passed. Full lab crate 32 passed; full engine crate 213 passed/4 known #292 failures/1 ignored, including doc tests. The known Windows reopen test also failed (#292). Three additional unchanged service tests failed on timing/deadlines: `wrong_dataset_and_timed_out_providers_never_publish_fallback` (20 ms deadline, provider call count 0), `forgetting_a_revoked_root_purges_its_names_and_keeps_an_anonymised_record` (`roots.revoke` deadline), `placement_refuses_identity_unverified_rows` (read deadline). Both earlier engine timeout cases passed in this run. No failing existing test was edited or skipped |
| 2026-10-17 | same final lane, exact service test binary from the workspace run | same | `C:/lw/target/patho-r34/debug/deps/loomward_service-2cd7cfb8d03ab454.exe --exact inventory::tests::wrong_dataset_and_timed_out_providers_never_publish_fallback --nocapture --test-threads=1`; repeat for exact names `tests::forget_tests::forgetting_a_revoked_root_purges_its_names_and_keeps_an_anonymised_record` and `tests::placement_refuses_identity_unverified_rows` | All three passed separately (1 each), 0.92 s / 2.30 s / 1.47 s, with the same assertions, deadlines and binary. This does not turn the full workspace run green. A package-only cargo recheck was interrupted during compilation because it rebuilt a different feature graph; these checks reuse the workspace binary |
| 2026-10-17 | same final lane | same | `git -C C:/lw/wt/patho-r34 diff --check` | Exit 0. New pathological fixture directory empty after tests. Removed 60 disposable synthetic databases left by existing tests under this worktree's ignored `target/test-roots` (created during this run; absolute scope and absence of reparse entries checked). Raw logs remain under the external lane target, not in Git |
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
| 2026-10-17 | this PR merged with main fe3176c (the merge brought #292, r5, r7 and documentation) | same | `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`; `git diff --check` | fmt and clippy clean; 1,068 passed, 1 ignored, 0 failed; diff check clean. Before the merge, the same test run showed the 5 host failures of #292 plus 2 `loomward-service` tests that depend on host load (a 20 ms deadline in `wrong_dataset_and_timed_out_providers_never_publish_fallback`); neither pair touches this PR's code |
