# Duplicates LOWs: #378 and #359

2026-10-17, Windows 11 (KRASPYON), Sol lane `dups-378`, base `00bb610`.
Left uncommitted for the driver. Synthetic fixtures only; no personal folder was scanned.
The refreshed `origin/main` was `f308bed`; its intervening #593-#595 changes do not resolve these LOWs
or change the duplicate reader, duplicate entry points, HTTP route, capability DTO or dialog code.

## Item reconciliation

| Issue item | Evidence and disposition |
|---|---|
| #378: share-write on content opens | Retained deliberately. `1a724c6` (#243, hardening #238) introduced deny-write and `native_existing_writer_is_a_visible_sharing_violation` pins it. The same `Reader::open_relative` now serves `CopySource::open` and tree comparison. `docs/47-copy-and-verify.md` sections 3 and 6 explicitly rely on deny-write for copy correctness. Changing the common open would weaken that guarantee. A separate inspection sharing policy needs a product decision; no existing safety test was weakened. |
| #378: stale status rows | Still present at `docs/16-implementation-status.md:129` and `:195`; the later q-10 rows at `:212` and `:256` describe the opening. This lane is explicitly forbidden to edit that file. Driver correction: append a pointer from the two historical rows to the q-10 row, preserving history. |
| #378: service candidate cap after root open | Fix: select bounded catalogue candidates before `granted_root` opens native root metadata. Keep the session root gate first and the explicit freshness claim across selection, native grant resolution and submission. Keep #577/#590's retry, candidate re-read and pre-emption only on resource conflict. |
| #378: `effects.content_read` naming | Document the DTO: it means arbitrary file-content access through the API; local duplicate hashing does not return contents. Keep the wire name and `ConstFalse`, matching the existing semantics and schema. |
| #378 comment: revoke/re-grant admission | Already fixed by `2dd7015` in review on #376: active-only counting in `duplicate_read_admission_inner`; existing engine `duplicate_admission_rechecks_durable_class_identity_and_ambiguity` and service `duplicates_follow_a_fresh_grant_after_the_same_folder_was_revoked` tests. |
| #378 comment: identity change transition | Already fixed by `23a55cc`, merged as #388. Existing `a_replaced_root_found_by_inspection_records_identity_changed` checks the durable transition and `roots.changed` event. Preserve it while moving the open. |
| #378 comment: stale UI completions | Epoch generations already added by `b2c74b9` and `23df7b4` (#472); deferred jobs.list / inspect / cancel tests in `app/tests/unit/duplicates-store.test.ts`. `refresh` also checks the captured generation. The store is constructed per view. Source inspected; browser reconnect and arbitrary client-replacement behavior were not executed in this Rust lane. |
| #359: old personal gate after candidate cap | `content_read_gated` was retired by `ac3f9db` (#376), so that specific refusal no longer exists. Fix the remaining cheap dataset-class refusal before the cap, without moving durable admission I/O before the candidate bound. Add exact-cap coverage (2 and 200 candidates). |
| #359: stop hidden by catalogue refusal | Already fixed by `d82abff` in #358: `report.stop`, hydration stop and budget reasons precede the catalogue fallback. Existing `duplicate_budget_stop_takes_precedence_over_a_path_refusal` pins it. No contract expansion is needed. |
| #359: drain holds an SSE permit | Fix: drain an authenticated events request before opening its subscription or reserving an SSE slot. Header/auth/origin gates and CORS remain in their existing order; the byte and timeout bounds remain unchanged. New deterministic test polls a stalled body and checks all stream permits remain available. |
| #359: long early-refusal drain / connect-timeout hypothesis | Still bounded by `read_timeout` and `DRAIN_LIMIT`. No shorter arbitrary timeout introduced: Windows reply preservation is the reason for draining, and the issue records a hypothesis with 15/15 non-reproducing runs. The SSE permit defect is addressed separately. Connection-slot occupancy and the historical connect-timeout cause remain open. |
| #359: empty native disclosure pages | Unreachable through safe production construction since `c1b0e3c` (#133): `DisclosureSummary.items` is `BoundedVec<_,1,25>`, with a private Vec, no mutable dereference and minimum checks in both constructor and deserializer. `bounded_collections_enforce_their_limits` already checks empty-array rejection. No redundant native guard or unsafe invalid fixture added. |
| #359: wall-clock HTTP tests | Existing socket timeout tests remain; no test was weakened. The new SSE regression uses explicit future/body polling rather than a timing threshold. A general migration of the older socket/deadline tests remains separate work. |

## Verification

Working directory is this worktree; every command uses the inherited
`CARGO_TARGET_DIR=C:\lw\target\pool-b` and `CARGO_BUILD_JOBS=4`. Build-lock waits are expected.

Regression-first observations:

- `cargo test --manifest-path C:/lw/wt/dups-378/Cargo.toml -p loomward-engine duplicate_dataset_class_refusal_precedes_candidate_budget`:
  **RED**, 0 passed / 1 failed. The old code returned the candidate-budget message instead of
  `dataset_class_mismatch`. The fix calls the existing `check_root` before the candidate cap.
- `cargo test --manifest-path C:/lw/wt/dups-378/Cargo.toml -p loomward-http sse_body_drain_does_not_hold_a_stream_permit`:
  **RED**, 0 passed / 1 failed. Only 3 of the 4 stream permits remained available while the
  request body was stalled; the expected count is 4.
- `cargo test --manifest-path C:/lw/wt/dups-378/Cargo.toml -p loomward-service duplicates_select_candidates_before_opening_root_metadata`:
  **RED**, 0 passed / 1 failed. The missing synthetic root was refused before the
  `candidates_ready` phase was reached. After moving the native root resolution below bounded
  selection, the same missing root must still be refused, but only after that phase.

All three expected failures were assertions, not setup or compilation errors. All three tests
passed in the final touched-crate run, as did the existing exact-cap test (2 and 200 candidates).

Final checks, with the worktree and environment above:

| Exact command | Result |
|---|---|
| `cargo fmt --manifest-path C:/lw/wt/dups-378/Cargo.toml --all --check` | PASS |
| `cargo test --manifest-path C:/lw/wt/dups-378/Cargo.toml -p loomward-engine -p loomward-service -p loomward-http -p loomward-protocol --no-fail-fast -- --test-threads=3` | **691 passed, 5 failed, 1 existing ignored test**; exit 1. Failures and isolated reruns below. |
| `cargo clippy --manifest-path C:/lw/wt/dups-378/Cargo.toml -p loomward-engine -p loomward-service -p loomward-http -p loomward-protocol --all-targets -- -D warnings` | PASS, exit 0, zero warnings. |
| `git -C C:/lw/wt/dups-378 diff --check` | PASS, exit 0. |

The crate totals include integration tests and doctests: engine 238 passed / 4 failed;
HTTP 81 passed / 1 failed; protocol 54 passed; service 318 passed / 1 ignored.
The ignored test is the existing `handlers::scrub_copy_tests::large_sparse_backup_memory_probe`
(manual 1 GiB streaming-memory probe); no test was newly ignored or weakened.

The #577/#590 cases passed, including `f3_duplicates_claim_fences_candidate_selection`,
`f3_duplicates_leave_refresh_running_when_a_worker_is_available`,
`f3_duplicates_preempt_a_running_watcher_refresh`, and
`f3_owner_preemption_does_not_spend_refresh_attempt`.

### Existing timing failures and isolated controls

These failures occurred in unchanged tests and paths outside the changed behavior. The first
two hit two-second channel receives. The filesystem cases hit `roots.forget` and `search.query`
deadlines. The HTTP helper killed its child after a three-second startup wait; its message
"overlapping static server started" does not establish that the child started listening.

Reruns used the **same executables Cargo just built**, one named test per process, with
`--exact --test-threads=1`, package-root working directories and `CARGO_MANIFEST_DIR` set to
the corresponding package. This avoids recompilation and retains the exact compiled feature set.

| Executable below `C:/lw/target/pool-b/debug/deps/` | Exact test argument | Isolated result |
|---|---|---|
| `loomward_engine-6527fcbf82d0fdb7.exe` | `duplicates::tests::duplicate_job_cancel_and_root_revocation_reach_the_worker_and_terminal_state` | 1 passed, 0 failed (15.31 s) |
| `loomward_engine-6527fcbf82d0fdb7.exe` | `scan::watch::native_tests::concurrent_scan_start_reserves_root_before_touching_watch` | First isolation: 0 passed / 1 failed, same timeout (4.92 s). Final paired rerun: 1 passed / 0 failed (2.89 s). |
| `pathological-a967f2edbcad0ecc.exe` | `multi_thousand_hardlinks_keep_every_name_and_shared_identity` | 1 passed, 0 failed (94.71 s) |
| `pathological-a967f2edbcad0ecc.exe` | `concurrent_create_delete_rename_grow_reconciles_exactly_to_fresh_scan` | 1 passed, 0 failed (222.50 s) |
| `serve-d1ee1a3640cb70f5.exe` | `followup_stored_grants_refuse_static_without_recording_new_grants` | 1 passed, 0 failed (24.06 s) |

For example, the cancellation retry command was
`C:/lw/target/pool-b/debug/deps/loomward_engine-6527fcbf82d0fdb7.exe duplicates::tests::duplicate_job_cancel_and_root_revocation_reach_the_worker_and_terminal_state --exact --test-threads=1`;
the other rows use the same command shape with their literal executable and test argument.
All five originally failing cases eventually passed alone, without source or assertion changes.
The first isolation round was 4 passed / 1 failed, not a clean 5/5 run.

The final watcher comparison also ran the pre-fix production-engine executable
`C:/lw/target/pool-b/debug/deps/loomward_engine-7eb36cfee022ea41.exe scan::watch::native_tests::concurrent_scan_start_reserves_root_before_touching_watch --exact --test-threads=1`:
1 passed / 0 failed in 0.40 s. That executable was built at 21:12:44, before the production
class-check change, and is the executable from the initial RED observation above. The current
executable then passed under the same settings. No further rerun or broader suite was added.

Raw logs remain ignored in `.loomward/dups-378-tests.log`, `dups-378-clippy.log`,
`dups-378-rerun-*.log`, `dups-378-watch-baseline.log` and `dups-378-watch-paired.log`.
The initial failed attempts remain in those logs. The combined four-crate run was **not** clean;
the isolated successes without source changes are consistent with shared-host timing pressure.

## NOT verified / residual risk

No workspace-wide suite, app/browser, native-shell, real cloud-placeholder or real personal-root
checks in this lane. No external product-service integration, owner-file/process effect, release,
commit, push or PR. Test effects stayed within synthetic fixtures and their own child processes.
`HUMAN_TODO.md` is unchanged; q-10 already authorises local bounded personal duplicate inspection.
No owner decision is inferred for sharing policy, drain timeout or other human-action entries.
