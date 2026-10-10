# Relist containment - #269

Windows native tests, 10 October 2026. Source on `fix/relist-containment-269`, measured before commit,
based on `85621b7b2feb64c91439f8d2ccdb0dd389cff9a9`. Commands ran at the worktree repository root.
Synthetic disposable directories only; no personal scan, commit, push or remote publication.

## Changed

- Every native listing re-proves directory containment before emitting metadata, including
  reopened and retained handles. Failure returns `Incomplete(OutsideScope)`; the existing
  pipeline publishes that outcome and keeps the directory and run incomplete, with no sweep.
- Choose normalised handle paths over an NTFS parent-ID walk: the same query works across
  native enumeration strategies and retained-handle filesystems, including the ReFS path.
  Physical ReFS qualification was not run. Both operands use
  `GetFinalPathNameByHandleW(FILE_NAME_NORMALIZED | VOLUME_NAME_DOS)` and component-wise
  `CompareStringOrdinal` with Windows case-insensitive comparison. String prefixes cannot
  admit a sibling whose name merely starts with the root name.
- Each native directory shares its pinned root handle. The pipeline retains and accounts for
  the root until scan completion; reopen accepts that handle instead of the stored path.
  Root rename follows the granted object, even with a new directory at the old pathname.
  In the retained-descendant fixture Windows refuses the root rename; the test asserts
  `PermissionDenied`, then proves the unchanged root still relists completely.
- The reopen trait callers, native handle-release oracle and Windows API feature selection
  are updated together. Identity, volume, attributes, elevation and no-recall checks remain.
  `personal_scan_gated_184` stays enforced.

API references: [normalised handle paths](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew)
and [Windows ordinal comparison](https://learn.microsoft.com/en-us/windows/win32/api/stringapiset/nf-stringapiset-comparestringordinal).

## Verified

The moved-out and root-rename oracles failed against the original implementation.
The within-root control passed originally. The final three engine tests cover retained and
reopened handles with Extended, Both and Find enumeration. Three native unit tests cover
component boundaries, Unicode case, drive boundaries, root equality, normal/verbatim/short
path aliases and a real failed handle-path query. Every new test has a runtime-failing mutation.
Elevated-host branches assert refusal; this run exercised the native non-elevated paths.

| Command | Result |
|---|---|
| `cargo fmt --all --check` | PASS: exit 0 |
| `cargo test --workspace` | PASS: 627 passed, 0 failed, 1 ignored; exit 0 |
| `cargo test -p loomward-windows -p loomward-engine --all-features` | PASS: 231 passed, 0 failed, 0 ignored; exit 0 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS: exit 0 |
| `git diff --check` | PASS: exit 0 |

The workspace ignore is pre-existing: `verbatim_device_namespace_state_paths_are_refused`,
open defect #246. No new test was ignored. The personal-scan refusal test passed in the workspace.

Nine mutations were killed by runtime test failures, not compilation errors:

| Mutation | Discriminating test |
|---|---|
| `omit_containment` | `native_relist_containment_refuses` |
| `reject_all` | `native_relist_containment_allows` |
| `old_root_path` | `native_relist_containment_follows` |
| `case_sensitive` | `normalized_containment_compares` |
| `allow_query_failure` | `native_containment_failed` |
| `no_root_normalization` | `native_containment_normalizes` |
| `lose_root_anchor` | `native_relist_containment_refuses` |
| `root_release` | `native_child_handles_close_but_root` |
| `omit_containment_retained` | `native_relist_containment_refuses` |

All mutated files were restored byte-for-byte before final gates.
An initial mutation-runner text-encoding error produced compile failures; none was counted.
Explicit UTF-8 writes fixed the runner, and all nine reported mutations failed at runtime.

Loaded loop: **30/30 passes**, six tests/pass (180 test executions),
18 native pipeline scenarios/pass (540 scenarios). Two bounded file-I/O load threads used
a separate disposable synthetic tree and stopped/joined before cleanup. No load-worker error.
Raw logs and runner receipts stay ignored under `target/contain269/`.

After the full gates, only an explanatory code comment and this receipt changed. Final
`cargo fmt --all --check`, `git diff --check` and the six-test `containment` filter passed again.

## Tested source SHA-256

| File | SHA-256 |
|---|---|
| `crates/loomward-engine/src/scan/followups.rs` | `064ac8a05421dc0bec66e8cb2b5ed3ee4afcb6b16d49a88c655906a6e8c0f2e8` |
| `crates/loomward-engine/src/scan/pipeline.rs` | `d00eeaf6d3a907c76376d405769fd0ac9410f75357a1e2c720c836690e013d4a` |
| `crates/loomward-windows/Cargo.toml` | `dbbfa475a32334cfd71d94dc041e0efdcde8d6e848e5481ac787df1284324781` |
| `crates/loomward-windows/src/enumerate/mod.rs` | `7ac44560d89b5532d4864b41febca782aa865cef5627f8c24f9a77d027727081` |
| `crates/loomward-windows/src/enumerate/native.rs` | `4dce669d380df1be4fd65556b0e9c995aed421c902af04473d7ed77034a4c77b` |
| `crates/loomward-windows/src/enumerate/tests.rs` | `7476413605927ceb618224e9d61b857fad7cecd28a2322abbc7e2a170569f312` |

## NOT verified / residual risk

No hosted CI, physical ReFS/FAT/exFAT qualification, forced ID reuse, long benchmark,
personal scan or atomic exclusion of a concurrent move after the final path check.
Containment is an observation immediately before enumeration; this change does not lock
the namespace against later renames. The short-name API alias is exercised even when
short-name generation is disabled, so this is not universal physical 8.3 qualification.

Declined the parent-ID walk: filesystem-specific availability adds complexity to a boundary
already expressible using native normalised handle paths. No requested check was declined.

