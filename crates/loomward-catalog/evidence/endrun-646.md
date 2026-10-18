# EndRun singleton filtering (#646)

Windows 11 Home / KRASPYON, 2026-10-18. Baseline is
`76b4868f1a26599a9696c0740cd4b243e2e5fb8c` (`origin/main` at lane start),
including #647's bounded FILE scratch. Issue #646 was open with no comments;
its finalisation path was still present. No already-landed throughput fix was
found. Worktree `C:/lw/wt/endrun-646`, branch `fix/646-endrun-throughput`.

## Change and invariant

`rebuild_multilink` previously grouped every visible identity-bearing file row,
including singleton identities. The existing partial `file_identity` index can
identify possible hard links first. A singleton across the entire catalogue
cannot have two visible names on one volume. Filter by that necessary condition,
then retain the original volume, visibility, exclusion, count and allocated-byte
aggregation. Cross-volume or hidden duplicate identities are only candidates;
the final grouping still decides whether they are actual observed hard links.

No schema, dependency, persistent index, transaction, durability or cache setting
changes. `temp_store=FILE`, 128 MiB main cache, 16 MiB TEMP cache and batched
rollup stay in force. The SQL remains within the existing publication transaction.
No owner file or process capability is introduced.

## Regression before implementation

`cargo test -p loomward-catalog --lib multilink_finalisation_does_not_group_singleton_file_rows -- --nocapture`

The synthetic fixture has 10,000 distinct identities, a second name for one of
them, and one unknown identity. It checks the two flagged names, link count of
two and maximum known allocation of seven. The work budget counts SQLite VM
instructions rather than elapsed time, including all three finalisation writes.

- Before the production SQL change: **0 passed, 1 failed**, 440,200 instructions
  against a budget below 400,000.
- After: **1 passed, 0 failed**, within the same unchanged budget and exact-result
  assertions. Existing cross-root finalisation/revocation, excluded/absent
  ancestry, NULL allocation, bulk equivalence and rollback tests also pass.
- The initial exploratory 600,000-instruction budget passed the baseline; it was
  tightened before implementation to make the regression discriminate the change.

## Measurement method

Release build, four Cargo jobs, private target
`C:/lw/target/lane-endrun-646`. Both binaries use the same lockfile and unchanged
`bench`, `catalog_perf` and `memory_bounds` examples. Baseline executables were
copied before changing production SQL; candidate executables were copied after.
Raw binaries, runner scripts and stdout/stderr remain under ignored
`.loomward/endrun-646/`. No target files were cleaned or removed.

For each workload, three fresh processes per variant, sequentially interleaved.
All use 1M rows. The initial first `bench` pair overlapped the tail of the catalog
test suite. Those receipts/stdout/stderr are preserved separately in
`.loomward/endrun-646/diagnostic-overlapped-pair/` and excluded from the result.
That pair was repeated after the memory loop, with no lane tests or builds running.
The final `bench` order is candidate-2/baseline-2, baseline-3/candidate-3,
baseline-1/candidate-1. Other workloads ran baseline-1/candidate-1,
candidate-2/baseline-2, baseline-3/candidate-3. An initial diagnostic `bench` run
is also excluded. No production source changed during measurement.
Synthetic metadata only; no filesystem enumeration or personal data. The Windows
`K32GetProcessMemoryInfo` high-water RSS/private-commit counters are sampled every
100 ms; an own-process control checks the API/structure before launching children.
The existing `bench` also records publication/finalisation peaks internally.
The long-name publication fixture publishes and republishes 1M 255-byte names;
the rollup fixture seeds 1M directories and checks repaired depths and exact totals.

Commands, from the worktree with `CARGO_BUILD_JOBS=4` and the target above:

```powershell
cargo build --release -p loomward-catalog --example bench --example catalog_perf --example memory_bounds
<variant>/bench.exe --rows 1000000
<variant>/catalog_perf.exe 1000000
<variant>/memory_bounds.exe rollup 1000000
<variant>/memory_bounds.exe listing 1000000
py -3 C:/lw/wt/endrun-646/.loomward/endrun-646/measure.py
py -3 C:/lw/wt/endrun-646/.loomward/endrun-646/replace_overlapped_pair.py
py -3 C:/lw/wt/endrun-646/.loomward/endrun-646/curate.py
```

## Completed measurements

The [curated receipt](endrun-646.json) contains the 24 selected successful child
processes, exact unrounded values, binary/source/harness hashes and summary
calculations. All memory children passed row-count, totals and depth assertions.
All throughput children passed their fixture assertions; each `catalog_perf`
recorded the same seven existing `ResourceBudget` refusals on its large-root
duplicate probe (42 total), with all other error lists empty. Those refusals are
not completed duplicate-query timings.

| Metric, 1M rows; median [min-max], three runs | Baseline | Candidate |
| --- | ---: | ---: |
| bench through-EndRun rows/s | 35912.20 [28120.95-44850.27] | 39693.30 [37001.45-46753.37] |
| bench insertion seconds (unchanged phase) | 7.14 [6.46-8.24] | 8.29 [6.08-8.97] |
| bench EndRun seconds | 19.63 [15.18-29.13] | 16.25 [15.33-18.76] |
| bench hard-link rebuild seconds | 2.07 [1.92-2.46] | 0.41 [0.32-0.42] |
| catalog_perf through-EndRun files/s | 40959.70 [40558.02-46459.04] | 45097.24 [44714.57-47723.68] |
| catalog_perf listing seconds (unchanged phase) | 6.14 [4.80-6.84] | 7.32 [7.29-8.58] |
| catalog_perf EndRun seconds | 17.82 [16.72-18.27] | 14.88 [12.38-15.04] |
| 1M-directory rollup peak RSS MiB | 186.50 [186.17-188.07] | 186.47 [186.17-187.45] |
| 1M-directory rollup seconds | 70.91 [68.55-77.86] | 82.16 [70.37-100.95] |
| 1M long-name publication peak RSS MiB | 355.38 [351.75-355.84] | 357.96 [351.92-358.42] |
| 1M long-name publication peak private commit MiB | 416.43 [413.29-417.27] | 419.56 [413.43-419.75] |
| 1M long-name publication seconds | 84.93 [81.45-250.07] | 97.86 [84.51-144.20] |

Median through-EndRun rates increase **10.53% / 10.10%** (`bench` / `catalog_perf`)
versus current main with FILE scratch. Median EndRun latency falls **17.26% /
16.50%**. Paired through-EndRun gains have medians **10.53% / 9.17%** and ranges
**4.24-31.58% / -2.93-17.67%**. The hard-link phase falls about **80%**, with
non-overlapping ranges. The unchanged insertion/listing phases vary substantially.
The longer rollup/listing workload times are not evidence of a causal regression:
a temporary real-SQLite diagnostic with 10,000 directories and no files took
fewer than 100 VM instructions for **both** old and new hard-link queries, ruling
out an added directory walk. The cause of the wall-time spread remains unverified.
These shared-host observations do **not** prove complete recovery of the
earlier 22.25% loss against its older main snapshot. #646 remains open for the
residual bulk copy/FTS/index, commit, VACUUM and checkpoint costs; the next
optimization needs a discriminating profile rather than a larger cache chosen
from noisy timings.

Candidate long-name publication peaks at **358.42 MiB RSS**, below the requested
approximately 600 MiB ceiling in all three completed runs. This is a measured
process peak on this fixture, not an enforced process-memory cap or a universal
bound for all catalogue shapes.

## Proving checks

Commands run from `C:/lw/wt/endrun-646` with four Cargo jobs and the lane target:

- `cargo test --manifest-path C:/lw/wt/endrun-646/Cargo.toml -p loomward-catalog --lib multilink_finalisation_does_not_group_singleton_file_rows -- --nocapture`:
  red **0 passed / 1 failed**, then green **1 passed / 0 failed**.
- `cargo test --manifest-path C:/lw/wt/endrun-646/Cargo.toml -p loomward-catalog`:
  **277 passed, 0 failed, 1 existing ignored scale test**, exit 0.
- `cargo fmt --manifest-path C:/lw/wt/endrun-646/Cargo.toml --all --check`:
  PASS, exit 0.
- `cargo clippy -p loomward-catalog --all-targets -- -D warnings`:
  PASS, exit 0.
- `cargo test -p loomward-catalog --lib diagnostic_empty_file_finalisation -- --nocapture`:
  temporary diagnostic **1 passed**, both queries below 100 VM steps. The diagnostic
  was then removed; SHA-256 `f043109512043143aba5923ceb4ff7d60148077bc0fc3fbe6a6b1c3e95ff1be0`
  confirms the test file is byte-for-byte the passed suite/clippy version.
- `git -C C:/lw/wt/endrun-646 diff --check`: PASS, exit 0.
- Receipt validation: **24 successful selected children**, three per
  variant/workload, all required fixture assertions passed, unchanged DB settings
  hash matched baseline, summaries recomputed from raw receipts.

## Scope left open

This is a scoped Sol lane: no commit, push, PR, merge, workspace or app checks.
The driver owns integration and the workspace gate. `HUMAN_TODO.md` is unchanged;
no owner decision or safety gate is opened or closed. No quiet-host, cold-cache,
10M-entry, Linux, full-engine-memory, or physical crash claim. The existing
ignored developer-clutter scale test was not requested or executed.

The candidate identity scan reads the catalogue-wide partial index; a large
unrelated volume or a catalogue dominated by repeated identities may see less
benefit. FILE scratch can still need disk space; rollback/disk-full tests remain
in the catalogue suite. Remaining EndRun work includes bulk copy/FTS/index builds,
commit, VACUUM and checkpoints; this change does not remove those costs.
