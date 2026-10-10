# Personal-scan gate #184 close-out audit

Audit input: issue #184, body and all three follow-up comments.
The audited main and branch base are `55c4df76b3d7f8246a44c46002f20d627061e5e2`.
References labelled **main** below refer to that immutable source, not a moving branch.
References labelled **this branch** refer to the close-out change over that base, measured before it was committed.
Tests ran on Windows with synthetic disposable fixtures. No personal root was scanned.

`personal_scan_gated_184` remains unchanged in `crates/loomward-service/src/handlers.rs:154`.
This audit prepares the gate for the driver's separate opening decision; it does not grant or
enable personal scanning.

## Every issue item

Test names are exact Rust names without their module prefix, except where a command is named.
All listed Rust tests are covered by the workspace or all-features proving runs below.

| Item | Status | PR and source evidence | Test that pins the behavior |
| --- | --- | --- | --- |
| Original MEDIUM-2: persistent share-mode pins | **Fixed**, #202; missing UI disclosure completed in this branch | Main `crates/loomward-windows/src/enumerate/native.rs:495` opens watch pins with delete sharing. Main `crates/loomward-engine/src/scan/watch.rs:371` installs `StopOnExit`, joining native/dispatcher threads before terminal job publication; `:238` stops explicitly on revocation/teardown. Watches are scan-scoped, with a full baseline for every explicit refresh. This branch `app/src/views/health/View.svelte:249` discloses scan-only watching, possible parent-rename blocking during the scan, and unknown changes afterwards. | `completed_scan_releases_pins_and_refreshes_full_baseline` performs actual parent renames, sibling deletion and root removal after terminal completion, and proves idle changes stay unknown until refresh; `complete_partial_cancelled_and_failed_scans_release_watch`; `engine_teardown_stops_watch_with_an_outstanding_runner_reference`; `armed_watch_allows_root_delete_and_fails_closed`. UI copy is checked by app compilation and the existing health browser flow, without a copy-only regression test. |
| Original MEDIUM-3: attribute changes not watched | **Fixed**, #202 | Main `crates/loomward-windows/src/watch.rs:88` includes `FILE_NOTIFY_CHANGE_ATTRIBUTES` and `FILE_NOTIFY_CHANGE_SECURITY`. Main `crates/loomward-engine/src/scan/watch.rs:966` checks a real hidden-attribute change while armed. The original request for an idle hint is superseded by scan-scoped watching: there is deliberately no idle watch. | `filter_covers_attributes_and_security`; `native_attribute_change_during_armed_scan_hints_parent`; `armed_before_scan_burst_reconciles_names_moves_and_sums_like_a_fresh_scan`. Native ACL mutation and cloud hydration are not claimed. |
| Original MEDIUM-1: MemorySink absence by name rather than identity | **Fixed**, #202; durable integration #225 | Main `crates/loomward-engine/src/scan/sink.rs:503` builds presence with eligible native IDs and `:535` marks absence by that identity; names are a fallback only for identity-less observations. Main `crates/loomward-engine/src/scan/pipeline.rs:379` preserves a post-open child ID when the listing lacks one. Main `crates/loomward-service/src/sink.rs:299` resolves durable children by native identity first. | `same_name_new_directory_identity_matches_fresh_scan`; `post_open_and_name_only_directory_presence_matches_fresh_scan`; durable `absence_is_decided_by_identity_when_a_directory_is_replaced_at_its_name`. |
| Original LOW: global scan_watches lock held across probes | **Fixed**, #202 | Main `crates/loomward-engine/src/scan/mod.rs:463` clones the old watch under the map lock, then stops/starts outside it; `:554` removes before stop/join. Production no longer probes an idle watch for reuse. | `watcher_start_probe_does_not_hold_global_watch_map`. Its assertion is intentionally limited to the synchronous startup caller; it does not claim a simultaneous health/revoke stress test. |
| Original LOW: every start failure hidden as PermissionDenied | **Fixed**, #202 | Main `crates/loomward-engine/src/scan/watch.rs:260` maps native I/O codes to distinct `watch_io_error_<code>` failures, unsupported to capability-unavailable, and refusals to their own reason. | `watcher_start_error_preserves_native_cause`, including an actual missing-root native open. |
| Original LOW: checkpoint starvation under continuous churn | **Still open on audited main; fixed in this branch** | Main `crates/loomward-windows/src/watch.rs:263` services requests only after a zero-time wait times out. This branch `:115` polls the request channel before readiness; `:265` uses that poll for every completion, including immediate `ERROR_NOTIFY_ENUM_DIR`. Already-completed hints still go through the callback after rearming and before acknowledgement. | New `continuous_completions_do_not_starve_checkpoint`: an always-ready fake completion source receives consecutive checkpoints, validates the grant, and delivers before ack; a quiet-source control acknowledges immediately. Red-before-fix and removal/restoration proof below. Retained `completion_between_poll_and_request_is_delivered_before_ack` and both native suites cover ordering/integration. |
| Original LOW: revoked root can recreate a watch | **Fixed**, #212; service fencing covered by #227, cancel ordering by #233 | Main `crates/loomward-engine/src/scan/mod.rs:363` reserves a per-root gate through check/start/submission; `:543` fences that root before cancellation and watch removal. Main `crates/loomward-service/src/handlers.rs:390` commits durable revocation before engine cancellation and writer fencing. | `revoked_root_cannot_recreate_watch_from_stale_grant`; `revocation_during_start_cancels_the_reserved_watch`; `revocation_mid_scan_commits_cancels_and_fences_the_writer`; `root_cancellation_requests_job_before_stopping_watch`. |
| Original LOW: missing state_dir filter in Dirty::record | **No longer applicable to currently admitted product roots** | Main `crates/loomward-service/src/paths.rs:75` refuses a root both inside and containing state, before a picker grant (`crates/loomward-service/src/handlers.rs:486`) or lab registration (`crates/loomward-service/src/lab.rs:49`). State therefore cannot generate a root-relative hint in an admitted product watch. Main `crates/loomward-engine/src/scan/pipeline.rs:803` additionally excludes the state directory by opened identity. `Dirty::record` still has no path filter; this is not a claim that it does. Any future overlapping-root admission must reopen this requirement. | `paths::tests::refusals_by_rule` proves both overlap directions plus an accepted disjoint root; `native_state_root_is_excluded_by_handle_identity` proves traversal exclusion independently. No redundant watcher path filter added. |
| Round-2 MEDIUM: targeted pass clears a partial full pass through settled = !full || next.complete | **Fixed**, #202 | Main `crates/loomward-engine/src/scan/watch.rs:469` selects FullRoot when `!settled`, regardless of an unrelated targeted epoch. `:501` remains safe because a targeted pass is selected only after full coverage settled. No relist cap was raised. | `unsettled_full_pass_with_targeted_hint_never_claims_complete`; `unsettled_full_pass_with_clean_watcher_repeats_full_before_complete`. |
| Round-2 LOW: missing deterministic tests for clean/targeted epochs after an unsettled pass | **Fixed**, #202 | Main `crates/loomward-engine/src/scan/watch.rs:669` supplies the MovingTree-backed source fixture; `:699` injects controlled movement and targeted hints instead of relying on a native burst race. | Both `unsettled_full_pass_*` tests above: successful settling matches fresh-scan totals; continuously moving coverage stays partial. |
| Codex P1: checkpoint acknowledgement before callback delivery | **Fixed**, #202; preserved by this branch | Main `crates/loomward-windows/src/watch.rs:95` holds replies for ready completions; `:109` delivers the callback before replying. The main `:267` readiness recheck is preserved as a readiness observation after request receipt in this branch. | `completion_between_poll_and_request_is_delivered_before_ack`; new `continuous_completions_do_not_starve_checkpoint` also rejects early acknowledgement. |
| Codex P2: excluded directories stay in MemorySink presence; durable equivalent | **Fixed**, #202, #225 and #252 | Main `crates/loomward-engine/src/scan/sink.rs:499` omits excluded/refused directory entries from aggregate presence. The durable catalogue intentionally retains a visible excluded row, marks its contents unknown and counts no stale sums: `crates/loomward-catalog/src/writer.rs:1302`, `:1492`, `:1536`. `:894` shields unseen descendants/references from retirement; retaining them is not authoritative presence. | `directory_becoming_reparse_or_offline_matches_fresh_scan`; `refused_child_is_not_directory_presence`; `a_junction_replacing_a_directory_stays_visible_as_excluded`; `a_directory_excluded_for_one_scan_keeps_its_references_and_memberships`; `a_completed_traversal_never_retires_the_subtree_of_an_excluded_directory`; `breakdown_and_unique_totals_at_an_excluded_directory_count_nothing`. |
| #202-review MEDIUM-3: concurrent start check and submit are not atomic | **Fixed**, #212 | Main `crates/loomward-engine/src/scan/mod.rs:363` holds the same per-root reservation before the `root_jobs` check and through `jobs.submit`; a losing start returns Busy without touching the winning watch. | `concurrent_scan_start_reserves_root_before_touching_watch`; `revocation_during_start_cancels_the_reserved_watch`. |
| #202-review LOW: probe assertion also runs on job/dispatcher threads | **Fixed**, #212 | Main `crates/loomward-engine/src/scan/watch.rs:1046` records the startup caller's thread ID and `:1048` guards the assertion with it. | `watcher_start_probe_does_not_hold_global_watch_map`; exercised again in every loaded engine-suite run. |
| #202-review LOW: stale known-failing wording in evidence Part 1 | **Fixed**, #202; adjacent pointer clarified in this branch | Main `evidence/v3/l16-change-tracking.md:138` already marks Part 1 historical and superseded by Part 2. This branch `:257` repeats that qualification directly beside the former known-failing statement and uses past tense. Historical failed measurements remain intact. | `git diff --check`; checked the Part 2 lifecycle result against `completed_scan_releases_pins_and_refreshes_full_baseline`. No copy-only test. |

## Changed and mutation proof

The only runtime change is checkpoint scheduling. Native completion, failure, no-recall,
identity validation, pending-I/O cancellation/drain and scan limits retain their existing paths.
The fake-source poll seam is used directly by the native read loop, including overflow completions.
The UI adds the disclosure required by MEDIUM-2; the evidence edit qualifies historical results.

New test: `watch::native::tests::continuous_completions_do_not_starve_checkpoint`.
It first failed with the extracted original readiness-first algorithm at the runtime assertion
`a ready completion must not bypass the request` (exit 101), then passed with request-first
polling (exit 0). Removing the fix reproduced the same assertion failure (101); restoring the
source bytes passed again (0). No compile error, skipped test or enlarged timeout is counted
as mutation evidence. There are no other new tests.

## Proving gates

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | PASS, exit 0 |
| `cargo test --workspace` | PASS, exit 0; 586 test cases passed, one existing ignored test |
| `cargo test -p loomward-windows -p loomward-engine --all-features` | PASS on exact-command retry, exit 0; 209 test cases passed, none ignored |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, exit 0 |
| `git diff --check` | PASS, exit 0; checked again after writing this receipt |
| `npm.cmd --prefix app run check` | PASS, exit 0 |
| `npm.cmd --prefix app run test` | PASS, exit 0; 255 tests across 26 files |
| `npm.cmd --prefix app run build` | PASS, exit 0 |
| `py -3 scripts/test_app.py` with process-local `PYTHONIOENCODING=utf-8` | PASS, exit 0; all browser checks, including the health flow |

The first all-features run failed in the unchanged
`duplicate_reader::tests::native::native_stop_keeps_earlier_verified_group_and_later_input_unknown`
at `crates/loomward-windows/src/duplicate_reader/tests.rs:823` (expected the first group, got none).
Its entire source is identical to audited main. It gives all native inspection 100 ms and
intentionally sleeps 110 ms in the later-group deadline leg. The focused unchanged test and
the exact full-command retry passed. Early deadline expiry is a plausible explanation;
the original run did not record the report's stop reason, so the precise cause is unconfirmed.
This failure is retained as evidence, not dismissed as flaky or repaired outside this lane.

The workspace's existing ignored test is
`verbatim_device_namespace_state_paths_are_refused`, tracked by #246 for the state-path device
namespace guard. It was already ignored on audited main; this lane neither skipped nor
modified it. Its asserted property remains unverified by that test.

The first browser run exceeded the unchanged Atlas 50 ms drawing budget (59.0 ms).
Its error-reporting wrapper then encountered a console-encoding error. The UTF-8 rerun passed
the same browser assertions and performance threshold. Neither assertion nor threshold changed.

## Loaded watch-native repetitions

Final Rust source was built with `--all-features --lib --no-run`. Its two test binaries ran
these selectors in each of 60 rounds, with `--nocapture` and no ignored/disabled tests:

- `scan::watch::native_tests::` in the engine binary: **60/60 suite passes, 900 case passes**
  (15 cases per round).
- `watch::native::tests::` in the Windows binary: **60/60 suite passes, 420 case passes**
  (7 cases per round).

Aggregate: **120 suite invocations, 1,320 case passes, zero failures**. Native fixtures took
their execution paths, not elevated-refusal paths; root deletion, attributes and filesystem
renames were actually exercised. The new fake-completion test also ran in every Windows suite.

The #201-style harness used three busy-loop Python processes and one 64 KiB create/rename/delete
loop across 32 rotating synthetic names. Harness, test and load processes were restricted to
at most four available logical CPUs, leaving the rest for other lanes. Disk churn touched only the
harness's owned temporary directory; native tests used their own disposable fixtures. All four
load children remained live for the batch, performed work, exited with code 0, and the owned
load directory was verified absent after cleanup. This is bounded stress evidence, not a scale
benchmark or a field failure-rate estimate.

After load cleanup, the final UI copy passed `check` (exit 0), `test` (255 cases, exit 0),
`build` (exit 0), and the complete `scripts/test_app.py` browser run (exit 0, `ALL PASS`, using
process-local UTF-8 output). The final disclosure was also verified present in both production
and browser-fixture build bundles. No final-copy gate was skipped or weakened.

## NOT verified and residual risk

No personal-root scan or enablement, hosted CI, Linux execution, elevated native execution,
cloud-placeholder hydration, native ACL transition, restart/reboot or sustained scale test was
performed. Synthetic native execution and durable catalogue/service regressions are separate
from those unmeasured capabilities.

An active scan's list-access handle can still block a parent rename. Terminal stop/join releases
it, and the UI now says so. Continuous churn can still exhaust finite reconciliation or queue
bounds and produce partial coverage; this fix prevents ready-completion checkpoint starvation,
not arbitrary scheduler or callback stalls. No guarantee of field flake absence follows from
a finite loaded batch.

Declined: redundant state-path filtering while product grant admission excludes overlap;
persistent watching, new capabilities and gate removal are outside this lane's scope.

Recommended driver commit messages (no commits made):

- `Prevent checkpoint starvation during continuous native completions`
- `Document scan-scoped watching and close out gate 184`

Raw gate, mutation, browser and loaded-loop logs and the load harness remain gitignored under
`.loomward/gate184-proof/`; they are local diagnostics, not public artifacts.
