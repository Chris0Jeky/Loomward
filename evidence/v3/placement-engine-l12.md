# L12 placement engine: local Windows proof

10 October 2026. Worktree `C:/Users/jekyt/wt/lw-l12`, branch `feat/l12-placement`,
starting HEAD `e103787` (C2 merged). This is an uncommitted worker handoff; the driver commits.
Static placement and proposals only. Production code reads metadata; it moves no files and controls no processes.
Disposable test files and hard links are created only by tests and cleaned by `tempfile`.

## Changed

`loomward-engine::placement` now derives tier views and antichain candidates from a
`PlacementInput`, builds a reference-planner scenario, runs the unchanged #107 allocator,
and delegates atomic saving/list/get to a `ProposalStore`. These traits leave L2 catalogue
and L8 durable state ownership with those lanes. The C2 engine methods now take these
dependencies explicitly; placement reads/simulation also take an absolute `Instant` deadline.
`placement_simulate` takes an optional proposal store, required only for `save: true`.
The remaining C2 smoke test defers placement to its implemented entry-point tests.

The Windows link-count pass reuses the existing native identity observer, whose opens use
`FILE_READ_ATTRIBUTES`, share read/write/delete and
`FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS`. Listed and current offline/recall
flags, reparse entries and directories are refused. Matched observations require the listed
object identity; the engine additionally requires known allocation, matching EOF/allocation,
the source's native volume serial and all observed link names inside the group. ReFS legacy
64-bit IDs cannot establish unique relief. Object sharing is keyed by native volume serial
and file ID, never file ID alone.

Logical bytes feed transfer; known entry allocation feeds the candidate display. Unknown
allocation uses a labelled logical fallback. Verified unique allocation feeds planner
`source_bytes`. Owner what-ifs can use entry allocation but cannot override volume physics.
Destination estimates round every listed EOF to the largest known eligible target cluster.
Unknown flags, heat and movement history are absent from planner input, while the candidate
DTO retains its nullable observations. `rejected` remains exactly the core planner output;
service exclusions use their separate contract fields.

## Verified

Windows 11, Python 3.14.3, rustc 1.97.1. Commands executed in this worktree:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | PASS (also run by `verify.py`) |
| `cargo test --workspace` | PASS; final `verify.py` run includes 132 Rust tests |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS after correcting the link result enum and two test clones |
| `py -3 scripts/verify.py` | PASS: 216 Python tests with 1 symlink-privilege skip, 2 JS syntax checks, 9 JS assertions, 80 cross-language admission fixtures, and all Rust gates |
| `git diff --check` | PASS |

Placement has 19 tests. Its parity test writes a temporary JSON fixture and invokes `py -3`
against the real Python reference: 30 builder scenarios (10 cases x budgets 0, 1, 50,000)
match Rust field by field. Every case also compares engine/core `rejected` serialization
byte for byte. The Python-absent path prints `UNVERIFIED` and skips only this parity test.
Python was present and the comparison executed on this host.

Two Windows link-count tests prove matching native hard-link identity/link counts,
replacement refusal, all three listed placeholder flags, the 200,000-name cap and deadline
stops. A separate engine test feeds the real native pass into verified relief and the planner;
it measures the actual allocation rather than assuming a 100-byte file allocates 100 bytes.
The synthetic worked example independently pins entry allocation 200, unique relief 100,
destination estimate 256 and logical transfer 200.

Tests were written before the first scenario, native pass and engine-method implementations;
those tests failed at their stubs before passing. Additional red regressions pinned the
64-volume tier view versus the 32-volume planner limit, missing allocation combined with an
unparseable link observation (no panic), and partial-allocation logical fallback.
The original allocator v1 and its 51-case negative evidence were not changed; their existing
counterexample/oracle tests ran in the workspace suite.

## Conformance

Names below are test functions in `placement/tests.rs`, except the two `linkcount_*` tests.

| Requirement | Discriminating test |
|---|---|
| Doc 41 section 10; ADR-V3-13: declarations, device hints and unknown tier | `placement_tiers_prefer_declaration_then_hints_then_unknown` |
| Doc 41 section 10: pressure, candidates, model and proposals | `placement_engine_model_candidates_and_saved_proposals_roundtrip` |
| Doc 41 section 10; ADR-V3-13: excluded offline/unknown volumes | `placement_excludes_unknown_or_offline_volumes_but_retains_them_in_tier_view` |
| Doc 41 section 10: 64-volume view, bounded reference input | `placement_tier_view_supports_64_volumes_while_planner_is_bounded_to_32` |
| Doc 41 section 10; ADR-V3-13 amended: ancestry antichain | `placement_antichain_property_on_random_trees_and_root_children` (100 seeded trees; both bases) |
| ADR-V3-21; #117 partial #4: entry 200, relief 100, cluster rounding and successful plan | `placement_relief_maps_entry_200_to_unique_100_and_planner_accepts` |
| ADR-V3-21; #117 partial #4: known allocation, matched native identity, all names | `placement_requires_known_allocation_matching_identity_and_all_names` |
| #117 partial #4; native identity policy: serial binding and ReFS fallback | `placement_volume_binding_and_refs_legacy_ids_do_not_verify_relief` |
| Doc 41 section 10: metadata-only pass, cap/deadline | `linkcount_skips_placeholders_and_stops_at_deadline_and_file_cap` |
| ADR-V3-21: native hard links, replacement/placeholder refusal | `linkcount_disposable_hardlinks_match_identity_and_refuse_replacement` |
| ADR-V3-21: native pass actually feeds verified engine relief | `placement_native_link_pass_feeds_verified_relief_on_disposable_hardlinks` |
| Doc 41 section 10: all three pre-rejection reasons, including shared objects under what-if | `placement_pre_rejects_unknown_relief_shared_objects_and_incomplete_coverage` |
| ADR-V3-13 amended: unknown planner fields omitted, never null | `placement_unknowns_are_omitted_never_null_and_planner_reasons_stay_exact` |
| Doc 41 section 10: explicit heat policies and nullable overrides | `placement_explicit_heat_whatifs_and_null_overrides_do_not_change_physics` |
| ADR-V3-21: labelled entry-allocation what-if | `placement_whatif_uses_entry_allocation_and_labels_assumptions` |
| Doc 41 section 10: partial allocation remains unknown relief | `placement_partial_allocation_uses_logical_fallback_and_rejects_whatif` |
| #117 N5: deadline error, no job result, synchronous bounded search | `placement_deadline_before_and_after_input_is_an_error`; `placement_engine_rejected_is_byte_equal_to_core_and_node_budget_remains_synchronous` |
| #107 and doc 41 section 10: exact rejection and Python parity | `placement_python_reference_parity_through_generated_fixture_file` |
| Doc 41 section 10: safe integers, distinct names and checked rounding | `placement_refuses_duplicate_names_unsafe_integer_and_rounding_overflow` |

## Files changed

- `crates/loomward-engine/src/placement/mod.rs`
- `crates/loomward-engine/src/placement/tests.rs` (new)
- `crates/loomward-engine/tests/smoke.rs`
- `crates/loomward-engine/Cargo.toml` and `Cargo.lock` (existing workspace dependencies only)
- `crates/loomward-windows/src/linkcount.rs`
- `docs/16-implementation-status.md`, `handoff/CHECKPOINT.json`, this receipt

## Integration questions for L2 and L8

- **L2:** Can the adapter provide a single consistent read/revision boundary for both
  `snapshot` and `files` (including same-generation changes), with complete canonical ancestry
  across overlapping granted roots? `files` receives the snapshot's root generations and must
  return `StaleGeneration` rather than combine observations from different revisions.
- **L2:** Supply unique name row IDs, listed native object identity/allocation, source native
  volume serial, trusted granted paths and per-name link observations. The engine never accepts
  a service path. Groups beyond 200,000 files cannot establish complete estimates/relief and are
  pre-rejected; oversized adapter responses return `resource_budget`.
- **L8:** Wire the explicit input/deadline/store signatures. Store saves must check the deadline
  at their atomic commit, assign a creation time and proposal ID, and implement persistent
  list/get and root-generation staleness. Missing storage returns `capability_unavailable`
  for `save: true`; `save: false` works without a store.
- **L8:** Confirm the labelled modification-age mapping (30+ days -> 0.1, 7+ -> 0.5,
  younger -> 1.0). Default and assumed-only policies never infer heat from modification time.
  Surface exact source exclusion messages such as `source_tier_unknown` in protocol error detail.
- **L8/driver:** The existing reference planner accepts at most 32 included volumes and requires
  positive group quantities. The engine returns `resource_budget` for more volumes; reference
  validation rejects a zero-relief group. Unknown cluster sizes use the labelled known-cluster
  estimate/logical fallback. Broader semantics would need a contract/planner change outside L12.

## NOT verified

Live L2 catalogue and revision races; durable L8 proposal storage, restart/staleness and service
dispatch; live cloud-placeholder provider hydration; large-catalogue performance; native Tauri
or browser interaction; hosted CI. Proposal save/list/get are verified with an in-memory test
store, not claimed durable. No real-folder stress scan was performed.

## Residual risk

Metadata observations are point-in-time estimates, not race-free effect authority. Deadline
checks occur between OS calls and around the bounded planner; a blocked metadata call cannot
be forcibly preempted by this lane. Estimates cover default streams only and do not predict
compression, sparse layout, alternate streams or future mutations. Live safety and durability
depend on L2/L8 honouring their explicit adapter contracts.

`HUMAN_TODO.md` is unchanged: q-5 (personal teacher disclosure/tier) remains open and q-6 is
not yet needed. Neither blocks this static placement lane. No commit, push, PR or external
publication occurred. The driver must retain this dirty worktree until it commits the changes;
ignored `target/` and Python caches contain no required evidence.

Recommended commits (driver only): `Observe bounded Windows link counts for placement relief`,
then `Wire static placement simulation and proposals into the engine` (include tests and this
status receipt; no attribution trailers).
