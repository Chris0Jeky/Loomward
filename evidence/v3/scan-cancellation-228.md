# Scan cancellation ordering: #228

2026-10-10. Windows native execution with disposable synthetic fixtures.
Branch: `fix/scan-cancel-race-228`; starting head: `cf27a4ec6734026d1bb3a9d0b393c7e294e4eb60`.
No existing timeout changed. No commit or push performed.

## Cause and repair

The untouched concurrent-start test reproduced its zero-cancelled-jobs assertion failure.
The diagnostic batch retained the original assertion and captured job status immediately before
and after `scan_cancel_root`. Every one of its 15 failures showed:

```text
before: Queued, no error
after: Failed, coverage Partial, PermissionDenied: watch grant revoked
cancelled jobs: 0
```

The job was already registered: the preceding `root_jobs` assertion had passed. The existing
per-root gate serializes start through submission with revocation. The problem is the ordering
inside revocation: stopping the watch lets a queued runner enter `run_watched`, see the stopped
watch and fail before acquiring any scan bytes. The held `SCAN_BYTES` permit cannot hold this
early-exit path alive. The job then disappears from the active-job query.

`scan/mod.rs` now collects and requests cancellation of active jobs under that same per-root
gate **before** removing/stopping the watch. This marks the job's cancellation flag before watch
teardown can trigger its exit, so the registry records cancellation rather than failure.
Naturally terminal jobs remain excluded; submission and the revocation fence are unchanged.
The original concurrent-start test and its assertions remain unchanged.
The `granted_root` and `scan/pipeline.rs` regions edited by #225 are untouched.

## Loaded repetitions

Identical configuration for each batch, following the #201 method: three busy-loop Python
processes and one 64 KiB create/rename/delete loop with 32 rotating synthetic names in an owned
temporary directory. The harness, load children and test binary use affinity mask `15`.
Each invocation runs the exact named test with `--exact --nocapture`, recording exit code and
combined output. All load children exit and temporary churn is removed after each batch.

| Batch | Failures / runs | Observed rate |
| --- | --- | --- |
| Untouched baseline, original concurrent-start test | 11 / 60 | 18.33% |
| Diagnostic baseline, original assertion retained | 15 / 60 | 25% |
| Repaired source, original concurrent-start test | 0 / 60 | 0% |
| Repaired source, new deterministic ordering regression | 0 / 60 | 0% |

These are measured batch rates, not estimates of field incidence.

## Regression and mutation

`scan::watch::native_tests::root_cancellation_requests_job_before_stopping_watch` holds a
submitted scan runner with a channel, independent of native scan byte acquisition. Its interrupt
callback records whether the watch is still armed when cancellation is requested. It also checks
the returned job identity and `CancelRequested` state, stopped/removed native watch and eventual
`Cancelled` terminal state. Only existing fixture, registry and watcher APIs are used.

The new test failed against the original product ordering (exit 101, runtime ordering assertion),
then passed with the repair (exit 0). Explicit mutation: reverted only the product fix, observed
the same runtime assertion failure (exit 101), restored it and reran the test successfully
(exit 0). Compilation failures do not count as mutation proof.

Raw batch logs, JSON counts, frozen test binaries, local harnesses and mutation logs are retained
in gitignored `.loomward/race228/`. The public receipt includes only synthetic state and counts.

## Gates

All commands exited 0 on the repaired source. Tests used their default parallel runner.

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | PASS |
| `cargo test --workspace` | 428 passed, 0 failed, 0 ignored |
| `cargo test -p loomward-windows -p loomward-engine --all-features` | 151 passed, 0 failed, 0 ignored |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `git diff --check` | PASS |

Gate output and exit codes are retained locally as `gate-*.log` and `gates.json`.

## NOT verified

Hosted CI, non-Windows execution, elevated-host execution, personal-root scans and native desktop
flows were not run. No native file/process effect capability is added.

## Residual risk

Zero failures in 60 repetitions does not prove absence of every scheduler race. Root revocation
still waits for an in-progress start to leave its per-root gate and for existing watch shutdown.
