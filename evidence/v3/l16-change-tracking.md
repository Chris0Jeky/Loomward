# L16 change tracking receipt

Current result: the 2026-10-10 #184 Part 2 addendum below scopes watches to each scan and
passes the requested local gates. Earlier receipts retain their historical behavior and failures.

Measured 2026-10-10 on Windows, non-elevated native fixture runs, in
the `lw-l16` worktree, branch `feat/l16-change-tracking`, starting HEAD
`06750477fafb431afd0330fc721bb0bb2fb8c265`. The implementation was handed to the driver as a working-tree
change. The lane made no commit, push, release, elevation, journal mutation or real-root scan.

## Changed

- One recursive 64 KiB `ReadDirectoryChangesW` watcher per granted root; file-name,
  directory-name, size and last-write filters. The first request is armed before engine
  enumeration starts. The enumeration module's component-wise opens are reused, with
  asynchronous I/O only on the final component, no-recall flags, reparse/offline refusal,
  elevation refusal, expected identity checking and retained ancestor/root pins.
- Every pending request owns an RAII cancellation/completion guard. Cancellation waits for
  completion before releasing its buffer, OVERLAPPED, event, file or ancestor pins, including
  early exits. The next request is armed before delivering the preceding batch.
- Native callbacks feed a bounded 16-batch queue; a separate dispatcher does sink writes.
  Dirty parent directories coalesce into epochs, capped at 1,024 paths / 512 KiB of UTF-16
  names. Queue loss, malformed notifications, either overflow form, unresolved parents or
  unmatched renames require full-root reconciliation. Old/new rename parents both become dirty.
- Baseline and relists keep coverage repairing. An epoch is cleared only when a successful
  listing covers its captured value; notifications during that listing survive. Reconciliation
  uses targeted scopes and the existing N2 upserts-only behavior outside them. Reparenting
  discards obsolete in-flight tickets and forces a conservative full relist. A second drain
  catches changes during final publication before the job reports complete.
- MemorySink retains absence tombstones outside targeted scopes and repairs ancestor sums
  bottom-up after complete scoped reconciliation. Durable adapters must implement
  `watch_dirty` (default refuses watcher activation) and should implement
  `resolve_watch_directory` (unknown defaults to full relisting). Coverage still depends on
  each adapter's existing `finish_run` repair/publication contract.
- Watches survive between scans, mark idle changes stale, stop/join on grant revocation and
  engine teardown, and are re-established before a subsequent scan if coverage has failed.
  Four reconciliation passes (four since merging #178: a directory moved during a full pass is
  noticed one pass later) and the original aggregate entry budget bound a run; unsettled
  roots remain partial/repairing rather than looping indefinitely.

## Verified

All requested gates passed on the final implementation:

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | PASS |
| `cargo test --workspace` | 250 passed, 0 failed, 0 ignored |
| `cargo test -p loomward-windows --all-features -- --nocapture` | 34 passed, 0 failed, 0 ignored |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `git diff --check` | PASS |

The native tests use canonical OS-temp roots. On elevated hosts they assert native refusal
instead of claiming that their fixture watch ran. This host executed the native cases.
The burst oracle compares native directory IDs, exact UTF-16 names, file IDs, attributes,
logical sizes and final logical/allocated/count sums against an independent fresh scan.
It covers 200 create/rename operations with 100 deletes, an existing file deletion and size
change, both with and without a moved subtree. It also injects a file creation during final
publication and checks idle invalidation afterward.

Eight temporary rule-removal checks each produced an actual failing test (exit 101), not
an import/compilation failure; every temporary edit was restored:

| Rule removed | Discriminating test |
| --- | --- |
| ERROR_NOTIFY_ENUM_DIR classification | `both_overflows_discard_history` |
| Successful zero-byte overflow classification | `both_overflows_discard_history` |
| Early-exit I/O cancellation/completion | `pending_read_early_exit_drains_before_storage_is_reused` |
| Dirty epoch retention | `overflow_and_rename_parents_have_epochs` |
| Delivery of mid-scan hints | `armed_before_scan_burst_reconciles_names_moves_and_sums_like_a_fresh_scan` |
| Native stop on revocation | `revocation_stops_native_io_and_late_hints_cannot_reactivate_it` |
| Ancestor repair rollup | `repair_rollup_preserves_tombstones_outside_targeted_scope` |
| Post-publication drain | `armed_before_scan_burst_reconciles_names_moves_and_sums_like_a_fresh_scan` |

### Fixture measurements

Final all-features run, 64 KiB buffer:

- Continuously drained: 50 individual creates, no root invalidations; latency
  p50 / p95 / max **0.317 / 0.470 / 0.742 ms**. Latency spans the mutation syscall through
  callback delivery, not the kernel notification timestamp; one warm, shared-host trial.
- Deliberately stalled consumer: 10,000 creates, **root invalidation detected**, only **2 name
  hints before invalidation**, detected **1,769.935 ms from burst start**. This includes generation
  and the deliberate stall; it is neither the time the kernel buffer filled nor an overflow
  threshold estimate. The public hint API coalesces overflow forms; this trial did not log which
  native completion form occurred. Both forms are covered by classifier and engine injection tests.
- Burst reconciliation equals the fresh-scan oracle in both subtree variants. Revocation tests
  verify no later hints, released pins (the root can be renamed), and a stopped native channel.

## LW-007 hypothesis

The earlier [change-tracking research](../../docs/research/change-tracking.md) measured
non-elevated USN catch-up through directory handles on two NTFS volumes. Names were redacted,
128-bit V3 identity width matters, and records escaped the directory subtree; ACL-inaccessible
record visibility remains unknown. This is measured compatibility on that host, not universal
support or subtree confinement. No USN accelerator is implemented or enabled by L16. A future
explicit opt-in must filter catalogue IDs, retain journal/volume identity, stop at the captured
target USN, publish dirtiness before advancing its durable cursor, and fully reconcile on gaps
or unsupported formats. It must not create, resize or delete a journal.

The watcher completion/overflow rules follow Microsoft's
[ReadDirectoryChangesW contract](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-readdirectorychangesw).

## Files changed and recommended commits

1. `crates/loomward-windows/src/watch.rs`
2. `crates/loomward-windows/src/enumerate/mod.rs`
3. `crates/loomward-windows/src/enumerate/native.rs`
4. `crates/loomward-engine/src/scan/watch.rs`
5. `crates/loomward-engine/src/scan/mod.rs`
6. `crates/loomward-engine/src/scan/sink.rs`
7. `crates/loomward-engine/src/lib.rs`
8. `evidence/v3/l16-change-tracking.md` (this receipt)

Recommended commit messages, if the driver splits the native and engine seams:

- `Add guarded recursive Windows change watchers`
- `Reconcile watched scan roots with dirty epochs`

Raw local gate logs are gitignored under `.loomward/l16-proof/`; they are not publication
artifacts. `HUMAN_TODO.md` is unchanged; its remaining teacher decisions (q-5 and q-8) do not
block this fixture-only observer lane.

## NOT verified / residual risk

No durable catalogue/service integration, hosted CI, real personal roots, sustained scale,
reboot recovery experiment, USN production catch-up, or elevated execution was proved here.
Targeted L7 runs still traverse the root and publish only upserts outside their scope; this
preserves correctness but is not a measured targeted-I/O optimization. Each scan currently starts
with a conservative full baseline. Under continuous churn a bounded run stays partial and needs
another explicit refresh. Ancestor/root pins deny deletion sharing during the grant's watcher
lifetime; revocation/teardown releases them. Fixtures establish the local native boundary, not
instantaneous filesystem snapshots or durable catalogue crash consistency.


## 2026-10-10 - scan.start gates (#184)

The following Part 1 measurements are historical; Part 2 below supersedes the unresolved
ancestor-rename and stalled-consumer outcomes. The earlier failures remain evidence.

Measured on Windows, non-elevated, synthetic disposable fixtures, on a shared host with other
CPU/disk lanes running. Working-tree changes on `fix/l16-scan-start-gates`, base HEAD
`42a346a5c23b75fc0b263bd11fcfb7f5afd2588c`. No commit or push was made.
**This is a partial gate receipt: scan.start must remain gated.** Items 2 through 7 are implemented;
item 1 still has a failing ancestor-rename requirement. The failing test is retained and active.

### Changed / per-item status

| Item | Status and code | Regression tests |
| --- | --- | --- |
| 1: MEDIUM-2, user operations | PARTIAL. `crates/loomward-windows/src/enumerate/native.rs:383` adds delete sharing to every watch pin. `crates/loomward-windows/src/watch.rs:117` validates the granted path before checkpoint replies and every 250 ms while polling; path/identity loss or native I/O failure publishes RootDirty and sets the native failed flag (`:322`, `:353`). Engine checkpoint failure records RootDirty and failed, so the next scan rebuilds the watch. Root rename/deletion and sibling deletion succeed. Parent rename still fails with Win32 error 5 while the root list-access handle is retained. | `armed_watch_allows_root_delete_and_fails_closed` PASS; `renamed_root_fails_watch_and_replacement_refuses_granted_identity` PASS (includes next-scan `open_root` mismatch refusal); `armed_watch_allows_parent_rename_and_sibling_delete_and_fails_closed` FAIL at parent rename. |
| 2: MEDIUM-3, attributes | DONE. `crates/loomward-windows/src/watch.rs:84` includes ATTRIBUTES and SECURITY. | `native_attribute_change_after_complete_scan_hints_parent` runs `attrib +h` after complete coverage, checks the exact parent hint and stale sink coverage; `filter_covers_attributes_and_security` pins both flags. |
| 3: P1, checkpoint race | DONE. `crates/loomward-windows/src/watch.rs:92` supplies the fake-completion seam; `:267` rechecks the event after receiving the request. Signalled completion holds the reply until next-iteration rearm and callback delivery. | `completion_between_poll_and_request_is_delivered_before_ack` checks a completion arriving between initial poll and request, absence of early acknowledgement, callback-before-ack ordering and the quiet immediate-ack case. |
| 4: unsettled full coverage | DONE. `crates/loomward-engine/src/scan/watch.rs:435` selects FullRoot for a root dirty mark or `!settled`. `MAX_RELISTS` remains 4 (`:20`): the burst oracle passed all ten repeated runs, so no bound increase was needed. | `unsettled_full_pass_with_targeted_hint_never_claims_complete`; `armed_before_scan_burst_reconciles_names_moves_and_sums_like_a_fresh_scan`. |
| 5: deterministic move coverage | DONE. `crates/loomward-engine/src/scan/watch.rs:630` reuses the portable MovingTree source through a settling/continuously-moving fixture. The watch module is compiled for portable tests. A clean watcher repeats full traversal; a targeted hint cannot turn continuously unsettled full coverage into complete. Successful settling totals also match a fresh scan. | `unsettled_full_pass_with_clean_watcher_repeats_full_before_complete`; `unsettled_full_pass_with_targeted_hint_never_claims_complete`. |
| 6: identity presence and exclusions | DONE. `crates/loomward-engine/src/scan/sink.rs:482` excludes skipped/refused directories from presence; `:517` checks absence by eligible native ID, using name only for name-only identities. `crates/loomward-engine/src/scan/pipeline.rs:343` carries the opened child identity when a listing omits IDs, preserving identity-based presence through the native fallback. | `same_name_new_directory_identity_matches_fresh_scan`; `directory_becoming_reparse_or_offline_matches_fresh_scan` keeps the same ID when changing attributes; `post_open_and_name_only_directory_presence_matches_fresh_scan` covers PostOpen, no-ID and non-unique identity; `refused_child_is_not_directory_presence`. |
| 7: lock scope and real cause | DONE. `crates/loomward-engine/src/scan/mod.rs:201` clones the watch under the global map lock and runs health/start/stop probes outside it. Cancellation removes the watch before stop/join. `crates/loomward-engine/src/scan/watch.rs:236` preserves native I/O codes and distinct refusal/unsupported causes. | `watcher_start_probe_does_not_hold_global_watch_map`; `watcher_start_error_preserves_native_cause` also exercises an actual missing-root native open. |

### Measured MEDIUM-2 blocker

The requested delete-share change fixes root rename/deletion, but does not by itself fix ancestor
rename on this host. Discriminating unarmed controls measured:

- No retained handles: parent rename succeeds.
- Only retained metadata-access ancestor pins: parent rename succeeds.
- Retained root list-access handle, with delete sharing and NO_RECALL: parent rename fails
  with Win32 error 5, both with and without the ancestor pins.
- Adding backup intent did not change that outcome. ReOpenFile returned error 5.
  An absolute CreateFileW open with the same no-recall/reparse flags also blocked parent rename.
- Adding FILE_DIRECTORY_FILE while retaining NO_RECALL failed the native open with
  `STATUS_INVALID_PARAMETER` (`0xC000000D`, signed `-1073741811`). This matches the limited
  directory-option compatibility documented for [NtCreateFile](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntcreatefile).

All unsuccessful diagnostic changes were removed. No recall protection, reparse/offline refusal,
identity check or test assertion was disabled. The retained failing test performs sibling deletion
first (succeeds), then attempts the parent rename (fails). No notification/dirtiness claim is made
for that failed rename. A different native watch-open strategy still needs to pass this exact test
while preserving no-recall and component-wise refusal before scan.start can be enabled.

### Verified gates

`--no-fail-fast` was added to the two requested test commands to retain results for every target,
including doc tests, despite the known failing native gate. No tests were skipped.

| Command | Observed result |
| --- | --- |
| `cargo fmt --all --check` | PASS, exit 0 |
| `cargo test --workspace --no-fail-fast` | FAIL, exit 101: 343 passed, 2 failed, 0 ignored. Parent rename failed; the existing stalled-consumer fixture also exceeded its unchanged ten-second callback release wait. |
| `cargo test -p loomward-windows -p loomward-engine --all-features --no-fail-fast` | FAIL, exit 101: 126 passed, 1 failed, 0 ignored. Only the parent-rename gate failed. |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, exit 0 |
| `git diff --check` | PASS, exit 0; repeated after this receipt update |

The stalled-consumer failure was in an existing unchanged test, not an established pre-existing
failure. It passed in the all-features gate and every one of the ten subsequent repetitions.
Shared-host I/O contention is an explanation consistent with the timeout, not a proved cause;
this receipt does not classify the failure as flaky or claim the workspace gate is green.

### Ten native watch-suite repetitions

Ran each of these commands ten times, retaining the failing parent-rename case:

- `cargo test -p loomward-engine --all-features --lib scan::watch::native_tests:: -- --nocapture`
- `cargo test -p loomward-windows --all-features --lib watch::native::tests:: -- --nocapture`

All seven engine cases passed **10/10** (70 case passes). All six other Windows cases passed
**10/10** (60 case passes); the parent-rename case passed **0/10** (10 failures). Aggregate:
**130 case passes, 10 failures**. In particular, the native burst oracle, attribute hint,
root identity refusal, root deletion, overflow and cancellation tests each passed 10/10.
The stalled-consumer overflow test passed 10/10 without changing its timeout or assertions.
Every burst trial checks both moved-subtree variants against the fresh native oracle.
All timing is shared-host timing; these repetitions establish fixture reliability, not a
throughput, latency or cold-cache guarantee.

### Mutation checks

Each row first passed, then failed at the named test assertion with exit 101 after removing only
that rule, then passed again with exit 0 after byte-for-byte restoration. None of these red
results was a compilation failure. **15/15 final mutation checks discriminated the rule.**
The first exclusion-fixture mutation survived because it changed identity as well as attributes;
that fixture was corrected to retain identity, and its final mutation failed as required.

| Rule removed | Discriminating regression | Green / removed / restored exits |
| --- | --- | --- |
| delete sharing | `watch::native::tests::armed_watch_allows_root_delete_and_fails_closed` | 0 / 101 / 0 |
| path validation | `scan::watch::native_tests::renamed_root_fails_watch_and_replacement_refuses_granted_identity` | 0 / 101 / 0 |
| native failure flag | `watch::native::tests::armed_watch_allows_root_delete_and_fails_closed` | 0 / 101 / 0 |
| attributes filter | `scan::watch::native_tests::native_attribute_change_after_complete_scan_hints_parent` | 0 / 101 / 0 |
| security filter | `watch::native::tests::filter_covers_attributes_and_security` | 0 / 101 / 0 |
| checkpoint recheck | `watch::native::tests::completion_between_poll_and_request_is_delivered_before_ack` | 0 / 101 / 0 |
| callback before ack | `watch::native::tests::completion_between_poll_and_request_is_delivered_before_ack` | 0 / 101 / 0 |
| unsettled full scope with target | `scan::watch::tests::unsettled_full_pass_with_targeted_hint_never_claims_complete` | 0 / 101 / 0 |
| unsettled clean full scope | `scan::watch::tests::unsettled_full_pass_with_clean_watcher_repeats_full_before_complete` | 0 / 101 / 0 |
| identity presence | `scan::tests::same_name_new_directory_identity_matches_fresh_scan` | 0 / 101 / 0 |
| excluded presence | `scan::tests::directory_becoming_reparse_or_offline_matches_fresh_scan` | 0 / 101 / 0 |
| refused presence | `scan::sink::watch_tests::refused_child_is_not_directory_presence` | 0 / 101 / 0 |
| post-open presence identity | `scan::tests::post_open_and_name_only_directory_presence_matches_fresh_scan` | 0 / 101 / 0 |
| probe lock scope | `scan::watch::native_tests::watcher_start_probe_does_not_hold_global_watch_map` | 0 / 101 / 0 |
| start error cause | `scan::watch::native_tests::watcher_start_error_preserves_native_cause` | 0 / 101 / 0 |

### Recommended commits and exact resume point

Recommended driver commit messages, after resolving the retained failing gate:

- `Drain watch completions before checkpoint acknowledgement`
- `Preserve unsettled scan coverage and match directory presence by identity`

The working tree is uncommitted. Raw local gate, mutation and repetition logs plus the scripts
that generated them are gitignored under `.loomward/l16g-proof/`; they are not public artifacts.
Resume at the active `armed_watch_allows_parent_rename_and_sibling_delete_and_fails_closed`
test in `crates/loomward-windows/src/watch.rs:397`. Its missing prerequisite is a native open
strategy that permits ancestor rename while retaining no-recall/reparse refusal; resolve that
seam, rerun its discriminating mutation and the required gates, then integrate through the driver.
No weakened or ignored test is an acceptable substitute. Do not enable scan.start from this receipt.

### NOT verified / residual risk

The MEDIUM-2 ancestor-rename requirement is **known failing**, rather than untested. Its lifetime
interference with user operations remains a scan.start blocker. No safe alternative native open
strategy was established in the bounded diagnosis. The workspace release-wait timeout remains
an observed failure even though narrower repetitions passed.

No L8 service/durable catalogue adapter integration, UI disclosure, hosted CI, Linux run,
elevated-host execution, native ACL mutation/coverage transition, real cloud-placeholder hydration,
real personal scan, sustained scale or restart recovery was measured here. Existing native refusal
tests passed on this non-elevated host; their elevated refusal branches were not executed here.
Health checks run outside the global lock by inspection; the explicit lock regression probes
startup, not a simultaneous revoke/health race. Checkpoint starvation under continuous churn and
the already reported grant-revocation/start race remain #184 follow-ups. Runs still bound entry
counts and reconciliation passes, so continuous churn stays partial. No file/process executor,
new network request, install, elevation, release or scan-start endpoint was added.

### Part 2 - scan-scoped watch lifetime and stalled-consumer condition

Measured 2026-10-10 on Windows, non-elevated, on NTFS, with synthetic disposable fixtures.
All timings are **shared-host** timings; other lanes share CPU and disk. Starting HEAD is
`097035306a262eccddb600d7f13df05c05472f4f`, after `9363059`, on
`fix/l16-scan-start-gates`. Changes remain uncommitted; no push was made.

**Lifetime decision:** arm before enumeration, retain the watch through reconciliation and
the final post-publication drain, then stop and join it on every exit. Persistent watching
between scans was an extension, not required by docs/41 section 7 item 10. NTFS list-access
handles block ancestor renames despite delete sharing, so this extension interfered with the
owner's files. A future per-grant opt-in could revisit the decision; this implementation has none.

#### Changed / per-item status

| Requested item | Result and code | Discriminating tests |
| --- | --- | --- |
| 1. Arm and stop | DONE. `crates/loomward-engine/src/scan/watch.rs:348` installs an exit guard before validation; it stops/joins on complete, partial, cancelled, failed and unwinding exits. Reconciliation still owns publication and final draining before return. `scan/mod.rs:254` also stops on failed job submission. Revocation remains immediate (`scan/mod.rs:278`); `src/lib.rs:96` explicitly stops watches on engine teardown even when a runner retains an Arc. | `completed_scan_releases_pins_and_refreshes_full_baseline`; `complete_partial_cancelled_and_failed_scans_release_watch`; `engine_teardown_stops_watch_with_an_outstanding_runner_reference`; retained `revocation_stops_native_io_and_late_hints_cannot_reactivate_it`. |
| 2. Honest freshness | DONE. Native and dispatcher threads join in `scan/watch.rs:229`; no idle sink invalidation survives. A stopped checkpoint returns without manufacturing failed coverage (`:198`). Sink/report comments (`scan/sink.rs:149`, `scan/pipeline.rs:56`) describe run observations rather than live tracking. | The completed-scan test observes unchanged one-file totals after an idle creation, then two files after refresh. `armed_before_scan_burst_reconciles_names_moves_and_sums_like_a_fresh_scan` asserts a stopped native channel, unchanged dirty epoch and no idle repair transition. |
| 3. Full baseline | DONE. `scan/mod.rs:210` starts a fresh watch every time; stopped history is never reused. `scan/watch.rs:394` retains FullRoot as the first pass. `MAX_RELISTS = 4` and aggregate entry bounds are unchanged. | `completed_scan_releases_pins_and_refreshes_full_baseline` checks both start and refresh scopes and discovers the between-run file. Both unsettled-full-pass tests and the native burst oracle still pass. |
| 4. Native lifecycle regressions | DONE. The former armed-parent-rename test is rewritten at engine level (`scan/watch.rs:760`) so the real rename occurs after the completed job, before stopped-state assertions. Parent rename, sibling deletion and eventual root removal succeed. The fake-source root-open hook (`:1051`) deletes the root after the scan coordinator has started, while the watch is armed, then asserts failed checkpoint/RootDirty/native failure. Attribute coverage is checked while armed (`:936`), rather than implying an idle watcher. | `completed_scan_releases_pins_and_refreshes_full_baseline`; `complete_partial_cancelled_and_failed_scans_release_watch`; `native_attribute_change_during_armed_scan_hints_parent`; retained `armed_watch_allows_root_delete_and_fails_closed`, identity-refusal and revocation tests. Elevated-host branches assert refusal instead of claiming a native run. |
| 5. Stalled-consumer timeout | DONE. `crates/loomward-windows/src/watch.rs:485` waits for the producer's release condition, not a timer started before generation. `:493` places the release sender after the watcher in drop order: producer unwinding disconnects the callback before watcher join. Remaining readiness/observation deadlines and the 10,000-create overflow assertion are unchanged. | `stalled_native_consumer_reports_real_overflow`: 20/20 dedicated repetitions, plus 10/10 native-suite repetitions; controlled slow-generation experiment below. |

The UI can say **"Coverage as of the last completed scan; changes since then are unknown."**
A partial/cancelled/failed run must retain its partial/cancelled/repairing disclosure. Complete
means this run reconciled its observed window, not a live guarantee or an instantaneous snapshot.
No wire fields, schemas or contracts were changed. No UI implementation or disclosure was tested.

#### Verified gates

These exact commands ran against the final Rust changes, without skipping or weakening tests.
Wall times include Cargo/test execution and are shared-host observations, not performance claims.

| Command | Result | Shared-host wall time |
| --- | --- | --- |
| `cargo fmt --all --check` | PASS, exit 0 | 0.77 s |
| `cargo test --workspace` | PASS, exit 0; 347 passed, 0 failed, 0 ignored | 43.22 s |
| `cargo test -p loomward-windows -p loomward-engine --all-features` | PASS, exit 0; 129 passed, 0 failed, 0 ignored | 16.56 s |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, exit 0 | 4.99 s |
| `git diff --check` | PASS, exit 0; repeated after this receipt edit | 1.38 s before receipt edit |

The first run of the new terminal-outcome test incorrectly expected cancellation to return a
partial report; the actual API returned `EngineError::Cancelled`. The assertion was corrected to
pin that existing cancellation behavior. The final test and all gates passed.

#### Ten native watch-suite repetitions

Each command ran ten times, on the final Rust source:

- `cargo test -p loomward-engine --all-features --lib scan::watch::native_tests:: -- --nocapture`
- `cargo test -p loomward-windows --all-features --lib watch::native::tests:: -- --nocapture`

Engine: **10/10 suite passes, 100 case passes** (10 cases per run).
Windows: **10/10 suite passes, 60 case passes** (6 cases per run).
Aggregate: **160 case passes, 0 failures**. Both subtree variants of the burst oracle, the
post-publication mutation, root deletion, attributes, identity refusal, revocation and lifecycle
release checks passed in every relevant repetition. Native execution, rather than the elevated
refusal branches, ran on this host.

#### Stalled-consumer diagnosis and twenty repetitions

The earlier failing gate records callback `Timeout`, followed by producer `SendError`. The
callback's ten-second deadline began before the producer generated 10,000 files; the release
send could not happen until generation finished. Thus the fixture could expire precisely while
it was deliberately stalled. The old timer measured producer progress, not watcher failure.

Discriminating experiment: temporarily insert an eleven-second pause into generation, identically
in the fixed and old-wait versions. The fixed condition wait passed with generation **14,062.978 ms**;
restoring only the original ten-second wait reproduced callback `Timeout` and producer `SendError`
(exit 101). Restoring the condition wait passed with generation **13,911.495 ms**. The injected
pause was removed, source bytes were restored, and the unmodified fixture passed again.
This proves the premature-deadline mechanism. The precise disk/scheduler bottleneck in the earlier
gate was not instrumented and remains unknown; no flaky classification or timeout increase is used.

Dedicated command, run twenty times:

`cargo test -p loomward-windows --all-features --lib stalled_native_consumer_reports_real_overflow -- --nocapture`

**20/20 passes**, each producing a real root invalidation after 10,000 creates. Shared-host
generation min / median / max: **2,131.820 / 2,415.084 / 3,336.610 ms**.
Detection since burst start min / median / max: **2,167.226 / 2,447.978 / 3,373.869 ms**.
These include deliberate consumer stalling and fixture generation; they are not overflow
thresholds or kernel-event latency estimates. The native-suite repetitions add ten further
passes of this fixture; the dedicated pass count remains 20/20.

#### Mutation results

Each final mutation passed first, failed at a runtime test assertion after removing only its
fix, and passed after restoration: **7/7 final discriminating checks**, no compilation-failure
substitutes. The parent-rename mutation failed at the real filesystem rename with Win32 error 5.

| Fix removed | Test | Green / removed / restored exits |
| --- | --- | --- |
| Run exit stop guard | `completed_scan_releases_pins_and_refreshes_full_baseline` | 0 / 101 / 0 |
| Run exit stop guard | `complete_partial_cancelled_and_failed_scans_release_watch` | 0 / 101 / 0 |
| Run exit stop guard | `armed_before_scan_burst_reconciles_names_moves_and_sums_like_a_fresh_scan` | 0 / 101 / 0 |
| Engine teardown stop | `engine_teardown_stops_watch_with_an_outstanding_runner_reference` | 0 / 101 / 0 |
| Native failed flag | `complete_partial_cancelled_and_failed_scans_release_watch` | 0 / 101 / 0 |
| Attribute filter | `native_attribute_change_during_armed_scan_hints_parent` | 0 / 101 / 0 |
| Condition release wait, with identical slow-generation injection | `stalled_native_consumer_reports_real_overflow` | 0 / 101 / 0 |

The first native-failed-flag mutation survived the engine-only assertion: a disconnected checkpoint
already made the engine fail closed. The test was strengthened to assert the native flag as well;
its final mutation failed. All temporary mutations and injected delays were restored.

#### Driver handoff

Recommended commit messages:

- `Scope native watches to each scan`
- `Wait for stalled-consumer fixture generation to finish`

Raw gate, repetition and mutation logs/scripts remain gitignored under `.loomward/l16g2-proof/`.
They contain local diagnostic context and are not publication artifacts. The evidence addendum and
Rust changes are ready for the driver's review and commit; this worker made no commit or push.
The requested MEDIUM-2 lifetime and timeout repairs are locally proved. This receipt does not enable
a scan-start endpoint or settle other #184 follow-ups.

#### NOT verified / residual risk

No hosted CI, Linux run, elevated-host execution, durable catalogue/L8 integration, UI disclosure,
real personal roots, cloud-placeholder hydration, ACL mutation, sustained scale or reboot/restart
experiment was run here. Job-thread creation failure cleanup and the exit guard's panic-unwind
path are implemented but were not fault-injected. Engine teardown was tested with an outstanding
runner reference, not a blocked enumeration syscall. The earlier grant-revocation/start race and
checkpoint starvation under continuous churn remain separate #184 follow-ups.

While a scan is actively armed, NTFS ancestor rename can still be blocked by its list-access
handle. Pins are released before terminal job publication, which the real parent-rename test
proves. Changes between scans are unknown; a new explicit scan starts with full baseline coverage.
Bounded reconciliation under continuous churn can remain partial. No persistent watch, executor,
new network request, installation, elevation, release or real-root scan was introduced.
