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
