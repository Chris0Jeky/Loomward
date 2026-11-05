# Duplicate candidate count and cache: #1104, #1105

2026-11-02. Synthetic Linux SQLite evidence, source base remote main
`3f5aaf2b9dd6a9c49d1c0471f4390deb6df5d10a`, reconstructed locally as
`4db7f22494b7ba330945e7ba7df752e88da6ea55`.

## Scope and arithmetic

The count now groups eligible files once, computing `count(*)` and `sum(logical)`
for each repeated size, then summing those group totals. Selection and counting
still use the same root, size-floor, grant/tombstone visibility and excluded-ancestor
CTE. The count no longer revisits all eligible files through a size-membership
lookup. No schema, index, stored inventory, content read or file effect was added.

Both levels use integer `SUM`. Multiplying size by group count would allow SQLite
to promote overflowing arithmetic to REAL; the implementation retains SQL overflow
errors instead. Budget/deadline interruption remains unknown, never a short count
or zero. Other SQL failures remain errors.

## Tests first and local evidence

The coordinator froze the native tests-only patch before implementation. It adds
an instance-change regression with equal generation/subtree revision, independent
live-run and invalid-aggregate guards, root/floor key separation, interrupted-count
no-store behavior, hardlink flag eligibility, and overflow across size groups. The
existing fixture now returns the actual RegisterRoot `grant_id` for BeginRun.
The final patch also tests deadline interruption without storing an answer.

The native negative run initially stopped at rustfmt before behavioral tests;
the exact hosted formatter output was applied to the frozen test source. This is
formatting evidence, not a behavioral red or native pass. Native runtime results
are owned by the coordinator and are not claimed here.

The formatter-corrected tests-only head `cc6f7ee21ad1278e6c6153c63cc6920d2b7a6fcb`
then passed formatting and compiled in [PR #1136's native Linux job](https://github.com/Chris0Jeky/Loomward/actions/runs/41448113636/job/125286611768).
At 12:56 UTC the instance regression failed with 40 cached files where zero were
expected; 115 other catalogue unit tests passed and two measurements were ignored.
The cache stamp was then extended to include the catalogue instance, retaining
root generation, subtree revision, root/floor keys and both eligibility guards.
This is native behavioural red for the cache fix; the final implementation still
requires exact-head hosted qualification.

`python scripts/measure_duplicate_space_sql.py --require-improvement` was run
against the original production SQL and failed at the intended fewer-VM-steps
assertion. All 14 small parity checks passed first. After the grouped-query change,
the command passed, including all 14 parity checks. The checks use hand-derived
counts for floor boundaries, hardlink bit 2, each excluded bit, excluded and
absent ancestors, revoked grants, real zero, and integer overflow within a group
and across groups. They execute SQL extracted from `duplicates.rs` against the
checked-in schemas, indexes and exact `grant_view` views.

`git diff --check` and Python syntax parsing passed. Cargo, rustfmt and Clippy are
unavailable locally. No native compilation, workspace pass or Windows result is
claimed by this note.

## Bounded measurement

Python SQLite 3.53.1; 130,000 files in 130 directories below one root. Each query
gets a freshly seeded in-memory database. “First” is its first SELECT after setup,
not cold disk I/O. Five subsequent executions produce the warm SQL median; those
executions bypass the Rust cache entirely. Timings include Python/SQLite overhead
and vary with contention; VM steps are sampled in increments of 1,000.

| Shape | Query | First steps | First ms | Warm median ms |
| --- | --- | ---: | ---: | ---: |
| One size | Previous | 5,733,000 | 118.00 | 107.48 |
| One size | Grouped | 3,906,000 | 81.64 | 79.01 |
| 20,000 sizes | Previous | 21,733,000 | 1,119.42 | 911.91 |
| 20,000 sizes | Grouped | 4,306,000 | 134.45 | 110.44 |

The spread-size query uses about 80.2% fewer VM steps; the one-size query uses
about 31.9% fewer. Both queries return exactly 130,000 files: 1,300,000 bytes for
one size and 1,251,235,000 bytes for the spread fixture. All measured queries fit
the existing 32M-step budget. This establishes bounded synthetic SQL work, not a
new maximum root size, end-to-end latency, native cache-hit cost or Windows speed.

The existing ignored native measurement remains available and now prints the
first count after a write separately from its repeated measurements:

```text
cargo test --release -p loomward-catalog measure_the_candidate_space_count_at_130k -- --ignored --nocapture
```

It retains separate full-call, selection-only, count-only and warm cached-call
timings. No CI workflow was changed; the coordinator owns hosted execution and
the full integrated Rust gates before merge.

## Hosted Windows comparison (2026-11-05)

This addendum records subsequent native execution. The local-tooling limitations
and unchanged-workflow statement above describe the 2026-11-02 checkpoint.
[PR #1145](https://github.com/Chris0Jeky/Loomward/pull/1145) adds the focused
`duplicate-candidate-space` workflow and the same-fixture comparison in the
existing ignored benchmark. Production counting, eligibility and budgets are
unchanged by that measurement patch.

### Provenance and environment

- PR head: `48471bada8a6bb2d6cb0b5931ed6fc60a3a1d627`.
- Checked-out PR merge commit: `21dc317e41de283c700cf7e5a48c4874da63c24b`.
- [Run 41954090348, attempt 1, job 127004685195](https://github.com/Chris0Jeky/Loomward/actions/runs/41954090348/job/127004685195).
- [Receipt artifact 13382098659](https://github.com/Chris0Jeky/Loomward/actions/runs/41954090348/artifacts/13382098659),
  containing `duplicate-candidate-space-receipt.txt`; downloaded ZIP SHA-256:
  `69f17c0457a1ec418f23147c139ec7fbe0a8c0c2b84f2c02f065df688eaa54ee`.
  The extracted receipt matches the archive entry byte for byte. The workflow
  retains a text tail of at most 1,000 lines, each capped at 1,024 characters,
  for 14 days; the full output remains in the Actions log.
- Hosted `windows-latest`, Windows X64; image `win25-vs2026`, version
  `20261030.294.1`; `AMD64 Family 25 Model 17 Stepping 1, AuthenticAMD`,
  four logical processors.
- Rust `1.99.1` (`58cf8f9c35b355199727627d07ae34a4e2846d67`),
  `x86_64-pc-windows-msvc`, LLVM `23.1.1`; SQLite `3.51.3`;
  optimized release profile, `debug_assertions=false`.

The exact command was:

```text
cargo +stable test --release --locked -p loomward-catalog --lib duplicates::tests::measure_the_candidate_space_count_at_130k -- --exact --ignored --nocapture --test-threads=1
```

It compiled and reported **1 passed, 0 failed, 0 ignored, 118 filtered out** in
51.22 seconds, with `cargo_exit_code=0`. The workflow also requires that one-test
pass receipt, so a filter matching no test cannot qualify the measurement.

### Method and count results

Each shape populates one temporary SQLite catalogue with 130,000 files in 130
directories. Both queries use the same reader connection, size floor of one,
eligibility flags and unchanged 32,000,000-step work budget. The frozen reference
is the exact pre-grouping query from
`ff9add48774c3adac07e58f021926180116898ec`, including its original projection and
eligible-size membership lookup. The grouped side calls production
`candidate_space`. No native root grant, content read or file effect is exercised.

First uncached observations run sequentially before the warm-up: reference then
grouped for one size, grouped then reference for 20,000 sizes. Setup or the
preceding query can warm caches; OS and SQLite caches are not flushed. One paired
warm-up precedes ten interleaved measured pairs, alternating which query leads
so each leads five. Every count is checked against independently calculated
file/byte totals. Warm medians average the two middle samples; min/max show the
observed range. All figures below are milliseconds, rounded as printed.

| Shape | Count query | First uncached | Warm median | Warm min | Warm max |
| --- | --- | ---: | ---: | ---: | ---: |
| One size | Reference | 83.4 | 71.8 | 70.2 | 74.5 |
| One size | Grouped | 52.7 | 51.7 | 51.0 | 58.7 |
| 20,000 sizes | Reference | 860.1 | 837.2 | 808.2 | 873.1 |
| 20,000 sizes | Grouped | 103.6 | 88.0 | 83.0 | 110.0 |

Every comparison returned exactly **130,000 files**: **1,300,000 bytes** for one
size and **1,251,235,000 bytes** for 20,000 sizes. Both queries completed within
the existing work budget. The logged grouped/reference median ratios were
**0.721** and **0.105**, respectively, calculated from the unrounded durations.
The same run retained separate current-production call measurements:

| Shape | Call | Warm median | Warm min | Warm max |
| --- | --- | ---: | ---: | ---: |
| One size | Full uncached call | 173.1 | 172.1 | 180.4 |
| One size | Selection only | 121.5 | 120.9 | 123.6 |
| One size | Cached full call | 121.2 | 119.5 | 125.9 |
| 20,000 sizes | Full uncached call | 324.5 | 309.9 | 347.8 |
| 20,000 sizes | Selection only | 233.6 | 228.2 | 272.4 |
| 20,000 sizes | Cached full call | 235.2 | 227.1 | 268.3 |

These are one hosted runner's synthetic catalogue measurements; hardware, load
and cache state affect timings. The direct count comparisons bypass the result
cache. Fixture population and compilation are outside the timed calls. This
receipt establishes neither cold-disk nor end-to-end service/user-machine latency,
a maximum supported root size, nor a timing threshold for other machines. The
historical Windows 11 pre-grouping measurements remain documented in the
benchmark comment and concern a different machine and run.

### Qualification boundary

After the benchmark succeeded, normal CI on the measured PR head reported a
rustfmt failure in the benchmark assertions. That failed head remains part of
the evidence; the measurement success does not establish a full CI pass.
The whitespace-only assertion correction is included in the subsequent update
to PR #1145. Full exact-head qualification of that correction remains pending
when this addendum is prepared. No measurement is claimed for a later head by
this receipt.
