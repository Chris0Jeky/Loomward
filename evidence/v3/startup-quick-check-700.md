# Startup integrity checks: issue #700

Measured on KRASPYON, Windows 11, 2026-10-19, from worktree branch
`perf/700-quick-check-measure`, based on `aa4f6a78`.
Synthetic data only; no real scan, file operation, personal inventory or soak access.

## Changed

The duplicate paths were `Catalog::validate_stores` and `db::open_pair_with`, both calling
`db::inspect`. Preflight now reads the catalogue's ownership, version and dataset metadata
without its full `quick_check`. The full catalogue check remains in `db::open_pair_validated`
(the original open implementation), which preserves and rebuilds corrupt derived data.

Preflight still checks state.db integrity before static-overlap validation. Its returned,
consuming `ValidatedStores` handle opens the same directory and dataset without repeating
that state integrity check. Header, version and dataset checks still run on open. Direct
`Catalog::open` and `open_with` still run full checks on both stores. This handle is for the
immediate open after preflight, not a persistent validation cache.

When the deferred (no-integrity) catalogue inspection fails with a dataset or SQL error,
`validate_stores` re-inspects with the integrity check, on that failure path only, so a corrupt
catalogue that previously masked those errors is still repaired at open, not refused.

No static-overlap check moved. Catalogue and service measurement examples are retained;
the service example is named `startup_measure` to avoid a Cargo example output collision.

## Regression evidence

The test-only, thread-local hook records executions immediately before `PRAGMA quick_check`.
Before the fix, `normal_start_checks_each_database_once` failed with **2** executions for
catalog.db; its trace also recorded **2** for state.db. After the fix it passes with **1**
execution per database, for synthetic and personal sessions, with and without static serving.

Other regression tests:

- `corrupt_catalogue_is_preserved_and_rebuilt_at_normal_start`: damage a data b-tree page,
  keeping header and schema readable; one catalogue integrity check detects the damage,
  archives the exact corrupt bytes, and rebuilds the catalogue.
- `corrupt_catalogue_is_refused_at_static_start_without_rewriting`: unreadable catalogue
  version still refuses static-server startup and leaves the corrupt bytes unchanged.
- `preflight_verdict_on_a_corrupt_catalogue_is_unchanged_by_the_deferred_check`
  (loomward-catalog `tests/open.rs`): a dataset mismatch is still refused on a healthy
  catalogue, and does not turn a corrupt one into a refusal.
- `state_integrity_failure_still_precedes_static_overlap`: state data-page corruption wins
  over a simultaneous static overlap, with unchanged corrupt state bytes.

## Release measurements at 1M files

The existing synthetic fixture was verified with `SELECT count(*) FROM file`: **1,000,000**.
catalog.db was **199,159,808 bytes**. Both binaries used the same fixture and harness.
`CARGO_BUILD_JOBS=4`, `CARGO_TARGET_DIR=C:\lw\target\lane-w12-700`.

The before command (which built the release harness and ran the initial three timings) was:

```powershell
cargo run --manifest-path C:/lw/wt/w12-700/Cargo.toml --release -p loomward-service --example quick_check_measure -- C:/lw/target/lane-w12-700/qc/n1000000 3 before
```

The after build was:

```powershell
cargo build --manifest-path C:/lw/wt/w12-700/Cargo.toml --release -p loomward-service --example startup_measure
```

The before harness was originally named `quick_check_measure`; its release executable was
saved outside Git before building the fixed `startup_measure`. After one excluded warm-up,
three pairs alternated the before and after executables, each with `<synthetic-1m-dir> 1
<label>`. Each invocation separately times `Catalog::validate_stores`, then `Service::open`;
the validation measurement is not included in the service-open measurement. The service
measurement includes its own preflight and all other normal startup work.

| Pair | Before validate (ms) | After validate (ms) | Before service open (ms) | After service open (ms) |
|---|---:|---:|---:|---:|
| 1 | 1697.2614 | 9.5596 | 6182.6018 | 2256.5903 |
| 2 | 2185.7597 | 11.6451 | 5386.4657 | 3087.3660 |
| 3 | 1642.8486 | 6.0740 | 6681.5031 | 1822.5339 |
| Median | **1697.2614** | **9.5596** | **6182.6018** | **2256.5903** |

The host was shared with other lanes, a soak and local proving checks. These are warm,
contended measurements, not an isolated performance guarantee. The initial before run
without a warm-up measured validation at 12976.3114 / 1898.9961 / 1499.0526 ms and service
open at 3705.6354 / 3175.3891 / 3469.5796 ms. Its first validation had uncontrolled cache
state and is not labelled a controlled cold measurement. No cache eviction was attempted.

Raw local receipts are under the worktree's ignored `target/w12-700-*` files; synthetic
databases and the saved before executable remain in the lane's external Cargo target.

## Verified

- Working directory: `C:/lw/wt/w12-700`; Cargo commands supplied its absolute manifest path.
- `cargo test -p loomward-catalog -p loomward-service`: **715 passed, 0 failed, 2 ignored**
  (catalogue: 313 passed, 1 ignored; service including root coverage: 402 passed, 1 ignored).
- `cargo fmt --all --check`: passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed after
  replacing the measurement example's manual divisibility expression with `is_multiple_of`.
- `git diff --check`: passed.
- Release service measurement example built and ran successfully before and after the fix.

## NOT verified

Controlled cold before/after timing; 3M or 10M after timing; another host; real personal
catalogue performance; UI or native shell; hosted CI. The existing ignored 1M-row detector
and 1 GiB streaming-memory probes remain ignored; no tests were disabled or weakened.

## Residual risk and handoff

Shared-host load affects the timings; execution counts are the deterministic regression
oracle. `HUMAN_TODO.md` is unchanged, and this issue
introduces no owner decision. Status/checkpoint files were intentionally left to the driver.
