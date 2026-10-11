# First-scan bulk load — #148 (owner decision q-13)

11 October 2026, native Windows, non-elevated, warm/uncontrolled cache, hardware withheld.
Synthetic fixtures only; no real-folder scan was run in this lane. Branch
`perf/first-scan-bulk-load-148`, based on main `6e6682d`. Aggregate receipts append to
[first-scan-bulk-load-148.json](bench/first-scan-bulk-load-148.json); task-owned raw logs,
diagnostic receipts, mutation runner and frozen binaries stay in the ignored
`.loomward/bulk148/` of the lane's worktree.

Owner decision q-13: the KPI is end-to-end files/s through the service on lab and real trees
(250k rows/s kept as a stretch note); a first-scan bulk path builds with indexes deferred and
publishes atomically under the existing fence, A/B'd on a quiet host.

## KPI

`scripts/bench_scan_throughput.py` now makes files/s first-class. Every receipt gains a `kpi`
block (files/s by engine elapsed and by HTTP wall, the main phase seconds, transactions) and the
CPU percent sampled for 30 s before the scan, labelled `quiet host` below 25 percent and
`shared host` otherwise. `--max-cpu 25` refuses to measure on a busy host; `--summarize RECEIPT
[--first-index N]` prints per-variant medians and ranges. The scan itself is unchanged: a fresh
`loomward-serve` per run, `scan.start` over HTTP, `jobs.get` to completion, an independent disk
walk before and a committed `tree.slice` oracle after.

## Go/no-go diagnostic (shared host, design input only)

Before coding, the frozen main service scanned the 100k lab tier (100,000 files, 1,000
directories, 12,800,000 bytes) against seeds whose catalogue had its four `file` secondary
indexes replaced by one `file(dir_id)` index, same binary. Three interleaved pairs; elapsed
seconds and writer COMMIT seconds:

| Pair | Main elapsed | No deferred indexes elapsed | Main COMMIT | No-index COMMIT |
|---|---:|---:|---:|---:|
| 1 | 5.260 | 1.434 | 2.709 | 0.382 |
| 2 | 3.307 | 1.589 | 1.620 | 0.414 |
| 3 | 4.449 | 2.169 | 2.859 | 0.951 |

Dropping only `file_identity` gave 2.253/1.701 s against main 5.929/3.755 s. Both runs have
2,008 transactions. The identity key is a little-endian NTFS file ID, so its index takes
randomly placed inserts and every listing commit rewrites about one index leaf per file. Index
maintenance is the dominant writer cost on this KPI, so the design could pay; it was built.
These are shared-host observations used only to decide whether to build.

## Design and implementation

Design note: [catalogue HANDOFF](../../crates/loomward-catalog/HANDOFF.md), section "First-scan
bulk load (#148, owner decision q-13): design". In short: an opt-in writer flag (set by the
service and the P4 bench) marks a `full` run of a root with no earlier run and no live rows at
`BeginRun`. Its listings run the unchanged publication code against `bulk_dir`/`bulk_file`, which
open creates from the live definitions without the four file secondary indexes. IDs come from the
live sequences, shared both ways around every bulk publication. `EndRun` completed or cancelled
swaps the build into the live tables inside its savepoint, building the file indexes once when the
build is at least as large as the live file table; failed runs, revocation and reopen discard the
build. The service's child-row lookup also reads `bulk_dir`.

## Tests and mutations

Six new catalogue tests (`crates/loomward-catalog/src/writer/bulk_tests.rs`):

- **Equivalence.** The same first scan through both paths, completed and cancelled, alone, beside
  a larger ordinary refresh of another root (ordered-insert branch, shared IDs) and beside a
  second concurrent first scan. Every catalogue table row (instance ID aside), the schema, every
  writer reply, and the reader API's slices, breakdowns, searches, duplicate candidates, child
  pages, paths and node details are compared line by line: all equal. The scan covers staged and
  direct listings, excluded, identity-less and lossy names, a hard link, a partial listing, a
  stale publication and its relist, and a final.
- **Atomicity.** A separate reader after every committed command of the build sees only the root
  directory and an empty child page; a read snapshot opened before EndRun keeps the empty root
  after the EndRun commit; a new read then sees all 5 directories and 11 files, with all four file
  indexes rebuilt.
- **Crash between build and swap.** Dropping the writer after the last committed build command
  leaves nothing visible; reopen leaves the root directory only, run failed, root partial, bulk
  tables empty, indexes intact.
- **Swap commit failure.** A deferred foreign-key fault fails the EndRun COMMIT: nothing is
  visible, the writer closes, reopen discards the build.
- **Revocation mid-build** discards the build and cancels the run; EndRun is then refused.
- **Failed run and second scan.** A failed EndRun discards the build; after either a discarded or
  a completed first scan, the next scan of the root publishes each listing incrementally.

Two service tests that inspected first-scan rows mid-run were adapted, not weakened: the fixture's
child lookup reads `bulk_dir` as the sink does, and the N1 test now asserts nothing is visible in
`file` before EndRun, the stale/relist behaviour in the build, and the published row afterwards.

Fourteen mutations, each restored byte for byte, all killed by executed tests (exit 101, no
compile failures): never build in bulk; ignore the prior-run guard; skip the swap; omit sequence
sharing before, or after, a bulk publication; drop indexes without rebuilding; keep builds across
open; keep the build on revocation; publish a failed run; retire references for bulk listings;
treat every swap as the only build; the sink ignoring `bulk_dir` (four Windows lab-scan service
tests fail); the service without the flag (the N1 test fails).

A one-off probe forced the flag on for the whole catalogue suite: every reader-query test file
(`queries`, `query_bounds`, `query_mapping`, `reader_guards`, `open`) passed unchanged on the
bulk path; the failures were tests that read live rows before EndRun, as designed. The probe was
reverted and is not a gate.

## Gates (final, branch head `225fac1`)

| Command | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo test --workspace` | exit 0; 843 passed, 0 failed, 0 ignored |
| `cargo test -p loomward-catalog --all-features` | exit 0; 159 passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | exit 0 |
| `py -3 scripts/verify.py` | exit 0; 227 Python tests (1 existing symlink-privilege skip), JS syntax, 9 JS boundary and 80 parity fixtures, Rust format/tests/lint |
| `npm.cmd --prefix app run build` | exit 0 |
| `py -3 scripts/test_app.py --live-serve` | exit 0; ALL PASS (live serve) |
| `git diff --check` | exit 0 |

During development one full `cargo test -p loomward-service` run failed
`meaning_work_budget_is_an_honest_prefix` at an `ok()` call; its message was not captured. It
passed alone three times and in six later full runs, including the final gate. That handler has a
2 s deadline, which a loaded shared host can exceed; the cause is not proven.

## A/B measurement: UNVERIFIED (host never quiet)

The host stayed busy with other lanes. The lane's driver (`.loomward/bulk148/ab.py`) samples CPU
for 30 s and measures only below 25 percent; every sample from 19:48 onwards read 26.8–67.4
percent, and the one run that started was refused by `--max-cpu 25` at 32.2 percent. No A/B
receipt was recorded and no speedup is claimed.

Shared-host pilots, excluded from any comparison: two lab pairs gave main 2.816/2.653 s against
branch 1.576/1.675 s, with writer COMMIT 1.294/1.403 s against 0.464/0.564 s; one 1M P4 bench
gave branch 4.65 s through the build plus 6.62 s EndRun (88,349 rows/s through EndRun), with the
swap's copy and four index builds about 2.7 s of that EndRun. A same-time main 1M run took
31.9 s on that overloaded host and is not a comparable baseline.

Exact next step on a quiet host (frozen binaries: main `6e6682d`, branch `225fac1`, in ignored
`.loomward/bulk148/{main,branch}/`; the 100k lab and its seed state in `C:/Users/jekyt/wt/lab148`,
recreated with `scan_throughput_fixture STATE ROOT` if gone):

```sh
# 3 interleaved pairs of the lab KPI, each refused unless the 30 s CPU sample is below 25 percent
for i in 1 2 3; do for v in main branch; do
  py -3 scripts/bench_scan_throughput.py --max-cpu 25 --exe .loomward/bulk148/$v/loomward-serve.exe \
    --variant $v --root ../lab148/root --seed-state ../lab148/seed --state .loomward/bulk148/ab-$v-$i \
    --receipt evidence/v3/bench/first-scan-bulk-load-148.json --source "$(cat .loomward/bulk148/$v/SOURCE)"
done; done
py -3 scripts/bench_scan_throughput.py --summarize evidence/v3/bench/first-scan-bulk-load-148.json
# 3 interleaved pairs of the P4 microbench; compare P4.through_end_run_rows_per_second
# (main lacks the field: entries / (P4.insert_seconds + finalise_seconds))
for i in 1 2 3; do for v in main branch; do .loomward/bulk148/$v/bench.exe --rows 1000000 > .loomward/bulk148/ab-p4-$v-$i.json; done; done
```

`py -3 .loomward/bulk148/ab.py MINUTES` runs both sets, waiting up to MINUTES for a quiet host.
