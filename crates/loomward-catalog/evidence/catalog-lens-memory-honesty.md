# Catalogue lens fixes, Windows 11 (KRASPYON), 2026-10-18

Base: `3ad18703c2f5c47aa676d99e932e69e09986b584`, branch
`fix/catalog-lens-memory-honesty`. The findings were originally against `4162325`;
all three paths remained present at this lane's newer base. The driver committed
the initial pass as `bbf5bfb53b9315ed17201b00e7b25db2dcc00026`; the throughput-trade
follow-up below is uncommitted. Initial pass: Rust `1.97.1 (8bab26f4f 2026-07-14)`, debug builds, four build jobs,
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
  long-name/rollup cases reopened the memory trade; the initial pass did not
  establish ordinary P4 throughput parity. The follow-up below settles the
  requested selection rule. No adaptive cache policy was added.
- An initial no-native-ID relist reproduced slow matching after its first publication
  (255.879 s) and exceeded 1.25 GiB RSS, but was stopped before completion and is not
  used as a completed measurement. Its owned synthetic fixture was removed. The
  `match_file` NULL-identity invalidation lookup is a candidate to investigate
  separately. It is outside the three requested
  fixes, and no matching semantics were changed here.

## Three-way throughput/memory decision (2026-10-18)

**Keep `temp_store=FILE`, 128 MiB main cache and 16 MiB TEMP cache.** Compared with
main, HEAD/FILE loses **2.77% median P4 insertion throughput**, below the requested
10% threshold. Independently, HEAD/MEMORY cannot keep the adversarial 1M publication
under approximately 600 MiB: all three long-name listing runs peak above **1,920 MiB**,
versus **351.95-353.90 MiB** for FILE. Both arms of the supplied retention rule hold.
No production code, setting or test changed in this continuation; the original
test-first FILE/cache pins and batched-rollup regressions remain intact.

This is a measured trade, not throughput parity: **through-EndRun P4 throughput is
22.25% lower** for HEAD/FILE than main. HEAD/MEMORY's through-EndRun median is lower
still. The rollup-only FILE workload also takes longer. The historical 37.5% P4c
rejection remains valid for that earlier fixture/settings; it is not erased by this
different, much larger adversarial memory benefit.

### Source, commands and order

- A: frozen `origin/main` = `ae2012c49bab12be5581f00b0152d33189b1391d` catalogue sources
  (256 MiB main cache, MEMORY, original heap rollup).
- B: HEAD = `bbf5bfb53b9315ed17201b00e7b25db2dcc00026` (128 MiB main / 16 MiB TEMP,
  FILE, batched postorder rollup and bounded reader caches).
- C: B with **only** the writer connection's `temp_store` changed to MEMORY;
  all other changes, including both cache settings, remain. Recorded source hashes
  confirm that only `src/db.rs` differs between B and C.

The same HEAD harnesses and lockfile are used for all variants. A's catalogue source
directory was loaded from a Git archive of the pinned main, built, and HEAD bytes
restored; B and the temporary C were then built and copied to separate executables.
All production source bytes were restored before measurement and the final gates.
A/B compares the requested snapshots, which also contain other catalogue changes;
it does not isolate a causal effect of the TEMP pragma. B/C isolates that setting.

All commands run from `C:/lw/wt/catalog-fix`, on Windows 11/KRASPYON, with
`CARGO_BUILD_JOBS=4` and `CARGO_TARGET_DIR=C:/lw/target/lane-catalog-fix`.
No target directory was cleaned or deleted. Release build, repeated for A/B/C:

```text
cargo build --release -p loomward-catalog --example bench --example catalog_perf --example memory_bounds
```

The historical P4 command in HANDOFF is
`cargo run -p loomward-catalog --release --example bench -- --rows 1000000`.
`catalog_perf` is the separately requested catalogue path example. After building,
each measured child is launched directly so compilation is outside its measurement:

```text
<variant>-bench.exe --rows 1000000
<variant>-catalog_perf.exe 1000000
<variant>-memory_bounds.exe rollup 1000000
<variant>-memory_bounds.exe listing 1000000
```

Three rounds, each in this exact order: **bench A/B/C, catalog_perf A/B/C,
rollup A/B/C, listing A/B/C**. Each child starts a fresh process and synthetic
database; own measurements and checks never overlap. Other host lanes remain
uncontrolled. The local invocations were:

```text
py -3 C:/lw/wt/catalog-fix/.loomward/temp-store-trade/build_variants.py
py -3 C:/lw/wt/catalog-fix/.loomward/temp-store-trade/measure.py
py -3 C:/lw/wt/catalog-fix/.loomward/temp-store-trade/curate.py
```

The memory runner reads Windows `K32GetProcessMemoryInfo` high-water RSS and
private-commit counters every 100 ms, using the same native counters/cadence as
the existing PowerShell wrapper. Its API/structure is checked against the live
runner process first. `bench` also records its native publication and EndRun
high-water counters internally. These are measured process peaks, not OS memory
caps. The decision uses the 600 MiB threshold; the original wrapper's separate
400 MiB threshold and earlier receipts are unchanged.

### Medians and spread

Every cell is the median and **[minimum-maximum] of three fresh processes**.
P4 insertion excludes EndRun on today's bulk path; EndRun includes the final swap,
index work and finalisation. `catalog_perf` rates use its 1M file count divided by
listing time, or listing plus EndRun time. Memory-case seconds include fixture
seeding/staging, both listing publications when applicable, and EndRun.

| Metric: median [min-max], three fresh processes | A: origin/main | B: HEAD / FILE | C: HEAD / MEMORY |
| --- | ---: | ---: | ---: |
| P4 insert rows/s (bench) | 194,533 [132,008-225,376] | 189,138 [114,764-203,589] | 170,916 [162,482-216,905] |
| Through EndRun rows/s (bench) | 60,624 [30,765-61,360] | 47,136 [32,067-49,895] | 43,972 [39,436-62,651] |
| Listing files/s (catalog_perf) | 311,448 [292,873-314,197] | 296,047 [192,197-343,316] | 392,511 [242,546-397,571] |
| Through EndRun files/s (catalog_perf) | 64,283 [54,772-84,597] | 62,708 [54,962-78,864] | 55,981 [52,796-83,640] |
| P4 publication peak RSS MiB (bench) | 110.89 [110.85-111.28] | 110.04 [109.57-110.34] | 110.74 [110.28-111.04] |
| P4 through EndRun peak RSS MiB (bench) | 546.80 [546.56-546.92] | 326.36 [326.33-326.47] | 403.59 [402.98-403.68] |
| Rollup peak RSS MiB | 509.30 [508.89-509.68] | 186.72 [186.57-186.90] | 325.69 [325.69-326.38] |
| Rollup peak private commit MiB | 520.72 [520.41-520.73] | 185.94 [185.58-186.77] | 330.04 [329.93-330.05] |
| Rollup seconds | 21.817 [16.434-22.499] | 54.448 [49.649-56.887] | 40.640 [40.215-45.392] |
| Long-name listing peak RSS MiB | 2,092.25 [2,086.54-2,093.14] | 352.64 [351.95-353.90] | 1,920.50 [1,920.47-1,923.41] |
| Long-name listing peak private commit MiB | 2,131.06 [2,130.19-2,137.38] | 414.04 [413.27-415.32] | 1,955.58 [1,953.12-1,961.27] |
| Long-name listing seconds | 91.494 [86.265-140.989] | 129.054 [84.010-321.192] | 109.203 [70.886-132.997] |

Unrounded per-run values, medians/ranges, source/harness/binary hashes and the
selection calculation are in [the curated receipt](catalog-temp-store-trade.json).
All **36 child processes exited 0**: nine `bench` runs passed accounting, indexes
and temporary-directory cleanup assertions; all eighteen memory cases passed
row-count/totals/depth assertions. The nine `catalog_perf` processes passed their
file-count and bounded-query assertions, but each recorded seven `ResourceBudget`
refusals for its large-root duplicate probe (63 total). Those refusals are not
successful duplicate queries; all other reader/refresh error lists were empty.

### Follow-up proving checks

Run after all measurements, on the restored HEAD production sources, with the
environment and working directory above:

- `cargo fmt --all --check`: PASS, exit 0.
- `cargo clippy -p loomward-catalog --all-targets -- -D warnings`: PASS, exit 0.
- `cargo test -p loomward-catalog`: **269 passed, 0 failed, 0 ignored**, exit 0.
- `git -C C:/lw/wt/catalog-fix diff --check`: PASS, exit 0.
- Receipt validation: 36 successful children, three samples per variant/workload,
  all stored accounting/totals assertions matched, and table medians/ranges agree
  with the unrounded receipt. A/B source hashes match their pinned Git revisions;
  B/C differ only in `src/db.rs`. Production sources remain identical to HEAD.

Logs are retained with the local raw measurements. No behavior change was selected,
so no new regression test or test mutation was needed in this follow-up.

### Scope and retained artifacts

The comparison settles the requested setting selection. It does not establish
250k through-EndRun P4, cold-cache or controlled-host performance, 10M scale,
whole-engine memory, Linux, hosted CI or physical crash behavior. FILE still
requires TEMP disk space. In particular its long-name elapsed times span
84-321 seconds, so a timing gain or universal throughput guarantee is not claimed.

The local runner scripts, main-source archive, copied release binaries and raw
stdout/stderr are retained under ignored `.loomward/temp-store-trade/` for the
driver. The curated synthetic receipt is tracked; no personal inventory is present.
`HUMAN_TODO.md` remains unchanged with no new owner action. No commit, push, PR,
merge, workspace or app checks; the driver owns integration and the workspace gate.
