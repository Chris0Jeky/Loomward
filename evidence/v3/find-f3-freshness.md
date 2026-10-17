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

## Issue #550: bounded identity-gap probe and replay edges (2026-10-17)

KRASPYON / Windows 11 Home, lane `fix/550-f3-identity-gap`, starting HEAD
`de301c4bf660a0d4b3c39678755667d178b374b0`. Synthetic SQLite fixtures only.
Commands use `--manifest-path C:/lw/wt/f3-gap/Cargo.toml`,
`CARGO_TARGET_DIR=C:/lw/target/f3-gap`, `CARGO_BUILD_JOBS=3`,
`RUST_TEST_THREADS=1`. Raw logs stay in that target directory, outside Git.

### Query measurement

Before changing the production gap query, `LOOMWARD_F3_GAP_SCALE=1 cargo test
-p loomward-service --lib f3_gap_probe_scale -- --nocapture` built a complete,
fully identified synthetic root with **1,000,000 directories** (one root and
999,999 direct children). The old gap probe took **384.0116 ms / 7,000,016 VM
steps**. A replay selection with one decoded file record under the final directory
took **716.2383 ms / 7,000,414 VM steps**, with a progress callback counting each
SQLite instruction (`gap-baseline.log`). That callback adds overhead; this is not
an uninstrumented wall-time claim.

The replacement reads the root directory's current `sub_noid` aggregate by its
primary key and probes `dir_missing_identity(root_id)`, a partial index containing
only NULL directory IDs that are not `absent_pending`. Fresh catalogues and existing
catalogues gain the index; it is derived state, so no precious-state migration or
schema-version change is needed. Index construction is a one-time directory scan
when an existing catalogue first opens. No directory scan remains in the no-gap
catch-up query.

The retained comparison test runs both SQL paths on the same fixture. It asserts
the identical target set and fewer than 100 VM steps for the replacement gap probe.
The million-directory fixture is opt-in; ordinary suites use ten directories and
retain the existing 100k resolver test. Timing covers gap detection plus catalogue
target selection, excluding fixture construction, native journal IO, refresh jobs,
and bookmark persistence. VM counts are measured separately from uninstrumented
wall times.

Same-fixture comparison (`gap-comparison.log`, 1 passed / 0 failed):

| Measured work | Before | After |
|---|---:|---:|
| Gap probe, uninstrumented | 290.7282 ms | 10.7 us |
| Gap probe, VM steps | 7,000,016 | 31 |
| One decoded-record catch-up selection, uninstrumented | 306.3406 ms | 671.8 us |
| Same selection, VM steps (separate counted run) | 7,000,414 | 429 |
| Counted selection wall time, including callback overhead | 511.2199 ms | 592.7 us |

Both selections returned exactly `{1000000}`. These are single warm in-memory runs
on a shared host, not a disk benchmark or a latency guarantee. The instruction count
demonstrates removal of the per-directory scan independent of host timing.

### Unidentifiable directories: conservative stale remains necessary

The proposed bounded parent `OpenFileById` resolution is **not implemented**.
The current native journal API provides a bounded journal read, not a bounded
historical-ancestry resolver. Even a successful parent open would establish the
parent's location now, not its location when an earlier journal record occurred.
A parent may have moved outside the root during the replay window; unidentifiable
ancestors prevent the existing catalogue from associating that earlier event with
the root. Treating a current outside location as historical outside coverage could
discard a relevant event. A lookup-count cap alone would not supply that proof.

Until a complete window-aware ancestry proof exists, unresolved events plus a live
identity gap still require a full refresh. A genuine NULL-ID directory can therefore
continue to prevent incremental catch-up for unrelated volume events. This is an
explicit conservative limitation, not evidence that no in-root change occurred.
The #548 production replay regression, identified-root control, and root `sub_noid`
control remain in place.

### Small replay edges

- `absent_pending` directories retained beneath excluded directories no longer count
  as NULL-directory gaps. Their old subtree counters also cannot override the current
  root aggregate. The regression failed against the original query
  (`gap-retired-red.log`); visible NULL directories and root file gaps still require
  conservative refresh.
- An in-root directory record supersedes a cached `Outside` classification when
  following its newly observed child chain. A currently visible directory still uses
  its precise catalogue scope. The cross-batch regression failed with `None` instead
  of targets `{1,4}` before the change (`recreated-red.log`); the existing catalogue
  rename test still pins precise child scopes. Journal identities select existing
  refresh scopes only and do not authorise native access.
- Existing catalogues lacking the partial index gain it on reopen. The reopen test
  failed with `QueryReturnedNoRows` before the change (`gap-index-red.log`).

### Verification and driver handoff

| Command (manifest omitted) | Result / receipt |
|---|---|
| `cargo test -p loomward-service --lib freshness:: -- --nocapture` | 11 passed, 0 failed; `freshness-green.log` |
| `LOOMWARD_F3_GAP_SCALE=1 cargo test -p loomward-service --lib f3_gap_probe_scale -- --nocapture` | 1 passed, 0 failed; `gap-comparison.log` |
| `cargo test -p loomward-service` twice | Each 294 passed, 0 failed, 1 ignored; `service-1.log`, `service-2.log` |
| `cargo test -p loomward-windows --all-features` | 196 passed, 0 failed, 1 ignored; `windows.log` |
| `cargo test -p loomward-catalog` | 264 passed, 0 failed, 0 ignored across 18 targets; `catalog.log` |
| `cargo fmt --all --check` | Exit 0; `fmt.log` |
| `cargo clippy --workspace --all-targets -- -D warnings` | Exit 0, no warnings; `clippy.log` |
| `cargo test --workspace --no-fail-fast` once | **1,446 passed, 1 failed, 2 ignored**, 82 targets; exit 101; `workspace.log` |
| `cargo test -p loomward-service --lib tests::f3_completed_foreground_job_does_not_mean_background_catalogue_is_quiescent -- --exact --nocapture` | Isolated rerun: 1 passed, 0 failed, 1.87 s; `quiescent-rerun.log` |
| `git -C C:/lw/wt/f3-gap diff --check` | Exit 0; `diff-check.log` |

The ignored service test is the existing manual 1 GiB streaming-memory probe; the
ignored Windows test is the existing copy throughput lab. Neither was enabled or
weakened for this change.

**Workspace failure:** the unchanged
`f3_completed_foreground_job_does_not_mean_background_catalogue_is_quiescent`
test exhausted its final 30-second wait. The tracker retained
`epoch=4 full=true auto=true attempts=3 paths=0 targets=0 started=None failed=false`
after completed full scans. Its exact isolated rerun passed, as did both dedicated
service runs. This test uses a fresh service and live watcher, not restart replay;
the live-watch completion/retry path is unchanged by this patch. A timing or
refresh-budget interaction is suspected, not proved. The original HEAD has not
been shown to reproduce it, so this is **not** a confirmed regression attribution
or a clean workspace result. Follow-up is tracked in
[#586](https://github.com/Chris0Jeky/Loomward/issues/586); no test or retry budget was
weakened, and the workspace run was not repeated to hide the failed receipt.

`HUMAN_TODO.md` is unchanged; no new owner decision or safety gate is opened.
Protected checkpoint/status files remain unchanged. This Sol lane leaves the diff
uncommitted for the driver; no push, PR mutation, merge, or deployment is performed.
The only GitHub write was the synthetic failure report in issue #586.

**NOT verified:** genuine NTFS identity-query failure, non-Windows execution,
hosted CI, app/Tauri, the ignored manual memory/throughput probes, and physical
power-cut/restore behaviour. A fully green workspace run and the cause of #586
remain unverified. No new native parent resolver is claimed.

**Residual risk:** index creation scans existing directory rows once at catalogue
open; live identity gaps can still require a full refresh on unrelated unresolved
volume events. Synthetic fixtures remain in ignored `target/test-roots/`; raw
receipts are already outside the worktree in `C:/lw/target/f3-gap/`. The worktree
is not removal-ready until the driver preserves/commits the uncommitted diff.

## 2026-10-17: refresh attempt recovery and conditional pre-emption (#583 items 1-3, #586)

Sol lane `fresh-583`, branch `fix/583-586-refresh-attempts`, base
`47279505bb983290d74cc9bdbfff1b15d1db9bea`, Windows 11 Home / KRASPYON.
All Cargo commands use `--manifest-path C:/lw/wt/fresh-583/Cargo.toml`,
`CARGO_TARGET_DIR=C:/lw/target/fresh-583`, `CARGO_BUILD_JOBS=3` and, for full
suites, `RUST_TEST_THREADS=3`. Raw receipts are outside the worktree in
`C:/lw/target/fresh-583/receipts/`. Only disposable synthetic lab roots are used.

### Findings and changed behaviour

1. **Claim TOCTOU refuted.** The existing explicit-claim mutex covers the check,
   engine submission and automatic-job recording. A new gate immediately after
   the check proves the mutex is held, and a concurrent explicit claimant sees
   the submitted job. The corrected test passes on the original implementation
   (`claim-baseline.log`, 1 passed). Deliberately dropping that mutex before the
   gate makes the same test fail (`claim-mutant.log`, 1 failed); that mutation was
   removed. The initial `red.log` also contains a test-harness error: the first
   gate check read an arrival from fixture setup. Clearing old arrivals fixed
   the test before the baseline/mutation comparison. No claim-fence repair was
   needed.
2. **Retry retirement and cancellation confirmed.** Three injected non-Busy
   start errors leave attempts at 3 even after a new hint; a terminal automatic
   cancellation leaves attempts at 1 (`red.log`). Cancelled automatic jobs now
   return their attempt. Three unsuccessful attempts still exhaust the budget;
   only a new filesystem hint received after a 30-second cooldown reopens one
   attempt. Expiry alone does not schedule another pass. Repeated hints before
   expiry do not extend or bypass the cooldown. Successful healthy full passes clear both the attempt
   count and cooldown; retry eligibility never clears stale coverage.
3. **Unnecessary duplicates cancellation confirmed.** Scans and duplicate jobs
   have separate job-conflict kinds. A refresh normally reserves all currently
   available scan workers, but increasing the cap while it runs leaves room for
   hashing. The spare-worker test originally saw the refresh cancelled
   (`red.log`). Inspection now attempts normal admission first and cancels the
   recorded automatic refresh only after an actual worker-budget refusal.
   Candidate selection is repeated after draining that refresh so its final
   publications are included. The existing full-pool pre-emption, explicit
   owner-job Busy, candidate-fence and expired-request tests remain in place.
4. **Completion/accounting race confirmed.** A gate after engine submission can
   let a full automatic pass complete before its job is recorded. Counting the
   attempt afterwards overwrites the successful pass's reset (`completion-red.log`).
   Attempts are now counted before submission; Busy starts return their attempt.
   The completion gate test waits for the newly submitted job, rather than
   inferring quiescence from any earlier completed foreground job.

New tests stop only their fixture's background pump while retaining its native
watch, use named gates/fault injection, and advance only the fixture's cooldown.
They neither sleep through the 30-second cooldown nor weaken the live-watch
quiescence assertion. The original #586 workspace failure remains historical
negative evidence; a successful repetition is not proof of its precise cause.

### Loaded reproduction and checkpoint fix

The initial loaded repetition, before checkpoint serialization, reproduced #586:
`load-1.log` has **312 passed, 2 failed, 1 ignored** in the service unit binary,
plus the passing root-coverage integration test. The failures were the unchanged
`f3_completed_foreground_job_does_not_mean_background_catalogue_is_quiescent`
(`epoch=3 full=true auto=true attempts=3`, with completed full passes) and
`copy_tests::lab_copy::sources_from_two_granted_roots_are_refused` (one of its
two roots remained stale). The run was retained; the remaining initial load
experiment was stopped, rather than counted as a clean five-run result.

Temporary diagnostics in a further loaded service run found healthy watchers
(`failed=false`) refusing completion checkpoints in **13-30 microseconds**, as
well as search checkpoints being refused. These were immediate queue refusals,
not the native checkpoint's two-second receive timeout. That diagnostic run
passed 315 tests in total, with 1 ignored (`diagnostic-service.log`), illustrating
why an isolated pass cannot dismiss the race. The diagnostics were removed.

Search and completion previously submitted concurrently to the same watch's
single-slot checkpoint queue. Immediate refusals made otherwise completed full
passes ineligible to reset the attempt budget or clear stale coverage. Both
callers now use one per-root checkpoint mutex, acquired after releasing the
tracker mutex; native callbacks remain free to record hints. Missing watches
still remain unsupported/stale, and the existing native timeout and failure
checks remain intact.

`f3_search_and_scan_completion_serialize_watch_checkpoints` gates a search inside
the checkpoint path while an actual full scan finishes and waits for that same
root's checkpoint. It failed before the mutex was acquired (`checkpoint-red.log`,
1 failed), and the F3 subset passes afterwards (`f3-checkpoint-green.log`,
**27 passed**). The fast-completion accounting test was also rerun with the
corrected new-job selection: moving the increment back after submission fails
(`completion-mutant.log`, 1 failed), and restoring the fix passes
(`completion-green.log`, 1 passed). Neither test weakens the final stale/coverage
assertions or replaces the live watcher with demo data.

### Final verification and driver handoff

Final source is unchanged across these checks (only this evidence appendix is
written afterwards). Commands ran on Windows, from the assigned worktree, using
these explicit Cargo settings and manifest:

```powershell
$env:CARGO_TARGET_DIR='C:/lw/target/fresh-583'
$env:CARGO_BUILD_JOBS='3'
$env:RUST_TEST_THREADS='3'
cargo test --manifest-path C:/lw/wt/fresh-583/Cargo.toml -p loomward-service
# The service command above ran twice.
cargo fmt --manifest-path C:/lw/wt/fresh-583/Cargo.toml --all --check
cargo build --manifest-path C:/lw/wt/fresh-583/Cargo.toml --workspace --all-targets
cargo clippy --manifest-path C:/lw/wt/fresh-583/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path C:/lw/wt/fresh-583/Cargo.toml --workspace --no-fail-fast
git -C C:/lw/wt/fresh-583 diff --check
```

For the five consecutive repetitions, invoke both already-built service test
executables below on each iteration. This avoids Cargo's target-directory lock
serializing the service tests behind the parallel workspace build. Both service
test targets run; the service has zero doctests.

```powershell
foreach ($run in 1..5) {
    & C:/lw/target/fresh-583/debug/deps/loomward_service-0cd028a8b3f776ca.exe --test-threads=3
    & C:/lw/target/fresh-583/debug/deps/root_coverage-24e3c38de7970d6b.exe --test-threads=3
}
```

The sibling Cargo process ran the workspace build, then Clippy, then the full
workspace tests. `final-workspace-timeline.log` and `final-load-runs.csv` retain
UTC boundaries: the repetitions span **18:32:55-18:40:19**, while the workspace
pipeline spans **18:31:30-18:44:11**. The first repetition overlaps compilation,
Clippy and the start of workspace testing; the remaining four overlap the
workspace tests. This is five repetitions under a parallel workspace workload,
not a claim that compilation continued through every repetition.

| Check | Final result | Receipt under `C:/lw/target/fresh-583/receipts/` |
|---|---|---|
| Service suite, twice | Each **316 passed, 0 failed, 1 ignored** (315 unit + 1 integration) | `service-final-1.log`, `service-final-2.log` |
| Service suite, five consecutive repetitions | Each **316 passed, 0 failed, 1 ignored**; both binaries exit 0 on all five runs | `final-load-1.log` through `final-load-5.log`, `final-load-runs.csv` |
| Formatting | Exit 0 | `final-fmt.log`, `final-workspace-exits.json` |
| Parallel workspace build, all targets | Exit 0 | `final-parallel-build.log` |
| Workspace Clippy, all targets, warnings denied | Exit 0, no warnings | `final-clippy.log` |
| Full workspace tests, no fail-fast | **1,477 passed, 0 failed, 2 ignored**, 82 targets; exit 0 | `final-workspace.log`, `final-workspace-exits.json` |
| Whitespace/diff check | Exit 0 after writing this appendix | `final-diff-check.log` |

The existing ignored manual streaming-memory and copy-throughput probes were not
changed or enabled. The long Windows staging-limit test creates 200,000 synthetic
entries; it completed successfully in the full workspace run. No failed final
check was skipped, weakened or relabelled as flaky.

**Changed:** bounded retry reopening after a new hint/cooldown, cancellation
refunds, attempt counting before submission, conditional duplicates pre-emption,
and serialized service watch checkpoints. Six regression/gate tests added;
claim TOCTOU refuted without changing the already-correct fence.

**Verified:** the above Windows checks and the red/green/mutation evidence in
this appendix. The original #586 symptom was reproduced before checkpoint
serialization and did not recur in either final dedicated service run, any of
the five loaded repetitions, or the final full workspace run.

**NOT verified:** hosted CI, non-Windows execution, app/Tauri/browser behaviour,
the two ignored manual probes, physical power-cut/restore, or personal-data
workloads. No publication, push, merge or deployment was performed.

**Residual risk:** genuine watch/I/O/start failures can still leave a root
honestly stale. After three failed attempts, a new hint received after the
30-second cooldown is required to reopen one automatic attempt. Checkpoints for
one root now wait behind each other; the native timeout is unchanged.

`HUMAN_TODO.md`, `AGENTS.md`, `.agent-harness/`, the implementation-status file
and checkpoint JSON are unchanged; no owner decision or safety gate is opened.
The diff is **left uncommitted** on `fix/583-586-refresh-attempts` at base HEAD
`47279505bb983290d74cc9bdbfff1b15d1db9bea` for the driver. Raw receipts already
live outside the worktree. The worktree is not removal-ready while its diff is
uncommitted. No additional out-of-scope fix is pending from this lane.

### PR #590 fix round 1: dead watchers and the single retry

Retry reopening now lives only in `Tracker::record`, once an actual filesystem
hint arrives after cooldown. `Tracked::dirty` cannot reopen it, so searches with
failed watch checkpoints and internal dirty marking cannot grant more scans.
Reopening sets attempts to 2 of 3, leaving exactly one attempt. A failed retry
requires another cooldown and a new hint; successful healthy full scans still
reset the budget normally.

The new dead-watcher regression stops the native watcher and verifies checkpoint
failure, runs the original three full refreshes, then performs searches across
three expired cooldowns without any new jobs. The updated hint regression checks
the one-attempt allowance, consumes it with an injected start failure, rejects
another hint during cooldown, and finally proves recovery after another expiry
and hint. Both tests failed before the production fix (2 failed), then passed
(2 passed). Cooldowns use `expire_retry`; no cooldown sleeps were added.

Watcher restart remains unchanged: docs/46 specifies stale coverage and refresh,
but no bounded watcher restart policy. A dead watcher remains honestly stale;
automatic refresh cannot spin solely because users search. `HUMAN_TODO.md` and
all protected driver-owned files are unchanged. Changes are left uncommitted.

Receipts for this round are under `C:/lw/target/fresh-583/receipts/review1-*`.
Checks ran on Windows with `CARGO_TARGET_DIR=C:/lw/target/fresh-583`,
`CARGO_BUILD_JOBS=3` and `RUST_TEST_THREADS=3`. Each Cargo command used
`--manifest-path C:/lw/wt/fresh-583/Cargo.toml`:

- `cargo test -p loomward-service`, twice: each **317 passed, 0 failed, 1 ignored**.
- `cargo fmt --all --check`: exit 0 after correcting formatting.
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0.
- `cargo test --workspace --no-fail-fast`: **1,478 passed, 0 failed, 2 ignored**,
  82 targets; exit 0, including the 200,000-entry staging-limit fixture.
- `git -C C:/lw/wt/fresh-583 diff --check`: exit 0 after this note.

Hosted CI, non-Windows execution and app/Tauri/browser behaviour were not tested.
No commit, push, PR mutation or merge was performed in this fix round.
