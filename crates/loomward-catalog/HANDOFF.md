# L2 round 2 receipt - 2026-10-10

## Changed

Architecture revision 2 (PR #110) and issue #117's L2 errata are implemented in
`C:/Users/jekyt/wt/lw-l2`, branch `feat/l2-catalog`. HEAD remains
`6499bc8ea71d6d8b17c8684d9c8cac095b6019ba`. `git fetch origin` completed and
`git merge --no-commit origin/main` imported main at
`c64b2588bef48020e54eb797fd9522a815de6fef`. Cargo.lock was regenerated with
`cargo generate-lockfile` and its conflict resolved. The merge remains pending:
**no commit or push was made; the driver owns both.** The initial tree was clean.
Staged changes outside L2 are imported main, not new L2 work.

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

# L2 catalogue receipt — 2026-10-09

Changed: uncommitted work in `C:/Users/jekyt/wt/lw-l2`, branch `feat/l2-catalog`, base and HEAD
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
