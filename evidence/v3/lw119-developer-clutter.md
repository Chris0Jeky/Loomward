# LW-119 first slice: catalogue developer-output advice

Source base: `879c6ce2656928587219c3f25da15dbaeed288f2`.
Branch: `feat/410-developer-clutter`. Left uncommitted for the wave driver.
Platform: Windows 11, KRASPYON; cargo/rustc 1.97.1. Synthetic catalogue lab only.
Refs #410, #67, #92. This does not complete those issues.

## Changed

`Reader::developer_clutter(root_id, cap)` provides bounded advice using only the
catalogue. Version 1 matches Cargo, Node, Python, .NET and Gradle manifests to
their conventional direct-sibling output directories. It deduplicates anchors,
suppresses nested output candidates and ranks known logical sizes before unknowns.
Go has no universal project-local output directory, so `go.mod` alone lists none.

The result preserves unknown size/allocation and supplies an anchor, explanation,
regeneration hint, unknown cost class and explicit unique-content/prerequisite
uncertainty. It uses the reader's existing grant, transaction and work/deadline
guards. Excluded/reparse/protected/tombstoned ancestors and stale anchors are
shielded. No file/process capability or safety gate is opened.

The service pattern crosses coordinated central routing, protocol DTO/schema,
examples, generated TypeScript and mock transport. This slice therefore takes
the task's permitted library-only path. No capability is advertised as served.

## Verified

All Cargo invocations used `CARGO_BUILD_JOBS=4` and the lane's separate
`CARGO_TARGET_DIR=C:/lw/target/clutter-119`, from `C:/lw/wt/clutter-119`.
Commands below use `--manifest-path C:/lw/wt/clutter-119/Cargo.toml`.

- Test-first: `cargo test --manifest-path C:/lw/wt/clutter-119/Cargo.toml -p loomward-catalog --test developer_clutter`
  against the empty API returned exit 101: **0 passed, 4 failed**. Failures were
  the absent candidates, missing bounds, and absent unknown-size handling.
  The initial missing-API compile failure was not counted as a behavioral red.
- The same command after implementation and the writer-publication fixture:
  **5 passed, 0 failed**. Includes all 16 manifest/output pairs, source/dataset
  decoys, absent anchors, wrong adjacency, multiple anchors, nested dependencies,
  revoked scope, reparse/stale ancestors and anchors, null allocation, incomplete
  and unscanned sizes, zero versus unknown ranking, caps and deadline cancellation.
- `cargo test --manifest-path C:/lw/wt/clutter-119/Cargo.toml -p loomward-catalog`:
  **223 passed, 0 failed** (exit 0).
- `cargo fmt --manifest-path C:/lw/wt/clutter-119/Cargo.toml --all --check`:
  passed (exit 0).
- `git -C C:/lw/wt/clutter-119 diff --check`: passed (exit 0).
- `cargo clippy --manifest-path C:/lw/wt/clutter-119/Cargo.toml --workspace --all-targets -- -D warnings`:
  passed (exit 0).
- `cargo test --manifest-path C:/lw/wt/clutter-119/Cargo.toml --workspace --no-fail-fast`:
  **1,162 passed, 0 failed, 0 ignored** (exit 0), including the workspace-enabled
  crash sweeps and Rust protocol/schema/example tests.

The five previously reported #292 cases were exercised and passed in this run:
`native_dirty_epoch_relists_by_verified_identity`, the three
`native_relist_containment_*` cases, and
`reopen_follows_identity_after_rename_and_rejects_changed_volume_or_root`.
This observation does not claim to fix #292 on other roots or hosts.

## NOT verified

No live project inspection, manifest-content validation, rebuild timing,
dependency/toolchain/credential availability, active-build detection or unique
output proof. The detector cannot assert that the contents of a conventional
output directory are disposable. No app check/test/build or TypeScript generation
was run; their files and the wire surface are unchanged. Rust contract tests ran
within the workspace suite. No hosted CI, commit, push or PR was attempted in this
Sol lane.

## Residual risk and follow-ups

Names and recorded aggregates establish advice only. A partial or stale scan can
omit projects; exact-case and direct-adjacency rules deliberately miss custom
layouts. Bytes observed are not necessarily reclaimable bytes. The SQLite VM
budget can refuse a large query rather than silently return an empty result.

Follow-ups: coordinated service exposure, measured regeneration evidence
(LW-087), model lifecycle/files (LW-086), and stale worktrees with lease and
dirty-state evidence (LW-076). No generic cleanup action is included.
`HUMAN_TODO.md` q-34 records the selected scope and stays unchanged; no new human
action is needed for this library slice. The wave driver owns the checkpoint and
implementation-status updates, review, commit and integration.

## #515: join shape bounded (2026-10-17)

Branch `fix/515-clutter-join-bound`, lane `C:/lw/wt/clutter-515`, KRASPYON, Windows 11, rustc 1.97.1,
debug test build, synthetic lab only. Refs #515, #410.

**Before** (`EXPLAIN QUERY PLAN` of the v1 SQL, now `crates/loomward-catalog/tests/fixtures/developer-clutter-v1.sql`):
`MATERIALIZE safe`, `MATERIALIZE matched` with a full `SCAN d` over every directory, `SEARCH f USING
INDEX file_by_allocated (dir_id=?)` inside it, a `SCAN rules` with an `AUTOMATIC COVERING INDEX` on `safe`,
and a second recursive `covered` walk over every descendant of every match. Cost per parent was files x
child directories x rules, and `safe`/`covered` walked every `node_modules`/`target` interior.

Lab (`wide_lab`): one directory with 50,000 files, 2,000 child directories and four output-named children
(`target`, `build`, `bin`, `obj`; anchors `Cargo.toml`, `build.gradle`, `demo.csproj`), beside a project with a
`node_modules` of 20,400 directories (200 packages, each 100 deep plus a nested `node_modules`).

| SQL | VM steps | Wall (debug) | Result |
| --- | ---: | ---: | --- |
| v1, run to completion | 9,623,967,800 | 307.6 s | 5 candidates (the real reader's 2,000,000-step guard would refuse: `ResourceBudget`, no candidates) |
| v1, aborted at the cap the test uses | 30,000,000 | about 1.2 s | not finished |
| now | 783,000 | about 20-80 ms | 5 candidates, the real `Reader` answers |

That is about 12,000 times fewer steps (the 5 candidates of v1 and now are the same by count; row-level
equality is the differential test below, since v1 cannot finish inside a test).

**After**: one top-down walk that stops at the first matching output, so dependency and build interiors are
never visited. A directory's files are read once, and only when it has a child with one of the nine output
names; that read yields the lowest anchor id per rule family and each output-named child takes its anchor
from it. Plan now (pinned by `query_plan_reads_the_tree_only_through_indexed_lookups`): every access is a
`SEARCH` on `dir`/`file` through `dir_by_allocated (parent_id=?)`, `file_by_allocated (dir_id=?)` or the primary
key; the only `SCAN`s are the recursive CTE's own rows; no automatic index.

Ceiling: cost is (directories outside outputs) + (files in directories that have an output-named child),
about 12 VM steps per such file. There is no `(dir_id, name)` index and adding one is a catalogue schema bump
(`SCHEMA_VERSION` 8 to 9, migration, Python oracle, fixtures), left out of this slice; with it the second term
would become a seek. A directory of about 150,000 files that also holds an output directory would still reach
the 2,000,000-step guard and return `ResourceBudget`, which stays an honest refusal.

Proof: `cargo test -p loomward-catalog --test developer_clutter` 8 passed. New: the wide/large lab (fails on v1:
test red before the rewrite), the plan pin, and `rewritten_detector_matches_the_original_on_random_trees`
(16 seeded random trees of 300 directories with random manifests, shields, stale runs, flags and reparse
attributes, caps 3 and 200: rows, order, anchors and providers identical between v1 and now). The five
pre-existing tests are unchanged and pass.

## #515 follow-up: current-main audit and 1M-row lab (2026-10-18)

Starting HEAD `4babf6710c6425086520a8697d495dd15dff2d12`, branch `fix/515-clutter-join`.
`origin/main` was refreshed to `cfbcdce1b55cea0b88804a0acfb936fd794d1139`; its detector SQL and
detector tests are identical to starting HEAD. Issue #515 was still open with no comments.
The join rewrite and EXPLAIN plan regression were **already fixed by** `b55afd8`, merged in
[PR #558](https://github.com/Chris0Jeky/Loomward/pull/558) as `26e95b7` on 2026-10-17.
No production SQL, caller, schema, rule or capability changes are needed here.

This follow-up expands the existing wide lab to 3,000 child directories and 51,000 dependency
directories (still 50,000 parent files), retaining every prior assertion. V1 exhausts the
30,000,000-step cap in 773.7 ms; current SQL returns the five expected candidates in 875,700
steps / 48.1 ms, and the real reader answers within its 2,000,000-step guard. The existing
random-tree differential test still checks 32 old/new result sets. A new explicit test hides
the candidate itself by excluded flags, reparse attributes and excluded listing state, proving
that the same candidate is returned before and after each shield is applied and removed.

The explicit benchmark extends that lab to exactly **1,000,000 combined file and directory
rows**, filling only the already-matched `node_modules` interior. It checks unchanged query
results and VM-step counts across that growth, all five real-reader candidates, complete
coverage and no truncation. It prints both query plans. It is opt-in to keep a million-row
fixture out of every ordinary crate run; no existing test was ignored or weakened.

Initial lab: 160,512 file/directory rows. Added 839,488 interior files in 111.95 s; complete
benchmark including initial construction: 141.79 s. At 1M rows, three current-query runs each
spent **875,700 VM steps**, exactly the pre-growth cost, in **34.924 / 27.298 / 27.569 ms**.
The real reader returned all five candidates in **37.619 ms**, complete and untruncated.
The populated EXPLAIN plan still has only two `SCAN t` operations on the recursive CTE;
all base-table accesses are indexed `SEARCH` operations, without an automatic index:

```text
SEARCH d USING INTEGER PRIMARY KEY (rowid=?)
SEARCH c USING INDEX dir_by_allocated (parent_id=?)
SEARCH f USING INDEX file_by_allocated (dir_id=?)
SCAN t
SEARCH p USING INTEGER PRIMARY KEY (rowid=?)
SEARCH d USING INDEX dir_by_allocated (parent_id=?)
SEARCH c USING INDEX dir_by_allocated (parent_id=?)
SEARCH f USING INDEX file_by_allocated (dir_id=?)
SCAN t
SEARCH f USING INTEGER PRIMARY KEY (rowid=?)
SEARCH d USING INTEGER PRIMARY KEY (rowid=?)
USE TEMP B-TREE FOR ORDER BY
```

V1 still plans `SCAN d`, `SCAN rules`, an `AUTOMATIC COVERING INDEX` on the safe-tree CTE,
and a second recursive `covered` traversal. No million-row v1 execution was necessary:
the smaller wide lab already exhausts its 30M-step cap. Raw synthetic benchmark output is
local only, `.loomward/clutter-515-bench.log`; this section is the curated measurement.

Platform: KRASPYON, Windows 11, rustc 1.97.1, debug test profile;
`CARGO_BUILD_JOBS=4`, inherited `CARGO_TARGET_DIR=C:/lw/target/lane-clutter-515`.
All commands run from `C:/lw/wt/clutter-515`; cargo commands below use
`--manifest-path C:/lw/wt/clutter-515/Cargo.toml`.

Final verification:

- `cargo test --manifest-path C:/lw/wt/clutter-515/Cargo.toml -p loomward-catalog --test developer_clutter -- --nocapture`:
  9 passed, 0 failed, 1 opt-in benchmark ignored (run explicitly below).
- `cargo test --manifest-path C:/lw/wt/clutter-515/Cargo.toml -p loomward-catalog --test developer_clutter million_row_output_interiors_do_not_increase_detector_work -- --ignored --nocapture`:
  1 passed, 0 failed, 9 filtered out; 141.79 s, results above.
- `cargo test --manifest-path C:/lw/wt/clutter-515/Cargo.toml -p loomward-catalog -- --test-threads=4`:
  266 passed, 0 failed, 1 opt-in benchmark ignored; all executed test binaries and doc tests passed.
- `cargo fmt --manifest-path C:/lw/wt/clutter-515/Cargo.toml --all --check`: passed.
- `cargo clippy --manifest-path C:/lw/wt/clutter-515/Cargo.toml -p loomward-catalog --all-targets -- -D warnings`:
  passed, zero warnings.
- `git -C C:/lw/wt/clutter-515 diff --check`: passed.

The unchanged detector's baseline was 8/8 passing before these test additions; v1's negative
performance control remains in the wide test. There is no new production behavior requiring
a new implementation red/green cycle. An initial compile of the new benchmark used unsupported
`usize` SQLite decoding; corrected to `i64` before the successful runs. Raw full-crate output is
local only, `.loomward/clutter-515-catalog-tests.log`. No workspace suite, app checks, hosted CI,
commit, push or PR was performed: the lane leaves these changes uncommitted for the driver.

The `.sln` anchor concern remains an owner/product choice in #515: it is an explicitly
documented version-1 rule, and the existing provider matrix covers it. This lane does not
silently change that rule. `HUMAN_TODO.md` q-34 is answered; no owner decision or human-action
entry was changed. The pre-existing ceiling for huge manifest-parent directories remains;
1M total rows with pruned output interiors do not establish that every 1M-row shape will fit
the work budget. No personal inventory, cold-cache or service/UI benchmark was run.
