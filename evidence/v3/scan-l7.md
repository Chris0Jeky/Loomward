# L7 native scan handoff - LW-006 / LW-101

10 October 2026, Windows 11. Branch `feat/l7-scan`, base/head
`e1037873dfebe11dc9fb8a209b72eda44693b92e`. Changes are **uncommitted and unpushed**, as requested;
the driver owns review, integration and commits. No dependency on `feat/l2-catalog` was introduced.

## Changed

- Width-preserving native source seam lives in `loomward-windows::enumerate`; the engine re-exports it.
  Enumeration starts with Extended, falls back to Both on unsupported-class errors before emitting
  entries, then Find. Child opens use `NtCreateFile` relative to the parent handle. Post-open attributes
  and same-width IDs are validated before listing. An unverifiable listed ID refuses the child;
  zero IDs are absent, and ReFS 64-bit IDs cannot identify or reparent objects uniquely.
- A bounded worker pool sends revision-tagged chunks to one writer under a shared 64 MiB semaphore.
  Arena, queued directory handles/tasks, native buffer headroom and chunk vector capacities are reserved.
  Scheduling uses a mutex/condition-variable FIFO deque, avoiding recursive native-buffer retention.
  Byte, entry, directory and depth exhaustion produces partial coverage, never a complete absence claim.
- `ListingTicket` binds catalogue directory, input revision and old parent; `DirListing`, `ListingDone`,
  `DirFinal` and `InvalidateChains` form the L2 seam. Targeted runs downgrade listings outside their
  explicit directory scope to upserts-only. Only complete full-root runs may sweep globally.
- Jobs support registered kind runners, retained status/list/cancel, one active scan per root and
  immediate cancellation acknowledgement. Terminal records remain for the ten-minute idempotency window.
  Root cancellation and a separate writer-fence hook support L8's commit-revocation-first order.
- Epoch-scoped event streams replay up to 1,024 events within 60 seconds, distinguish replay gaps,
  epoch changes and live subscriber overflow, and send unsequenced, unreplayed silence heartbeats.
  Scan jobs publish final progress and job state; progress stays unknown for the Both class until
  L1 adds its missing wire strategy variant.
- `EngineConfig` is non-exhaustive with a constructor. `GrantedRoot` fields are private; its ordinary
  constructor is crate-private and includes expected native identity. An explicit `lab` feature exposes
  only a marked scale-lab fixture factory. Scan start/refresh rejects class mismatches before I/O.
  Busy/stale/internal errors preserve optional structured wire detail.
- The lab CLI now implements `bench scan --tier S|M --runs N` against existing, marked fixtures.
  Its in-memory sink retains directory aggregates, not a million-entry inventory.

## Verified

Final source hashes and command receipts: [scan-gates.json](bench/scan-gates.json).

- `cargo fmt --all --check` - PASS.
- `cargo test --workspace` - **153 passed** on Windows.
- `cargo clippy --workspace --all-targets -- -D warnings` - PASS.
- `cargo test -p loomward-windows --all-features` - **24 passed**.
- `cargo run -p loomward-lab --release -- bench scan --tier S --runs 20` - 20 complete exact matches.
- `cargo run -p loomward-lab --release -- bench scan --tier M --runs 20` - 20 complete exact matches.
- Each of N1, N2, N3, partial #2, partial #8 and N6 was temporarily disabled; its named runtime
  test failed, and original source bytes were restored. [Mutation receipt](bench/scan-errata-mutations.json).
  Missing-API compilation failures were initial test scaffolding, not claimed as behavioral red proof.
- Additional observed red-to-green cases: zero IDs being promoted to identity, sequenced heartbeats,
  and no-ID staged entries marked identity-eligible.

### Conformance

| Doc 41 / binding #117 rule | Proving test |
|---|---|
| Section 4.1 record bounds / zero success / buffer end versus EOF | `malformed_next_entry_offset`; `name_must_fit_its_record`; `zero_length_buffer_and_name_are_malformed`; `buffer_end_is_not_directory_end` |
| Section 4.1 strategy classes and preserved widths | `native_handle_relative_open_and_all_strategy_widths` |
| Section 4.1 relative child opens | `child_open_remains_bound_to_renamed_parent_handle` |
| Section 4.1 post-open attributes / reparse refusal | `junction_never_opened_or_followed`; `post_open_offline_race_is_refused_before_listing`; `placeholders_and_reparse_attributes_are_refused` |
| Section 4.1 / partial #2 ID replacement, missing query, 64-bit compatibility | `post_open_attributes_and_id_replacement_are_rejected`; `missing_post_open_id_does_not_trust_listing`; `native_handle_relative_open_and_all_strategy_widths` |
| Section 4.1 absent ID policy | `absent_zero_ids_do_not_become_unique_observations`; `writer_failure_unblocks_producers_and_releases_shared_bytes` |
| Section 6.3 / N3 ReFS 64-bit downgrade | `id_width_is_preserved_and_refs64_is_not_unique`; `obsolete_run_final_and_nonunique_refs_cannot_reparent` |
| Section 7.4 shared memory / cancellation / writer failure | `shared_bytes_wait_is_cancellable_and_permits_release`; `writer_failure_unblocks_producers_and_releases_shared_bytes`; `staged_pipeline_has_exact_totals_and_releases_bytes` |
| Section 7.7 / N1 exact listing input revision | `same_run_obsolete_final_is_rejected_n1`; `cross_run_ticket_cannot_finalize_an_unlisted_directory` |
| Section 7.8 / N2 scope-bound absence | `targeted_run_never_sweeps_unrelated_tombstones_n2`; `targeted_pipeline_never_establishes_absence_outside_scope`; `incomplete_listing_does_not_authorize_absence` |
| Section 7.6 / partial #8 both reparent chains | `reparent_invalidates_both_chains_and_restart_repairs` |
| Section 7.9 restart to repairing | `reparent_invalidates_both_chains_and_restart_repairs`; `writer_install_is_startup_only` |
| Section 7 cancellation acknowledgement / handle lifetime | `blocked_worker_cancel_ack_does_not_close_live_handle`; `cancels_blocked_native_io_without_closing_its_handle` |
| Section 5.3 / semantics Section 7 reconnect / overflow / heartbeat | `resumes_inside_window_and_lagged_outside`; `reconnect_replay_expires_after_sixty_seconds`; `heartbeat_after_configurable_silence_and_epoch_change`; `heartbeat_is_unsequenced_and_not_replayed` |
| N4 terminal retention prerequisite | `kind_dispatch_terminal_retention_and_cancel_ack` |
| Section 14 / N6 M remains 1M; oversized case separate | `m_tier_stays_one_million_n6`; `oversized_stress_is_separate_with_partial_oracle_n6`; S/M manifest benchmarks |
| #144 sealed grant / class check / error detail | `dataset_class_is_checked_before_start_or_refresh`; `root_identity_mismatch_never_enters_writer`; `busy_scan_preserves_job_id_wire_detail`; `checked_sums_report_byte_overflow_detail` |
| Section 7 self-exclusion and ?11 worker caps | `native_state_root_is_excluded_by_handle_identity`; `scan_worker_cap_is_shared_across_roots_and_released` |
| L8 revocation writer fence | `fence_rejects_late_staging_and_final`; `blocked_worker_cancel_ack_does_not_close_live_handle` |

The oversized fixture is a generated **mock source**, not M and not a 2.5M-file on-disk directory.
Its partial oracle is exactly 2M indexed entries, unknown allocation, incomplete coverage and no sweep.
The native cancellation primitive was exercised on a blocked **owned named-pipe call**; the pipeline
handle-lifetime test uses a deliberately blocked synthetic directory source. Neither is a hung-volume test.

### Benchmarks

| Tier | Files | Median wall s | Files/s from median | Enumeration worker s (sum) | Sink s | Complete manifest matches |
|---|---:|---:|---:|---:|---:|---:|
| S | 100,000 | 0.024259 | 4,122,266 | 0.137167 | 0.009219 | 20/20 |
| M | 1,000,000 | 0.205443 | 4,867,530 | 1.321334 | 0.129974 | 20/20 |

Eight workers, 16 KiB native buffers, existing generated lab trees. Logical bytes, allocation bytes,
file and descendant-directory counts match the independent manifests on every trial.
Raw runs: [S](bench/scan-s.json), [M](bench/scan-m.json).

Enumeration worker time is summed across eight workers and includes callback/backpressure time.
Sink time is measured inside staging/publication calls. These phases **overlap**; subtracting either
from wall time is invalid. The sink does no SQLite I/O or per-file identity indexing, so these are not
persistent-catalogue throughput numbers. Cache is warm/uncontrolled, including the slower first trials;
no cold-cache, HDD, CPU-time or RSS qualification was performed.

## NOT verified

Physical ReFS fallback behavior, a genuinely unsupported-class filesystem, configured Cloud Files
placeholders and no-hydration canaries, hung/removable volumes, SQLite atomic publication/rollups,
power-loss recovery, service/native-dialog grants, watchers, HTTP/Tauri delivery and hosted CI.
The field/method design for config and private grant construction is checked by Rust compilation;
ordinary service construction awaits L8's proof boundary.

## Residual risk and integration questions

**L2:** implement `ScanSink` with durable root/run recovery and a single writer. Map tickets to stable
catalogue directory IDs, stage until one directory transaction publishes, retain unseen children for
all incomplete outcomes (including `OutsideScope`), and reject both inactive-run and stale-input finals.
Coordinate ancestor revision changes with the arena's input tickets: a child publication must not
cause the adapter to stamp an obsolete aggregate with the writer's current revision. Both move chains
must be invalidated even if a run fails. The aggregate-only fixture sink is not a file-row/absence oracle.
Its interrupted roots remain unreadable/repairing; actual bottom-up repair is the L2 writer's responsibility.
Persisted interrupted job records must be loaded as failed rather than silently retried.

**L8:** resolve grants from authoritative `state.db` rows and supply expected handle identity and
volume identity. The ordinary `pub(crate)` constructor cannot be called directly from a separate
service crate: wire an engine-owned grant resolver or a sealed durable-grant proof, rather than
reopening a raw public path/class constructor. Commit revocation first, call `scan_cancel_root`,
then `scan_writer_fence`; fencing must be quick and reject already queued and future messages.
Install the sink once, before serving roots. Supply committed revisions when emitting catalogue events.
The current scan pool cap is configurable; trusted seek-penalty hints and incremental progress/target
selection can be integrated with L8/L16. `scan_refresh` conservatively traverses the whole granted root;
explicit targeted scopes permit upserts outside their reconciliation set, never absence.

**L1/L6:** the wire enumeration enum has no `file_id_both_directory_info` variant. Native observations
keep the actual Both class and 64-bit width; final wire progress remains null for that class. Add the
variant through the shared-contract owner. Adapters must treat hello as unsequenced control, and
pass parsed `Last-Event-ID` as `EventResume` without discarding the epoch.

**L16:** establish watches before scanning and dirty/relist until the run is reconciled. No watcher
or USN catch-up behavior is claimed in this lane.

[HUMAN_TODO.md](../../HUMAN_TODO.md) is unchanged: q-5 remains open; q-6 is not yet needed.
No new owner action is needed for the fixture-only work here.

## Recommended commits

1. `feat(windows): add validated handle-relative enumeration`
2. `feat(engine): add bounded scan jobs and revision-fenced staging`
3. `test(scan): qualify native fixtures and M-tier throughput`

No attribution trailers. The driver owns all commits and review. The worktree is not removal-ready
while these requested uncommitted changes remain. The sole ignored build survivor is `target/`,
a disposable Rust build cache; no personal diagnostics or creative source were created.

## Files changed

- `Cargo.lock`
- `crates/loomward-engine/Cargo.toml`
- `crates/loomward-engine/src/budgets.rs`
- `crates/loomward-engine/src/error.rs`
- `crates/loomward-engine/src/events.rs`
- `crates/loomward-engine/src/jobs/mod.rs`
- `crates/loomward-engine/src/lib.rs`
- `crates/loomward-engine/src/scan/mod.rs`
- `crates/loomward-engine/src/scan/pipeline.rs`
- `crates/loomward-engine/src/scan/sink.rs`
- `crates/loomward-engine/src/scan/source.rs`
- `crates/loomward-engine/src/scan/tests.rs`
- `crates/loomward-engine/tests/smoke.rs`
- `crates/loomward-lab/Cargo.toml`
- `crates/loomward-lab/src/windows.rs`
- `crates/loomward-windows/Cargo.toml`
- `crates/loomward-windows/src/enumerate/mod.rs`
- `crates/loomward-windows/src/enumerate/native.rs`
- `crates/loomward-windows/src/enumerate/tests.rs`
- `docs/16-implementation-status.md`
- `evidence/v3/bench/scan-errata-mutations.json`
- `evidence/v3/bench/scan-gates.json`
- `evidence/v3/bench/scan-m.json`
- `evidence/v3/bench/scan-s.json`
- `evidence/v3/scan-l7.md`
- `handoff/CHECKPOINT.json`

Record layout source: [Microsoft FILE_ID_EXTD_DIR_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_extd_dir_info).
