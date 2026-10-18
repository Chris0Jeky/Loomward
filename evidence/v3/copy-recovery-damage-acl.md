# Copy recovery: detected damage and destination ACLs

Windows synthetic lab evidence, 2026-10-18. Base: `3ad18703c2f5c47aa676d99e932e69e09986b584`.
Branch: `fix/copy-recovery-damage-acl`. Changes are uncommitted for the driver.
Commands run in the ops-fix worktree with `CARGO_BUILD_JOBS=4` and its exported
`CARGO_TARGET_DIR=C:\lw\target\lane-ops-fix`. No personal copy, commit, push or merge.

## Changed

Both lens claims reproduced against this base. Recovery now journals `needs_attention`
when that transition is legal. For waiting states without that journal edge, status
reports the observed attention state while preserving the journal's legal history.
Unjournaled entries remain untouched; later status reads report their staging-relative
names. The status contract adds `unjournaled_items` (first 64 names) and
`unjournaled_item_count` (total observed), with matching DTO, examples and generated types.
These required fields travel together with the producer and consumers; older status
payloads without them do not meet the updated contract.

Destination metadata confirmation reuses the C1 security reader and refuses explicit
ACEs. This covers normal and recovery confirmation, files, nested directories, the
published source folder and a multi-selection's generated wrapper folder.

## Regression evidence

Before the fixes, these commands each failed one regression (zero passed):

```powershell
cargo test -p loomward-windows --features copy-primitives recovery_records_unjournaled_batch_damage -- --nocapture
cargo test -p loomward-windows --features copy-primitives explicit_destination_aces_prevent_recovery_commit -- --nocapture
```

The first returned `copying` for an unjournaled empty file. The second returned
`committed` after an explicit allow ACE was added following rename; C1 independently
reported a security-descriptor difference. The normal confirmation path also falsely
committed. Extending the regression to multi-selection independently reproduced the
wrapper-folder omission before fixing it.

The service restart regression first failed because the status lacked the unjournaled
names. It then passed with `needs_attention`, the name, count 1, and the leftover kept.
The native tests also check an unchanged `blocked` journal with an attention status,
source preservation and published bytes. Existing successful-copy tests remain controls.

## Verified

Final proving commands (all passed):

```powershell
cargo test --manifest-path C:/lw/wt/ops-fix/Cargo.toml -p loomward-windows -p loomward-service -p loomward-protocol --features loomward-windows/copy-primitives --no-fail-fast
cargo fmt --manifest-path C:/lw/wt/ops-fix/Cargo.toml --all --check
cargo clippy --manifest-path C:/lw/wt/ops-fix/Cargo.toml -p loomward-windows -p loomward-service -p loomward-protocol --all-targets --all-features -- -D warnings
py -3 C:/lw/wt/ops-fix/scripts/gen_contracts_ts.py --check
npm.cmd --prefix C:/lw/wt/ops-fix/app run check
npm.cmd --prefix C:/lw/wt/ops-fix/app run test
npm.cmd --prefix C:/lw/wt/ops-fix/app run build
git -C C:/lw/wt/ops-fix diff --check
```

Rust: **604 passed, zero failed, 2 existing ignored tests**. Protocol: 54 passed.
Service: 344 unit tests plus 1 integration test passed, 1 existing ignored test.
Windows: 197 unit tests plus 8 decoder tests passed, 1 existing ignored test;
this includes the default crash and append-fault matrices and the 200,000-entry bound
fixture (the Windows unit suite took 467.80 seconds on the shared host).
App: 529 tests in 49 files passed; check reported zero errors/warnings;
production build passed. The missing local app tools were restored with
`npm.cmd --prefix C:/lw/wt/ops-fix/app ci --offline --ignore-scripts --no-audit --no-fund`
(66 packages from the local cache). Formatting, generated-type freshness and scoped
clippy (all targets/features, warnings denied) and `git diff --check` passed.

## NOT verified

Whole-workspace tests are reserved for the driver. No personal-data copy, power-cut
test, full crash sweep, throughput benchmark or hosted CI was run by this lane.

## Residual risk

The already-tracked post-confirmation window (#526) is unchanged. Copy stays lab-only;
`HUMAN_TODO.md` q-47 (Q1-Q3) and the C8 prerequisites, including G5/G9, remain open.
No gate, authority file, implementation-status file or checkpoint was changed.
