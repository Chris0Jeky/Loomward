# Catalogue P4 (#148): bounded streaming publications - 2026-10-10

## Changed

Uncommitted on `perf/catalog-p4` in worktree `lw-p4`, starting and final HEAD
`533473f08b3f7f369fe19b575fcfd03aaf64d9c8`. Initially clean; no commit or push.
The driver owns committing this work. This receipt supersedes earlier performance
figures, not their correctness or recovery evidence.

- Ordinary listing commands now stream into an open writer transaction rather than
  batching only the initial queue snapshot. Applied payloads release their byte permits
  so producers can refill the bounded queue. Cumulative input reservations stop at
  64 MiB; elapsed work stops the next command at 250 ms; idle receive is at most 1 ms.
  A single atomic listing and COMMIT can exceed the time target. Control commands end
  the batch, savepoints isolate failures, and every input retains its own revision.
  Receipts still follow COMMIT and reference reconciliation.
- Resolved references retain singleton publication transactions. The pending-reference
  protocol and #171 ordering remain: state-only FULL preparation, catalogue-only FULL
  publication with witness, state-only FULL confirmation, then acknowledgment. NORMAL
  is restored after catalogue commit/rollback. No transaction writes both files.
- Empty old-file sets already skipped matching; they now also use plain multi-row
  INSERT instead of UPSERT. Refreshes retain matching, incarnation rules and UPSERT.
  File names, raw names and identity parameters borrow the existing observations;
  the 128-row parameter buffers reserve capacity once per decoded chunk.
- Catalogue cache increases from 64 to 256 MiB; temporary tables use memory. A
  database-specific passive checkpoint hook raises only main's threshold to 16,384
  pages (about 64 MiB at the retained 4 KiB page size). State keeps its default
  1,000-page checkpoint, FULL sync and cache settings. The hook ignores checkpoint
  contention/errors exactly as SQLite's default auto-checkpoint does. The existing
  128-statement cache was sufficient and stays unchanged.
- Timings cover staging, validation dry runs, state preparation, invalidation, rollup,
  multilink accounting, commits and final checkpoint. The bench adds allocated P7,
  executable checks for all declared indexes and an isolated deferred-index probe.
  No index is removed from the live catalogue; provisional readers retain their paths.

Owned changes: `src/writer.rs`, `src/db.rs`, `examples/bench.rs`, this handoff and
[benchmark evidence](../../evidence/v3/bench/catalog-1m.json). No dependency,
schema, fixture, file/process effect or owner-decision change.

## Verified

Windows 11, Intel i5-13600K, rustc 1.97.1. All required commands pass:

```text
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
cargo run -p loomward-catalog --release --example bench -- --rows 1000000
```

Workspace: **316 passed**, zero failed or ignored; catalogue: **66 passed**.
Existing N1/N2/N3, partial #8/#10, publication/recovery and random-tree rollup
oracle tests remain green. Two new tests were independently mutation-tested:

- `streamed_transactions_bound_bytes_and_release_applied_payloads`: removing the
  next-command byte check fails with exit 101 (one transaction instead of two).
- `checkpoint_threshold_is_raised_only_for_catalogue`: raising the state's threshold
  to 16,384 fails with exit 101 (1,226 state WAL frames, zero checkpointed).

Both production mutations were restored byte-for-byte; all 66 catalogue tests,
fmt and workspace clippy passed again. Source hashes and mutation results are in
the benchmark evidence.

Three baseline and three final 1M runs passed the independent accounting oracle;
final runs also assert every declared index, both P7 bases and temporary-directory
removal. P4 includes generation, staging, publication and queue backpressure;
EndRun/finalisation and the index diagnostic are outside its timing.

| Phase (seconds; per-metric median) | Before | After |
| --- | ---: | ---: |
| P4 elapsed | 60.632 | 18.821 |
| Staging (including JSON serialization) | 0.790 | 0.559 |
| Validation dry run | 0: no references | 0: no references |
| State preparation (includes dry run when required) | 0.059 | 0.026 |
| Publication INSERT, including inline indexes | 21.144 | 6.718 |
| Name validation inserts | 0.603 | 0.523 |
| Other publication work, excluding invalidation | 1.988 | 1.746 |
| Invalidation | 0.018 | 0.010 |
| COMMIT/fsync, including passive checkpoints | 35.770 | 7.824 |
| EndRun rollup (outside P4) | 0.004 | 0.003 |
| EndRun multilink accounting (outside P4) | 1.534 | 1.138 |
| EndRun explicit checkpoint (outside P4) | not instrumented | 0.119 |
| Transactions through P4 | 256 | 71 |

SQLite updates secondary indexes inside INSERT; their inline cost cannot be
separately timed by this instrument. The isolated probe measures a different path:
copying the same 1M file rows into the real schema with foreign keys and seeded
parents, then creating the four file indexes. Medians: **0.676 s** copy without
secondary indexes, **1.784 s** index creation and **1.205 s** COMMIT. This is an
index-build diagnostic, not staged P4 or proof that live index deferral is safe.
Phase medians are independent summaries; generation, admission, savepoint/revision
work and scheduling are not fully partitioned, so the rows are not an additive wall clock.

| Gate | Before median | After median | Before/after median p95 |
| --- | ---: | ---: | ---: |
| P4 rows/s | 16,510 | **53,186** | not applicable |
| P6 slice + serialization | 43.250 ms | 31.825 ms | 51.284 / 40.993 ms |
| P7 logical 200-row page | 0.996 ms | 0.634 ms | 1.300 / 0.954 ms |
| P7 allocated 200-row page | not instrumented | 0.499 ms | not instrumented / 0.815 ms |
| P8 budgeted substring search | 16.935 ms | 13.499 ms | 22.516 / 15.697 ms |

Query results summarize three runs of 21 warm probes each; each table value is the
median of run summaries. P8 reports `budget_hit` on all 63 final probes. P6 retains
2,500 nodes and a payload below 1.5 MB. All final per-run query p95 values meet their
time targets at 1M; P6's 10M/HTTP gate remains unverified. There is no measured
logical-query regression; allocated P7 has no instrumented baseline comparison.

P4 improves **3.22x**, but **250k remains unmet**. Baseline P4 runs:
54.035 / 60.632 / 78.653 s. Final: 21.679 / 17.116 / 18.821 s
(46,175 / 58,483 / 53,186 rows/s). Host load and disk/cache state were not controlled.

Exploratory single warm measurements, in order:

| Candidate | P4 rows/s | Disposition |
| --- | ---: | --- |
| Streaming transactions | 23,657 | retain |
| Plus plain first-listing INSERT | 34,458 | retain |
| Plus 256 MiB cache, memory temp, global checkpoint 16,384 | 76,958 | replace global checkpoint with per-database hook |
| Remove raised checkpoint threshold | 44,813 | retain raised main threshold |
| Per-database checkpoint hook | 69,307 | retain; state checkpoint pinned by test |
| Borrowed row parameters and reserved buffers | 72,335 | retain; small gain is within host variability |
| New-catalogue 8 KiB pages | 42,774 | reject, restore 4 KiB |
| Remove memory temp tables (4 KiB pages) | 37,191 | reject, restore memory temp |

The fastest exploratory observation was **76,958 rows/s**; the final three-run best
is **58,483**, median **53,186**. These bound what this experiment observed, not
the hardware's theoretical ceiling. Indexed INSERT plus commit/checkpoint still
account for most P4 time (about 14.54 s of the independent phase medians).

## NOT verified

250k throughput, 10M/cold-cache gates, controlled host-load comparison, intent-bearing
throughput, HTTP/native enumeration integration, Python/UI, process RSS, hosted CI,
physical power loss or storage-device fsync behavior. No commit, push or external action.

## Residual risk

The writer's SQLite cache budget is now 256 MiB, and a checkpoint is a threshold,
not a WAL size cap: pinned readers can delay it. Ordinary receipts can wait for the
batch's 250 ms scheduling target plus a final atomic listing/COMMIT. References retain
the conservative singleton path, so this no-reference benchmark does not predict
personal catalogues with resolved references.

The deeper candidate is a separate fresh-root bulk catalogue: sequential row load,
index creation once, and one publication boundary instead of many random index/WAL
writes. The isolated copy/index/commit subtotal is about 3.66 s, versus 14.54 s of
current insert/commit phase medians. It could remove substantial I/O; it does not
prove 250k through staging. Atomic replacement would need a design for provisional
readers, other roots, stable IDs/instances, reference witnesses and reopen/recovery
before it could preserve the current rules. Do not drop live indexes for a demo.

[HUMAN_TODO.md](../../HUMAN_TODO.md) was read and preserved. q-5 (personal teacher
disclosure) and q-8 (teacher sandbox path) remain owner decisions unrelated to P4;
this work adds none. Keep this uncommitted worktree for the driver. Raw exploratory
receipts and check logs are task-owned, gitignored `.loomward/p4/` files; build output
is in ignored `target/`. No personal inventory or scan was used.

Recommended commits:

1. `perf(catalog): batch publications and reduce insert overhead`
2. `bench(catalog): record P4 phase and query medians at 1M rows`

---

# Issue #171: durable publication before reference confirmation - 2026-10-10

## Changed

Uncommitted on `feat/l2-catalog-durable`, starting HEAD `75d6dce`.
The driver commits. This section supersedes the preceding receipt's blanket
NORMAL durability and unchanged-reader-predicate claims.

1. Prepare pending reference intents in a state.db-only FULL transaction.
2. If intents exist, set main.synchronous=FULL before the catalogue transaction.
3. Commit the publication and witness atomically to catalog.db; FULL fsyncs its WAL
   commit before any reference confirmation. Restore NORMAL after commit or rollback.
4. Confirm the matching witness and clear intents in a state.db-only FULL transaction
   before acknowledgment. Recovery still confirms matching or discards unmatched intents.

No transaction writes both files. Publications without intents retain NORMAL under
ADR-V3-22. A pragma error follows the writer's existing failure/close path.
Membership inspection accepts either the confirmed binding or the pending binding
only when its catalogue instance and publication token match the committed witness.
Surviving hard links therefore retain membership between publication and confirmation;
unpublished pending bindings cannot leak into the old listing.
Module documentation records the same ordering; no schema or public API changes.

## Verified

Windows 11: `cargo fmt --all --check`, `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings` and `git diff --check` passed.
All 57 catalogue tests passed, including the existing publication/recovery tests.

Both new tests were independently mutation-tested on the final test source:

- `publication_sync_is_full_only_with_intents_and_normal_is_restored`: replacing
  the targeted FULL switch with NORMAL failed (exit 101, observed 1 instead of 2).
  A test hook reads the pragma immediately before COMMIT and after restoration,
  before confirmation; with no intents it stays NORMAL. state.db stays FULL.
- `hardlink_membership_survives_publication_before_confirmation`: removing the
  pending-binding alternative failed (exit 101, surviving membership was empty).
  The test also rejects the unpublished pending binding, preserves the old listing's
  memberships, and checks the surviving membership after recovery confirmation.

The deferred-FK COMMIT-failure test additionally checks NORMAL restoration after
rollback. Both mutations were restored and all eight publication tests passed again.

The required synthetic 1M staged benchmark passed its oracle and all-index checks;
its temporary database was removed:
`cargo run -p loomward-catalog --release --example bench -- --rows 1000000`.
P4: **66.647 s, 15,019 rows/s**, target 250,000 still missed (#148).
These publications carry no intents and remain NORMAL. The previous receipt was
60.960 s, 16,421 rows/s; independent single warm runs do not isolate regression.
[Current benchmark receipt](../../evidence/v3/bench/catalog-1m.json).

One same-host control temporarily removed only durability selection/restoration,
returning the writer to its original NORMAL transaction path; the fixed source and
current receipt were then restored. Control P4: **56.771 s, 17,632 rows/s**.
The fixed run was 14.8% slower in throughput; unchanged P4 performance is **not proven**
by these single, sequential warm measurements. Commit time differed by 10.010 s
(43.006 s fixed / 32.996 s control), while listing time stayed close
(22.483 s / 22.712 s); this does not establish the cause of the difference.
Both oracles/index checks passed and both temporary databases were removed.
[Control receipt](evidence/issue171-no-intent-control.json).

## NOT verified

Physical power loss, storage-device fsync behaviour, 10M/cold-cache performance,
Python/UI and hosted CI. No commit or push.

## Residual risk

Power-loss ordering is pinned through SQLite's actual connection mode and transaction
boundary; destructive hardware fault injection was not performed. The existing P4
throughput miss remains separate from this correctness fix.
NORMAL without intents is verified; identical no-intent throughput is not established.
[HUMAN_TODO.md](../../HUMAN_TODO.md) was read and preserved; no new owner decision.

Recommended commit: `fix(catalog): make reference publications durable before confirmation`.

---

# PR #147 final fix round - 2026-10-10

## Changed

Uncommitted on `feat/l2-catalog`, base `4ca83e9a1c114578bdf9146c8dfc06fdc15bd7ae`.
The driver commits and pushes. This section supersedes the historical eager
state-before-catalogue retirement/rebinding description and its failure limitation below.

1. Commit pending retirements/rebindings to state.db with catalogue instance and publication token.
2. Keep effective reference state, binding and collection membership unchanged while pending.
3. Commit the publication and token atomically in a catalog.db-only transaction.
4. Confirm matching intents and clear the journal in a state.db-only transaction before acknowledgment.
5. On failure/startup, confirm matching tokens or discard unmatched intents before repair; repeat is a no-op.

Schema 3 adds the journal with a backed-up v2 upgrade; existing bindings and instances
survive. Pending references retain the existing reader predicates. Resolved references
already force singleton writer publications, so one journal belongs to one savepoint.
If confirmation fails, the actor closes without acknowledgment; recovery retries the
intact journal. No transaction writes both persistent files. F1 hard-link rebinding
uses the same protocol; F2 incarnation replacement and round-2 validation stay intact.

## Verified

Windows 11, rustc 1.97.1: `cargo fmt --all --check`, `cargo test --workspace`
(239 passed, catalogue 55, zero failed/ignored),
`cargo clippy --workspace --all-targets -- -D warnings`, `git diff --check`.
Existing F1/F2, N1/N2/N3/#8/#10 and round-2 regressions pass.

The four requested tests pass and each fails (exit 101, changed bindings versus the
original resolved bindings) when committed eager retirement/rebinding is restored:

- `catalogue_commit_failure_reverts_pending_references_and_preserves_listing`
- `crash_before_catalogue_commit_recovery_discards_pending_references`
- `crash_after_catalogue_commit_recovery_confirms_pending_references`
- `pending_reference_recovery_twice_is_idempotent`

Test-only stop points leave persisted databases at the two commit boundaries.
The real COMMIT failure uses SQLite's deferred FK constraint, after state commits;
the publication dry run cannot detect it. Membership, search and recovered slice
checks use actual readers. Additional tests cover the v2 upgrade and confirmation
failure followed by restart retry.
[Mutation receipt](evidence/pr147-final-regression-mutations.json) and
[final source/proof receipt](evidence/pr147-final-fix.json).

The commit path changed, so the 1M staged benchmark was rerun:
`cargo run -p loomward-catalog --release --example bench -- --rows 1000000`.
Oracle and indexes passed; temporary database removed. P4: **60.960 s, 16,421 rows/s**
against 250,000; still unmet (#148). Previous warm receipt: 18,432 rows/s;
these are single warm host measurements, not an isolated performance comparison.
[Benchmark receipt](../../evidence/v3/bench/catalog-1m.json).

## NOT verified

Hosted CI/review threads, physical power loss or actual OS termination at these new
fault points, 10M/cold-cache, HTTP/native scanner integration, Python/UI.
No commit, push or external action. Root status/checkpoint integration remains with the driver.

## Residual risk

P4 remains below target; 1M P6/P14 do not prove the 10M gates. Catalogue durability
remains NORMAL under ADR-V3-22; physical power-loss durability is unmeasured.
[HUMAN_TODO.md](../../HUMAN_TODO.md) was read and preserved: q-5 remains open for
real metadata disclosure after sandbox canaries; q-6 is not yet needed. No new owner
choice arises from this fix. Keep the worktree until the driver commits and pushes.

Recommended commit: `fix(catalog): reconcile pending reference retirements after publication`.

---

# PR #147 fix-round receipt - 2026-10-10

## Changed

Uncommitted fixes on `feat/l2-catalog`, starting HEAD
`6bbeb81ecd4d7f60d27f39c506398b5b65a4dd56`. The driver commits and pushes.
The initial worktree was clean; the driver's query budget CAS loop is preserved.

- F1: use the same deterministic, creation-aware file matching in state preparation
  and publication. Commit a surviving hard-link row/born-run binding before deleting
  the old location; retire the reference when no continuity remains. Test:
  `surviving_hardlink_keeps_the_durable_reference_and_membership` (direct and staged,
  collection membership retained, later absence retires the binding).
- F2: creation changes allocate new row IDs and born-run values, including during
  incomplete refresh. Retire old subtree references first; quarantine directories
  as hidden `absent_pending` tombstones and clear their native identities. Replacement
  directories start `unlisted`, with no inherited descendants or totals. Extended
  `creation_time_edits_retire_directory_and_file_bindings_without_auto_reattachment`
  checks a later-run partial refresh, old incarnation rejection, descendant reference
  retirement and exact totals after relisting. The existing reused-creation-time
  explicit-reconciliation regression remains unchanged and green.
- F3: advance the catalogue revision for each command inside its savepoint, retaining
  commit batching. Dirty propagation coalesces within each input, rather than across
  distinct inputs. `two_listings_in_one_writer_batch_reject_the_older_final` queues both
  listings before starting the actual writer loop, rejects the 150-byte final and
  accepts the 200-byte final.
- F4: advance the search watermark at each returned row. Test:
  `budgeted_search_resumes_after_the_last_returned_match` interrupts inside a SQLite
  batch after matches, then checks all 60 results appear exactly once.

## Verified

Windows, cargo/rustc 1.97.1: `cargo fmt --all --check`, `cargo test --workspace`
(207 passed, zero failed/ignored; 45 catalogue tests),
`cargo clippy --workspace --all-targets -- -D warnings`, and `git diff --check` pass.
All existing errata tests and `sql_rollup_matches_brute_force_for_random_trees` pass.
The final four regression tests each fail with exit 101 against the committed
pre-fix production sources. Exact working bytes were restored afterward;
`evidence/pr147-fix-regressions.json` records failures, proof and source hashes.

The hot path changed, so the synthetic 1M bench was rerun:
`cargo run -p loomward-catalog --release --example bench -- --rows 1000000`.
It checks 1,000,000 files plus 1,001 directories, all indexes and the accounting oracle,
and removes its temporary directory. Receipt: `../../evidence/v3/bench/catalog-1m.json`.
P4: 54.307 s, **18,432 rows/s; target 250,000 still missed**. Dirty updates: 3,001.
P6/P7/P8 p95: 43.443/1.049/15.847 ms; P8 hit its budget in all 21 probes.
This is one warm host measurement, not an isolated speedup or throughput ceiling.

## NOT verified

Hosted CI/review threads, 10M, cold cache, HTTP/native scanner integration,
physical power-loss durability, Python/UI. No commit, push or other external action.
Coordinator-owned root status/checkpoint documents are left for driver integration.

## Residual risk

P4 remains materially below target; 1M P6/P14 are not their 10M gates. Reference
binding collisions retain the existing unique constraint and fail closed; this round
proves the reviewed one-reference hard-link case. Hidden quarantined rows remain
until the existing full-traversal sweep. State-first retirement is conservative
if subsequent derived publication fails.

`../../HUMAN_TODO.md` was read and preserved; q-1/q-2/q-5 are now closed in live
state, unlike the historical receipt below. This fix round requires no new owner
choice. The driver owns the reviewed commit, push and cleanup of this worktree.
Recommended commit: `fix(catalog): preserve continuity and fence aggregate revisions`.

---

# L2 round 2 receipt - 2026-10-10

## Changed

Architecture revision 2 (PR #110) and issue #117's L2 errata are implemented on branch
`feat/l2-catalog`. The driver committed the work as two commits, after the merge of main at
`c64b258`, and then merged current main. Cargo.lock keeps main's pins and adds only the
catalogue's dependencies.

Migration 2 supplies AUTOINCREMENT/born_run, revisioned observations, root grants
and revocations in precious state, continuity/incarnation bindings and FULL/NORMAL
per-file durability. State commits precede derived commits; no transaction writes
both files. V1 precious tables remain in `legacy_v1_*` archives. Compatible rows
are copied, old references are retired, and incompatible label/disclosure history
is preserved without inventing provenance. A separate adapter is required to serve
that archived history. Catalogue rebuilds derive roots only from active grants.

Refresh now stages bounded chunks, publishes atomically, preserves incomplete
listings, reconciles native identity before names and retains unresolved tombstones.
Reparenting invalidates both ancestor chains; stale same-run finals are rejected.
ReFS fallback IDs are path-only. Observed deletion and mutable creation time retire
references; reused identity/creation time does not revive an earlier incarnation.
Explicit reconciliation is the only reattachment path after continuity is lost.

The old depth-by-table rollup is replaced by one checked bottom-up directory pass.
Ancestor invalidation coalesces shared chains per writer transaction, using cached
parent lookups and prepared updates; it remains depth-bounded, not constant-time
for a lone listing. Complete listing totals are accumulated from incoming chunks;
partial totals use one combined query. New files use cached multi-row INSERTs;
old listing metadata stays in temporary SQL tables instead of duplicated Rust maps.
Search paths use one ancestor query and cache shared parent paths. All declared
indexes remain, including allocated ordering; only a redundant parent-only index
was removed. P4 remains missed, as measured below.

Owned files: `src/{catalog,state}.sql`, `src/{db,lib,model,query,slice,writer}.rs`,
`tests/{open,queries,writer}.rs`, `tests/fixtures/{catalog,state}-v1.sql`,
`examples/bench.rs`, `README.md`, this receipt,
`evidence/round2-regression-mutations.json`, and the root benchmark receipt
`evidence/v3/bench/catalog-1m.json`; also the merge resolution in `Cargo.lock`.
Temporary editing/mutation helpers were removed. No personal data, real scan,
installer, release, OS effect or model request occurred.

## Verified

Windows, Rust/cargo 1.97.1. Required Cargo checks pass on the final source:

```text
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p loomward-catalog --release --example bench -- --rows 1000000
git diff --check
git diff --cached --check (imported-main whitespace finding, described below)
```

The L2 unstaged whitespace check passes. The cached merge check reports an existing
extra EOF blank line at `crates/loomward-telemetry/README.md:120` imported from main;
that other lane file was preserved. This is not a catalogue regression.

Workspace: 104 tests passed, none failed or ignored. Catalogue: 42 tests
(2 unit, 7 open/migration, 13 queries, 20 writer, including the crash-child helper).
The random-tree oracle covers eight deterministic seeds and twenty directories
per seed. The benchmark checks 1,000,000 files plus 1,001 directories against an
independent accounting oracle and confirms removal of its temporary directory.

### Conformance and regression map

| Architecture / ADR / errata | Test(s) |
|---|---|
| Section 6: strict paired schema, monotonic rows and incarnation | `fresh_pair_has_strict_schema_and_separate_data_classes`; `reused_creation_time_keeps_retired_incarnation_until_explicit_reconciliation` |
| Section 6 / ADR-V3-22: FULL state, NORMAL catalogue | `db::tests::writer_connection_uses_full_for_precious_state_and_normal_for_catalogue` |
| Section 6 / ADR-V3-22: state-first consent and durable revocation | `grant_survives_a_crash_between_precious_and_derived_commits`; `durable_revocation_fences_queued_observations_and_catalogue_rebuild` |
| Section 6 / ADR-V3-22: backups and migration preservation | `startup_keeps_seven_owned_backups_and_preserves_migration_backup`; `v1_migration_preserves_precious_history_and_moves_existing_consent_before_rebuild` |
| Section 7: staged atomic publication and shared cancellable byte budget | `staged_chunks_are_invisible_until_atomic_publication_and_cancellation_is_bounded`; `duplicate_staged_names_roll_back_the_whole_publication`; `queued_staged_publications_isolate_failure_and_commit_other_directories`; `writer::tests::failed_commit_closes_the_writer_and_wakes_producers` |
| Section 7 / ADR-V3-18: identity-first moves and incomplete listings | `source_first_move_retains_descendants_until_destination_is_listed`; `directory_identity_can_move_between_parents_without_order_dependent_unique_failure`; `incomplete_listing_never_establishes_absence` |
| N1 / ADR-V3-19: exact input revision, not just same run | `obsolete_same_run_final_cannot_publish_current_revision` |
| N2 / ADR-V3-18: complete-root sweep only | `targeted_run_keeps_an_unrelated_pending_tombstone` (also checks a later full sweep) |
| N3 / Section 6.3 / ADR-V3-20: ReFS 128-bit uniqueness/reference eligibility | `refs_64_bit_fallback_cannot_reparent_or_bind_durable_references` (128-bit positive control) |
| Partial #8 / ADR-V3-19: old AND new ancestor invalidation | `b_first_reparent_invalidates_both_chains_before_cancelled_repair` |
| Partial #10 / ADR-V3-20: retired incarnation despite reused creation time | `reused_creation_time_keeps_retired_incarnation_until_explicit_reconciliation`; `creation_time_edits_retire_directory_and_file_bindings_without_auto_reattachment` |
| Sections 7-8 / ADR-V3-19: repair and checked bottom-up sums | `sql_rollup_matches_brute_force_for_random_trees`; `invalid_listing_rolls_back_and_restart_marks_only_interrupted_roots_stale`; `atlas_arithmetic_is_checked_and_unscanned_generations_are_null` |
| Section 8 / ADR-V3-19: single provisional arena snapshot, clamp diagnostic | `live_overlay_requires_one_complete_arena_snapshot`; `live_other_clamps_are_counted_and_committed_other_never_clamps` |
| Section 8: allocated index and bounded name/modified sorts | `allocated_keysets_put_unknowns_after_known_zero_and_use_both_indexes`; `name_and_modified_sorts_refuse_over_10000_direct_children` |
| ADR-V3-21: entry bytes, scoped unique objects and default-stream coverage | `unknown_allocation_hardlinks_and_failed_run_rollup_are_distinct`; `unique_objects_are_separate_from_entry_bytes_and_not_relief` |
| Semantics section 5: subtree-bound children, catalogue-bound search, instance binding | `children_pages_are_keysets_bound_to_revision_sort_basis_and_anchor`; `budgeted_absent_search_resumes_and_catalogue_instance_invalidates_cursors`; `complete_breakdown_cache_is_generation_bound` |

Each of the five binding errata rules was removed separately: its named test failed
with exit 101, and the original source was restored. The mutation receipt is
`evidence/round2-regression-mutations.json`. No failing test was weakened or skipped.
N2 is deliberately conservative: targeted runs never globally delete tombstones;
no recorded-scope targeted absence proof is claimed.

### Final benchmark versus round 1

| Measure | Round 1, direct publication | Round 2, staged publication |
|---|---:|---:|
| P4 insertion | 25.716 s; 38,925 rows/s | 64.497 s; **15,520 rows/s (target 250,000 missed)** |
| Finalisation | 1.537 s | 1.855 s |
| P6 slice p50 / p95 | 21.958 / 22.837 ms | 44.870 / 48.904 ms |
| P7 children p50 / p95 | 0.795 / 0.850 ms | 0.996 / 1.291 ms |
| P8 search p50 / p95 | 32.458 / 34.775 ms | 16.617 / 18.434 ms |
| P8 budget hits | 21 / 21 | 21 / 21 |
| P14 catalogue | 126,676,992 B; 126.550 B/entry | 155,160,576 B; 155.005 B/entry |

The staged result is 60.1% below the old direct-listing rate and achieves 6.2% of
P4's target. P6 p95 and P7 are below their time thresholds in this 1M run; P6's
10M/HTTP gate remains unproven. Search exhausts its budget, not a full-search proof.

Measured P4 writer work: commit 40.691 s (63.1% of elapsed); listing 22.854 s,
including indexed file insertion 20.355 s (31.6% of elapsed) and name validation
0.586 s. Dirty propagation made 2,260 row updates for 1,001 directories, not a
full ancestor CTE for every file. Before allowing ListingDone into the existing
no-resolved-reference coalescing path, the staged probe was 130.318 s / 7,681 rows/s,
with 92.380 s in commits. The batch-isolation regression proves malformed staged
publication preserves other queued directories and the prior committed listing.
Coalescing reduced the measured commit cost, but commits and indexed insertion
remain the bottlenecks. There is no claim that removing quadratic rollup achieved
P4 or that this host measurement establishes an absolute throughput ceiling.

The final receipt measures `StageChunk` plus `ListingDone`, including file generation
and producer backpressure, with producer reservations taken before allocation. Round
1 used direct listings: this comparison includes the revised schema and staged path,
so it is not an isolated algorithm experiment. Index presence and accounting are
checked, not assumed. Writer timings overlap (file insertion is inside listing time)
and must not be summed twice. The host load was not controlled. These are observed
single-host rates, not a proven hardware ceiling. The best preliminary revision-2
direct-publication probe reached 32,713 rows/s; the final staged rate is the P4 result.
Do not label this performance-qualified: reopen the ADR-V3-06/index strategy review
for bulk indexed insertion and commit/staging overhead. Deferred index creation is
an architecture choice for the driver, not silently implemented in this slice.

## NOT verified

No 10M catalogue, cold-cache, HTTP, peak-private-commit calibration, physical power
loss, native L7 scanner/arena integration, L8 authenticated IDs/cursors, renderer,
Tauri, mapped-network drives, personal dataset or Python/UI suite. SQL and process
crash tests do not prove power-loss durability. No fresh-context review or hosted CI
was run by this worker; the driver owns review and publication.

## Residual risk and driver resume

P4 misses materially; P6/P14 receipts are 1M observations, not their 10M gates.
Migration preserves incompatible v1 labels/disclosures in archive tables but does
not expose them through a compatibility adapter. Unique-object totals are bounded
and are never space-relief evidence. Dataset locking, four-reader cap, arena lifetime,
native grant verification and authenticated transport remain engine obligations.

Driver: review this uncommitted diff and pending merge; wire L7's shared byte permits,
`ListingDone`, exact `DirFinal.input_revision` and complete arena snapshots; wire L8's
instance/kind/row/born-run authentication and cursor signatures. Commit the merge and
L2 changes in the driver-owned sequence. Suggested status/checkpoint edits (not made
here): record revision-2 catalogue/errata proof and P4 miss in docs/16 and
handoff/CHECKPOINT, and attach this conformance map to issue #117. No new public issue
or review request was posted.

`HUMAN_TODO.md` was read and preserved except for imported main: q-1, q-2 and q-5
remain open; q-3/q-4 are closed by the owner. This synthetic L2 task needs no new
owner decision and resolves none of those remaining decisions.

Recommended commit messages (no attribution trailers):

1. `fix(catalog): conform refresh and durable state to architecture revision 2`
2. `perf(catalog): replace depth rollups and record staged million-row measurements`

The worktree remains deliberately retained with uncommitted work. Its only ignored
build survivor is `target/`; no benchmark database is retained. Driver commits and
pushes before its own guarded cleanup. No worktree removal was attempted.

---

# Round 1 historical receipt (superseded by round 2 above)

# L2 catalogue receipt â€” 2026-10-09

Changed: uncommitted work in the `lw-l2` worktree, branch `feat/l2-catalog`, base and HEAD
`f4c6f1ae28dd999f6654f9cb18dffdeedf970bf1` (`arch/v03-architecture`). The driver commits.
Initial worktree was clean. No push, PR, merge, issue, real-disk scan or external application
action occurred. Network use was Cargo package-registry resolution only.

Files changed: root `Cargo.toml` has only the authorized glob member edit; `Cargo.lock` was
regenerated. New owned crate files: `Cargo.toml`, `src/{lib,db,model,writer,query,slice}.rs`,
`src/{catalog,state}.sql`, `tests/{open,writer,queries}.rs`, `examples/bench.rs`, `README.md`,
this receipt, and `evidence/catalog-1m-before-optimization.json`. Required benchmark exception:
`evidence/v3/bench/catalog-1m.json`. Coordinator-owned files were not edited.

## Acceptance

- MET: doc 41 section 6 schema is migration 1 for both STRICT databases; WAL/NORMAL/FKs and application/version markers are set.
- MET: foreign/newer/unowned-nonempty databases and UNC/device paths refuse; quick_check runs at open; precious migration backup, rollback and interrupted-backup retry are tested.
- MET: single writer thread, BeginRun/DirListing/DirFinal/EndRun, bounded queue and coalesced whole-listing commits; see the explicit oversized-listing exception in README.
- MET: refresh preserves IDs, identity renames and rename swaps; already-present directory identities reparent across listings with descendant depths preserved and cycles refused; unchanged listing revision stays stable; removed subtrees cascade.
- MET: logical, known allocated and allocation-unknown sums remain separate; sparse allocation and hard-link flags are tested; no reclaimability/effect claim.
- MET: slice folding, pre-order/depth/node bounds, Atlas/volume/root anchors, approximate_files/approximate_live and explicit arena overlay.
- MET: keyset children cursors bind listing_rev, anchor, sort and basis; aggregate changes also invalidate them.
- MET: path, inspect, bounded-budget search, generation-bound search cursors, and breakdown; contract projections validated against the actual JSON Schema.
- MET: random-tree SQL rollups equal independent brute-force sums (eight deterministic seeds, twenty directories each).
- MET: an abrupt child-process exit inside an uncommitted SQLite transaction preserves the prior committed listing; restart also recomputes incomplete totals from committed rows.
- MET: required 1M benchmark generated metadata in-process, matched the oracle and deleted the temporary directory.
- MISSED PERFORMANCE HYPOTHESIS: P4 is 38,925 rows/s against 250,000; doc 41 section 19's review trigger fires. No indexes were dropped or checks weakened.

## Verified

Windows, Rust/cargo 1.97.1. These exact commands completed with exit 0 on the final code:

```text
cargo test -p loomward-catalog
  open:    test result: ok. 5 passed; 0 failed; 0 ignored
  queries: test result: ok. 7 passed; 0 failed; 0 ignored
  writer:  test result: ok. 7 passed; 0 failed; 0 ignored
  (19 tests includes the crash-child helper; the crash test also observes exit code 17.)
cargo clippy -p loomward-catalog --all-targets -- -D warnings
  Finished `dev` profile ... (exit 0)
cargo fmt --all --check
  no output, exit 0
git diff --check
  exit 0; Git emitted only its LF-to-CRLF advisory
cargo test --workspace
  all above plus the existing 19 core tests passed
cargo clippy --workspace --all-targets -- -D warnings
  Finished `dev` profile ... (exit 0)
cargo run -p loomward-catalog --release --example bench -- --rows 1000000
  oracle.matched: true; temporary_directory_removed: true; exit 0
```

Red evidence observed before implementation/fixes: fresh/unsafe-open tests; writer identity,
accounting and recovery tests; query/fold/budget tests; aggregate-finalisation cursor test;
breakdown-cache test; interrupted-backup restart test; restart committed-total assertion; cross-parent directory identity move/cycle test.
An initial compile/API mismatch was corrected before treating a test run as red evidence.

| Receipt | Final measurement | Scope |
|---|---:|---|
| P4 | 25.716 s, 38,925 rows/s | Generation + writer backpressure, all indexes; target missed |
| P6 | p50 21.958 ms / p95 22.837 ms; 2,500 nodes; 1,383,727 bytes | Warm catalogue + serialization, 1M files |
| P7 | p50 0.795 ms / p95 0.850 ms | 200-row logical-size page in a 1,000-file directory |
| P8 | p50 32.458 ms / p95 34.775 ms; 21/21 budget hits | Absent substring, 2M SQLite-op budget |
| P14 | 126,676,992 bytes; 126.550 bytes/entry | Checkpointed catalogue, 1M files + 1,001 dirs |

## NOT verified

No 10M run, cold-cache run, HTTP round trip, live scanner/arena integration, renderer, Tauri,
process-memory calibration, Windows/native file identity acquisition, mapped-network-drive
test, personal catalogue or full Python/UI gate. There is no remote PR/CI result for this lane.
L1/L8 protocol crates are absent from this base, so projections were validated directly against
the checked-in contract rather than compiled against their future DTOs.

## Deviations, coordinator work and residual risk

All dependencies are ADR-V3-02-approved. `jsonschema` has default features disabled so its
validator cannot fetch external schemas. `time` sets this crate's declared minimum Rust to
1.88; the root manifest was not widened. Reader pooling, dataset instance locking, and
root/depth-1 breakdown warming are engine responsibilities; README names the calls. Metadata
root-registration is internal, never a path-accepting view command or filesystem grant.
Native cursors/references require L8 signing/remapping before transport. One oversized listing
can exceed the normal 200k-entry memory estimate while exclusively occupying the entry quota.
P4 remains materially below target; use the measured miss for the ADR/index strategy review.

Driver integration: coalesce the identical workspace glob edits from #108/#109, regenerate
the lockfile with all lanes, wire L7's writer receipts and live overlay, cap readers at four,
and let L8 map each contract-shaped projection and sign its internal cursors. Proposed status
updates (not made here): mark this catalogue slice implemented with Windows-only synthetic
proof; record the P4 miss and 10M/HTTP limits in docs/16 and handoff/CHECKPOINT; reconcile
LW-005/LW-009/LW-011's native catalogue portion without claiming the other UI/OS gates.

HUMAN_TODO.md was read and preserved: q-1 through q-4 remain open in this base. L2 requires no
owner decision to finish its synthetic proof; it does not resolve the real-disk grant item.

Recommended commits:

1. `feat(catalog): persist v3 observations with atomic refresh and bounded views`
2. `perf(catalog): record million-row Windows catalogue measurements`

Worktree is deliberately **not removal-ready**: all work is uncommitted. Final ignored survivor
is `target/` (rebuildable Cargo output); no personal data or database is retained. The driver
must commit/push before cleanup and run its integration/review gate. No deletion was attempted.

## PR #147 connector fix round 2 â€” 2026-10-10

### Changed

Uncommitted changes from clean `feat/l2-catalog` HEAD
`6c2b6d116179c4cbb4f69b45be3b747ae4bf826d`; the driver commits and pushes.

1. Before retiring or rebinding precious references, execute the actual publication
   in a catalogue-only transaction and roll it back. This covers every publication
   rejection, for direct and staged listings, while preserving state-before-catalogue
   commit ordering. The extra pass runs only when resolved references exist. Tests:
   `duplicate_staged_names_preserve_durable_references_and_old_listing` and
   `conflicting_staged_families_preserve_durable_references_and_old_listing`.
   Both retain the omitted file and replaced directory bindings, state/catalogue
   revisions and old rows; a valid replacement in the next run still succeeds.
2. All database inspection queries map SQLite CORRUPT/NOTADB errors to
   `CorruptDatabase`; other SQL errors remain SQL errors. Only catalogue corruption
   takes the existing archive/rebuild route; state corruption is reported untouched.
   Test: `corrupt_schema_page_rebuilds_only_the_derived_database`, using an invalid
   schema b-tree page with readable application/version headers, checks both files,
   exact archived bytes, a healthy rebuilt catalogue and preserved precious bytes.
3. Clear/set file link flags across every directory of every root on the rebuilt
   volume. Test:
   `multilink_flags_cover_both_roots_in_either_finalisation_order_and_clear_after_removal`
   checks both finalisation orders, both flags after either root finalises, and the
   surviving flag clearing after the other link is removed.

### Verified

Windows: `cargo fmt --all --check`, `cargo test --workspace` (211 passed,
49 catalogue tests; zero failed/ignored),
`cargo clippy --workspace --all-targets -- -D warnings`, and `git diff --check` pass.
Every final new test also fails with exit 101 against the original committed
production sources for its expected defect. Fixed source bytes were restored
exactly; hashes and commands are in `evidence/pr147-fix-round2.json`.
No existing test was weakened.

Recommended commit: `fix(catalog): preserve references and recover derived state`.

### NOT verified

Hosted CI/review threads, Python/UI, scale performance with durable references,
physical power-loss durability. No commit, push, merge or external action.
Shared implementation-status/checkpoint integration remains with the driver.

### Residual risk

Resolved references add a rollback-only publication pass; its performance impact
has not been measured. This does not create cross-file atomicity or establish
physical power-loss guarantees. The existing P4 throughput miss still stands.
Worktree is not removal-ready because these changes are uncommitted.
`HUMAN_TODO.md` was read and preserved: q-5 remains open, q-6 is not yet needed;
neither blocks this synthetic catalogue fix.

## PR #173 single fix round — 10 October 2026

### Changed

Uncommitted worker changes on `feat/l2-catalog-durable`, starting at
`7066d7ea953b350fda4b1a75cdf04576a8c1d1cc`. The driver commits and pushes.

1. A rejected staged chunk fences its `(run_id, dir_id)` in the writer across
   batches. Later chunks reject; a queued complete outcome becomes partial and
   upsert-only, including precious-reference preparation. Explicit incomplete
   reasons remain intact. The fence expires when the run ends. Regression:
   `rejected_chunk_fences_queued_completion_without_deleting_old_entries` queues
   the accepted prefix, a sequence gap and completion before waiting, with and
   without a durable reference; omitted files and directories survive, a different
   directory completes, and the failed listing never completes. Before the fix,
   the old-file count was 0 instead of 1.
2. Root origins are checked against `st.meta.dataset_class` before a precious
   grant is written. Synthetic accepts fixtures and registered lab roots; personal
   accepts owner grants. Regression:
   `root_origins_must_match_the_dataset_before_any_grant_is_written` checks both
   rejection directions and positive controls, including registered labs. Before
   the fix, synthetic/owner_granted incorrectly returned a root.
3. Validated outcomes are stored separately from the listing lifecycle state and
   projected as contract coverage states. Schema 4 adds nullable `listing_outcome`
   without rebuilding or dropping observations. Old rows fall back to their old
   coverage; their previously lost reasons cannot be recovered. Regressions:
   `incomplete_listing_reasons_survive_children_slices_and_restart` exercises
   direct/staged publication, all six incomplete coverage states, stale children,
   restart and schema validation; before the fix denied projected as partial.
   `invalid_incomplete_reason_cannot_publish_or_delete` rejects empty, complete and
   invented incomplete reasons. `v3_upgrade_preserves_observations_and_legacy_coverage`
   pins the additive migration, original instance, retained rows and new cancelled
   writes. Existing v2 durable-reference migration tests remain green.
4. Search yields examined rows internally with a match flag, advancing the
   unchanged cursor only after each row is fully examined. The lookahead match
   still resumes after the last returned match. Regression:
   `absent_budgeted_search_advances_examined_rows_and_terminates` uses a fixed
   500-op budget and checks strictly increasing watermarks through termination;
   before the fix it stalled at file ID 0. Additional sparse-search proof:
   `sparse_budgeted_search_keeps_every_match_without_duplicates`.
5. The corruption fixture now deliberately restores a healthy page-1 WAL,
   checkpoints/truncates it, switches the closed fixture to DELETE journaling,
   verifies no WAL/SHM remains, and only then corrupts the main file. The probe is
   read-only and closed before archive/rebuild. It covers page sizes 512, 4096 and
   65536 (including header encoding 1), with all original exact-archive-byte,
   healthy-rebuild and unchanged-precious-state assertions. sqlite_schema's root
   is page 1, whose b-tree header is at byte 100 regardless of page size
   ([SQLite file format](https://www.sqlite.org/fileformat.html)). A valid WAL
   can supply that page instead of the main file
   ([SQLite WAL](https://www.sqlite.org/wal.html)). Omitting fixture cleanup
   reproduced the masking mechanism on Windows: the damaged schema returned
   `Ok(25)` instead of CORRUPT, failing the test with exit 101. Restoring the exact
   fixture bytes returns green. This is evidence for the portable repair, not a
   claim that the Linux job's precise environment was reproduced. Archive code
   already uses rename after inspection closes; it needed no platform-specific
   change.

Public API: no method signatures, command/reply shapes, DTOs or cursor fields
changed for L8. The exported `SCHEMA_VERSION` value changes from 3 to 4; existing
databases migrate additively. Root-origin and incomplete-reason rejection is
intentional validation tightening.

Recommended commit messages:

- `fix(catalog): fence failed staged listings`
- `fix(catalog): enforce dataset-specific root origins`
- `fix(catalog): preserve incomplete listing coverage`
- `fix(catalog): advance search cursors past examined rows`
- `test(catalog): isolate schema corruption from WAL recovery`

### Verified

Windows: `cargo fmt --all --check`, `cargo test --workspace` (304 passed,
64 catalogue tests; zero failed/ignored),
`cargo clippy --workspace --all-targets -- -D warnings`, and `git diff --check`.
The four behavioral regressions were observed red against the starting source
before implementation. The WAL-masking experiment was observed red with cleanup
omitted, and the test was restored byte-for-byte. No existing test was weakened.
All observations in the tests are disposable synthetic fixtures.

### NOT verified

Linux execution and the precise cause of the reported Ubuntu job; hosted CI,
the separate L8 service runtime, scale performance, Python/UI and physical
power-loss durability. No commit, push, merge or external action.

### Residual risk

Linux must execute the final corruption fixture to establish hosted portability.
WAL masking is reproduced locally and now excluded by construction; page-size
assumptions and live handles are also explicitly checked. Old generic incomplete
rows retain partial coverage because their original reason was never stored.
Schema 4 cannot be opened by the older schema-3 binary. The worktree remains
owned by the driver and is not removal-ready while changes are uncommitted.
`HUMAN_TODO.md` was read and preserved; q-5 remains open and q-6 is not yet needed,
neither relevant to this synthetic fix round.
