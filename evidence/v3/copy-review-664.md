# Copy review fix round 1, PR #664

Windows 11 Home, KRASPYON, non-elevated token, 2026-10-18. Base:
`4718e1cb96970f3d5a6d6ae1c5402f5e914c68ca`, branch `fix/636-copy-lows`.
This receipt describes the uncommitted working-tree fix; it is not hosted CI proof.
Copy remains lab-only and C8 remains closed. `HUMAN_TODO.md` is unchanged.

## Changed

1. `copy.status` re-observes non-terminal operations without a resident worker. Failed observations
   and pending post-publish confirmation report `needs_attention` without changing the journal.
   Accessible unjournaled names remain visible.
2. Cancel/abandon checks observation failure before confirming or withdrawing a pending publish;
   neither owner abandon nor revocation-cancel journals an unavailable published tree as withdrawn.
3. The per-step durable-stop check uses `Store::state_epoch`: compare the cached replay with the
   independently checked database head/tail in a deferred transaction. Read/replay history only on
   a cold cache or changed head, without cloning the item vector on a cache hit.
4. Recovery observes and validates the destination parent's recorded identity before inferring
   child damage. A missing or replacement parent is retryable, including after a missing-child read.
5. A fenced control no longer exempts an admitted step from an epoch mismatch.

## Regression evidence

The original code failed the two service restart-status tests, the cancel test, the missing-parent
test, and the admitted-step epoch test. The original replay-based state check failed with journal
body reads denied. Extending the parent test to an empty replacement directory also failed before
the parent identity check was added. All were assertion/runtime failures, not elevated-token skips.

| Finding | Regression test names (module prefixes omitted) |
| --- | --- |
| 1 | `restart_status_reports_unreadable_staged_copy_attention_without_journaling`, `restart_status_reports_unreadable_published_copy_attention_without_journaling`, `idle_status_keeps_unjournaled_names_during_observation_failure` |
| 2 | `cancel_unreadable_published_copy_does_not_withdraw_or_journal`, `revocation_cancel_unreadable_published_copy_does_not_withdraw_or_journal` |
| 3 | `state_epoch_cache_avoids_journal_body_reads`, `state_epoch_refreshes_external_fence_and_rejects_head_damage` |
| 4 | `missing_destination_parent_is_retryable_not_staging_damage` (Verified and AfterPublish, missing and replaced parent) |
| 5 | `admitted_step_rechecks_epoch_even_when_control_is_fenced` |

The access-denied tests reopen while denied, check unchanged journal rows, restore read access and
reopen again to prove retry. The unjournaled-name test retains the accessible name across repeated
opens while another unjournaled item is unreadable. Existing live-worker and detached-worker tests
remain in the applicable suites. Native fixtures explicitly return early when `running_elevated()`.

## Verification

All Cargo commands use `CARGO_BUILD_JOBS=4` and the lane's own `CARGO_TARGET_DIR`.
Commands below run from the worktree root (executed with its absolute `--manifest-path`).
Raw logs are gitignored under `.loomward/review-664/`.

| Command | Result |
| --- | --- |
| `cargo test -p loomward-windows --all-features` | PASS: 229 passed, 0 failed, 2 ignored; includes the final parent identity check |
| `cargo test -p loomward-service --lib copy` | PASS: 48 passed, 0 failed, 1 ignored; includes the final parent identity check |
| `cargo test -p loomward-operations` | PASS: 78 passed, 0 failed |
| `cargo test --workspace --no-fail-fast` | PASS: 1,644 passed, 0 failed, 4 ignored; final run after the parent identity check |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS after the final code change |
| `cargo fmt --all --check` | PASS |
| `git diff --check` | PASS |

## 20,000-file performance measurement

`durable_stop_20k_lab` creates 20,000 empty files, previews/approves a synthetic copy, then times
`exec::run` through `committed`. Source creation, preview/admission, compilation and fixture cleanup
are outside the executor timer. Both paired runs used `CARGO_PROFILE_TEST_OPT_LEVEL=1`, all Windows
features, four build jobs and the lane target directory. These are single runs on a shared host,
not controlled release-build measurements.

| Version | Executor time | Test result |
| --- | --- | --- |
| Base `4718e1c` plus the identical benchmark, in an ignored `git archive` snapshot | 1,029.609 s | 1 passed, 0 failed |
| Fixed working tree | 86.624 s | 1 passed, 0 failed |

The paired run was **11.89 times faster** (91.59% lower executor time). Exact invocation for each
workspace was `cargo test --manifest-path <workspace>/Cargo.toml -p loomward-windows --all-features durable_stop_20k_lab -- --ignored --nocapture`,
with `CARGO_PROFILE_TEST_OPT_LEVEL=1` set. Logs: `perf-before-opt1.log`, `perf-after-opt1.log`.

An initial unoptimized fixed run took 109.077 s. Its unoptimized baseline did not finish: it was
intentionally fenced after 1,886.1 s of process elapsed time when replaced by the optimized pair.
That interruption produced one expected benchmark assertion failure (`Copying` versus `Committed`;
1,893.42 s including shutdown). It is not a completed before measurement or a regression-suite
failure. All three earlier benchmark fixtures were independently matched to their 20,000 synthetic
names and cleaned after their processes exited. The final benchmark explicitly closes its Store
before closing its TempDir, and that cleanup passed. The temporary archived source snapshot was
removed; only gitignored raw logs and measurement helpers remain under `.loomward/review-664/`.

## NOT verified / residual risk

No hosted CI for this uncommitted diff, personal copying, or power-loss durability was claimed.
The default crash suite ran; the opt-in exhaustive `LOOMWARD_CRASH_SWEEP=full` was not requested.

The live PR also has an unresolved [cross-control check/effect race comment](https://github.com/Chris0Jeky/Loomward/pull/664#discussion_r4305629758).
The state check still releases the store lock before the OS call; an independent control/store can
fence in that gap. This pre-existing review finding is outside the five assigned fixes and remains
for the driver. No commit, push, merge, thread resolution, safety-gate or checkpoint edit was made.
