# Jobs lifecycle #287 and duplicates follow-ups #299

## Changed

Prepared on `fix/jobs-lifecycle-287`, based on `b430cbff7bdda2749717cb60bfff1a320a5c686c`.
The patch is uncommitted and unpushed. Learning files were not touched. Tests use synthetic fixtures.

Clearing terminal evidence on revocation is intentional: the existing contract promises that a
revoked root does not continue supplying its report's relative names. Keeping job IDs, terminal
outcomes and timestamps preserves recovery without retaining those names. Clearing evidence
increments and publishes the revision once; repeating revocation is silent. Pending revocation
uses a private flag, so its permission error appears only in the terminal snapshot.

Reports are limited to 50 and 4 MiB of serialized report data, as well as the existing 600-second
window. Eviction removes evidence rather than removing the job's recovery/status record, and
publishes the changed revision. A worker waits on the same-root admission fence (the blocking
worker option explicitly allowed by #299); submit-time callers still receive immediate Busy.

### Every requested item

Test IDs below resolve to full test names and source locations in Verified. All Fixed rows have
an executed regression or mutation that fails when the stated behavior is removed.

| Item | Disposition and source | Test / mutation |
|---|---|---|
| #287 cancelled before start: no started_at or silent rev | **Fixed** - `crates/loomward-engine/src/jobs/mod.rs:283`; only an uncancelled job records/publishes Running | T1 / prestart |
| #287 spawn failure must preserve pending cancel | **Fixed** - `crates/loomward-engine/src/jobs/mod.rs:169`; pending cancellation wins, with no invented spawn error on ordinary cancellation | T2 / spawn-cancel-priority |
| #287 revision exhaustion must not panic under the mutex | **Fixed** - `crates/loomward-engine/src/jobs/mod.rs:426`; checked revision allocation returns Internal before visible mutation; scan progress propagates it at `crates/loomward-engine/src/scan/mod.rs:528` | T3 / revision-exhaustion |
| #299 non-terminal revocation is idempotent | **Fixed** - `crates/loomward-engine/src/jobs/mod.rs:369`; revocation stages refusal privately and leaves already-requested cancellation unchanged | T4 / revocation-idempotence |
| #299 permission error only with terminal state | **Fixed** - `crates/loomward-engine/src/jobs/mod.rs:320`; pending snapshots carry no error, late staging cannot restore evidence | T4 / revocation-terminal-error |
| #299 terminal report must stop serving revoked names | **Fixed** - `crates/loomward-engine/src/jobs/mod.rs:375`, `crates/loomward-engine/src/jobs/mod.rs:411`, `crates/loomward-engine/src/scan/mod.rs:631`; root revocation also finds terminal duplicate reports and clears them once | T4 / terminal-revocation-redaction |
| #299 report count/byte cap and maximum jobs.list response | **Fixed** - `crates/loomward-engine/src/jobs/mod.rs:21`, `crates/loomward-engine/src/jobs/mod.rs:142`; 50 reports and 4 MiB, newest first; status records remain recoverable | T6 / retention-count, retention-bytes |
| #299 one over-depth path must not fail the query | **Fixed** - `crates/loomward-catalog/src/duplicates.rs:75`; omit the path with path_depth, preserve valid pairs and partial coverage; `crates/loomward-engine/src/duplicates.rs:160` carries the refusal into stop | T7, T8 / path-depth, wire-path-reason |
| #299 oversized reader groups/links must yield bounded partial evidence | **Fixed** - `crates/loomward-engine/src/duplicates.rs:128`; cap group/link projection, preserve indices and recompute listed link count; partial/truncated with report_budget | T8 / wire-group-bounds, wire-link-bounds |
| #299 terminal cancellation must preserve an existing stop | **Fixed** - `crates/loomward-engine/src/jobs/mod.rs:326`; cancelled is inserted only for None, preserving byte/time/entry stops | T5 / cancel-reader-stop |
| #299 transient runtime admission contention | **Fixed** - `crates/loomward-engine/src/scan/mod.rs:213`, `crates/loomward-engine/src/duplicates.rs:55`, `crates/loomward-engine/src/scan/mod.rs:180`; workers wait, then recheck revocation and durable grant; request admission stays nonblocking | T9, T12 / worker-admission-wait, worker-runtime-call |
| #299 Busy must describe duplicates rather than scanning | **Fixed** - `crates/loomward-engine/src/jobs/mod.rs:123`; retains detail.job_id | T5 / duplicates-busy-message |
| #299 queued example claims started_at | **Fixed** - `contracts/v3/examples/commands/duplicates.inspect.result.json:10`; now null | T11 / queued-example |
| #299 terminal example finishes before creation | **Fixed** - `contracts/v3/examples/dtos/duplicates-result.json:13`; finishes after created_at/started_at | T11 / terminal-example |
| #299 mock must validate min_bytes | **Fixed** - `app/src/lib/transport/mock-scan.ts:114`; optional exact unsigned u64 string, no null/leading zeros/overflow; shares validation with max_bytes | T10 / mock-min-bytes |
| #299 comment: public inspect stop coverage | **Fixed (coverage)** - `crates/loomward-windows/src/duplicate_reader/tests.rs:876`; public inspect with a byte stop preserves the earlier verified pair and partial/stop/byte evidence; elevated fixture asserts refusal | T13 / public-reader-stop |
| #299 hydration repeatedly opens the root chain | **Declined** - `crates/loomward-engine/src/duplicates.rs:80`; keep fresh native bindings and the existing max_ms bound; root-chain reuse is a separate performance change. | Already tracked in #299 |
| #190 history: submission must publish Queued | **Already fixed** - `crates/loomward-engine/src/jobs/mod.rs:271`; the existing Queued-before-Running regression remains green | `submit_publishes_queued_before_running` at `crates/loomward-engine/src/jobs/mod.rs:752` |
| #190 history: spawn failure must publish terminal; comment asks for submit-path coverage | **Fixed coverage of existing transition** - `crates/loomward-engine/src/jobs/mod.rs:212`, `crates/loomward-engine/src/jobs/mod.rs:359`; injectable local spawn seam exercises submit's actual failure branch, replacing the old artificial mark_spawn_failed-on-a-running-job test | T2 / submit-spawn-terminal-call |
| #190 history: repeated cancellation must be silent; comment flags sleep margin | **Already fixed; test hardened** - `crates/loomward-engine/src/jobs/mod.rs:384`; test at `crates/loomward-engine/src/jobs/mod.rs:807` controls worker release and waits for completion, without sleep-based silence windows | T14 / idempotent-cancel-condition |
| #190 history: Both final scan progress | **Declined for this lane** - `crates/loomward-engine/src/jobs/mod.rs:62`; Both still has no wire strategy value, and remains an explicit unknown; adding that contract value is outside #287/#299. | Already tracked in #190 |
| #190 history: least-capable watched strategy across passes | **Declined for this lane** - `crates/loomward-engine/src/scan/watch.rs:504`; watch reconciliation is outside this jobs/duplicates slice. | Already tracked in #190 |

`contracts/v3/semantics.md:245` documents omission reasons, bounded report projection,
retention/eviction, terminal redaction, idempotent revocation, and preservation of earlier stops.
No file/process effect, native identity authorization, or personal content-read gate was added.

## Verified

Executed on Windows. Native tests run against disposable synthetic files and keep elevation refusal
assertions rather than treating elevated identity opens as available.

### Tests and mutation results

| ID | Test and source |
|---|---|
| T1 | `cancelled_before_start_has_no_start_time_or_unpublished_revision` - `crates/loomward-engine/src/jobs/followups.rs:55` |
| T2 | `submit_spawn_failure_publishes_terminal_and_preserves_pending_cancel` - `crates/loomward-engine/src/jobs/followups.rs:105` |
| T3 | `exhausted_revision_returns_internal_without_changing_or_poisoning_registry` - `crates/loomward-engine/src/jobs/followups.rs:152` |
| T4 | `duplicate_revocation_is_silent_until_terminal_and_clears_retained_names` - `crates/loomward-engine/src/jobs/followups.rs:198`; includes ordinary-cancel escalation, repeated revocation, late staging and already-terminal evidence |
| T5 | `cancel_preserves_a_reader_stop_and_duplicates_busy_names_the_work` - `crates/loomward-engine/src/jobs/followups.rs:279`; None plus byte/time/entry stops |
| T6 | `terminal_report_retention_caps_count_bytes_and_jobs_list_transport_size` - `crates/loomward-engine/src/jobs/followups.rs:327`; 51 sequential jobs, 50 retained reports, recoverable oldest status, and an oversized valid report to exercise the byte cap |
| T7 | `duplicate_candidates_skip_one_over_depth_path_without_losing_valid_pairs` - `crates/loomward-catalog/tests/queries.rs:128` |
| T8 | `duplicate_report_bounds_reader_groups_and_links_and_carries_path_refusals` - `crates/loomward-engine/src/duplicates.rs:172` |
| T9 | `duplicate_worker_admission_waits_for_same_root_gate` - `crates/loomward-engine/src/scan/tests.rs:297`; tests both admission and refusal after the fence opens |
| T10 | `validates optional min_bytes as an exact unsigned byte count` - `app/tests/unit/duplicates-store.test.ts:14`; malformed values and positive absent/zero/u64-max controls |
| T11 | `test_duplicate_examples_validate_requests_and_terminal_evidence` - `tests/test_v3_contracts.py:58`; both chronologies and schema checks |
| T12 | `duplicate_job_cancel_and_root_revocation_reach_the_worker_and_terminal_state` - `crates/loomward-engine/src/duplicates.rs:241`; extended to hold the actual runtime gate, with an elevation refusal assertion |
| T13 | `native_public_inspect_stop_keeps_verified_group_and_marks_partial` - `crates/loomward-windows/src/duplicate_reader/tests.rs:876`; public inspect, 48-byte budget, earlier verified pair retained |
| T14 | `cancel_is_idempotent_with_no_duplicate_event` - `crates/loomward-engine/src/jobs/mod.rs:807`; deterministic pending/terminal cancellation assertions |

**22/22 mutations detected.** Each mutation temporarily removed one fix or stop behavior, ran the
named test and produced an expected assertion/test failure, not a compile/setup failure. Source
bytes were restored in finally blocks after every mutation. The complete restored suites then
passed. The mutation names in the item table cover all 22 checks; T2, T4, T5, T6 and T8 each have
separate mutations for their distinct assertions. The public reader test pins an existing stop
behavior by removing stop propagation and watching that new test fail.

The transport test serializes a valid ResponseEnvelope, rather than estimating path sizes:
**50 retained reports produce a 3,022,015-byte jobs.list envelope**, below the HTTP client's 8 MiB
cap (`app/src/lib/transport/http.ts:17`). Tauri returns the same envelope without a separate response
cap (`native/src/lib.rs:117`, `app/src/lib/transport/tauri.ts:30`). The byte-cap phase separately
asserts retained report data <= 4 MiB and a jobs.list envelope < 8 MiB.

### Gate outputs

| Command | Result |
|---|---|
| `cargo fmt --all --check` | Exit 0 |
| `cargo test --workspace` | Exit 0; 711 passed, 0 failed, 0 ignored, including doc tests |
| `cargo test -p loomward-windows --all-features` | Exit 0; 93 passed, 0 failed, 0 ignored |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Exit 0 |
| `py -3 scripts/verify.py` | Exit 0; 223 Python tests with 1 existing symlink-privilege skip; 2 JS syntax checks, 9 JS boundary assertions, 80 cross-language fixtures; Rust format, 711 tests and clippy pass |
| `npm.cmd --prefix app run check` | Exit 0; 356 files, 0 errors, 0 warnings; visualization check passes |
| `npm.cmd --prefix app run test` | Exit 0; 27 files, 297 tests passed |
| `git diff --check` | Exit 0 |

Raw gate and mutation logs are local-only under the gitignored `.loomward/jobs-followups/` directory;
`gates.json` and `mutations.json` retain the exact commands and exits. They are not publication data.

Recommended commit message: `Fix job lifecycle and bound duplicate evidence`.
The driver commits the completed patch; no commit, push or PR was performed here.

## NOT verified

Hosted elevated Windows CI was not run. The elevated refusal branches remain asserted in the
native fixtures but were not exercised by this run. No Tauri native build/window/IPC round trip,
app build or browser e2e was run; this lane verifies report serialization and the existing
transport caps, not a native-shell transport run. Personal content inspection remains refused.

## Residual risk

Retention pressure can remove duplicate evidence before 600 seconds; the documented rev/event
makes that removal observable and the job ID/outcome remain recoverable. Revocation redacts
current job snapshots; it cannot retract copies clients already received. The historical Both
progress and watched-strategy items remain tracked in #190. No long benchmark or root-chain
optimization was added.
