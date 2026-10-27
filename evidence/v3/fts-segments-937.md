# #937 item 1: synthetic FTS segment measurements (Windows, 2026-10-27)

Status: **measurement delivered; production merge change parked**. Segment layout contributes
a few milliseconds here. The reported 211–750 ms first query was not reproduced, and a bounded
merge that met the proposed regression target was not established. `writer.rs` is unchanged.
This does not resolve #937 item 1; item 2 was outside the assignment.

Base: `ae27e2987f536e66398ed220170977d1707c7057`, branch `sol/937-fts-merge`.
No Git writes, real-folder scanning, or external publication. `HUMAN_TODO.md` was unchanged.

## Method

`crates/loomward-catalog/tests/fts_merge.rs` retains an explicitly ignored, reproducible bench.
It uses `Catalog::open_with`, `RegisterRoot`, `BeginRun`, `DirListing`, and `EndRun` on disposable
synthetic stores with both indexes enabled. Names, identities, directory layout and insertion
order are identical in each arm: 1,000 directories with 1,000 deterministic generated names each.

* Incremental: root-directory publication, then 1,000 targeted runs (1,001 EndRuns total).
* Bulk: one full first-scan run with bulk publication enabled (one EndRun).
* Each measurement drops the build Catalog, reopens Catalog and Reader, then times two identical
  `search` calls for `report`, limit 20, budget 50,000,000, requiring 20 hits and no budget cut.
* The retained bench uses client scope `[1]`. The earlier owner control omitted that scope.
* The same-store control executes full `optimize` on both indexes **only in the disposable bench**,
  closes that connection, and repeats the reopen/search measurement. This is not a production fix.
* Segment counts decode the structure record at `_data.id=10` (including the token index's V2
  marker) and assert agreement with `count(DISTINCT segid)` from `_idx`.

## Measurements

All query times are milliseconds. “First” means fresh SQLite connection, not cold OS cache.

| Setup, 1M names | Name segments | Token segments | First | Second |
| --- | ---: | ---: | ---: | ---: |
| Incremental, client, unchanged writer | 17 | 10 | 3.475 | 2.219 |
| Same incremental store, bench-only full optimize, client | 1 | 1 | 1.153 | 0.594 |
| Bulk, client, unchanged writer | 3 | 1 | 1.317 | 0.767 |
| Same bulk store, bench-only full optimize, client | 1 | 1 | 1.285 | 0.934 |
| Incremental, owner, unchanged writer | 17 | 10 | 3.134 | 1.890 |
| Same incremental store, bench-only full optimize, owner | 1 | 1 | 2.193 | 1.001 |
| Bulk, owner, unchanged writer | 3 | 1 | 1.177 | 0.695 |
| Same bulk store, bench-only full optimize, owner | 1 | 1 | 1.037 | 0.578 |

An initial 300k-name owner comparison (240 directories, 1,250 names each) gave 8 name segments
and 1.679/1.034 ms incremental, versus 2 segments and 0.895/0.548 ms bulk. Full optimize of the
incremental store gave 1 segment and 0.883/0.514 ms. Its initial token decoder was incomplete;
those token counts are deliberately not reported.

The client benchmark's incremental build took 187.379 s; EndRun p50/max were 114.794/1295.638 ms.
Bulk build took 18.317 s, including 15136.073 ms for its one bulk EndRun. Owner comparison build
times were 144.587 s incremental and 36.239 s bulk. Shared-host load changed during these runs;
these are individual observations, not latency distributions or isolated-host speed claims.

## Bounded merge experiment and stopping point

A proposed regression published 96 directories × 256 names across 97 EndRuns, with and without
the token index, requiring no more than three segments and a successful client query. Against
unchanged production code, it failed on `search_name retained 8 segments after 97 small
publications` (0 passed, 1 failed; release run 2.28 s). An earlier 96 × 32 fixture failed on the
token index retaining 4 segments.

Three production prototypes ran only after publication, under the existing zero busy timeout,
with a temporary 10 ms progress-handler deadline per enabled index and four merge attempts:

| Prototype | FTS pages per step | Remaining name segments | EndRun p50 (ms) |
| --- | ---: | ---: | ---: |
| Continue positive merge; start a negative merge only after positive no-op | 64 | 7 | 23.433 |
| Same strategy, smaller step | 16 | 6 | 19.897 |
| Positive merges only, pairwise `usermerge=2` | 16 | 7 | 15.674 |

The unchanged regression fixture's EndRun p50 was 11.362 ms. All three prototypes failed the
same three-segment assertion. The deadline can roll back an unfinished statement, and the
three-segment target is not a guarantee of best-effort bounded work under ongoing ingestion.
No target was relaxed and no failing production change was retained. The experiment stopped
at the three-attempt gate; there is **no post-fix 1M measurement** and no production regression
test claimed green.

For the driver, the last unproven prototype and regression source are retained outside Git:
`E:/loomward-build/sol937-merge-attempt.patch` and
`E:/loomward-build/sol937-fts-merge-attempt.rs`. The patch also contains an injected-error unit
test that was written but **not run**; it is draft material. Raw before/attempt/check logs are
`E:/loomward-build/sol937-*.log`.

Next bounded investigation: establish a realistic convergence/latency acceptance condition for
maintenance under continuing insertion, and reproduce the original stall's query shape/cache
conditions before spending recurring EndRun work on this seam. The large stall's actual cause
remains unidentified; these results do not prove segment count alone explains it.

SQLite's [merge documentation](https://sqlite.org/fts5.html#the_merge_command) describes positive
continuation, negative-merge restart behavior, page limits and total-changes detection; its
[structure record documentation](https://sqlite.org/fts5.html#structure_record_format) describes
the segment-count field. The bundled SQLite source supplies the contentless-delete V2 marker.

## Commands and proof

Working directory for every command: `E:/loomward-build/wt/sol-937`.
Every cargo invocation used these environment variables (build jobs were also limited for later runs):

```powershell
$env:CARGO_TARGET_DIR='E:/loomward-build/target-sol937'
$env:RUST_TEST_THREADS='3'
$env:CARGO_BUILD_JOBS='3'
cargo test -p loomward-catalog --release --test fts_merge synthetic_fts_segments_first_query_bench -- --ignored --nocapture
```

Client benchmark output tail:

```text
incremental client_root_1: name_segments=17 token_segments=10 first_ms=3.475 second_ms=2.219
same store optimized client_root_1: name_segments=1 token_segments=1 first_ms=1.153 second_ms=0.594
bulk client_root_1: name_segments=3 token_segments=1 first_ms=1.317 second_ms=0.767
same store optimized client_root_1: name_segments=1 token_segments=1 first_ms=1.285 second_ms=0.934
test synthetic_fts_segments_first_query_bench ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 219.54s
```

Regression red command (source retained outside Git as above):

```powershell
cargo test -p loomward-catalog --release --test fts_merge completed_small_publications_bound_fts_segments -- --nocapture
```

```text
search_name retained 8 segments after 97 small publications
test completed_small_publications_bound_fts_segments ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1 filtered out; finished in 2.28s
```

Final proving commands:

```powershell
cargo fmt --all --check
cargo clippy -p loomward-catalog --all-targets -- -D warnings
cargo test -p loomward-catalog
git diff --check
```

Final retained-tree results:

* `cargo fmt --all --check`: exit 0, no output.
* `cargo clippy -p loomward-catalog --all-targets -- -D warnings`: exit 0;
  `Finished dev profile [unoptimized + debuginfo] target(s) in 5.11s`.
* `cargo test -p loomward-catalog`: exit 0; **381 passed, 0 failed, 2 ignored**, summed across
  26 result lines (including doc-tests). One ignored test is this manually executed bench; the
  other is the pre-existing million-row clutter benchmark. No final suite failure or pre-existing
  failure was observed. The unit result was `99 passed; 0 failed; 0 ignored` in 36.29 s.
* `git diff --check`: exit 0. The two new untracked files also had no whitespace diagnostics
  under `git diff --no-index --check -- /dev/null <file>` (exit 1 denotes their added content).
* `git diff --quiet -- crates/loomward-catalog/src/writer.rs`: exit 0; production writer equals HEAD.

Final test output tail:

```text
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests loomward_catalog
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Only `crates/loomward-catalog/tests/fts_merge.rs` and this evidence file are new in the retained
worktree. Nothing was staged, committed or pushed; the driver owns integration.

## Unverified and residual risk

No cold Windows filesystem-cache eviction, fresh OS process for each query, real 1.1M-entry
store, representative multi-root/deletion-heavy long-lived catalogue, or original slow query
was exercised. Catalog reopening itself may warm the OS cache; open time is excluded. There is
no justified production latency improvement claim. No public issue/PR was changed by this worker.
