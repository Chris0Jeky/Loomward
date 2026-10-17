# Find F3: persistent watches and restart freshness

2026-10-17, KRASPYON, Windows 11 Home, native Rust/MSVC. Base `77c4d34`
(F1 #476, F2 #513). Refs #404 (LW-113), #13 (LW-007), #14 (LW-008).
Peer implementation in `feat/404-f3-watchers-usn`, left uncommitted for the driver.
Only disposable synthetic trees were scanned or mutated. No elevation, personal scan,
journal modification, download, package publication or external provider was used.

## Non-elevated measurement

A read-only Python/ctypes probe checked `TokenElevation` (0) before opening handles on the
task-authorised workspace's volume. The valid read used the queried journal ID and next USN,
`READ_USN_JOURNAL_DATA_V1`, versions 2..3, all reasons, and zero timeout/bytes-to-wait.
No journal records, names, IDs or machine paths were printed or retained.

| Handle / operation | Native result |
|---|---|
| Volume handle, `GENERIC_READ` | Open refused: error 5, `ERROR_ACCESS_DENIED` |
| Volume metadata handle / query | Error 1, `ERROR_INVALID_FUNCTION` |
| Granted directory, `FILE_LIST_DIRECTORY`, backup semantics / query | Success, 80 bytes |
| Same directory / `FSCTL_READ_USN_JOURNAL` | Error 5, `ERROR_ACCESS_DENIED` |
| Same directory / `FSCTL_READ_UNPRIVILEGED_USN_JOURNAL` | Success, 408 bytes (including concurrent volume activity) |

The elevated helper is unnecessary for this measured path and remains gated. The service restart
test also exercises the actual guarded Rust directory-handle implementation and journal records;
it is not a simulated success substituted for the measurement.

## Changed

- The service arms one native recursive watcher per active, identity-verified root that passes the
  current session's scan gate. Watches survive scans and stop on revocation, identity change and
  service shutdown. Unsupported/offline roots remain stale. The engine's standalone `RootWatch`
  retains its scan-scoped lifetime; the service reuses the native watcher and engine refresh pipeline
  without enabling the catalogue sink's unsupported scan-time watch callbacks.
- `state.db.root_freshness` stores a bookmark per durable grant, tied to its catalogue instance.
  The grant binds the volume and root identity. The bookmark is committed with FULL state-store
  synchronisation **after** a complete, quiet full scan publishes. It retains the **pre-scan** journal
  head: conservative replay avoids losing a change racing the final listing. A crash between
  catalogue publication and bookmark persistence replays older hints instead of skipping changes.
- On restart, V2/V3 records are validated, retain their identity widths, and never supply paths.
  Directory ancestry and previous catalogue parents select scopes only within the granted root.
  New directories inherit the nearest known ancestor. Moved-out objects refresh their old parent;
  same-volume siblings never become roots or results. NTFS width matching is qualified here;
  ReFS journal catch-up remains unsupported and visibly stale.
- Kernel overflow/failed-watch hints use the existing native `RootDirty` classification and the
  engine's full-root refresh path. Journal wrap/recreation, unsupported records, missing bookmarks
  or failed journal access leave a dirty full-root scope; a successful explicit full scan repairs it.
- Ordinary `search.query` hits carry F1's `freshness: {state, at}`. `federated_search` and the ranked
  index stay off. Search page coverage also exposes tracked stale roots using its existing state
  fields; `roots.list` retains the historical scan state. No protocol or frontend extension is needed.

## Bounds and limitations

Journal reads stop at a captured head, at most 256 x 64 KiB per root. Scope classification has
a five-second budget, a 100,000-entry ancestry cache and a 1,024-scope cap; exhaustion leaves
the root dirty. Pending watcher paths are capped at 1,024. Refresh work is event-driven and
debounced, with at most three consecutive unsuccessful attempts. A successful full scan resets
the budget even if later hints require another pass; otherwise late batches could leave a quiet
root stale permanently. The regression fixture pins this case with an exhausted retry budget.
There is no periodic whole-disk scan. Stale timestamps conservatively retain the last trusted
observation time, rather than claiming coverage until the later notification time.

**Performance limitation:** the existing targeted pipeline enumerates the root and only authorises
absence in selected directories; it leaves root coverage partial. This slice runs that existing
targeted pass, then one full-root fallback before claiming freshness. Optimising this into a truly
local, coverage-preserving refresh is a separate slice. Continuous churn or errors can exhaust
the attempt budget and leave stale results until an explicit refresh.

## Verification

All cargo commands use the task's separate target directory and `CARGO_BUILD_JOBS=4`.
Workspace and service tests use `RUST_TEST_THREADS=1`. Commands run from the assigned worktree,
with its absolute `--manifest-path`; commands below are shown relative to that root. Every Git
command uses `git -C` for that worktree. Raw logs stay in the external lane target, outside Git.

| Final command / coverage | Result |
|---|---|
| `cargo fmt --all --check` | Exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | Exit 0, no warnings |
| `cargo test -p loomward-service --lib f3_ -- --nocapture` | 8 passed, 0 failed |
| `cargo test --workspace --no-fail-fast` | Exit 0: **1,231 passed, 0 failed, 0 ignored**, across 73 unit/integration/doc-test targets |
| Touched crates within the final workspace run | Catalogue 257; engine 241; service 215; Windows 122; all passed (integration/doc tests included) |
| Default catalogue crash sweep, included in the workspace run | 9 passed, 0 failed; `test-hooks` is enabled by the service's existing dev dependency |
| New journal decoder/bookmark tests, included in the Windows tests | 3 passed, 0 failed |
| `git diff --check` | Exit 0 |

The five #292 cases identified in the task did **not** reproduce on this checkout/host: the four
engine relist/dirty-epoch cases and the Windows reopen-after-rename case passed. This lane does
not claim to fix #292.

Earlier runs found and fixed two regressions: the initial touched-crate run had 830 passes and one
failure because an extra `roots.list` override changed historical scan-state semantics (removed);
the first workspace run had 1,228 passes and one failure because late watch batches exhausted the
refresh budget despite successful full scans. A controlled exhausted-budget regression reproduced
that stale state (0 passed, 1 failed) before the reset fix. Removing watch setup/removal serialization
also made the revocation regression fail with a surviving watch. A subsequent lock-order test exposed
a state-store guard held across the setup wait; only its owned, deadlocked test process was stopped,
the guard lifetime was fixed, and both race/budget cases pass in the final complete suite. No existing
test assertions, skips or deadlines were weakened.

- Red regression: `cargo test -p loomward-service --lib f3_search_reports_freshness_without_enabling_federated_search -- --nocapture`
  reached a real search result and failed because its hit had no `freshness` field. The corrected
  request includes all required nullable v1 fields; an earlier invalid request was not counted as red.
- Native F3 tests cover restart rename/delete/create against a fresh scan and independent filesystem
  totals, persistent watch updates, overflow-to-stale with a timestamp, journal-ID and retention gaps,
  unchanged restart `caught_up`, revocation/forget, revocation racing watch setup, and the federation
  flag staying off. Setup/removal are serialized so an in-flight watch cannot outlive the revoke
  acknowledgement or be recreated during shutdown.
- Overflow is injected through the same `RootDirty` hint used by native zero-byte/1022 completions;
  this service fixture does not claim a forced kernel-buffer overflow measurement.
- Journal unit tests cover V2/V3 widths, the directory bit, truncated/unsupported records, journal
  recreation, wrap and future cursors.

## NOT verified / residual risk

No ReFS/FAT journal qualification, Linux run, multi-volume or large-root latency qualification,
real reboot, abrupt service-process termination at a new bookmark write, or sustained-churn soak.
The catalogue's existing crash sweep is distinct from injecting a crash into this new service-state
write. No UI rendering changes were made. `HUMAN_TODO.md` is unchanged; no new owner decision is needed
for this non-elevated, granted-root slice. Existing owner-only helper/disclosure choices remain open.

## Driver handoff

The worktree stays on its assigned branch at base `77c4d34`, with the implementation and this receipt
uncommitted. No push, PR or merge was attempted. The ignored `target/` is preserved, including test
fixture output; it is not removal-ready while the source is uncommitted. Raw logs in the external
lane target are `workspace-final.log`, `clippy-final.log`, `f3-final.log`, and the earlier negative
receipts `crates-test.log`, `workspace-test.log`, `budget-red.log`, `revoke-red.log`.
