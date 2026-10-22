# Personal duplicate inspection: shared writes (#378)

2026-10-22, Windows, base `0ceca9d2`. Synthetic fixtures only.

## Reconciliation

1. **Fixed here:** inspection now opts into `FILE_SHARE_WRITE` for content opens.
   `Reader::new` retains `FILE_SHARE_READ | FILE_SHARE_DELETE`; only
   `inspect_with_cancel_hook` selects `with_shared_writes`. Copy and tree callers keep
   their deny-write policy. Neither copy code nor `native/tree.rs` changed.
   This supersedes the share-write disposition in `duplicates-lows-378-359.md`:
   the inspection policy is now separate from the shared reader's default.
2. **Already fixed:** `loomward-service/src/duplicates.rs:39` selects capped catalogue
   candidates before `granted_root` at line 43. Existing regression
   `duplicates_select_candidates_before_opening_root_metadata` makes the root
   unopenable and checks that selection happened before native refusal.
3. **Already fixed:** `loomward-protocol/src/dto/session.rs:23` documents the legacy
   file-content return flag. `contracts/v3/semantics.md:580-586` documents local,
   read-only inspection, the separate observation capability, and that no command
   returns file content. No wire field, DTO or contract changed here. Existing
   `hello_reports_duplicate_content_reads_separately_from_effects` covers the
   observation/effect distinction for synthetic and personal sessions.

## Signature and writer regressions

`native.rs::hash` calls `finish` before finalizing a digest. `finish` verifies the
opened handle and reopens the name through pinned parents. `signature` compares
native object identity, size, last-write time, change time, attributes, link count
and ancestor identities. No USN is recorded. The exact-comparison path also calls
`finish` on both handles before returning equality.

New tests use real Windows file handles and keep the writer open through the
result assertion:

- `duplicate_hash_allows_an_existing_writer_without_changes`: default reader still
  refuses the writer; inspection's shared-write reader returns the expected SHA-256.
- `duplicate_hash_rejects_writes_during_read_with_writer_still_open`: both same-size
  and size-changing writes after the first block produce `ChangedDuringRead`,
  never a digest.
- `native_writer_can_open_during_inspection_and_changes_never_confirm_duplicates`:
  public inspection permits a writer to open after the first read; an unchanged
  writer permits the verified pair, while a same-size write leaves partial changed
  evidence and no duplicate group.

The original exclusive-handle refusal remains. The existing writer-refusal test
now explicitly withholds read sharing, preserving `SharingViolation` coverage for
an incompatible writer rather than rejecting every writable handle.

Regression first: the two new hash tests failed on the original implementation
(0 passed, 2 failed, sharing violations). Disabling inspection's shared-write opt-in
also made the public regression fail on the writer open (0 passed, 1 failed).

## Verification

Final checks use `RUST_TEST_THREADS=4`, `CARGO_BUILD_JOBS=6`, offline dependency
resolution and a worktree-local Cargo target. Logs are retained under
`.loomward/issue378/`; earlier logs are in its `initial-logs/` subdirectory.

| Exact command | Result |
|---|---|
| `cargo test -p loomward-windows --all-features duplicate_hash -- --nocapture` | Before fix: 0 passed / 2 failed (sharing violations). Initial shared-write probe: 2 passed / 0 failed. |
| `cargo test -p loomward-windows --all-features native_writer_can_open_during_inspection_and_changes_never_confirm_duplicates -- --nocapture` | With inspection opt-in temporarily disabled: 0 passed / 1 failed (writer open refused). Restored before final checks. |
| `cargo test -p loomward-windows --all-features native_control_and_named_stream_content_are_read_only -- --nocapture` | Unmodified HEAD control: 0 passed / 1 failed, same initial equality assertion as final run. Original bytes restored before final checks. |
| `cargo test -p loomward-windows --all-features duplicate` | Final: 52 passed / 1 failed. Sole failure is the baseline-reproduced tree test. All three new tests and existing writer/exclusive sharing refusals passed. |
| `cargo test -p loomward-service duplicates` | Both runs: 11 passed / 0 failed. Includes the existing candidate-order regression. |
| `cargo test -p loomward-service` | One run: 507 passed / 1 failed / 2 existing ignored. Failed in concurrent scan worker sharing, detailed below. Integration tests were not reached after the library failure. |
| `cargo test -p loomward-service issue776_default_scan_shares_workers_with_an_owner_scan_and_freshness -- --nocapture` | 1 passed / 0 failed in isolation, with unchanged source. |
| `cargo fmt --all --check` | PASS, exit 0. |
| `cargo clippy -p loomward-windows --all-targets --all-features -- -D warnings` | PASS, exit 0, no warnings. |
| `cargo clippy -p loomward-service --all-targets -- -D warnings` | PASS, exit 0, no warnings. |
| `git diff --check` | PASS, exit 0. |

Two earlier Windows duplicate runs were 51 passed / 1 failed and 51 passed / 2 failed.
The latter also encountered the source-walk test racing temporary synthetic Rust fixtures
because `TEMP` was under a non-excluded worktree directory. Moving Windows test temporary
files under excluded `.loomward/` resolved that setup failure without source changes.
The final 52/1 result above is still a failing suite.

The unchanged tree test fails at `native/tree.rs:691`, before its named-stream mutation:
comparing its control snapshots does not report equality because short-name metadata is
`metadata_unreadable`. Unmodified HEAD reproduced the same failure. No tree code or test
was changed.

The service failure is
`tests::issue776_default_scan_shares_workers_with_an_owner_scan_and_freshness`,
at `tests.rs:5675`: a concurrent scan returned partial coverage and an internal
`catalogue writer failed` error. The unchanged test passes alone (4.91 s).
Its scan path does not use the modified duplicate-reader sharing policy; the underlying
catalogue error is collapsed by the existing sink error mapping and remains unidentified.
The full service run is not green; the isolated pass does not establish its root cause.
Neither existing ignored measurement was changed or run.

## Limits

No personal inventory, cloud-placeholder qualification, application/browser or
native-shell run. No commit, push or publication. The signature is a point-in-time
observation; it does not grant file effects or promise future equality.

Intended commit: `Allow shared writes during duplicate inspection`.
