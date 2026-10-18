# Diagnostic bundle follow-ups (#437)

2026-10-17, KRASPYON / Windows 11. Sol lane `fix/437-diagnostics-bundle`,
starting at `f308beda9ac37c79d08e4146f47c4dddfa93d922` (also `origin/main` at inspection).
No commit, push, PR or merge from this lane. Synthetic fixtures only.

## Reconciliation with main

PR [#559](https://github.com/Chris0Jeky/Loomward/pull/559), merged as `d893e38`, already addressed:

| #437 item | Existing implementation and evidence |
|---|---|
| Cancel versus unavailable or busy dialog | `a533f71`: `SaveDialog` separates `Cancelled`, `Busy` and `Unavailable`; service test `a_dialog_that_never_opened_is_not_reported_as_a_cancel`; native test `save_dialog_opens_in_every_session_retitled_and_one_at_a_time`. rfd errors remained ambiguous, addressed below. |
| Root cap | `a533f71`: `diagnostic_roots` reports `root_cap_exceeded`; `more_than_64_roots_are_one_unknown_naming_the_cap` checks 64 roots and the 65th root. |
| Save state across navigation | `8d6dd93`: the module-level `diagnosticsPanel` owns the tracker, displayed bytes and outcome; `diagnostics-panel.test.ts` checks pending-save freeze, recovery and unseen notices. |
| Placeholder refusal | `a533f71`: `cloud_placeholder` has its own reason and message; `cloud_placeholder_attributes_are_recognised` checks offline/recall attributes. |
| Real-window digest check | `d4bfe4d`: `native/tests/webview2_probe.py` invokes the real engine and checks `crypto.subtle`, UTF-8 byte count and SHA-256 in WebView2. PR #559 records this check passing in the real window; its full probe was 18/20, with picker-navigation and P9 failures. That is historical proof, not a new probe run in this lane. |
| Recheck just before rename | `a533f71`: `write_to` rechecks a previously absent target before rename; `a_file_that_appears_after_the_check_is_not_replaced` preserves its bytes and checks temporary-file cleanup. The remaining check-to-rename gap is still documented. |
| Concurrent preview amplification (issue comment) | `8d6dd93`: `DiagnosticsPanel.refresh` allows one active preview and a dirty follow-up; the existing burst test holds the first response, sends 100 invalidations, and checks two calls, maximum one in flight, rejection of stale text/digest and the newest verified preview. |

## Remaining defect and change

rfd 0.17.2's Windows `FileSaveDialogImpl::save_file` ends with `run(self).ok()`.
An HRESULT failure and a user cancellation therefore both arrive as `None`.
`DesktopDialogs::save_with` still converted either to `SaveDialog::Cancelled`.

The native boundary now preserves this as `SaveDialog::NoSelection`; the service returns
`refused` / `dialog_no_selection`, with no byte count, and the app explains that no location
was returned, cancellation versus failure is unknown, and nothing was saved. Known cancellation from an adapter,
and declining Loomward's own replace confirmation, still return `cancelled_by_user`.
No new dependency or alternate dialog implementation is needed. The selected path stays
inside Rust, and the existing digest, target and exact-byte write checks are unchanged.
The existing `dialog_unavailable` message no longer assumes the window is the cause: that result
also covers an unavailable start folder or adapter. Native Cargo refreshed the lockfile for the
already-declared `loomward-ask` path dependency; no external dependency was added or upgraded.

## Verification

All Cargo invocations use `CARGO_BUILD_JOBS=4` and the assigned shared
`CARGO_TARGET_DIR=C:\lw\target\pool-a`. Commands run from this worktree; paths below are relative to it.

| Command | Result |
|---|---|
| Before the fix: `npm.cmd --prefix app run test -- tests/unit/diagnostics.test.ts` | RED: 3 passed, 1 failed; `dialog_no_selection` was missing. |
| `npm.cmd --prefix app run test -- tests/unit/diagnostics.test.ts tests/unit/diagnostics-panel.test.ts --maxWorkers=2` | GREEN: 7 passed. |
| `npm.cmd --prefix app run test -- --maxWorkers=2` | PASS: 504 tests in 48 files. |
| `npm.cmd --prefix app run check` | PASS: 421 files, 0 errors, 0 warnings. |
| `npm.cmd --prefix app run build` | PASS. |
| `py -3 -m unittest tests.test_v3_contracts -v` | PASS: 6 tests. |
| `py -3 scripts/gen_contracts_ts.py --check` | PASS: generated TypeScript is up to date. |
| `cargo fmt --all --check` and `cargo fmt --manifest-path native/Cargo.toml --all --check` | PASS. |
| `cargo test -p loomward-service diagnostics -- --nocapture` | PASS: 15 diagnostics unit tests, 0 failed; the unrelated integration target selected 0 tests. |
| `py -3 .loomward/diag437_native_mapping.py` (ignored, standalone source harness) | RED before the native change: 0 passed, 1 failed, because the extracted `save_with` returned `Cancelled`. GREEN after: 2 passed, including a chosen-path control. The harness compiles the exact method, enum and regression source with only the constructor and rfd builder stubbed; it proves the mapping, not native crate compilation, rfd/COM behavior or a real window. |
| `cargo test --manifest-path native/Cargo.toml --lib dialogs::tests::save_dialog_none_does_not_identify_a_user_cancel -- --exact --nocapture` | PASS in the actual desktop crate: 1 passed, 0 failed, 18 filtered out. An earlier filter omitted `dialogs::tests::` and selected zero tests; that invocation proved compilation only. |
| `cargo test --manifest-path native/Cargo.toml --quiet` | PASS: 60 passed, 0 failed, 1 existing ignored test (19 library; 35 passed + 1 ignored integration; 2 and 4 in the other integration targets). |
| `cargo clippy --manifest-path native/Cargo.toml --all-targets -- -D warnings` | PASS. |
| `cargo clippy -p loomward-protocol -p loomward-service --all-targets -- -D warnings` | PASS. The enclosing test/clippy script returned 101 because it deliberately preserved the earlier combined test failure, not because clippy failed. |
| `cargo build --manifest-path native/Cargo.toml`; `C:\lw\target\pool-a\debug\loomward-desktop.exe --self-test` | PASS: build succeeded and all 14 shell self-test checks passed. No real save dialog was driven. |
| `py -3 scripts/test_app.py` | FAIL outside the changed seam: Atlas hover measured 30 fps against its 50 fps threshold; the earlier 2,500-node first-draw check passed at 47.6 ms. The run stopped before the diagnostics browser leg. |
| `py -3 .loomward/diag437_browser.py` (ignored scratch runner, initial run) | PASS: calls the existing `test_app_views.check_diagnostics_save`, checks the new `dialog_no_selection` message through the fake engine and rendered panel, then retries the unchanged Atlas hover check in a fresh browser. The hover retry measured 59.88 fps (16.7 ms median, 83.4 ms p95). This does not make the full browser suite green. The final runner omits the already-measured Atlas retry. |
| `py -3 scripts/test_app.py` (second full run) | FAIL: `Page.goto` timed out after 8,000 ms loading the initial mock page. No threshold or timeout was weakened. Full browser completion remains unverified. Raw log: ignored `.loomward/diag437-app-e2e-retry.log`. |
| Final wording: focused 7-test command above, production build, then scratch browser runner | Unit tests and production build PASS. The browser recheck timed out on `about:blank` before application code loaded, so final wording has unit proof only; the earlier browser run proves the same refusal path with its earlier wording. |
| `cargo test -p loomward-protocol -p loomward-service` (first combined run) | FAIL before tests: E0609 in unchanged `crates/loomward-service/src/clutter.rs:91`, claiming `DeveloperClutterCandidate.kind` is absent. The unchanged catalogue source declares `pub kind` in `src/developer_clutter.rs:14`; no clutter/catalogue content was changed by this lane. The earlier diagnostics and native regression runs compiled successfully. This is a source/artifact inconsistency; stale shared artifacts are suspected, not established as the cause. The conditional clippy command did not run after this failure. |

For the second attempt, only this worktree's catalogue `src/lib.rs` timestamp was refreshed (no content change).
Cargo fingerprint logging confirmed that the catalogue dependency was stale and rebuilt it. The run then
stopped before tests with E0425: eight clutter DTOs were absent from the protocol artifact even though
`dto/clutter.rs` declares them, `dto/mod.rs` re-exports them, and `lib.rs` re-exports `dto::*` (all unchanged).
For the third and final test attempt, the protocol `src/lib.rs` and `src/dto/mod.rs` timestamps were also
refreshed without content changes. No shared cache file was deleted and no clutter source was changed.

The third run compiled the current source and passed all **54 protocol tests** (12 contract, 10 fuzz-envelope,
29 L1a contract, 3 query-grammar). The service unit run reported **319 passed, 2 failed, 1 existing ignored**:

- `tests::a_scan_started_past_its_deadline_reports_the_started_job`: expected a deadline error but got a queued job.
- `tests::copy_tests::lab_copy::sources_from_two_granted_roots_are_refused`: setup hit `ResourceBudget` / `scan worker pool full`.

Both passed alone without source/assertion changes, using a hash-verified frozen copy of that same freshly
built service test executable (`.loomward/diag437-service-tests.exe`, SHA-256
`9014fd9a21ed1c03ef7dbb511dc0c07b74e3659a191c6cc1b68e70d8d3a43457`):

```
.loomward/diag437-service-tests.exe --exact tests::a_scan_started_past_its_deadline_reports_the_started_job --nocapture --test-threads=1
.loomward/diag437-service-tests.exe --exact tests::copy_tests::lab_copy::sources_from_two_granted_roots_are_refused --nocapture --test-threads=1
```

Each reported 1 passed, 0 failed. The supplied wave brief identifies this timing/worker-pool family as
pre-existing under host load; the original full-run failures remain recorded here. The fail-fast-skipped
service integration binary was also hash-verified and frozen as `.loomward/diag437-root-coverage.exe`;
running it with `--nocapture --test-threads=1` passed its one
`completed_dirty_root_never_advertises_complete_coverage` test. The combined Cargo command remains a failed
run; isolated passes are not relabelled as a green combined run.

The full 504-test app run preceded the final copy-only refinement; its seven affected diagnostic/panel tests
were rerun after that refinement. No browser timeout or frame budget was relaxed.

No lane verification process remains running. The workspace-wide suite was deliberately left to the driver,
as the lane brief requires. Hosted CI was not run (this Sol lane does not commit, push or create a PR).
The ignored `.loomward/diag437_*` source harnesses, generated test programs, frozen test executables and
browser log are disposable synthetic verification artifacts; this document retains the curated receipts.

## Open scope

- Manual observation of a new-file save and Loomward's overwrite question remains an owner
  acceptance item. Automated stand-ins are not that observation; `HUMAN_TODO.md` q-15 remains untouched.
- The comment's real-window 100k/1M scan churn measurements (request counts, SQLite query cost,
  p95 freshness, throughput and RSS) are separate from the existing mocked coalescing test.
- rfd still cannot identify the underlying cause of `None`; this change reports that uncertainty.
- The existence recheck narrows, but does not close, the check-to-rename race.
