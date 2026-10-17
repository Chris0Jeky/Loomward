# Scan wave 2 claim reproduction

Windows 11, KRASPYON, 2026-10-17. Base: `fe3176cfe0526899390181efd47f19e57e2ad3e6`,
branch `fix/muse-w2-scan-findings`. Synthetic disposable directories only; no personal scans.
The driver commits this uncommitted lane.

## Claims

1. **Not confirmed: late revocation publishes Complete.**
   `revocation_after_scan_finish_cannot_publish_complete` pauses after a successful
   `MemorySink::finish_run(..., true)` on both the native watched and unwatched paths.
   It revokes the root, observes `CancelRequested`, then releases the runner. Both jobs
   terminate with `Cancelled` coverage; the subsequent writer fence refuses another run.
   `jobs::Registry` checks the cancellation flag while holding the same records lock used
   by cancellation, before publishing the terminal job. No production change for this claim.
   Source inspection also found that the durable writer's `EndRun` calls `active_run`,
   which requires an active grant. The new test checks engine ordering and the memory
   sink; it does not inject this exact late-finish window into SQLite.

2. **Confirmed: an unwatched rescan retains the previous watch object.**
   `unwatched_rescan_removes_prior_watch` completes a watched native scan, makes the same
   sink refuse watch support, then completes an unwatched rescan of the same root.
   On the base implementation its final assertion fails: `scan_watches` still contains
   the old watch. Move removal and shutdown before the watched/unwatched branch, releasing
   the retained object and its dirty state for both paths. The reproduction confirms
   retained watch state, not a live native-handle leak: `run_watched` already stops native
   resources on exit. No cross-run dirty-epoch contamination was demonstrated.

3. **Not confirmed: ReFS 64-bit IDs deduplicate distinct directories.**
   `refs_legacy_directory_ids_are_not_deduplication_or_reopen_keys` passes with the base code.
   The native source calls `validate_identity(..., refs = true)`, marking a 64-bit ID
   `NonUnique` with or without a listed ID. Both pipeline key functions reject that basis.
   The test also verifies that a full 128-bit ReFS identity remains eligible without
   truncation. No production change. Real ReFS enumeration remains unverified.

All new tests are in `crates/loomward-engine/src/scan/w2_tests.rs`, registered as a
test-only child of `pipeline`; Windows-specific tests and imports are gated together.

## Verification

Commands run from the lane worktree, with `CARGO_BUILD_JOBS=4` and
`CARGO_TARGET_DIR=C:\lw\target\scan-w2`; full suites also use `RUST_TEST_THREADS=4`.
Every Cargo invocation includes `--manifest-path C:/lw/wt/scan-w2/Cargo.toml`.

- Baseline `cargo test -p loomward-engine w2_tests -- --nocapture`: **2 passed, 1 failed**,
  specifically the retained-watch assertion.
- After the lifecycle fix, the same focused command: **3 passed, 0 failed**.
- `cargo test -p loomward-engine`: **219 passed, 0 failed, 1 existing ignored**
  (206 unit, 10 integration, 3 doc tests). The four engine failures named in the task
  passed on this run.
- The first engine Clippy pass caught two unnecessary literal unwraps in the new test;
  those unwraps were removed before the remaining checks.
- Final `cargo clippy -p loomward-engine --all-targets -- -D warnings`: **PASS**.

- `cargo clippy --workspace --all-targets -- -D warnings`: **PASS**.
- `cargo test --workspace --no-fail-fast`: **1,024 passed, 0 failed, 1 existing ignored**,
  summed across 61 test harness summaries (including doc tests). All five host failures
  named in the task passed in this run. The service's
  `revocation_mid_scan_commits_cancels_and_fences_the_writer` and the catalogue's
  `refs_64_bit_fallback_cannot_reparent_or_bind_durable_references` also passed.
- `cargo fmt --all --check`: **PASS** after the test-only Clippy correction.
- `git -C C:/lw/wt/scan-w2 diff --check`: **PASS**.

Full check logs remain in the lane target directory as `w2-engine-test.log`,
`w2-engine-clippy-final.log`, `w2-workspace-clippy.log` and `w2-workspace-test.log`.

Noticed: Cargo resolved an already-declared `loomward-lab` `sha2` dependency missing
from the base lockfile. The generated lock-only addition was removed from this lane;
no dependency declaration changed.

## Boundaries

No commits, pushes, PRs, merge, release or new capability. `HUMAN_TODO.md` and the
driver-owned status/checkpoint files are unchanged. Linux execution, a physical ReFS
volume and the exact late-finish window through the durable service are not verified by
the new tests.
