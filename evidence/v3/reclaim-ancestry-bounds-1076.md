# Reclaim ancestry boundary regressions (#1076)

Source base: `085f79fca49125b1290652bea42ca0c215dfde08` in PR #1137. These tests address the three
remaining ancestry cases in the [#1076 review comment](https://github.com/Chris0Jeky/Loomward/issues/1076#issuecomment-6524769764).
The production dedupe and shared selection ancestry query are unchanged.

## Cases

The new `reclaim_nested_` tests in
`crates/loomward-service/src/reclaim_tests.rs` cover:

- **A file's own directory is its only listed ancestor.** The existing real
  catalogue fixture supplies a file and its containing directory. Only these
  two candidates are listed. The directory remains, absorbs exactly one file,
  and does not acquire a partial mark. This distinguishes the file's
  `strict_from = 0` boundary from directory self-exclusion.
- **The actual 100,000-row budget is spent.** A small in-memory parent graph
  gives 200 distinct leaf candidates 500 ancestry rows each. The next nested
  candidate remains visible and the plan becomes partial. The outer candidate
  records only the 200 proved absorptions. A fresh-budget control must absorb
  that same retained candidate, establishing that its ancestry was readable.
  Both plan groups stay within the existing 200-candidate group limit.
- **The root lies at or just beyond the depth boundary.** A 512-directory chain
  reaches its root at 511 parent edges and dedupes the descendant. A
  513-directory chain stops before the root, retains the descendant, and marks
  the plan partial without claiming an absorption.

Existing ordinary nesting, sibling, cross-group, and missing-parent cases remain
in the suite. The observe-progress and cross-group design questions are separate
from these ancestry assertions.

## Validation and limits

`git diff --check` passes. A local Python/SQLite fixture check executed the exact
ancestry SQL extracted from `reclaim/nested.rs`: the two depth cases each return
512 rows, reaching the root only for the 512-directory chain; each budget-test
leaf returns a complete 500-row chain, and the first 200 total 100,000 rows.
This validates fixture construction and SQL boundaries, not Rust test execution.
The existing algorithm starts a full ancestry walk while any budget remains, so
a final walk can overshoot 100,000 by at most 511 rows. This regression pins
exhaustion and stopping at zero, not a strict global 100,000-row ceiling.

Cargo and rustfmt are unavailable locally, so compilation, formatting, and the
new Rust tests remain unverified pending native CI. The focused native command
is `cargo test -p loomward-service --lib reclaim_nested_`; the normal formatting
and workspace gates still apply. No production behavior change or native pass
is claimed by this receipt.
