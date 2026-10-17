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
- Ordinary `search.query` hits carry F1's `freshness: {state, at}`; `at` is `null` for a `live`
  hit (no catch-up time yet), while stale and caught-up hits carry it (#553). `federated_search` and the ranked
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

## PR #531 fix round (2026-10-17)

Fix-round starting HEAD: `c406fdee798b5e2f125bd8b09ded70ef12314514`, branch
`feat/404-f3-watchers-usn`, KRASPYON / Windows 11 Home. This addendum supersedes the
earlier driver-handoff state above. The Sol lane leaves its edits uncommitted for the driver;
no commit, push, PR mutation or merge is part of this lane.

### Changed and regression receipts

1. The restart epoch is captured with `Tracked` insertion, before native watch startup, and
   passed into catch-up. `f3_watch_event_between_start_and_journal_read_cannot_be_caught_up`
   injects the actual tracker `RootDirty` event after watcher startup and before journal access.
   Automatic retries are held so a subsequent full scan cannot hide the result. Before the fix:
   **0 passed / 1 failed**, `lost the in-window watch event` (`fix-epoch-red.log`). After the
   epoch-only fix the same regression passed in `fix-index-red-baseline.log`.
2. Journal IO/decode completes without the service DB mutex. Resolution uses batches of 128
   records, deduplicates parent/file identities, and takes/releases that mutex once per batch.
   The watch-setup mutex ends after the watch is installed; revoke can stop it during replay.
   Completion tolerates a root removed by revocation. Deadline/lookup failure retains stale,
   full-refresh-required coverage. Visibility walks upward only from identity matches, and
   rejects excluded/absent ancestors. Previous in-root parents (including multiple hard links)
   remain refresh scopes; a journal parent never grants a path or an open.
   `f3_journal_replay_does_not_hold_db_or_watch_setup` pauses the journal-read seam and checks
   that a service read and revocation can finish before releasing it. It failed on the reviewed
   implementation (`journal replay held the service database mutex`). The query-plan test also
   failed there: the old query walked the whole visible tree rather than seeking by file ID.
   The combined red receipt was **10 passed / 2 failed** (`fix-index-red-baseline.log`), with
   the epoch fix already green. An initial indexed rewrite still allowed SQLite to reorder
   joins into range scans; the plan assertion caught it. The final query fixes the input-first
   join order and materializes candidate identities. `fix-reach-green.log` records **6 passed /
   0 failed**, including the query-plan assertion and the 100k-directory fixture. Relevant plan:

   ```text
   SEARCH d USING INDEX dir_identity (root_id=? AND file_id=?)
   SEARCH f USING INDEX file_identity (file_id=?)
   SEARCH d USING INTEGER PRIMARY KEY (rowid=?)
   ```

3. Record classification is now a pure function of `(record, lookup)`. Unit cases pin move-in,
   move-out through the previous catalogued parent, rename within the root, a hard-link event
   from outside, outside and unresolved parents (both skipped), and new-directory chains across
   batch boundaries. Database tests additionally pin hidden ancestors, all visible in-root
   hard-link parents, foreign-root refusal, V2/128-bit aliasing and no V3 truncation. Deadline
   expiry before/after a batch and query failure return loss of coverage rather than empty
   success. Decoder coverage tests valid aligned padding, non-aligned record lengths, odd name
   offsets, overflowing name lengths and unknown versions for both V2/V3, requiring refusal
   without panic. These are added coverage and extraction checks; no decoder behavior was
   changed, and no pre-existing decoder failure is claimed.

### 100,000-directory catch-up resolution measurement

`f3_catch_up_100k_directories` constructs an in-memory catalogue using the production
`catalog.sql` table/index definitions: one root, 99,999 direct child directories, and 128
decoded new-file journal records with distinct known parents. Both paths must select exactly
the same 128 scopes. Construction, native journal IO and subsequent refresh scans are excluded.
The before path preserves the reviewed per-record visible-tree CTE **and** previous-parent
lookup; it runs to completion to measure the work, without stopping at the old five-second
deadline. The after path is the production batched resolver, including its five-second deadline.
The old measured workload exceeds that budget: production would abandon continuity and stay
stale, rather than wait for all 72.6 seconds of resolution work.

| Same-process sample, shared host | Elapsed |
|---|---:|
| Before: reviewed per-record resolution | 72.6211607 s |
| After: indexed batch resolution | 107.1768 ms |

Command: `LOOMWARD_F3_BASELINE=1 cargo test -p loomward-service --lib f3_ -- --nocapture`
(PowerShell sets the environment variable before invoking Cargo). Receipt:
`fix-f3-comparison.log`. This is one paired synthetic sample on a shared host, not a cold-disk
or end-to-end large-root latency claim. The earlier parent-only baseline was 47.1132031 s;
the final paired sample above includes both old queries and is the comparable result. Ordinary
test runs omit the slow before measurement but still build the 100k fixture, exercise the indexed
resolver and assert exact scopes within its deadline.

### Fix-round verification and remaining test concern

The final source is the uncommitted working tree based on the starting HEAD above. Commands
below use its absolute `--manifest-path`, `CARGO_BUILD_JOBS=4`, the lane's separate target
directory, and `RUST_TEST_THREADS=1`. The slow baseline flag is set only for the paired comparison.

| Command | Observed result / receipt |
|---|---|
| `cargo fmt --all --check` | Exit 0; `fix-fmt-final.log` |
| `cargo clippy --workspace --all-targets -- -D warnings` | Exit 0, no warnings; `fix-clippy-final.log` |
| `cargo test -p loomward-service --lib freshness:: -- --nocapture` | 6 passed, 0 failed; `fix-reach-green.log` |
| `LOOMWARD_F3_BASELINE=1 cargo test -p loomward-service --lib f3_ -- --nocapture` | 12 passed, 0 failed; `fix-f3-comparison.log` |
| `cargo test -p loomward-service -p loomward-windows -p loomward-engine --no-fail-fast` | Exit 101: 645 passed, 2 failed, 1 ignored; `fix-crates-final.log` |
| `cargo test -p loomward-service --lib -- tests::a_junction_replacing_a_directory_stays_visible_as_excluded tests::a_live_scanned_ntfs_grant_can_be_labelled_and_collected --exact --nocapture` | Exit 0: both passed with unchanged source/assertions; `fix-conflicts-isolated.log` |

The required crate run had two service failures outside the reported #292 set:

- `a_junction_replacing_a_directory_stays_visible_as_excluded`: its foreground `scan.start`
  received retryable `Busy`, `root already scanning`.
- `a_live_scanned_ntfs_grant_can_be_labelled_and_collected`: `collections.members` received
  retryable `StaleGeneration`, `collection changed while reading` / `revision_changed`.

These errors are consistent with background refresh overlapping foreground calls. Both tests
start with a fresh service, before restart journal catch-up is used, and passed the isolated
control unchanged. This establishes intermittent outcomes; it does **not** establish a
base-HEAD reproduction or the precise notification timing. No assertions, skips, scan budgets
or production retry semantics were changed to hide them. All 16 F3/resolver tests in that same
broader run passed. The five task-listed #292 cases also passed; this lane does not fix #292.
The one existing ignored Windows test is `effects::exec::tests::throughput_lab`.

| Final broader command / coverage | Observed result / receipt |
|---|---|
| `cargo test --workspace --no-fail-fast` | Exit 101: **1,306 passed, 1 failed, 1 ignored**, across 74 targets; `fix-workspace-final.log` |
| Service unit tests in that workspace run | **231 passed, 0 failed**; both earlier conflict cases and all F3/resolver tests passed |
| USN decoder/bookmark tests in that workspace run | **4 passed, 0 failed**, including the added V2/V3 alignment/offset/length/version cases |
| Default catalogue crash sweep in that workspace run | **9 passed, 0 failed**; no catalogue write/schema changes were made by this fix round |
| Same workspace-built `pathological-ac73e433cb4da9c8.exe --exact concurrent_create_delete_rename_grow_reconciles_exactly_to_fresh_scan --nocapture` | Exit 0: **1 passed, 0 failed**, unchanged binary; `fix-churn-isolated.log` |
| `git diff --check` | Exit 0 |

The workspace's single failure was
`loomward-engine/tests/pathological.rs::concurrent_create_delete_rename_grow_reconciles_exactly_to_fresh_scan`,
at the catalogue-observation comparison (line 596). Its filesystem file count and `(path, logical
bytes)` checks passed. The failed row was the root directory: all compared fields matched except
`sub_complete`, **0 versus 1** in the fresh scan. This integration uses `Service`; neither service
starts from an existing scan/bookmark, so restart catch-up is not used. The foreground refresh
had reported complete before the observation was taken. A background refresh overlapping that
observation is consistent with the documented targeted-pass partial-coverage behavior, but its
precise scheduling was not instrumented. This case passed in the earlier required-crate run,
failed in the workspace run, then passed the **same workspace binary** in isolation: 4,000
initial files, 128 mutation batches, 65 overlapping enumeration, 4,041 matching catalogue
observations, and 4,000 files matching the filesystem walk. No test or production behavior was
changed between these runs.

**The workspace gate remains red.** The isolated pass is not a replacement for an all-green
workspace receipt. Resolving the lifecycle tests' background-refresh/quiescence assumption is
a separate remaining concern, outside the three requested fixes. A next bounded investigation
can capture the outstanding refresh job and root coverage at the churn test's observation
snapshot. No origin/main or base-HEAD reproduction is claimed for these intermittent outcomes.
The #292 cases did not reproduce in either broader run.

### NOT verified / residual risk / driver state

Hosted CI for these uncommitted edits, non-NTFS/non-Windows qualification, sustained-churn soak,
and crash injection at the service bookmark write remain unverified. The existing ignored
`throughput_lab` was not enabled. The paired timing measures decoded-record resolution only;
native journal IO and subsequent refresh latency are excluded. The existing targeted scan then
full-root fallback remains unchanged.

All raw receipts named in this addendum remain in the external lane target, outside Git.
`HUMAN_TODO.md`, the tier/delegation files, `AGENTS.md`, and the protected implementation-status
and checkpoint files are unchanged. No new owner decision is needed. The worktree remains on
`feat/404-f3-watchers-usn` at the starting HEAD with the three fixes and this receipt uncommitted
for the driver; the new `src/freshness/reach.rs` module is untracked until the driver adds it.
No commit, push, PR edit, merge or cleanup was performed. The worktree is not removal-ready.

## Round 2: quiescent catalogue comparison (PR #531)

2026-10-17, KRASPYON, Windows 11 Home/MSVC, uncommitted changes over `bab9d42`.
The control uses local `origin/main` at `c8056bd8697eb2bfe7707e03822ea5610a2799fa`
in a separate detached worktree. Every Cargo invocation sets `CARGO_BUILD_JOBS=4`.
The branch, base and load build use separate external target directories; raw receipts
below remain in the branch's external target directory, outside Git. Native churn runs
set `LOOMWARD_REQUIRE_NATIVE=1`, so an unavailable native case cannot pass vacuously.

### Finding and discriminating seam

The comparison's assumption was too strong: a completed foreground job is not a
quiescent service catalogue. The existing targeted pipeline deliberately leaves
root coverage partial; F3 schedules a full-root fallback before clearing stale freshness.

`f3_completed_foreground_job_does_not_mean_background_catalogue_is_quiescent`
deterministically queues a name hint for a synthetic child directory through
`Tracker::record`, then holds the resulting background targeted scan just after
catalogue publication, before freshness completion. At that observation:

- The earlier foreground job still reports `completed`, coverage `complete`.
- The persisted root's `sub_complete` is **0**, reproducing the mismatching field.
- The tracker is stale, and the public search response reports partial coverage
  and stale hit freshness, rather than falsely reporting complete/live.
- Releasing the gate and waiting through freshness plus job idleness lets the
  full fallback finish; persisted root `sub_complete` is then **1**.

The gate and fixture preparation compile only in tests. The latter clears completed
fallback bookkeeping and ages the debounce clock on an idle fixture, forcing the
targeted path rather than relying on native scheduling. Initial setup probes that
waited for an arbitrary native/injected notification did not reach the targeted gate;
their timeout receipts are `round2-overlap-seam.log` and `round2-overlap-seam-2.log`.
The second reported `full=true`, `auto=false`, no pending scopes and a healthy watch.
The controlled targeted setup passed (`round2-overlap-seam-3.log`); the final suite
also checks the public partial/stale fields added after that initial seam receipt.

This establishes the proposed background-publication overlap through the production
SQLite sink. It does not reconstruct the exact scheduling of the earlier workspace
failure: an instrumented unchanged comparison passed 10/10 here
(`round2-instrumented.log`). The state directories are siblings of the synthetic scan
root, so their catalogue/WAL writes are outside watcher reach. The watch filter excludes
last-access notifications. No self-generated catalogue-write event or incorrect
freshness claim was observed, and no production refresh behavior is changed.

### Change and proof

The churn test now waits for the service's **public search freshness/coverage and
job-idle signals**, with a 300-second deadline and diagnostic failure, before
observing either catalogue. It yields while checking those signals; it adds no
sleep or fixed settling delay. The filesystem walk, exact file tuples, stable
observation fields, row counts and root `sub_complete` comparison are unchanged.

The load process repeatedly rebuilds `cargo build --workspace` in its own target,
cleaning only its engine/service/lab/http build artifacts between builds so warm
Cargo no-ops cannot stand in for compilation load. `round2-load-continuous.log`
records build start/end timestamps spanning the base and fixed-test loops.

All commands use the appropriate absolute `--manifest-path`; below it is omitted
for readability. The control's manifest belongs to its detached worktree; every
other command uses this branch. The repeated case is
`concurrent_create_delete_rename_grow_reconciles_exactly_to_fresh_scan`.

| Command / check | Result / receipt |
|---|---|
| Control: `cargo test -p loomward-engine --test pathological concurrent_create_delete_rename_grow_reconciles_exactly_to_fresh_scan -- --exact --nocapture`, 10 iterations under load | **10 passed, 0 failed**; `round2-base-load-10.log` |
| Fixed branch: same command, 20 iterations under load | **20 passed, 0 failed**; `round2-fixed-load-20.log` |
| `cargo fmt --all --check` | Exit 0; `round2-fmt.log` |
| `cargo test -p loomward-engine -p loomward-service --no-fail-fast` | Exit 0: **474 passed, 0 failed, 0 ignored**, including the final deterministic seam; `round2-crates.log` |
| `cargo clippy --workspace --all-targets -- -D warnings` | Exit 0, no warnings; `round2-clippy.log` |
| Default catalogue crash sweep, within the workspace command | **9 passed, 0 failed**, 168.86 s; `round2-workspace.log`. `LOOMWARD_CRASH_SWEEP` was unset: this is the default sampled sweep, not the exhaustive `full` mode. |
| `cargo test --workspace --no-fail-fast` | Exit 0: **1,308 passed, 0 failed, 1 ignored**, across 74 targets; `round2-workspace.log` |
| `git diff --check` | Exit 0 |

The base control ran from 11:41:30 to approximately 11:43:11 local time; the fixed
loop ran from 11:43:28 to approximately 11:47:25. Each successful case reported
4,041 exact catalogue observations and 4,000 files matching the independent
filesystem walk. The detached base worktree's `git status --porcelain --ignored`
was empty after its control completed; it was removed with plain `git worktree remove`.

The broader gates set `RUST_TEST_THREADS=1` to limit pressure on the shared host;
the separate build workload continued during their initial phase. The workspace
service suite passed **232/232**, including the new deterministic overlap test.
The existing ignored case is `effects::exec::tests::throughput_lab`; it was not
enabled or changed. `round2-gates.json` records all three broad command exit codes
as zero. The toolchain reports Rust 1.97.1, `x86_64-pc-windows-msvc`.

### NOT verified / residual risk / driver handoff

The historical natural failure's exact notification scheduling was not recaptured;
the deterministic seam reproduces its partial-root observation and proves the
freshness/idle condition needed before comparing. The 20-run load sample is bounded,
not a sustained-churn soak. Hosted CI for the uncommitted change, non-Windows
qualification and the existing ignored throughput lab remain unverified.

`HUMAN_TODO.md` and all other protected files are unchanged; no new owner decision
is needed. No commit, push, PR mutation or merge was performed. The branch remains
`feat/404-f3-watchers-usn` at `bab9d4290c9f395e233c9f5ba761e152fd7713ca` with five
modified files left for the driver. Suite-created synthetic fixtures remain under
the ignored `target/test-roots/`; raw receipts are outside the worktree in
`C:/lw/target/search-f3/`, so they survive later worktree removal. The worktree is
not removal-ready while these edits are uncommitted.


## Issue #545: NULL-directory identity catch-up (2026-10-17)

KRASPYON / Windows 11, `fix/545-f3-null-file-id`, base `origin/main`
`4acd744393bbe6e1970c6bd4967d59006085026b`; committed in #548 (merge `28e5752`).

**Reachable:** the production catalogue writer fixture creates a verified NTFS grant,
root state `complete`, current complete aggregates, `sub_noid=0` and one NULL-id
directory. Engine `scan/pipeline.rs` accepts successful identity-less child opens
(existing synthetic `NameOnlyTree` tests); the service sink preserves missing IDs.
Catalogue `writer.rs::publish` accepts them, while `rollup` computes completeness
independently of identity. `sub_noid` counts files, so it misses directory gaps.

Decoded-record replay now checks NULL directory IDs or positive `sub_noid` once per
root. Unresolved records with either condition retain stale/full-root-required
coverage; fully identified roots retain the outside-parent skip. Existing journal,
deadline, batch and scope-count bounds remain. Item 2 also fixes `new_dirs` overriding
known directory reach; the rename test now retains the directory's own child scope.
The regression exercises production replay/completion without automatic repair:
baseline logic **6 passed / 2 failed** (`noid-red-clean.log`), fixed **8 / 0**
(`noid-green.log`), including identified-root and `sub_noid` controls and the 100k test.

Cargo uses `--manifest-path C:/lw/wt/f3-noid/Cargo.toml`, `CARGO_BUILD_JOBS=4`,
`CARGO_TARGET_DIR=C:/lw/target/f3-noid`, `RUST_TEST_THREADS=1`. Receipts stay there.

| Command (manifest omitted) | Result / receipt |
|---|---|
| `cargo test -p loomward-service` twice | Each 237 passed, 0 failed; `service-1.log`, `service-2.log` |
| `cargo test -p loomward-windows` | 120 passed, 0 failed; `windows.log` |
| `cargo test -p loomward-engine` | 241 passed, 0 failed; `engine.log` |
| `cargo fmt --all --check` | Exit 0; `fmt.log` |
| `cargo clippy --workspace --all-targets -- -D warnings` | Exit 0, no warnings; `clippy.log` |
| `cargo test --workspace --no-fail-fast` once | 1,335 passed, 0 failed, 1 ignored, 75 targets; `workspace.log` |
| `git -C C:/lw/wt/f3-noid diff --check` | Exit 0 |

**NOT verified / residual risk:** physical NTFS identity loss, non-Windows execution,
hosted CI, app/Tauri, exhaustive crash sweep and the existing ignored throughput lab. Identity gaps conservatively require
a full refresh even for an unrelated outside event. `HUMAN_TODO.md` and protected
files are unchanged; no new owner decision. No commit, push, PR mutation or merge;
the uncommitted worktree is not removal-ready.
