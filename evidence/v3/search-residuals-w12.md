# Wave 12 search residuals (#442, #552, #520)

Windows 11, KRASPYON, 2026-10-18. Worktree `C:/lw/wt/w12-search`, branch
`fix/442-search-residuals`, starting HEAD `415979e173cd6ffc832aad85814f9b5c09448aa3`.
Committed by the driver in #692. Synthetic temporary databases only.

## Reconciliation

Read #442, #552 and #520 including comments, and merged PRs #575, #609, #628 and
#635. All three issues remain open; this lane does not claim to finish them.

- (a) Already fixed by #628 (`8fc985a`): ranked search returns `ResourceBudget`
  when a budget cut leaves no hits. Existing test:
  `a_ranked_budget_cut_without_returned_hits_refuses`.
- (b) Already fixed by #628: invalid ancestry is negatively cached per request.
  Existing tests: `cyclic_ancestry_is_negatively_cached_for_every_affected_folder`
  and `missing_ancestry_is_negatively_cached_for_every_affected_folder`.
- (c) Already handled in the merged ancestry implementation: over-depth ancestry
  is refused, the hit is skipped, and both search paths set `partial` when a hit
  was skipped. Existing test: `an_over_depth_path_does_not_hide_valid_ancestors`.
  No incomplete ancestry is returned as a complete location. #635 (`34cac73`)
  consolidated the ancestry query and recorded the measured N+1 improvements.
- (d) Already fixed by #628: `commands.json` and `semantics.md` describe the
  owner-issued LW-067 grant and LW-069 adapter. Contracts were left unchanged.
- (e) Added the missing #520 coverage in `writer/bulk_tests.rs`, reusing the
  existing queue-path token oracle (`fts5vocab` term/key pairs, `detail=none`).
  The scale fixture adds 40,001 files to the existing mixed tree, proves a round
  indexes exactly 20,000 keys, reopens without opting in again, and verifies
  completion, full token parity, row count and FTS integrity. The retry test
  refuses the first real SQLite COMMIT after token inserts and queue deletion,
  checks that no tokens or queue removals were published, and permits the second
  commit only after those checks. The writer retries without a command or reopen
  and produces the exact queue-path index.

The ranked index and `federated_search` remain disabled by default. No production
code, dependencies, schemas, central status files or owner decisions changed.

## Verification

Every cargo command uses `CARGO_BUILD_JOBS=4` and
`CARGO_TARGET_DIR=C:/lw/target/lane-w12-search`.
Full-suite runs also use `RUST_TEST_THREADS=3`.

Focused baseline:
`cargo test --manifest-path C:/lw/wt/w12-search/Cargo.toml -p loomward-catalog --lib backfill -- --test-threads=3`:
3 passed, 0 failed. To prove the two new tests discriminate, temporarily changed
the writer's successful-round branch to stop even when more keys remain and its
failed-round branch to stop rather than retry. The same command then produced
1 passed, 2 failed: the scale test reported unfinished backfill and the retry test
timed out waiting for the second COMMIT. Restored both branches exactly before
the final gates; `writer.rs` has no final diff. These were deliberate mutation
failures, not failures of the retained implementation.

Final gates:

| Command | Result |
| --- | --- |
| `cargo test --manifest-path C:/lw/wt/w12-search/Cargo.toml -p loomward-catalog` | 298 passed, 0 failed, 1 existing ignored scale benchmark |
| `cargo fmt --manifest-path C:/lw/wt/w12-search/Cargo.toml --all --check` | PASS |
| `cargo clippy --manifest-path C:/lw/wt/w12-search/Cargo.toml --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --manifest-path C:/lw/wt/w12-search/Cargo.toml --workspace --no-fail-fast` | 1,676 passed, 0 failed, 4 existing ignored tests; exit 0 |
| `git -C C:/lw/wt/w12-search diff --check` | PASS |

Catalogue output is retained at
`C:/lw/target/lane-w12-search/catalog-test.log` (outside Git).
Workspace logs are `workspace-clippy.log` and `workspace-test.log` in that same
target directory. The workspace run includes the existing Windows staging-limit
test that creates 200,000 extra synthetic files; it passed and was left unchanged.
The Windows unit binary took 1,943.68 seconds on this shared host (229 passed,
2 ignored). No unexpected failure needed a retry. Final formatting passed again.

## Limits

No new 10M or quiet-host measurement, real-disk scan, app/browser exercise or hosted
CI run. Python verification is not required because contracts are unchanged.
The unmeasured writer hypotheses on #552 and the remaining #520 performance
qualification stay with those issues. `HUMAN_TODO.md` was read and left unchanged;
this test coverage needs no owner decision. The soak was not touched.
