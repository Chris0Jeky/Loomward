# Catalogue writer follow-ups (#309)

Windows 11, KRASPYON, 2026-10-17. Lane `writer-309`, base
`f308beda9ac37c79d08e4146f47c4dddfa93d922` (also `origin/main` and the live
remote `main`, checked with `git ls-remote origin refs/heads/main`).
Synthetic SQLite fixtures only. Changes are left uncommitted for the driver.

## Reconciliation with main

| Finding | Current evidence / disposition |
| --- | --- |
| Repeated chain walks | Already fixed by `640e127` / #315: `chain_length` reuses per-command proven lengths. `chain_lengths_reuse_proven_seeds_but_count_the_unproven_prefix` pins reuse and the bound. The historical synthetic benchmark is in `crates/loomward-catalog/HANDOFF.md`; no new throughput claim on this shared host. |
| Reparent fails to bound descendants | Already fixed by `640e127` / #315; `reparent_bounds_the_whole_subtree_and_keeps_the_batch_and_end_run_usable` pins rejection, exact-bound acceptance and usable EndRun. |
| Rolled-back parent cache contaminates a later chain check | Already covered by `640e127` / #315: `rejected_reparent_clears_parent_cache_before_the_next_chain_check`. |
| Stale stored depth rejects a shallow reparent | Already fixed by `640e127` / #315; `a_shallower_intra_batch_reparent_does_not_trust_stored_depth`. |
| Move order changes acceptance of a flattening listing | Already fixed by `d49299f` / #340; heights are checked after the complete listing applies. `flattening_ancestor_and_descendant_is_independent_of_listing_order` checks both orders. |
| Batch rollback loses successful callers' errors | Already fixed by `d49299f` / #340: `aborted_ok_batch_reports_the_failure_to_every_caller` checks flush and commit failure. |
| Post-commit reference confirmation changes committed replies | Already fixed by `d49299f` / #340: committed replies survive; `confirmation_failure_closes_writer_and_recovery_retries` checks writer closure and reopen recovery. |
| Reparent suppresses an unrelated stale ticket | Already fixed by `80a2482` / #340: `RepairSink` propagates errors; `repair_reparent_does_not_hide_an_obsolete_run_or_unrelated_ticket`. |
| Refused begin lacks finish | Not a defect: `80a2482` / #340 documents that a failed begin accepts no run and owns partial-begin cleanup. `refused_scan_begin_does_not_finish_the_existing_run` protects the other active run. |
| Targeted repair with failed reopen reports complete (#303) | Already fixed by `f8c359a` / #308: `settled` requires `next.failed == 0`; `targeted_repair_with_failed_reopen_never_completes` also asserts no absence sweep. |
| SQLite auto-rollback behavior | Existing executed regression `a_savepoint_after_auto_rollback_would_start_and_commit_a_fresh_transaction` pins the fresh-transaction hazard. The writer checks `is_autocommit` before flush and rejects a failed commit. |
| TEMP scratch-table rollback | The new `publication_temp_tables_roll_back_creation_and_rows_with_the_savepoint` passes against bundled rusqlite/SQLite for all four scratch schemas: both table creation and row changes roll back. No lifecycle change is needed. |

## Remaining depth refusal

The subtree-height query still included descendants behind `absent_pending` rows.
Those branches are hidden from `visible_dir` and rollup, and may remain after a
refresh or beneath an excluded directory. Their retained metadata must neither
reject a shallow visible move nor be deleted to allow it.

The height walk now stops before hidden rows. Restoring an `absent_pending`
directory also validates the subtree it would reveal, including when its parent
identity has not changed. Otherwise a later listing could expose the retained,
now-overdeep chain without entering the reparent guard. Validation stays inside
the command savepoint and uses the final listing topology.

The `PublicationWindow` comments now include StageChunk-led publication batches,
matching the existing staging gate introduced in #395.

## Left open

- OutsideScope traversal failures: still conservative. Entries arrive before
  ListingDone, and the pipeline records the traversal failure in `report.failed`.
  Both `RepairSink.valid` and the `next.failed == 0` settling gate currently refuse
  completion. Merely buffering/suppressing the former would not fix settling.
  A separate change must define which failures can be excused while preserving
  the failed-reopen regression and honest counts; this lane does not change scan
  scope or coverage policy.
- No new scale benchmark on the shared host. Accepted reparent validation still
  walks the visible subtree.

## Verification

The new regression tests and static gates pass. Every default-feature catalogue
test passed across the runs and isolated retries below, but no single full-suite
invocation was clean. The three observed failures were unchanged watchdog or
deadline checks; their isolated passes are recorded rather than labelled a clean
suite. Their underlying timing sensitivity was not repaired in this lane.

All Cargo commands use `CARGO_BUILD_JOBS=4` and the supplied shared
`CARGO_TARGET_DIR=C:\lw\target\pool-c`. Absolute manifest path:
`C:/lw/wt/writer-309/Cargo.toml`.

- RED, original production source: `cargo test --manifest-path
  C:/lw/wt/writer-309/Cargo.toml -p loomward-catalog hidden_ -- --nocapture`:
  **0 passed, 2 failed** (72 filtered out). The shallow move returned
  `Invalid("tree depth limit")`; the overdeep restoration did not return that
  error. Both are assertion failures after successful compilation.
- GREEN, same command after the fix: **4 passed, 0 failed**, including both new
  regressions and the existing hidden-search and hidden-multilink regressions.
- `cargo clippy --manifest-path C:/lw/wt/writer-309/Cargo.toml
  -p loomward-catalog --all-targets -- -D warnings`: PASS.
- `cargo fmt --manifest-path C:/lw/wt/writer-309/Cargo.toml --all --check`: PASS.
- `git -C C:/lw/wt/writer-309 diff --check`: PASS.
- Initial full crate run, `cargo test --manifest-path
  C:/lw/wt/writer-309/Cargo.toml -p loomward-catalog -- --test-threads=3`:
  aborted by the existing five-minute watchdog in
  `the_deepest_build_the_writer_accepts_is_discarded_or_swapped_in_one_cascade`.
  The watchdog explicitly invoked process abort (Windows exit `0xc0000409`);
  there was no assertion failure reported. This is not a passing suite.
  Bulk/incremental parity and catalogue disk-full recovery completed successfully
  beforehand. The timed-out test and watchdog are unchanged; that test only
  creates new directory observations, with no moved/restored branch to validate.
- Isolated rerun: `cargo test --manifest-path C:/lw/wt/writer-309/Cargo.toml
  -p loomward-catalog --lib
  writer::bulk_tests::the_deepest_build_the_writer_accepts_is_discarded_or_swapped_in_one_cascade
  -- --exact --test-threads=1 --nocapture`: **1 passed, 0 failed**, 207.95 seconds,
  with the original five-minute watchdog unchanged. The parallel suite timed out;
  the isolated run passed. A quiet-host baseline was not measured.
- Serial full crate retry (`--test-threads=1`): **74 unit tests passed, 1 failed**
  in 624.24 seconds. The deepest-tree test passed in this run, as did every new
  test and all earlier #309 catalogue regressions. The failure was the unchanged
  `end_run_checkpoint_does_not_wait_for_a_reader_and_restores_write_timeout`:
  its one-second receipt deadline elapsed. Its fixture lists one file and no
  directories, so the changed moved/restored-directory path is not entered.
  Cargo stopped before integration tests; this is not a passing full suite.
- Isolated checkpoint rerun: `cargo test --manifest-path
  C:/lw/wt/writer-309/Cargo.toml -p loomward-catalog --lib
  writer::tests::end_run_checkpoint_does_not_wait_for_a_reader_and_restores_write_timeout
  -- --exact --test-threads=1 --nocapture`: **1 passed, 0 failed** in 12.62
  seconds, with the one-second receipt deadline unchanged.
- All 16 default-feature integration targets, serial and with `--no-fail-fast`:
  **191 passed, 1 failed, 0 ignored**. All 35 writer integration tests passed.
  The failure was the unchanged
  `scrub_retries_progress_between_continuous_commands_and_keep_reader_snapshot`
  in `tests/forget.rs`: its three-second scrub-progress deadline elapsed.
  Exact integration command:

```powershell
cargo test --manifest-path C:/lw/wt/writer-309/Cargo.toml -p loomward-catalog --test catalog_findings --test developer_clutter --test forget --test leading_space_windows --test model_contract --test open --test open_guards --test queries --test query_bounds --test query_mapping --test reader_guards --test search --test search_rank --test sqlite_path_regression --test writer --test zero_port_components --no-fail-fast -- --test-threads=1
```

- `cargo test --manifest-path C:/lw/wt/writer-309/Cargo.toml
  -p loomward-catalog --doc`: PASS, **0 doc tests**.
- Isolated scrub rerun: `cargo test --manifest-path
  C:/lw/wt/writer-309/Cargo.toml -p loomward-catalog --test forget
  scrub_retries_progress_between_continuous_commands_and_keep_reader_snapshot
  -- --exact --test-threads=1 --nocapture`: **1 passed, 0 failed** in 12.10
  seconds, with the three-second progress deadline unchanged.
- No tests or timeouts have been disabled, skipped or relaxed. There is no single
  green full-suite run; isolated passes do not erase the earlier failed attempts.

No commits, pushes, PRs, personal scans, hosted CI, app or native-shell checks.
Unchanged engine/watch regressions were inspected, not rerun. Workspace-wide
gates belong to the driver. The feature-gated `crash_sweep` target was not run
(these were default-feature commands). `HUMAN_TODO.md` is unchanged; this
patch requires no owner grant or product-gate opening.
