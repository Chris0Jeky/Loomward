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

## Review-round addendum - M1-M4 and L1 (10 October 2026)

Windows 11, non-elevated, on the shared host. Starting PR head:
`5c213838ce11ebd26a9d301c841cf7599000c67e`. The main comparison is an unmodified local
archive of `origin/main` at `6052bad5f4b3254b8252808c43a08ab314f2b0a6`; it was built separately
with `cargo build -p loomward-lab --release --offline`. The branch build used the same
command at the worktree root. No push, merge, personal scan or gate opening in this round.

This addendum supersedes the earlier receipt's `OutsideScope` containment classification,
DOS-path and case-insensitive comparison, and the obsolete `case_sensitive` mutation.
The source hashes below describe the final LF Git contents, including the two test files whose
earlier pre-commit hashes did not match the committed bytes. The preceding receipt is retained
as history, not current proof of those properties.

### Changed

- M1: normalised path query failures preserve the native error as `Incomplete(Io(code))`.
  A successfully queried path outside the root returns `Incomplete(ContainmentUnproven)`.
  Both count in `failed`, invalidate watch repair, leave coverage partial and prevent a sweep.
  `OutsideScope` now describes only the pipeline's own successful-listing targeted downgrade;
  its targeted-pass exemption is retained. The catalogue maps containment failures to `partial`,
  while a targeted downgrade remains `stale`.
- M2: containment uses `FILE_NAME_NORMALIZED | VOLUME_NAME_NT`, a 256-code-unit initial buffer
  and retry when `n >= len`, bounded by the native path limit. It queries the root once when
  root and directory share the same handle, and both root and child otherwise. Every listing
  still checks containment, including a queued first listing and repeated retained listings.
- M3: compare UTF-16 components exactly, including stored case. Synthetic `proj` / `PROJ`
  and Unicode case variants are refused; equal roots, actual aliases and within-root moves pass.
  The now-unused Windows globalization feature and ordinal-comparison call are removed.
- M4: a fixture-only injected `ERROR_NOT_SUPPORTED` (50) on every normalised path query
  reports one failed root listing, zero examined entries, partial coverage and no sweep.
  Another test injects the error only during a targeted repair after a successful baseline,
  preserving the earlier child rows and preventing full-root completion.
- L1: Find checks containment after ancestor pins, identity binding and path revalidation,
  before opening the search. Its pins stay alive throughout enumeration. Handle-based
  enumeration retains its immediate pre-list containment observation.

The regression surface is the native source and its fixture injection, engine repair and
relist tests, the incomplete-reason enum, catalogue outcome mapping, their Cargo feature
selection and this receipt. No new dependency. No edits to the prohibited status, checkpoint,
agent, authority or human-action files; `HUMAN_TODO.md` remains unchanged.

### Verified

Before the fix, three engine oracles failed at runtime: targeted query failure and a moved-out
directory incorrectly returned `complete: true`; an always-failing filesystem query reported
`failed: 0`. Three Windows oracles also failed at runtime: a real pipe-handle path failure was
misclassified, exact case comparison admitted the case variant, and query tracing observed
two root DOS queries with 32768-code-unit buffers instead of one NT query with 256.
The existing within-root and root-handle rename controls passed. Test setup compilation errors
were not counted as red evidence. Repair tests pass with Extended, Both and Find strategies;
the original relist tests still cover retained and reopened handles.

| Command (worktree root) | Result |
|---|---|
| `cargo fmt --all --check` | PASS, exit 0 |
| `cargo test --workspace` | PASS: 632 passed, 0 failed, 1 pre-existing ignored (#246), exit 0 |
| `cargo test -p loomward-windows -p loomward-engine --all-features` | PASS: 235 passed, 0 failed, 0 ignored, exit 0 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, exit 0 |
| `git diff --check` | PASS, exit 0 |
| `cargo test -p loomward-service --all-features containment_failures_are_partial` after mutation restoration | PASS: 1 passed, exit 0 |

All 11 mutations were killed by runtime assertions, with no compile failure counted:

| Mutation | Discriminating oracle |
|---|---|
| query failure becomes targeted `OutsideScope` | targeted repair remains partial |
| moved-out path becomes targeted `OutsideScope` | moved-out repair remains partial |
| injected failed query returns an empty successful path | every-query failure is counted and partial |
| case-fold components | exact-case component test |
| switch NT query flags back to DOS | actual query flags and count |
| restore the 32768 initial buffer | small-buffer query tracing |
| query the root twice | one-root-query tracing |
| remove long-path retry | native long-path query succeeds after growth |
| omit handle-strategy containment | moved-out relist refuses metadata |
| omit pinned Find containment | moved-out Find relist refuses metadata |
| catalogue containment outcome becomes `stale` | catalogue `partial` mapping test |

Mutated files were restored byte-for-byte. The final containment loop passed **30/30 runs**:
10 tests per run, **300 test executions**, with two separate synthetic I/O load workers.
The workers performed 22628 and 23480 successful 64 KiB
write/read cycles over 32 bounded disposable files; no load-worker error. Both stopped and
joined before their tree was removed and before the final benchmark. No new ignored test.
After mutations, the restored service mapping test, formatting and whitespace checks passed.

### M-tier native scan benchmark - shared-host

Used the existing **1M-file synthetic M tier on G:**, not a generated fallback.
`loomward-lab.exe bench scan --tier M --runs 1` ran in a fresh process for each measurement,
interleaved main / branch / main / branch / main / branch, with eight workers and the native
Extended enumerator. Aggregate-only in-memory sink, no SQLite. All six final-source runs
matched the exact manifest: 1,000,000 files, 6,325 descendant directories, logical and allocated
bytes and sink totals; complete, zero failed/skipped, no limit hit. No cache clearing or
host-policy change. Cache label is warm/uncontrolled, never a cold-cache claim.

| Pair | Main wall (s) | Branch wall (s) |
|---|---:|---:|
| 1 | 3.6255555 | 0.7830042 |
| 2 | 0.7177986 | 0.7198157 |
| 3 | 0.7070002 | 0.7798874 |
| Median | 0.7177986 | 0.7798874 |

| Median metric | Main | Branch |
|---|---:|---:|
| Files/s | 1,393,148 | 1,282,236 |
| Enumeration wall (s) | 0.2580669 | 0.2947733 |
| Summed enumeration worker (s) | 1.5290579 | 1.9160017 |
| Process peak working set (bytes) | 11,472,896 | 12,218,368 |

Final-source median wall time was 8.65% higher;
median throughput was 7.96% lower.
Three samples on a concurrent host do not establish a stable performance bound. The first
main sample was visibly slower and remains included. An initial interleaved batch before the
final query-tracing flag binding and service assertion gave medians
0.7769949 s main / 0.7852800 s branch;
the final-source batch above follows a rebuild. Both batches passed the exact manifest oracle.
All timings are **shared-host/uncontrolled**.

### Final source SHA-256 (LF Git contents)

| File | SHA-256 |
|---|---|
| `crates/loomward-engine/Cargo.toml` | `1e19c32b6eb9e068425f09bf489822d005eac8bd8e36a469ba0939954f79e311` |
| `crates/loomward-engine/src/scan/followups.rs` | `f7ced4f2f38a29455d0d7fb28f4c2c9c7f3a8899d0900b83607ee40a9dc039f0` |
| `crates/loomward-engine/src/scan/pipeline.rs` | `d00eeaf6d3a907c76376d405769fd0ac9410f75357a1e2c720c836690e013d4a` |
| `crates/loomward-engine/src/scan/watch.rs` | `d1c8c8fff824144d6987d14d2fb409743b543ba79676665952aace6e1b94dcd7` |
| `crates/loomward-service/src/sink.rs` | `c8da1ac7f4b785cdc7bd464bb3cdf45472138fc64f4c6f06ed909f625aa70488` |
| `crates/loomward-windows/Cargo.toml` | `0d3bdfdc18bbaf38e52f3667bb84f659f3ec7d9b402d486ddfab2f17eb591e8c` |
| `crates/loomward-windows/src/enumerate/mod.rs` | `72d9e93fadb9bbf27b17a4064ff9a990652d84c70f5e966309f71bca0984f76a` |
| `crates/loomward-windows/src/enumerate/native.rs` | `133bc9f942a73c6c1cd5d9d6a255266585613958e28d7d59914d0e8c6b25351a` |
| `crates/loomward-windows/src/enumerate/tests.rs` | `b5eb5e3ba13b19ee8f50b7863a69d875f19fe424ea13820bf7d095c012bdc284` |

Raw gate, red/green, mutation, load and benchmark logs and the local runners remain ignored
under `target/contain269-round2/`; no operational output or selected private path is committed.
The final staged/committed source hashes are checked against the table before handoff.

### NOT verified / residual risk

No newly pushed-head hosted CI, personal scan, physical ReFS/FAT/exFAT, RAM-disk or user-mode
filesystem qualification, forced ID reuse, cold-cache benchmark or case-sensitive NTFS sibling
fixture. Case variants are synthetic component tests; unsupported-query behaviour is injected
and also exercised with a real failing pipe handle. No new capability gate is opened.
Handle enumeration does not atomically exclude a namespace move after its final path query;
Find now retains its namespace pins across the containment check and the entire search.
`HUMAN_TODO.md` is unchanged; personal teacher disclosure remains owner-controlled.
