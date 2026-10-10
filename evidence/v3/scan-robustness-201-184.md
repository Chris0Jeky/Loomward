# Scan robustness: #201 and #184

Measured 2026-10-10 on Windows, non-elevated, with disposable synthetic NTFS fixtures.
Branch: `fix/scan-start-race-and-burst-flake`; starting and final HEAD:
`f910c89372e827d42791b25a9de30b87720663fc`. The Rust diff is uncommitted; no push occurred.
Other lanes shared the host throughout. No existing timeout or reconciliation limit changed.

## Changed and cause

**#201 is a test assumption defect.** The untouched baseline panicked at
`crates/loomward-engine/src/scan/watch.rs:1181`:

```text
old and new rename parents must both be relisted
```

A second batch retained that assertion but moved it after the fresh-scan oracle and added
test-only diagnostics. All 13 failures recorded channel overflow, and all reached the
successful names/identity/attribute, totals and ancestor-rollup comparisons before panicking.
Representative failure (`diagnostic-009.log`):

```text
burst move_sub=false channel_overflows=1
scopes=[FullRoot, FullRoot, FullRoot, Targeted([2]), Targeted([1]), FullRoot]
fresh rows and sums match
```

The bounded 16-batch native-to-dispatch channel drops hints under load. The existing dispatcher
records `RootDirty`, so reconciliation correctly performs full passes rather than the one
`Targeted` pass containing both rename parents that the test previously required.

- `scan/watch.rs:31`, `:149`: retain a test-only channel-overflow count in `Dirty`.
- `scan/watch.rs:33`, `:477`: retain actual full-relist count, excluding baseline and final
  publication (both also use `FullRoot` labels). These fields/increments are absent outside tests.
- `scan/watch.rs:1328`, `:1416`: accept either the existing targeted parent reconcile or an
  actual full relist with recorded channel overflow. All fresh-scan, complete-coverage, rollup,
  absence and stopped-watch assertions remain. Diagnostics print counters and scopes.

**#184 is a start/revocation ordering defect.** The old `root_jobs` check preceded worker
reservation, watch replacement and submission without one per-root reservation. The barrier
regression failed on the original implementation: the losing start blocked in the sink probe
instead of returning `Busy`. Revocation-before-start also reached the forbidden sink probe.

- `src/lib.rs:64`, `scan/mod.rs:202`, `:223`: a short global map lookup gets a per-root mutex;
  start reserves it with `try_lock` before any watch probe/stop/start and holds it through submit.
  A concurrent loser returns `Busy` without touching the watch or reserving workers.
- `scan/mod.rs:391`: revocation takes that same per-root gate, permanently fences the root id,
  then stops/removes its watch and cancels its submitted job. A start already in progress is
  observed and cancelled; a stale grant presented after revocation is refused.
- `scan/watch.rs:1039`: scope the existing global-map probe assertion to its start caller,
  excluding asynchronous job/dispatcher callbacks.

## Loaded repetitions

Identical harness configuration in every batch: three busy-loop Python processes plus one
64 KiB create/rename/delete loop using 32 rotating names in an owned temporary directory.
The harness, load children and test binary share affinity mask `15` (four logical CPUs),
leaving the remaining CPUs available to other lanes. Each run executes the exact burst test
with `--exact --nocapture`, retaining stdout, stderr and exit code. No load process touches
outside its temporary directory; all owned load children exit and the churn directory is removed.

| Batch | Failures / runs | Rate |
| --- | --- | --- |
| Untouched baseline | 26 / 60 | 43.33% |
| Diagnostic baseline, original assertion retained | 13 / 60 | 21.67% |
| Final source and test binary | 0 / 60 | 0% observed |

Two preliminary repaired binaries also passed 60/60 each, before the final proof was refined
to count actual relists. Their 120 passes are separate from the final-source batch above.
The forced four-core saturation deliberately reproduces more failures than the issue's
historical 3-7% shared-host rate; these are measured batch rates, not an estimate of field risk.
Final batch: **72 / 120 subtree cases recorded channel overflow**; every case
matched the fresh-scan oracle. Each successful test invocation covers both move variants.

## Verified

Four new discriminating tests:

- `scan::watch::native_tests::concurrent_scan_start_reserves_root_before_touching_watch`:
  barrier release, blocked winner probe, losing `Busy`, exactly one job, armed winner watch
  stopped and removed by `scan_cancel_root`.
- `scan::watch::native_tests::revocation_during_start_cancels_the_reserved_watch`:
  revocation during the pre-watch probe observes and cancels the submitted job; no watch remains.
- `scan::watch::native_tests::revoked_root_cannot_recreate_watch_from_stale_grant`:
  start and refresh fail before any sink/watch probe, even before the separate writer fence;
  root rename succeeds with no leaked pins.
- `scan::watch::native_tests::overflow_fallback_requires_overflow_and_a_full_relist`:
  publication alone, an unrecorded overflow, or one targeted parent cannot satisfy the fallback.

The existing deterministic forced-overflow test also asserts at least two actual full relists.
Native execution ran here; the elevated-host refusal branches were not used.

Final-source gates (exit 0 throughout):

| Command | Result | Shared-host seconds |
| --- | --- | --- |
| `cargo fmt --all --check` | PASS | 0.68 |
| `cargo test --workspace` | 414 tests passed | 41.99 |
| `cargo test -p loomward-windows -p loomward-engine --all-features` | 147 tests passed | 12.89 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS | 2.51 |
| `git diff --check` | PASS | 0.10 |

**Mutation proof: 7/7 runtime assertion failures (exit 101), each followed by restoration
and a passing affected test (exit 0).** No compilation failures count as mutation proof.
Removed separately: start reservation, revoked flag, cancellation serialization, overflow
fallback, recorded-overflow requirement, actual-relist requirement, and actual-relist counter.
The cancellation-serialization mutation returned zero cancelled jobs instead of one, proving
the exact pre-installation watch gap. All mutations were restored before final gates/loops.

Raw logs, result JSON and harnesses are gitignored in `.loomward/scan-robustness-proof/`.
`before-*.log` and `diagnostic-*.log` retain the assertion/stderr; `after-proof-final-*.log`
contains final loaded evidence; `gate-*.log` and `mutations.json` retain the gate/mutation proof.
The frozen binaries remain in `%TEMP%/lw-flake-proof/` for local replay and are not publication
artifacts. No load child survives either harness exit.

Recommended commits:

1. `Accept full-root burst reconciliation after watcher overflow`
2. `Reserve scan roots before starting watches and serialize revocation`

## NOT verified

No hosted CI, Linux execution, elevated-host execution, personal-root scan, native desktop/UI
flow, hung-device I/O, or job-thread-spawn failure injection. Windows workspace tests include
the durable service suite, but no new durable-service concurrency scenario was added here.

## Residual risk

Zero failures in 60 final repetitions does not prove absence of all scheduler races. Under
continuous churn, bounded reconciliation can still return partial coverage. Revocation for the
same root waits for an in-progress start to leave its per-root gate; other roots do not share
that gate. The revocation fence lasts for the engine lifetime and the revoked root id; a new
grant gets a new root id. No persistent watcher or new native effect is introduced.
