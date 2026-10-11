# L14: synthetic learning from engine to app

Measured 2026-10-11 on Windows, shared host. Branch `feat/learning-wiring`, base
`f42d0a15c45bbab9b0d6aa16f9cbb556d52edf3d`. No commit or push performed.
Every model number below is **synthetic smoke, not real-user accuracy**.

## Changed

1. `crates/loomward-engine/src/learning/mod.rs`: durable human feedback and original-result
   receipts; identical replay across restart, changed-payload refusal; next human revision and
   withdrawn-label retractions; resolved-reference filtering including published retirement
   intents; status, review queue, and a cancellable single-flight refit over `loomward-learn`.
   Queue cursors bind filters, `catalog_rev`, and `state_rev`. Refit snapshots human events only,
   checks cancellation and state revision before committing, and persists a candidate. Separate
   teacher rows and preferences never train this candidate. Scores remain relative scores, not
   probabilities; calibration and autonomy remain false.
2. `crates/loomward-service/src/learning.rs`: four handlers reuse service payload decoding,
   deadlines, mutation permits, FIFO admission, idempotency and session identity sealing.
   Durable replay precedes session-local node resolution. `handlers.rs` changes are dispatch
   lines only. `jobs.cancel` exposes the existing registry cancellation operation for refits;
   `scan.cancel` stays scan-only. Jobs retain the registry's sequence/revision lifecycle.
3. Personal sessions refuse feedback/refit with `capability_unavailable` and
   `detail.reason: learning_personal_gated`. Status exposes that gate without loading a stored
   candidate; queue is empty without inspecting one. A poisoned stored-candidate fixture pins
   this behavior. No personal metadata was collected or trained.
4. Protocol DTOs, command declarations, schema, semantics, examples and generated TypeScript
   agree on `gate`, `unresolved_human_events` and `jobs.cancel`. Example training counts exclude
   teacher labels. Contract tests cover the added command and examples.
5. `app/src/views/review/View.svelte`: real learning status, queue, human record/withdraw controls,
   ambiguous-outcome replay, refit job polling/cancellation, visible personal gate and unknowns.
   Failed refresh clears stale status/queue. `scripts/test_app.py --live-serve` records a synthetic
   durable node through HTTP, replays it, rejects changed content, observes status, refits through
   the UI, checks completion, records a second label by keyboard, and checks unavailable recovery
   and mobile overflow. The learning leg seeds disposable SQL fixtures: no scan or native root
   grant, no user paths or inventories.

## Verification

The following commands are the required gate. The final Rust test run uses process-local
`RUST_TEST_THREADS=1` on the shared host; the default parallel failures are recorded below.

| Command | Result |
|---|---|
| `cargo fmt --all --check` | PASS, through `verify.py` |
| `cargo test --workspace` | PASS, 698 tests, zero ignored; serialized |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `py -3 scripts/verify.py` | PASS: 222 Python tests run, one symlink-privilege skip; two JS syntax checks; nine JS boundary assertions; 80 cross-language fixtures; Rust fmt/tests/default clippy |
| `npm.cmd --prefix app run check` | PASS, zero errors/warnings |
| `npm.cmd --prefix app run test` | PASS, 293 tests in 26 files |
| `npm.cmd --prefix app run build` | PASS |
| `py -3 scripts/test_app.py` | PASS, 173 printed checks |
| `py -3 scripts/test_app.py --a11y` | PASS, 132 printed checks |
| `py -3 scripts/test_app.py --live-serve` | PASS, 36 printed checks |
| `git diff --check` | PASS |

Desktop and 390 px mobile learning screenshots were visually inspected. Live keyboard tests use
Enter to choose a review item and Space to record a human label. Automated accessibility checks
do not establish screen-reader acceptance.

### Negative tests and mutations

Each new Rust test and the new live learning leg was observed failing with its implementation
removed or a targeted defect inserted. Sources were restored after each experiment.

| Test | Deliberate defect / observed failure |
|---|---|
| `learning_durable_replay_conflict_and_retraction` | Accept changed replay content: expected `invalid_request` was missing. Select only non-retracted prior events: repeated retraction did not advance to revision 3. |
| `learning_personal_gate_and_unknown_status` | Bypass service/engine personal mutation gates: a forbidden refit was accepted. Remove personal queue guard: poisoned stored candidate was loaded and the queue failed. Before the status fix, the poisoned-candidate status assertion also failed. |
| `learning_queue_binds_both_revisions_and_filters` | Ignore bound revisions: stale cursor was accepted. |
| `learning_refit_is_human_only_candidate_and_retraction_withdraws` | Union teacher rows into training: training count became 2 instead of 1. |
| `learning_refuses_nondurable_invalid_and_unresolved_labels` | Include unresolved objects in training: expected zero fitted examples failed. |
| `learning_timed_out_feedback_precedes_refit_in_fifo` | Remove feedback FIFO ticket: following refit saw zero instead of one committed human label. |
| `learning_service_can_cancel_refit_jobs` | Route `jobs.cancel` through scan-only cancellation: refit cancellation was refused. |
| `learning_refits_are_exclusive_and_cancel_without_candidate` | Remove refit exclusion: second refit was accepted. Remove runner cancellation checks: cancelled refit persisted a candidate. |
| `live_learning` | Remove `Student.fit`: completion's one-example assertion failed. Preserve stale status on refresh error: the visible unknown-status assertion failed. |

13 targeted mutants killed, plus the original stub failures and the poisoned-candidate status
red test. Failures were behavioral assertions, not compilation errors. Learning fixtures never
open native identities, so these added tests need no elevated-host scan exception.

Two final parallel workspace runs failed existing native duplicate-reader timing assertions:
`native_stop_keeps_earlier_verified_group_and_later_input_unknown` returned no first verified
group; `native_deadline_during_read_is_partial` expired before reaching its intended
read hook. Both tests use a 100 ms budget. The former passed in isolation. The final discriminating
run sets process-local `RUST_TEST_THREADS=1` and passes, preserving every test and assertion.
Scheduling sensitivity is an inference from these controls, not a measured host diagnosis. No native test
or deadline was changed. Earlier integration failures were repaired at their cause: the generic
sequence test now uses distinct scan jobs rather than forbidden concurrent refits, command/example
counts include the new command, and live SQLite fixture handles close before Windows cleanup.

## NOT verified

Hosted elevated Windows CI; Tauri/WebView native UI; personal learning; real-user accuracy;
model promotion or calibration; manual screen-reader testing; learning beyond the declared
10,000-candidate / 10,000-raw-event budgets. No long benchmark, external request, model download,
package or release publication was performed.

## Residual risk and handoff

Synthetic candidates are advice only. Personal learning remains gated. Raw human history is
retained for durable replay; exceeding the refit event budget is a named resource refusal rather
than truncation. Queue cursors are session-local and bounded; eviction requires restarting paging.
Default parallel native timing checks remain sensitive to shared-host scheduling; their failures
are retained, not described as verified green.

Recommended commit: `Wire synthetic learning feedback, review, and refit jobs`.
Declined teacher operations, personal learning activation, model promotion, collections and
threads because they are explicitly outside L14.
