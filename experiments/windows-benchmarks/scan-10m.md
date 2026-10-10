# L17a native scan at the oversized L tier (LW-056)

Windows 11 Pro build 26300, shared host. Dataset: **10M lab tier (E: scale lab)**, NTFS.
Host hardware (processor, memory, device model and media) is withheld from public evidence, per
the 10 October 2026 correction in `evidence/v3/README.md`. Selected root is recorded only in the ignored
`.loomward/scan-roots.log`. The generated tree is retained for subsequent L17 work.

## Method

Owner grant: 10M files on the E: scale lab only. Mixed profile, seed 42, eight generation
threads; zero sparse payloads, 256 files per bucket, deterministic UTF-16 names and fan-out,
including occasional 40-component paths. This is the existing lab distribution, not all of
doc 41 section 14's proposed compressed/hard-link/reparse cases or its oversized single directory.
Generation requires a complete exact file/directory/logical-byte oracle before completing the
manifest, measured sparse allocation, a 20 GB free-space reserve, and scoped allocation/volume-delta
ceilings. The observed volume delta includes unrelated concurrent activity and NTFS metadata.

Commands, from this worktree (private selected path omitted):

```powershell
cargo build -p loomward-lab --release --offline
# $labRoot10m is the explicitly authorised path recorded in the ignored root log.
target/release/loomward-lab.exe generate --root $labRoot10m --files 10000000 --profile mixed --seed 42 --threads 8
cargo build -p loomward-lab --release --offline --target-dir target/scan-10m
py -3 experiments/windows-benchmarks/scan_10m.py --binary target/scan-10m/release/loomward-lab.exe
# Collector invokes the following benchmark once; it starts 20 fresh child processes:
target/scan-10m/release/loomward-lab.exe bench scan --tier L --runs 20
```

Separate scan build avoids overwriting the running generator's executable. The collector reads
generation aggregates from ignored logs; public results contain no selected root, filenames or
inventory. Binary/source hashes identify the uncommitted measurement build in the JSON receipt.

Every timing is **shared-host/uncontrolled**: other lanes share CPU and disk. Cache is
**warm/uncontrolled**. The initial attempt was labelled **first-after-generate**, but its validator
rejected it before recording timings. A small regression test reproduced expected directory-limit
listings incorrectly incrementing `failed`; counting now excludes limits while preserving real
errors. Accepted runs use warm/uncontrolled. Initial timings were not retained and are not
included in p50/p95. Generation's exact-oracle walk already primes metadata.
No reboot, elevation, cache flush or standby-list manipulation.
Each run gets a fresh process and aggregate-only memory sink, eight scan workers, 16 KiB native
buffers and unchanged 64 MiB engine accounting. There is no persistent catalogue or state disk.
Scan wall times cover `run_scan`, excluding process startup, CLI manifest-plan validation and
lab-grant construction; process-lifetime memory peaks include those earlier steps.

Enumeration wall is elapsed time through the final drained `ListingDone`, including scan setup
and backpressure. Enumeration worker time is summed across workers. Staging is summed writer
time in listing preparation, chunks and listing publication. Aggregation includes bottom-up
finals, their writer drain and the final root finish. Enumeration overlaps staging; these
durations must not be added/subtracted to infer exclusive phases or SQLite throughput.
`PeakPagefileUsage` and `PeakWorkingSetSize` are Windows process-lifetime high-water counters,
sampled after scan completion, including manifest-plan validation and setup. p50 is the median;
p95 uses nearest rank across 20 accepted warm/uncontrolled runs.

Complete runs must exactly match the independent manifest and memory sink. A limit-hit L run
instead requires incomplete coverage, manifest-bounded observed counts/bytes, sink agreement,
a repairing root and no sweep (N6 partial invariants). Full allocation is unknown/null; observed
subset allocation is separate. Scheduling chooses the retained subset, so this is not an exact
subset oracle and throughput uses **observed files**, never 10M divided by partial wall time.

## Results

Aggregate receipt: [scan-10m.json](../../evidence/v3/bench/scan-10m.json).

Generation: **10,000,000 files**, **62,892 descendant directories**, exact oracle PASS.
Process wall including plan, creation, verification and final output: **4,122.122 s**
(68.702 minutes), **2,425.935 files/s**. Internal generation timer including verification:
4,119.493 s, 2,427.483 files/s. Logical sparse size: 1,209,729,115,565,937 bytes;
payload allocation: 104,858,160 bytes; observed volume delta: 13,507,256,320 bytes.
All generation timings are shared-host/uncontrolled.

Every accepted native run hit the unchanged arena limit: **9,843,584 observed files**,
**48,769 descendant directories**, Extended strategy, zero failed/skipped entries,
incomplete coverage and no sweep. Partial invariant oracle **20/20 PASS**;
complete manifest matches **0/20**. Full allocation remains unknown/null; observed subset
allocation is 103,205,296 bytes. No full-10M engine completion is claimed.

| Metric | p50 | p95 (nearest rank, 20 runs) |
|---|---:|---:|
| Scan wall, s | 18.012287 | 27.076304 |
| Observed files/s | 546,529.637 | 627,662.542 |
| Enumeration wall, s | 14.282702 | 23.349110 |
| Enumeration worker time (sum), s | 96.704864 | 161.292731 |
| Writer staging, s | 14.139229 | 23.275011 |
| Aggregation/final publication, s | 3.709993 | 3.742309 |
| All writer calls (legacy persistence metric), s | 14.169436 | 23.306133 |
| Peak private commit, bytes | 39,010,304 | 39,321,600 |
| Peak working set, bytes | 39,071,744 | 39,378,944 |

Maximum lifetime peak private commit: **39,346,176 bytes (37.523 MiB)**;
working set: **39,542,784 bytes (37.711 MiB)**. Throughput percentiles are percentiles of
per-run observed-file rates, not reciprocals of wall percentiles.
First accepted warm/uncontrolled scan took **971.988 s** (10,127.266 observed files/s),
and is included in all summaries. The nearest-rank p95 is the 19th sorted value; the maximum
remains visible in raw runs. This large tail is not a cold-boot measurement.

## Proving checks

Windows local checks: `cargo fmt --all --check`, `cargo test --workspace`,
`cargo test -p loomward-windows --all-features`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `git diff --check`.
Final workspace suite: **336 passed**; Windows all-features suite: **34 passed**; fmt,
strict all-features clippy and diff whitespace checks: **PASS**. Receipt medians/p95 were
independently recomputed; source/binary hashes match the measurement build, and the aggregate
JSON passes a selected-root/private-path exclusion check. Hosted CI was not run in this lane.
The directory-limit counter regression failed before its repair and passed after it; its mixed
denial-plus-limit control keeps a real child I/O error visible. The E-only limit regression likewise
failed under the old 3M rule and passed after its repair. Four actual generate CLI refusals created
no roots (G: >3M, G: 10M, E: >10M, outside lab); no 10M generation is needed by those tests.
Two fresh S-tier control processes matched their exact manifest. Missing first-attempt timing is
retained explicitly as a limitation, not replaced by a later run.

## Scope and limitations

The G: warm full-10M-scan P2 target (100 seconds, 400 MB private commit) is not qualified by an
E: scale-lab run or by partial coverage. No catalogue bench, persistent publication, cold-cache
test, WinDirStat comparison, foreground-interference measurement or target extrapolation was run.
