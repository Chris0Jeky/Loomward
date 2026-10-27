# First-scan scratch throughput (#646), 27 October 2026

## Changed

Peer implementation in `E:/loomward-build/wt/sol-646`, branch
`perf/646-scratch-throughput`, starting at
`002b6e5c305235563c211fa2724f102db4cae254`. No Git writes, dependency/schema migrations,
owner data, filesystem traversal, or changes to the safety gates. The driver owns commits
and integration. `HUMAN_TODO.md`, the checkpoint and implementation-status files are unchanged.

The production change is 16 added lines in `src/writer.rs::swap_bulk`: suspend the named
`search_file_insert` trigram trigger, copy the files, populate those new search keys from
the existing `search_name_content` view in one statement, then restore the exact saved
trigger DDL. Everything happens inside the existing EndRun savepoint, before publication
commits. Other roots' search keys are not selected. Directory and delete/update triggers,
ranked-token handling, secondary indexes, rollback, compaction and checkpoints are retained.
Search is populated when EndRun returns; no background search backfill is introduced.

The new ignored `writer::bulk_tests::bulk_first_scan_throughput` benchmark constructs a
synthetic metadata tree: 1,000 directories, 1,000,000 files, staged in 1,000-file chunks.
Its normal names are 21 bytes; `LW646_LONG_NAMES=1` uses 255 bytes. It asserts exact file
and logical-byte totals, empty bulk tables, all four file indexes, FTS external-content
integrity, and successful temporary-directory removal. No new ordinary test is added:
the change preserves visible behaviour; existing bulk equivalence, atomicity, rollback,
neighbouring-root, ranked-index and scratch-budget tests exercise that contract.

### Cause and rejected alternative

`git log -S temp_store -- crates/loomward-catalog` identifies `bbf5bfb5` as the
publication-memory change: `temp_store=MEMORY` became FILE, the main cache fell from
256 to 128 MiB, and the TEMP cache became 16 MiB. Its reason was bounded memory on very
large publications and rollups. The earlier measured 1M long-name publication peak was
about 1,921 MiB with MEMORY versus 353 MiB with FILE; those are historical measurements,
not this fixture's new results.

Bundled SQLite's `sqlite3Insert` explicitly selects `useTempTable` for row triggers
(`libsqlite3-sys-0.37.0/sqlite3/sqlite3.c`, around line 137883). Thus the old
`INSERT INTO file SELECT ... FROM bulk_file` first spools *all* input rows into an
ephemeral table, then inserts them and runs the trigram trigger per file. FILE makes
this a large disk-backed intermediate. The final profile localises most of the
settings-related swap gap to that statement; the four secondary-index builds differ
little. Removing this insert trigger for the copy avoids that intermediate and the
per-file trigger calls. The canonical view supplies the identical sentinel and root
encoding for the explicit trigram fill.

A first candidate copied in 1,024-row keyset batches, preserving FILE and the trigger.
Three interleaved pairs were slower: median EndRun **8.090 [8.068-8.133] s** versus
**7.379 [7.345-7.889] s**; through-EndRun **90,432 [89,460-91,024]** versus
**97,267 [92,270-97,501] files/s**. It was removed. Its scalar receipts, the initial
settings comparison and preliminary successful set-based comparison are retained in
the JSON receipt; raw logs remain under ignored `.loomward/646/`.

## Verified

Windows 11 Pro 10.0.26300, i5-13600K, Rust/cargo 1.97.1. Release benchmarks, three build
jobs, `CARGO_TARGET_DIR=E:/loomward-build/target-646`, `RUST_TEST_THREADS=3`. Another build
could share the host. Own builds and tests did not overlap the measurements.

The [receipt](scratch-throughput-646.json) contains exact values, final binary/source
hashes, raw final SQL profiles and recomputable medians. Nine final fresh child
processes ran sequentially in this order:
`before / legacy / after / after / legacy / before / before / after / legacy`.
All nine passed. The same final benchmark harness was built against the original
production writer and the changed writer; the original writer was proved identical
to HEAD with `git diff --exit-code -- .../writer.rs` before building the control.

"Legacy" restores only the pre-regression **settings** on today's original writer:
MEMORY scratch and a 256 MiB main cache. It does not restore old rollup code or undo
subsequent fixes. Before/after use FILE, 128 MiB main and 16 MiB TEMP throughout.

| Phase, seconds unless stated; median [min-max], 3 runs | Legacy settings | FILE before | FILE after |
|---|---:|---:|---:|
| Staging + publication wall | 2.531 [2.514-2.635] | 3.038 [2.926-3.044] | 2.970 [2.955-3.090] |
| Staging SQL | 0.320 [0.312-0.324] | 0.529 [0.512-0.538] | 0.553 [0.546-0.554] |
| Listing publication | 1.635 [1.631-1.641] | 1.843 [1.831-1.851] | 1.819 [1.804-1.832] |
| Complete bulk swap wall | 3.363 [3.336-3.471] | 4.877 [4.851-4.881] | 3.530 [3.462-3.550] |
| File-copy SQL, including trigger work before | 2.335 [2.296-2.360] | 3.788 [3.757-3.830] | 0.533 [0.532-0.542] |
| Explicit trigram-fill SQL | included above | included above | 1.932 [1.881-1.956] |
| Four secondary-index builds, SQL total | 0.975 [0.964-1.046] | 1.021 [0.983-1.025] | 0.987 [0.982-0.988] |
| Hard-link rebuild | 0.108 [0.108-0.110] | 0.121 [0.121-0.125] | 0.127 [0.126-0.129] |
| Rollup | 0.005 [0.005-0.005] | 0.005 [0.005-0.005] | 0.005 [0.005-0.005] |
| Writer commit | 1.026 [1.020-1.279] | 1.293 [1.060-1.313] | 1.080 [1.063-1.303] |
| VACUUM SQL | 0.580 [0.578-0.607] | 0.992 [0.968-0.994] | 0.994 [0.993-0.997] |
| End checkpoint, including VACUUM | 0.998 [0.965-1.220] | 1.583 [1.388-1.598] | 1.415 [1.375-1.587] |
| Complete EndRun wall | 5.469 [5.468-6.086] | 7.879 [7.457-7.895] | 6.100 [6.092-6.575] |
| Files/s through EndRun | 124,999 [114,670-125,283] | 91,547 [91,468-96,316] | 110,362 [103,467-110,440] |
| Process peak RSS MiB | 515.727 [514.922-516.621] | 326.539 [326.531-329.551] | 322.500 [321.625-322.527] |
| Process peak private commit MiB | 524.566 [522.727-525.508] | 330.617 [330.215-333.453] | 342.480 [342.301-342.871] |

Swap wall timing is bracketed by the first bulk-root SELECT's completed profile callback
and the final bulk-directory DELETE's completed callback, omitting the tiny first lookup.
Individual SQL times are SQLite PROFILE durations with millisecond resolution. Copy,
trigram and secondary-index rows are parts of swap; VACUUM is part of checkpoint;
swap/commit/checkpoint are parts of EndRun. These rows must not be added together.

The initial three-pair settings experiment reproduced a **22.20%** median throughput
loss (97,231 FILE versus 124,977 legacy files/s). The final set observes a **26.76%**
gap between its control and legacy settings, illustrating host variation. The final
fix increases median throughput **20.55%**, reduces median EndRun **22.58%** and swap
**27.61%**, but remains **11.71%** below the legacy throughput median. This is a
worthwhile partial recovery; it does not establish complete recovery of historical
#646 or controlled-host causality for every subphase.

### Memory

FILE scratch, the 128 MiB main cache and 16 MiB TEMP cache remain unchanged. Temporary
ID sets and sorting in the explicit search fill can spill; the code does not load the
whole tree into Rust or switch any phase back to MEMORY. The FTS hash-size setting is
unchanged. Cache settings and this fixture are not an enforced process-memory cap.

In the final comparable fixture, median RSS falls about 4 MiB, while median private
commit rises about 12 MiB; both stay far below the legacy settings' peaks. Three
additional fresh-process 1M-file, 255-byte-name runs passed all assertions:

| Long-name candidate, 3 runs; median [min-max] | Value |
|---|---:|
| Staging + publication seconds | 5.091 [5.091-5.532] |
| EndRun seconds | 22.329 [21.253-22.764] |
| Files/s through EndRun | 36,470 [35,341-37,958] |
| Peak RSS MiB | 399.977 [399.062-402.367] |
| Peak private commit MiB | 431.211 [430.691-434.625] |

These are first-scan bulk fixtures, distinct from the historical 353 MiB repeated-listing
fixture. The maximum exceeds 400 MiB by 2.37 MiB, but all three stay below the approximate
600 MiB ceiling discussed in the earlier #646 evidence. Windows
`K32GetProcessMemoryInfo` high-water counters were sampled every 100 ms and after exit,
with an own-process ABI control. Peaks include integrity assertions and writer shutdown.
Long-name measurements preceded the final wall-clock marker in the benchmark, using
the identical production writer; their binary hash is recorded separately.

### Commands and gate tails

All commands ran in this worktree with the target/thread/build-job environment above:

```text
cargo test --release -p loomward-catalog --lib --no-run
py -3 .loomward/646/measure.py final baseline legacybaseline file file legacybaseline baseline baseline file legacybaseline
LW646_LONG_NAMES=1 py -3 .loomward/646/measure.py long-names file file file
py -3 .loomward/646/curate.py
cargo fmt --all --check
cargo clippy -p loomward-catalog --all-targets -- -D warnings
cargo test -p loomward-catalog
git diff --check
```

The inline `LW646_LONG_NAMES=1` above denotes the environment; PowerShell used
`$env:LW646_LONG_NAMES='1'`. The ignored benchmark can also be run without the local
measurement runner:

```text
cargo test --release -p loomward-catalog --lib writer::bulk_tests::bulk_first_scan_throughput -- --ignored --exact --nocapture
```

All requested gates exited **0** on the final source:

```text
cargo fmt --all --check
  [no output]
cargo clippy -p loomward-catalog --all-targets -- -D warnings
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 17.73s
cargo test -p loomward-catalog
  test result: ok. 2 passed; 0 failed; 0 ignored; ...
  Doc-tests loomward_catalog
  test result: ok. 0 passed; 0 failed; 0 ignored; ...
  aggregate across 26 test/doc-test binaries: 386 passed, 0 failed, 3 ignored
git diff --check
  [no output]
```

The three normal-suite ignores are the new #646 benchmark (run explicitly above),
the existing developer-clutter scale benchmark, and the existing #937 FTS segment
benchmark. The existing bulk/incremental row-and-query equivalence, reader atomicity,
failed-swap commit rollback, neighbouring-build compaction, FILE/cache settings and
scratch-disk-full recovery tests all passed. Raw gate logs remain in `.loomward/646/`.
Receipt validation recomputed summaries and checked every recorded final source hash.

## NOT verified

No Git writes, PR, hosted CI, workspace/app/native-shell gate, personal scan, physical
filesystem enumeration, cold-cache/quiet-host run, Linux run, 10M-row claim, new crash
sweep, or old heap-based rollup benchmark. The driver owns review, integration and
commits. This lane does not close #646 or change an owner decision.

## Residual risk

FTS population now uses a set-based query over the canonical view, with disk-backed ID
and sort intermediates; other catalogue shapes may show different gains. FILE scratch
still needs disk space. The existing transaction protects trigger DDL and all publication
rows together; no durable state is exposed between copy and search fill. Existing
compaction, file-index building and checkpoint costs remain. Do not turn the measured
cache limits or the observed sub-600-MiB peaks into a universal memory guarantee.

The driver should preserve ignored `.loomward/646/` logs, runner and comparison binaries
before removing the worktree. The tracked synthetic receipt contains the decision evidence.
