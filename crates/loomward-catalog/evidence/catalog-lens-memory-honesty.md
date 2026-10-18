# Catalogue lens fixes, Windows 11 (KRASPYON), 2026-10-18

Base: `3ad18703c2f5c47aa676d99e932e69e09986b584`, branch
`fix/catalog-lens-memory-honesty`. The findings were originally against `4162325`;
all three paths remained present at this lane's newer base. Changes are uncommitted
for the driver. Rust `1.97.1 (8bab26f4f 2026-07-14)`, debug builds, four build jobs,
own target directory `C:\lw\target\lane-catalog-fix`. No dependencies or persistent
schemas changed. No physical scan or owner metadata was used.

## Changed

- Set TEMP storage to FILE with a 16 MiB cache before recovery or TEMP views.
  Limit the main cache to 128 MiB: bundled SQLite's `sqlite3VdbeSorterInit`
  sizes an additional sort buffer from the main cache, not the TEMP cache.
  Listing names, old-file reconciliation and sorting can spill to disk.
- Replace the directory heap graph with a file-backed depth table and postorder
  batches of at most 1,024 IDs. Parent-index lookups avoid scanning the entire root
  per directory. Existing signed aggregate bounds, depth bounds and transaction
  boundaries remain; scratch exhaustion returns SQLITE_FULL and rolls back.
- Retained files inherit `excluded` coverage from their directory. Ordinary
  observed files in an incomplete directory retain their existing coverage rules.
- Vacuum recreates TEMP views only when changing storage mode actually dropped them.

## Test-first evidence

Commands run from this worktree with `CARGO_BUILD_JOBS=4` and the target above:

- `cargo test -p loomward-catalog --lib listing_scratch_is_file_backed_with_a_bounded_cache -- --nocapture`:
  **red**, 0 passed / 1 failed (`temp_store` was 2, expected 1); **green**, 1 passed.
  An added main-cache budget assertion also failed at -262144 before reducing it
  to -131072; the final suite includes both cache bounds.
- `cargo test -p loomward-catalog --test catalog_findings retained_file_under_a_placeholder_keeps_excluded_coverage -- --nocapture`:
  **red**, 0 passed / 1 failed (`complete`, expected `excluded`); **green**, 1 passed.
- The original directory graph exceeds the memory ceiling at one million directories
  (see the completed before/after receipts linked below), before replacing rollup.
- The postorder test's 10-million-VM-instruction ceiling failed with `OperationInterrupted`
  before adding parent-index hints, then passed after them. It also checks >2 batches,
  reparented depths, exclusions, hidden absent rows and aggregate-overflow rollback.
- A separate scratch-disk-full test verifies unchanged committed totals and successful
  restart recovery. The existing vacuum test initially caught duplicate TEMP view creation
  after switching to FILE; the conditional recreation fixes it without weakening its checks.

## Verified

- `cargo test -p loomward-catalog`: **269 passed, 0 failed, 0 ignored**.
- `cargo clippy -p loomward-catalog --all-targets -- -D warnings`: PASS.
- `cargo fmt --all --check`: PASS.
- `git -C C:/lw/wt/catalog-fix diff --check`: PASS.
- `cargo test -p loomward-catalog --features test-hooks --test crash_sweep -- --test-threads=1`:
  **10 passed, 0 failed**, including double-crash recovery, lifecycle and migration.
  This sampled sweep covered the file-backed/batched implementation before the final
  parent-index hints and cache reduction; those final changes are covered by the
  full catalogue suite, VM budget and completed memory workloads.

## Memory experiment

Build: `cargo build -p loomward-catalog --example memory_bounds`.
Run the wrapper separately for the original and changed production sources:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File crates/loomward-catalog/examples/memory_bounds.ps1 -Executable <binary> -Output <receipt.json> -Rows 1000000
```

The baseline binary uses the base's `db.rs`, `writer.rs` and `query.rs` with the same
benchmark harness. Each case starts a fresh process. The wrapper reads Windows'
process peak-working-set counter every 100 ms, reports bytes, checks the 400 MiB
ceiling, and preserves the child's exit code and totals assertions.

- **rollup:** directly seed one million directory rows with deliberately wrong cached
  depths, then call EndRun and assert totals and corrected depths. This isolates
  finalisation from enumeration and the live engine arena.
- **listing:** stage one million 255-byte ASCII names with unique synthetic native
  identities, publish, then stage and publish the same listing again. Producer
  chunks have 1,024 entries and use the writer's existing byte-budgeted queue.
  Both passes assert exactly one million live rows; EndRun asserts root totals.

| Workload (1M rows) | Before peak RSS (MiB) | After peak RSS (MiB) | Before / after seconds |
| --- | ---: | ---: | ---: |
| rollup | 509.85 | 186.93 | 69.62 / 93.78 |
| listing | 2086.54 | 353.66 | 348.16 / 256.86 |

Both baseline children and both changed children exited 0 with matching totals.
The baseline wrapper failed both memory ceilings as expected; the changed wrapper
passed both. Changed peaks also remain below 400,000,000 bytes (decimal 400 MB).
Times include fixture seeding/staging, publication and EndRun; shared-host single
runs do not establish throughput causality or ordinary P4 qualification.

Exact measurement commands (from this worktree):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:/lw/wt/catalog-fix/crates/loomward-catalog/examples/memory_bounds.ps1 -Executable C:/lw/wt/catalog-fix/.loomward/memory-bounds-before.exe -Output C:/lw/wt/catalog-fix/crates/loomward-catalog/evidence/catalog-lens-memory-before.json
powershell -NoProfile -ExecutionPolicy Bypass -File C:/lw/wt/catalog-fix/crates/loomward-catalog/examples/memory_bounds.ps1 -Executable C:/lw/target/lane-catalog-fix/debug/examples/memory_bounds.exe -Output C:/lw/wt/catalog-fix/crates/loomward-catalog/evidence/catalog-lens-memory-after.json
```

Raw completed receipts: [before](catalog-lens-memory-before.json),
[after](catalog-lens-memory-after.json). The baseline wrapper is expected to exit
nonzero for exceeded memory bounds; successful child exits distinguish that from
an incomplete or failed workload.

## NOT verified / residual risk / left open

- No whole-workspace or app checks, commits, push, PR or merge: this is a scoped Sol
  lane. The driver owns integration. `HUMAN_TODO.md` is unchanged; no owner decision
  or safety gate is opened or closed by this change.
- These are writer-only one-million-row measurements on a shared host, not a 10M-entry
  full-engine/private-commit qualification, cold-disk benchmark or physical crash test.
  File-backed scratch requires temporary disk space and can add I/O; disk-full
  rollback/restart is tested rather than relaxing publication atomicity.
- The historical P4c section of `../HANDOFF.md` records a rejected FILE-cache
  policy with a 37.5% median P4 loss on a different fixture. These adversarial
  long-name/rollup cases reopen the memory trade; they do not establish ordinary
  P4 throughput parity. No adaptive cache policy was added.
- An initial no-native-ID relist reproduced slow matching after its first publication
  (255.879 s) and exceeded 1.25 GiB RSS, but was stopped before completion and is not
  used as a completed measurement. Its owned synthetic fixture was removed. The
  `match_file` NULL-identity invalidation lookup is a candidate to investigate
  separately. It is outside the three requested
  fixes, and no matching semantics were changed here.
