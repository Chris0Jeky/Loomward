# First-scan bulk load — #148 (owner decision q-13)

11 October 2026, native Windows, non-elevated, warm/uncontrolled cache, hardware withheld.
Synthetic fixtures only; no real-folder scan was run in this lane. Branch
`perf/first-scan-bulk-load-148`, based on main `6e6682d`. Quiet-host aggregate receipts will append to
`bench/first-scan-bulk-load-148.json` (none yet; see the A/B section); task-owned raw logs,
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

Thirteen mutations, each restored byte for byte, all killed by executed tests (exit 101, no
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

## Catalogue size (P14): a measured regression

File size does not depend on host load, so this one comparison stands on the shared host. The 1M
bench, main `6e6682d` against this branch: **155.0 bytes/entry** (155,176,960 B) against
**229.9 bytes/entry** (230,080,512 B), with a target of 200. Main meets P14 and this branch does
not. The build is written once into `bulk_*` and copied once into `file`. The dropped build's
pages stay on the freelist and nothing reclaims them, so the file keeps both copies' worth until
later writes reuse those pages. Tracked in #386.

`P4.met` is judged on `through_end_run_rows_per_second` (`met_basis` in the receipt), because
`insert_seconds` no longer includes the swap and index builds. The same run read 218,593 rows/s
insert-only and 108,702 through EndRun on the shared host, so `met` is false.

## A/B measurement: UNVERIFIED (host never quiet)

The host stayed busy with other lanes. The lane's driver (`.loomward/bulk148/ab.py`) samples CPU
for 30 s and measures only below 25 percent; every sample from 19:48 onwards read 26.8–67.4
percent, and the one run that started was refused by `--max-cpu 25` at 32.2 percent. No A/B
receipt was recorded and no speedup is claimed.

Shared-host pilots, excluded from any comparison: two lab pairs gave main 2.816/2.653 s against
branch 1.576/1.675 s, with writer COMMIT 1.294/1.403 s against 0.464/0.564 s; one 1M P4 bench
gave branch 4.65 s through the build plus 6.62 s EndRun (1,001,001 entries in 11.27 s, about 88.8k rows/s through EndRun; an earlier figure of 88,349 rows/s does not reconcile with these rounded times), with the
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

## Follow-ups #386 — 17 October 2026

Native Windows 11, KRASPYON, shared host, synthetic metadata only. Baseline is current main
`04cb397ee2604113acfa0babe29dc1b0827d9b27`, not the pre-search-index main above. Work is committed
on `fix/386-first-scan-bulk`. Initial commands used `CARGO_BUILD_JOBS=4`; after the
driver's memory-pressure interruption all commands use `CARGO_BUILD_JOBS=3` and the lane-owned
`CARGO_TARGET_DIR=C:/lw/target/bulk-386` from `C:/lw/wt/bulk-386`.

### Changed

- A successful bulk `EndRun` (including failed/cancelled run finalisation) attempts `VACUUM main`
  after publication commits, before the existing non-waiting WAL checkpoint. It runs only when
  the freelist holds at least 1 MiB and at least 10% of the catalogue's pages. It never runs on
  ordinary incremental `EndRun`s. Lock waiting is disabled during maintenance and the original
  timeout is restored. A maintenance error cannot turn committed scan rows into a failed receipt.
  `checkpoint_seconds` includes the compaction cost.
- The in-memory bulk-run map is restored when a command's savepoint fails. Only the four command
  types that change this map take a snapshot; listing batches gain no map copy. A failed outer
  transaction still closes the writer and recovery remains unchanged.
- The existing P4/P14 bench additionally reports free pages and the search index's occupied bytes
  (`dbstat`, as in `search_bench`), so a remaining size miss can be distinguished from unused pages.

### Regression evidence

Before the production change:

```text
cargo test --manifest-path C:/lw/wt/bulk-386/Cargo.toml -p loomward-catalog --lib writer::bulk_tests::bulk_ -- --nocapture
1 passed, 3 failed, 65 filtered out; exit 101
```

- `bulk_map_follows_begin_savepoint_rollback`: retry got `NotFound` after the rolled-back run ID
  was reissued to an incremental refresh. The fault rejects one actual SQLite savepoint RELEASE
  through its authorizer; rusqlite rolls back the savepoint on drop.
- `bulk_map_follows_end_savepoint_rollback`: a listing after a rejected EndRun RELEASE appeared
  live (`1` file, expected `0`) instead of remaining in the rolled-back bulk build.
- `bulk_end_reclaims_large_freelists_without_changing_rows`: 30,000 fixture files left 1,125 unused
  pages after EndRun.

After the fix, the same command passed all 4 tests (the existing full equivalence test included).
The size test was then strengthened to require zero free pages, retain a pre-publication reader
snapshot, and keep a second root's unpublished build through compaction and its later publication.
It also checks row counts, logical-byte totals, SQLite integrity and FTS integrity.

### Measurement

Existing bench command for each variant:

```text
cargo run --release --manifest-path C:/lw/wt/bulk-386/Cargo.toml -p loomward-catalog --example bench -- --rows 1000000
```

There are 1,000,000 files and 1,001 directories. The first baseline ran before any production edit;
its binary is retained in ignored `.loomward/bulk386/baseline-bench.exe`. Raw benchmark receipts
and logs remain in ignored `.loomward/bulk386/`; the portable measurement receipt is
[`bench/bulk-load-followups-386.json`](bench/bulk-load-followups-386.json).

| Variant | Catalogue bytes | B/entry | Insert s | EndRun s | Maintenance s |
|---|---:|---:|---:|---:|---:|
| Current main | 311,488,512 | 311.177 | 15.058 | 25.944 | 0.158 |
| Compaction | 228,229,120 | 228.001 | 16.012 | 30.775 | 4.825 |
| Compaction, instrumented repeat | 228,229,120 | 228.001 | 5.475 | 12.504 | 1.830 |
| Compaction, 2M files | 456,900,608 | 228.222 | 10.707 | 30.129 | 4.330 |

The first comparison reclaimed 83,259,392 bytes (26.73%). EndRun increased by 4.832 s and
maintenance by 4.667 s in these observations. Host load is uncontrolled: no throughput improvement,
cold-cache result or quiet-host latency comparison is claimed. The faster instrumented repeat
also demonstrates that elapsed-time comparisons are load-sensitive here.

The instrumented 1M run had **zero freelist pages** and 81,403,904 bytes in the mandatory
`search_name` tables (81.323 B/entry); the remaining catalogue occupies 146.678 B/entry. The 2M
run also had zero free pages and 162,783,232 search-index bytes. Its 2,000,000 files plus 2,001
directories are the largest row count the existing benchmark accepts (`--rows 2000000`). Both
instrumented runs matched their independent count/byte oracle and removed their temporary stores.

Process high-water through publication and queries was 573,161,472 bytes (546.6 MiB) at 1M and
835,276,800 bytes (796.6 MiB) at 2M. A 500 ms monitor of each entire process, including its
isolated index probe, sampled the same peaks. Runs were serial and monitored with a 3 GiB kill
bound on the owned benchmark only; the 2M process exited 0. Both receipts were complete and stderr
was empty. The native high-water counters exclude the isolated index probe; the full-process
monitor is sampled, so it can miss short peaks. The 2M limit was retained instead of changing
the benchmark to attempt 10M on the shared 16 GB host.

**P14 remains open:** 228.001 B/entry exceeds the 200 B/entry target. The compulsory trigram
search index was added after #386's original 229.9 B/entry baseline (see
[`search-v1.md`](search-v1.md), size table); changing its representation is outside this slice.
The freed-build-page regression is fixed; the 10M size and peak-memory target is unverified.

### Failed/interrupted first scan: preserve the documented decision

No behavior change. The catalogue HANDOFF's first-scan design explicitly says failed EndRun
discards its build and leaves an empty partial root; its crash-safety section says reopen discards
an unpublished build. Existing tests
`a_failed_run_discards_the_build_and_the_next_scan_is_incremental` and
`a_crash_between_build_and_swap_leaves_nothing_visible_and_reopen_discards_the_build` pin that behavior. Cancelled
EndRun continues to publish its partial build atomically. No new owner/product decision is inferred.

### Gates and limits

Final gates (driver, same host): `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test -p loomward-catalog` (all suites green: lib 69 passed) and `git diff --check` pass. No personal scan, HTTP/app/native-shell change, publication or commit.
`HUMAN_TODO.md` was inspected and left unchanged; q-13 is the existing authority for the bulk path.
VACUUM rewrites the catalogue, needs temporary disk space and can increase peak WAL/disk usage;
a busy/full/error case leaves compaction undone and the committed scan intact. It runs synchronously,
so cancellation during EndRun also waits for this maintenance. Physical power-cut and actual IOERR
injection are not verified by the authorizer fault or SQLite integrity checks.


---

## 2026-10-22 - #386 / P14: compact sort keys and trigram document metadata
Baseline source: `2984f0d7ddd879027ad66ab59852aace8358fb7a`, clean at entry. Windows,
`rustc 1.97.1`, `cargo 1.97.1`. The baseline was measured before production edits; only
per-object `dbstat` instrumentation was added to the existing benchmark. No dependencies changed.

**Result: P14 remains open.** The same 1,000,000 files plus 1,001 directories (1,001,001
entries) now occupy **204,161,024 bytes / 203.957 B per entry**, down from
**228,241,408 bytes / 228.013 B per entry**: **24,080,384 bytes (10.55%) saved**.
The remaining gap is **3.957 B per entry**. No query, index access path, search tier or native
identity encoding was removed. The optional ranked token index remains optional and is absent
from both variants of this existing workload; this is not a measurement with ranked search enabled.

### Measurements and timing boundary

Build/run commands, from the worktree root:

```powershell
$env:CARGO_BUILD_JOBS='6'
$env:RUST_TEST_THREADS='4'
$env:CARGO_TARGET_DIR=Join-Path (Get-Location) '.loomward/i386/target'
$env:TEMP=(Resolve-Path .loomward/i386/tmp).Path
$env:TMP=$env:TEMP
cargo build --release -p loomward-catalog --example bench
# Preserve each variant's executable before changing production code.
& ./.loomward/i386/baseline-bench.exe --rows 1000000
cargo run --release -p loomward-catalog --example bench -- --rows 1000000
# The same changed executable, for subsequent alternating runs:
& ./.loomward/i386/after-bench.exe --rows 1000000
```

The initial build inherited an external Cargo target directory before it was detected. Subsequent
builds/tests explicitly used the worktree-local target above; no global configuration was edited.
The baseline executable was copied in read-only from that initial build. Raw JSON/logs and the
retained executables are in ignored `.loomward/i386/`. The baseline binary also saved a synthetic
catalogue copy for isolated SQL probes, after all timed scan/query phases; the permanent benchmark
still removes its temporary stores. No personal data or real filesystem enumeration was used.

First-scan wall time here means the existing harness's `P4.insert_seconds + finalise_seconds`,
through the `EndRun` receipt, including index construction, publication, compaction and checkpoint.
It excludes the later isolated SQL index probe. Build/process startup is outside this interval.
The first three runs establish the pre-edit baseline; the later runs were serially interleaved
A1/B1/A2/B2/A3/B3, with no concurrent build or test launched by this job.

| Run | Catalogue bytes | B/entry | Insert s | EndRun s | Through EndRun s |
|---|---:|---:|---:|---:|---:|
| Pre-edit 1 | 228,241,408 | 228.013 | 4.593257 | 17.607134 | 22.200391 |
| Pre-edit 2 | 228,241,408 | 228.013 | 3.505019 | 11.688650 | 15.193669 |
| Pre-edit 3 | 228,241,408 | 228.013 | 3.001404 | 9.408841 | 12.410245 |
| A1 baseline | 228,241,408 | 228.013 | 3.009099 | 9.548499 | 12.557597 |
| B1 changed | 204,161,024 | 203.957 | 3.309164 | 9.821774 | 13.130938 |
| A2 baseline | 228,241,408 | 228.013 | 3.091493 | 9.394501 | 12.485994 |
| B2 changed | 204,161,024 | 203.957 | 3.193958 | 9.042618 | 12.236576 |
| A3 baseline | 228,241,408 | 228.013 | 3.092867 | 15.167842 | 18.260709 |
| B3 changed | 204,161,024 | 203.957 | 3.080421 | 9.024505 | 12.104926 |

Alternating-run medians: **12.557597 s before, 12.236576 s after (-2.56%)**.
Individual paired changes were +4.57%, -2.00%, -33.71%;
all are below the +10% ceiling. These are observations on a shared, warm host, not a causal
speedup or quiet-host/cold-cache claim. The raw times above retain the slow A3 observation.

All nine runs exited 0, matched the independent count/byte oracle, reported zero freelist pages,
confirmed all schema indexes and removed their temporary stores. Each retained 1,000,000 files,
500,000,500,000 logical bytes, 496,873,078,784 known allocated bytes and 10,310 allocation-unknown
files. The denominator includes the 1,001 directories in both variants.

### Per-table and per-index allocation (rusqlite / bundled SQLite `dbstat`)

The benchmark now reports `P14.objects`: name, kind, owning table, occupied bytes, payload bytes
and unused bytes. Occupied page bytes below are identical across repeats within each variant.
Virtual tables/views have no pages of their own; their shadow tables are listed separately.
`sqlite_schema` is the 20,480-byte remainder between the object sum and the database file size.

| Object | Kind | Before bytes | After bytes | Change bytes |
|---|---|---:|---:|---:|
| `bulk_dir` | table | 4,096 | 4,096 | +0 |
| `bulk_dir_by_allocated` | index | 4,096 | 4,096 | +0 |
| `bulk_dir_by_logical` | index | 4,096 | 4,096 | +0 |
| `bulk_dir_by_root` | index | 4,096 | 4,096 | +0 |
| `bulk_dir_identity` | index | 4,096 | 4,096 | +0 |
| `bulk_dir_missing_identity` | index | 4,096 | 4,096 | +0 |
| `bulk_file` | table | 4,096 | 4,096 | +0 |
| `bulk_file_by_dir` | index | 4,096 | 4,096 | +0 |
| `dir` | table | 151,552 | 151,552 | +0 |
| `dir_by_allocated` | index | 20,480 | 20,480 | +0 |
| `dir_by_logical` | index | 20,480 | 20,480 | +0 |
| `dir_by_root` | index | 12,288 | 12,288 | +0 |
| `dir_identity` | index | 32,768 | 32,768 | +0 |
| `dir_missing_identity` | index | 4,096 | 4,096 | +0 |
| `ext` | table | 4,096 | 4,096 | +0 |
| `file` | table | 70,631,424 | 70,631,424 | +0 |
| `file_by_allocated` | index | 18,829,312 | 14,860,288 | -3,969,024 |
| `file_by_ext` | index | 12,992,512 | 12,992,512 | +0 |
| `file_by_logical` | index | 18,866,176 | 14,888,960 | -3,977,216 |
| `file_identity` | index | 25,161,728 | 25,161,728 | +0 |
| `meta` | table | 4,096 | 4,096 | +0 |
| `multilink` | table | 4,096 | 4,096 | +0 |
| `revision` | table | 4,096 | 4,096 | +0 |
| `root` | table | 4,096 | 4,096 | +0 |
| `scan_run` | table | 4,096 | 4,096 | +0 |
| `search_name` | table | 0 | 0 | +0 |
| `search_name_config` | table | 4,096 | 4,096 | +0 |
| `search_name_data` | table | 65,245,184 | 65,245,184 | +0 |
| `search_name_docsize` | table | 16,134,144 | 0 | -16,134,144 |
| `search_name_idx` | table | 20,480 | 20,480 | +0 |
| `sqlite_autoindex_ext_1` | index | 4,096 | 4,096 | +0 |
| `sqlite_autoindex_meta_1` | index | 4,096 | 4,096 | +0 |
| `sqlite_autoindex_multilink_1` | index | 4,096 | 4,096 | +0 |
| `sqlite_autoindex_root_1` | index | 4,096 | 4,096 | +0 |
| `sqlite_autoindex_stage_entry_1` | index | 4,096 | 4,096 | +0 |
| `sqlite_autoindex_volume_1` | index | 4,096 | 4,096 | +0 |
| `sqlite_sequence` | table | 4,096 | 4,096 | +0 |
| `stage_entry` | table | 4,096 | 4,096 | +0 |
| `volume` | table | 4,096 | 4,096 | +0 |
| `search_name_content` | view (new) | 0 | 0 | 0 |
| `sqlite_schema` | schema table | 20,480 | 20,480 | 0 |

### Separately justified changes

1. **`file_by_logical` and `file_by_allocated`: remove the explicit `id` key column.** SQLite
   already appends the rowid; `file.id` aliases it. `pragma_index_xinfo` previously showed both
   `id` and the implicit rowid. It now shows one copy. The indexes still cover `(dir_id, size DESC,
   id ASC)` pagination, including NULL allocated values. The two measured savings are 3,977,216
   and 3,969,024 bytes, respectively (7,946,240 combined). `EXPLAIN QUERY PLAN` tests require each
   covering index and reject a temporary ordering B-tree. Existing keyset/NULL paging tests pass.
   `file_identity` and `file_by_ext` are retained: identity/reference/reachability queries and the
   extension-only size-ordered search need them; the latter explicitly uses `INDEXED BY file_by_ext`.
   New plan assertions cover those retained paths too. No whole file index was dropped.
2. **`search_name`: external-content view plus `columnsize=0`, retaining full phrase positions.**
   The view derives exactly the old sentinel-normalised text and the same directory/file keys
   from `dir`/`file`; it stores no second copy of names. `search_name_docsize` disappears, saving
   16,134,144 bytes. Total trigram allocation falls from 81,403,904 to 65,269,760 bytes; postings,
   tokenizer, sentinels, case folding and tier/key ordering are unchanged. No search code uses
   trigram bm25/token counts. The existing contentless-delete layout cannot use `columnsize=0`
   (SQLite rejects that combination), so delete/rename triggers now submit the exact old text
   with the FTS `delete` command. Raw SQL writes and cascading removals remain covered. See
   [SQLite's column-size and external-content contracts](https://sqlite.org/fts5.html#the_columnsize_option).
   The source tables retain names, raw UTF-16, identities, timestamps, flags and all observations.

Both are forward-only **derived-layout migrations in the existing open transaction**, following
this crate's derived-index catch-up pattern; precious-state schema version 11 is unchanged.
Older child-sort indexes are detected by their explicit `id` column. The old trigram backing table
identifies the legacy layout; its index/triggers are rebuilt and backfilled atomically from source
rows. The frozen older-store SQL is `tests/fixtures/search-name-v1.sql` (the pre-edit search layout,
not application schema v1). Tests retain file metadata/search tiers, exercise raw renames/deletes
and reopen again. An authorizer rejects trigger creation after the replacement table is created:
rollback restores the old table/postings, then an unblocked retry succeeds. Existing stores may
have reusable free pages after rebuilding; this does not promise immediate shrinking on open.
The measured size is after the existing first-scan compaction, not a new VACUUM-on-open policy.

An external-content table's non-MATCH SELECT reads its source view. Consequently search parity
and crash-recovery verifiers now also run FTS `integrity-check` with rank 1, checking postings
against the external text; a source-row listing alone would be insufficient. Existing crash,
I/O-fault, planted-damage, forget/scrub and byte-for-byte search page/cursor checks remain green.

### Other candidates investigated / stopping point

- **Redundant indexes:** all four file indexes have distinct query uses. Only the two duplicated
  key columns were removed. Directory indexes are tiny on this workload; no needed index was cut.
- **`WITHOUT ROWID`:** an isolated copy of the compacted candidate with `file` keyed only by `id`
  occupied 204,025,856 bytes (203.822 B/entry), only 122,880 bytes below its 204,148,736-byte control.
  This exploratory SQL probe used Python's SQLite 3.50.4, not the final rusqlite benchmark.
  It also removes AUTOINCREMENT, so production adoption would require redesigning the shared
  live/bulk high-water sequences and ID non-reuse guarantee. It still misses 200. Empty
  `stage_entry`/`multilink` tables and their autoindexes cannot close the multi-megabyte gap.
- **Narrow encodings:** the million `file` rows already use integer flags/counts; text enums live
  in the 1,001 directory rows. Native 8/16-byte identity blobs retain their meaning and width.
  No cheap enum migration affects enough rows to close the remaining gap. Compressing synthetic
  IDs because they happen to have zero upper bytes would need a general, identity-preserving
  representation change across equality queries and references; it is not applied here.
- **FTS merge/tokenizer/name deduplication:** an isolated `optimize` plus VACUUM grew the
  implicit-rowid control from 220,295,168 to 220,315,648 bytes, so no new automatic FTS merge
  was added. These synthetic names are unique: a deduplicated-name table would add mappings
  without reducing the indexed-name count. `detail=column`/`none` removes phrase support,
  which the existing exact/prefix/token/substring tiers require; it is not an acceptable cut.

**Owner decision / next scope:** keep these measured, capability-preserving reductions and leave
P14 open at 203.957 B/entry. Dropping a needed index or phrase support to cross 200 violates this
job's constraints. Further lossless representation work (for example a sparse metadata or custom
search layout) needs a broader design/proof slice; this work does **not** prove that every possible
lossless design must exceed 200. No capability trade-off was silently selected.

### Regression and final proving receipt

Before production edits:

```text
cargo test -p loomward-catalog --test search trigram -- --nocapture
  1 passed, 2 failed (the two new storage/migration tests)
cargo test -p loomward-catalog --test open older_child_sort -- --nocapture
  0 passed, 1 failed (two stored copies of the rowid)
cargo test -p loomward-catalog --lib trigram_layout_migration -- --nocapture
  0 passed, 1 failed (the original layout never attempts the replacement DDL)
```

Afterward, the focused open/search run passed 33 tests and the migration rollback/retry run
passed 1 test. Final full commands (same worktree-local environment above):

| Command | Result |
|---|---|
| `cargo test -p loomward-catalog` | 326 passed, 0 failed, 1 existing ignored |
| `cargo test -p loomward-catalog --features test-hooks` | 346 passed, 0 failed, 1 existing ignored |
| `cargo test -p loomward-service` | 510 passed, 0 failed, 2 existing ignored |
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy -p loomward-catalog --all-targets -- -D warnings` | exit 0 |
| `cargo clippy -p loomward-catalog --all-targets --features test-hooks -- -D warnings` | exit 0 |
| `cargo build --release -p loomward-catalog --example bench` | exit 0 |
| 1M benchmark commands above | 6 baseline and 3 changed runs, all exit 0 |
| `git diff --check` | exit 0 |

Existing ignored tests were not changed or newly skipped: catalogue
`million_row_output_interiors_do_not_increase_detector_work`; service
`f3_issue782_measure_full_versus_targeted` and `large_sparse_backup_memory_probe`.

File-by-file: `examples/bench.rs` adds per-object accounting; `src/catalog.sql` shrinks sort keys;
`src/db.rs` applies transactional migrations and tests rollback/retry; `src/search.sql` supplies the
view/compact FTS layout and exact delete triggers; `src/writer.rs` corrects the deletion/merge
comment; `tests/open.rs` pins layout and query plans; `tests/search.rs` pins migration, metadata,
tiers and postings; `tests/crash_sweep.rs` strengthens postings verification;
`tests/fixtures/search-name-v1.sql` freezes the older layout. This section is the only evidence edit.

The 200 B/entry target is a measured miss. NOT verified: 10M scale, cold-cache/quiet-host performance,
end-to-end native filesystem scan timing, the optional ranked-index storage footprint, hosted CI,
and independent PR review. No Git write, commit, push, PR or publication was performed.
Residual risk: the first open of an older store rebuilds derived indexes synchronously and needs
working disk space; a rebuild failure rolls back. Source rows and precious state are retained.
The initial build's inherited external target-directory write is disclosed above.

Intended commits (driver-owned; none created here):

- `Shrink catalogue child-sort index keys`
- `Omit unused trigram document sizes without changing search tiers`
- `Record P14 catalogue storage and first-scan measurements`
