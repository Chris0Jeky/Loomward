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
