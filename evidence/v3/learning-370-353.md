# Learning follow-ups: issues #370 and #353

Lane `fix/370-353-learning`, Windows 11, 2026-10-17. Starting head `00bb610`.
The initial `origin/main` snapshot was `f39b75d`; later `git fetch origin main` checks refreshed it
to `f308bed` and then `4162325`, still without a competing learning fix. Synthetic SQLite fixtures only.
The driver commits and integrates this lane; no commit, push or PR was made here.

## Item reconciliation

| Issue item | Live evidence and disposition |
| --- | --- |
| #370: pending rebind, cross-root move, unresolved/rebuilt catalogue | Still present in `Store::redact`: the sweep joins the current `object_ref.row_id`. Record grant provenance per new feature snapshot, and sweep by that immutable provenance. |
| #370: swallowed redaction errors | Still present in both grant-ending handlers. Surface explicit revoke failures and retry cleanup before serving learning advice; do not serve a stale model if retry fails. |
| #370: revoke during refit | Add a `REFIT_HOOK` snapshot phase test: revoke after the training snapshot, expect `stale_generation` and no candidate. |
| #370: over-redaction | Add an independent active-root label to the provenance regression; its snapshot and training survive revoking the first root. A fresh label after a move belongs to the new grant. |
| #370: dataset mismatch test | Strengthen the service-open assertion to require the exact dataset mismatch error, rather than accepting any error. |
| #370: obsolete app gate code | Still present in `app/src/lib/stores/meaning.svelte.ts` and its unit test. Left open outside this Rust learning lane; `threads.meaning` cannot emit the former personal-learning refusal. |
| #370: permanent redaction | Document the existing permanent snapshot withdrawal, including `identity_changed`, in `contracts/v3/semantics.md`. |
| #353: clock steps | Still present in `Store::student`. New fits store the latest human-feedback row ID in private model JSON and compare that monotonic counter. Legacy models retain their timestamp guard until explicit refit. Refresh JSON on same-model-ID refits too. |
| #353: post-commit status failure | Still present in the refit worker. Capture the announcement's status before commit, under the transaction, and publish only after commit. Keep a diagnostic if status cannot be read. |
| #353: cold queue ID displayed as fitted | Already handled by `add9b70`: Review uses `status.model_id` for its fitted/no-model wording, not the queue's ID. Current `app/src/views/review/View.svelte` retains that distinction. No queue schema change needed. |
| #353: jobs errors and cancellation wording | Assigned to the jobs lane; not changed here. |

The normal service revoke path already reconciles pending references after publication
(`c3a6241`, `reconcile_references`; collision repair added by `2958b9`). That mitigation is
retained, rather than reimplemented. It explains why the first pending fixture passed through
the normal writer before any change. The direct engine-sweep regression keeps confirmation
outstanding; immutable provenance no longer relies on confirmation having succeeded. The
cross-root case did fail through normal service revocation on the original code.

## Legacy provenance decision

Historical events have no immutable original grant. Their current reference cannot prove where
their features came from: the object may already have moved or become unresolved. Backfilling
that reference would invent provenance. Blanket redaction would permanently discard feature
snapshots and require fresh labels for training. That product choice was asked separately and
is left open pending an answer. The old best-effort sweep remains for historical events;
the provenance fix applies to events recorded by the updated engine. **Keep #370 open.**

`HUMAN_TODO.md` remains the owner-action file and was not changed by this worker.

## Verification

All Cargo runs use `CARGO_BUILD_JOBS=4` and the assigned, inherited shared target pool;
the pool was never cleaned or removed. Commands run from this lane's worktree.

Before the corresponding behavior changes:

- `cargo test --manifest-path C:/lw/wt/learn-370/Cargo.toml -p loomward-service learning_ -- --test-threads=3`:
  14 passed, 4 failed. Three failures reproduced the moved-snapshot, swallowed-redaction-error and
  clock-step defects. The fourth was the strengthened dataset test's incorrect expected error prefix;
  live startup returns `state store: DatasetMismatch`, and the assertion was corrected to that exact error.
  The provenance cases were then split so each case is reported independently, with the pending case
  invoking the engine sweep after the grant's durable state changes (the revoke writer otherwise
  folds the staged rebind before the sweep).
- `cargo test --manifest-path C:/lw/wt/learn-370/Cargo.toml -p loomward-engine learning_refit_status_failure_announces_committed_candidate -- --exact learning::tests::learning_refit_status_failure_announces_committed_candidate`:
  0 passed, 1 failed: zero update events after the candidate committed. The strengthened test retains
  the committed-candidate and completed-job assertions and now requires a schema-valid update event.
- `C:/lw/target/pool-a/debug/deps/loomward_service-0cd028a8b3f776ca.exe learning_features_refuse_a_path_from_another_root_snapshot --test-threads=1`:
  0 passed, 1 failed, before the root-match guard. This used the executable just built by the
  provenance test run: it returned features despite the mismatched root views. The guard now
  returns `stale_generation`, preventing a mixed-root feature snapshot from acquiring false provenance.
- `cargo test --manifest-path C:/lw/wt/learn-370/Cargo.toml -p loomward-engine --lib learning_legacy_candidate_stays_stale_until_counter_refit`:
  0 passed, 1 failed against the intermediate candidate-status-only change. History inspection showed
  candidate retirement was introduced by `b53b8e1`; older stores can retain stale candidates. The final
  fix keeps their timestamp check and adds a feedback counter to new fits. The regression also proves
  a same-model-ID refit refreshes that counter when unresolved feedback leaves training unchanged.

After the provenance change:

- `cargo test --manifest-path C:/lw/wt/learn-370/Cargo.toml -p loomward-service learning_revocation_sweeps -- --test-threads=3`:
  3 passed, 0 failed. Each case also preserves the other active root's features and training;
  the moved case additionally records a fresh label under the new grant and proves it survives.

First full touched-crate run (`CARGO_BUILD_JOBS=4`, `RUST_TEST_THREADS=3`):

- `cargo test --manifest-path C:/lw/wt/learn-370/Cargo.toml -p loomward-engine -p loomward-service`:
  engine units: **219 passed, 1 failed**. All learning engine tests passed, including the committed
  announcement and revocation-mid-refit regressions. Cargo stopped before the remaining targets.
- The unchanged `scan::watch::native_tests::complete_partial_cancelled_and_failed_scans_release_watch`
  failed at `scan/watch.rs:1380`: `result.complete` was false where the complete case expected true.
  `git -C C:/lw/wt/learn-370 diff --exit-code HEAD -- crates/loomward-engine/src/scan/watch.rs` was clean.
- `C:/lw/target/pool-a/debug/deps/loomward_engine-4b5877c2b1b8957d.exe --exact scan::watch::native_tests::complete_partial_cancelled_and_failed_scans_release_watch --test-threads=1`:
  **1 passed, 0 failed**. The concurrent failure and isolated pass are recorded; the precise native
  watcher cause is not established. No watcher assertion or timeout was changed.

Earlier serial touched-crate retry (`RUST_TEST_THREADS=1`), before the final legacy-cache regression:

- `cargo test --manifest-path C:/lw/wt/learn-370/Cargo.toml -p loomward-engine -p loomward-service --no-fail-fast`:
  engine units 220 passed; pathological integration 9 passed; smoke 5 passed; telemetry 4 passed,
  1 failed; service units 322 passed, 1 failed, 1 existing manual probe ignored; root-coverage integration
  1 passed; engine doctests 3 passed. Service doctests failed with E0463 dependency-artifact lookup
  errors while the shared pool was also rebuilding. Artifact contention is suspected, not proven.
- Telemetry's unchanged `leases_share_renew_release_and_latest_is_not_an_on_demand_scan` timed out
  waiting ten seconds for its first sample. Running the just-built executable alone with
  `--exact leases_share_renew_release_and_latest_is_not_an_on_demand_scan --test-threads=1`
  passed 1/1 in 2.31 seconds.
- The unchanged service jobs test `tests::a_duplicates_job_started_past_its_deadline_reports_the_started_job`
  returned a queued job instead of the expected deadline error. Running the just-built executable
  alone with that exact name and `--test-threads=1` passed 1/1 in 3.57 seconds. Neither timing test
  was edited; both failures and the isolated controls are retained here.
- A redundant queued service-only Cargo process was cancelled before it started child work and
  replaced by the combined no-fail-fast run. No other lane's process was stopped.

Final clock compatibility check:

- `cargo test --manifest-path C:/lw/wt/learn-370/Cargo.toml -p loomward-engine --lib learning_`:
  **5 passed, 0 failed**, including the legacy-cache, committed-announcement, mid-refit-revocation
  and cancellation tests.

Final sequential proving run, after all Rust changes (`RUST_TEST_THREADS=3`):

- `cargo fmt --manifest-path C:/lw/wt/learn-370/Cargo.toml --all --check`: passed.
- `cargo clippy --manifest-path C:/lw/wt/learn-370/Cargo.toml -p loomward-engine -p loomward-service --all-targets -- -D warnings`: passed.
- `cargo test --manifest-path C:/lw/wt/learn-370/Cargo.toml -p loomward-engine -p loomward-service --no-fail-fast`: **passed, exit 0**.

| Final target | Passed | Failed | Ignored |
| --- | ---: | ---: | ---: |
| Engine unit tests | 221 | 0 | 0 |
| Engine pathological integration | 9 | 0 | 0 |
| Engine smoke integration | 5 | 0 | 0 |
| Engine telemetry integration | 5 | 0 | 0 |
| Service unit tests | 323 | 0 | 1 |
| Service root-coverage integration | 1 | 0 | 0 |
| Engine doctests | 3 | 0 | 0 |
| Service doctests | 0 | 0 | 0 |
| Total | **567** | **0** | **1** |

The existing ignored service test is the explicit manual 1 GiB streaming-memory probe,
`handlers::scrub_copy_tests::large_sparse_backup_memory_probe`; no ignore was added.
The native watcher, telemetry and jobs deadline tests that failed in earlier loaded runs all
passed in this final suite. Service rustdoc also completed successfully on the final sequential run.

`cargo fmt --manifest-path C:/lw/wt/learn-370/Cargo.toml --all --check` and
`git -C C:/lw/wt/learn-370 diff --check` passed after the final Rust source changes.

Not verified: whole-workspace tests (driver-owned), app checks (no app changes), personal database
migration or destructive legacy redaction. Legacy feature provenance and the obsolete app gate remain
open as described above; jobs items belong to the other lane. No file/process capability was added.
