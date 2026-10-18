# Live Learning refit: concurrent feedback refresh

2026-10-18, Windows 11, KRASPYON. Worktree `liveserve-refit`, branch
`fix/live-serve-refit-button`, starting HEAD `0bb1e6e`. Changes are uncommitted
for the driver. Only disposable fixtures were used; the personal-session leg
uses synthetic names and data, not an owner's inventory.

## Cause and repair

The button was neither removed nor renamed. `review/View.svelte` shows it
only when learning status has loaded successfully. Its load reads status and
the revision-bound review queue concurrently; a queue failure clears both.

The personal-learning live check waits for the feedback receipt message, then
records another label through HTTP. The message is set before `record()` has
finished its queue refresh. That second mutation can invalidate the in-flight
queue. A focused live reproduction failed on its third iteration, returning:

```text
learning.queue: stale_generation
Catalogue or feedback changed during review.
Learning status unknown.
Retry learning status
```

The engine was correctly refusing an obsolete snapshot. This was not a
learning gate or a surfaced redaction failure. One automatic retry recovered
one invalidation but a subsequent stress run exposed two invalidations during
the same HTTP mutation: reference registration and feedback advance state in
separate stages. Increasing retries would not make that harness interleaving
deterministic.

- The view now restarts status and queue from the first page once on
  `stale_generation`, following Collections/Explorer. Persistent invalidation
  or any other error still clears stale data and exposes the retry control.
  Superseded and unmounted requests cannot initiate this retry.
- The harness waits for the first feedback refresh to finish before the next
  HTTP mutation, explicitly refreshes after that mutation, and asserts **two
  human events** before refitting. Existing training-count, teacher-separation,
  abstention, calibration, autonomy and revocation assertions remain intact.

## Regression evidence

`npm.cmd --prefix app run test -- tests/unit/view-races.test.ts` before the
view change: **11 passed, 2 failed**. Recovery left status null and made only
two calls instead of restarting both reads. After the change: **13 passed**.
The five Learning cases pin successful recovery, bounded repeated invalidation,
non-stale failure, supersession and unmounting.

The untracked diagnostic script `.loomward/diagnose_refit.py` runs the existing
personal-learning live leg repeatedly and captures failed learning responses.
After both changes, **12/12 iterations passed (204 PASS lines)**. The script
and raw logs remain gitignored under `.loomward/` for the driver.

## Gate results

All commands run from `C:/lw/wt/liveserve-refit`. Cargo uses
`CARGO_BUILD_JOBS=4`, `CARGO_TARGET_DIR=C:/lw/target/lane-liveserve-refit`;
the workspace tests use `RUST_TEST_THREADS=3`.

- `npm.cmd --prefix C:/lw/wt/liveserve-refit/app run check`: 0 errors, 0 warnings.
- `npm.cmd --prefix C:/lw/wt/liveserve-refit/app run test`: 49 files, 558 tests passed.
- `py -3 C:/lw/wt/liveserve-refit/scripts/test_app.py --live-serve`:
  **149 PASS lines, ALL PASS**, plus 4/4 timing tests. This also runs
  `npm.cmd --prefix app run build` successfully and builds the real HTTP server
  and broker. Training on both labels, no autonomy, and revocation remain proved.
- `cargo fmt --manifest-path C:/lw/wt/liveserve-refit/Cargo.toml --all --check`: passed.
- `cargo clippy --manifest-path C:/lw/wt/liveserve-refit/Cargo.toml --workspace --all-targets -- -D warnings`: passed.
- `cargo test --manifest-path C:/lw/wt/liveserve-refit/Cargo.toml --workspace --no-fail-fast`:
  **1,593 passed, 0 failed, 3 ignored**, across 83 test executables/doc-test
  results. Existing ignores: million-row clutter scale benchmark, 1 GiB
  backup memory probe, and Windows copy throughput lab. No crate was edited;
  this run includes the engine, service, HTTP and protocol suites.
- `git -C C:/lw/wt/liveserve-refit diff --check`: passed.

Raw logs: `.loomward/review-red.log`, `review-green.log`, `app-check.log`,
`app-tests.log`, `liveserve-final.log`, `workspace-tests.log`, `clippy.log`,
`fmt.log`, and `diagnose-refit-final.log`.

## Scope and limits

No Rust crate changed. No commit, push, PR, release or external model call was
made. Tauri/WebView2 and real-user model accuracy are unverified. A continuously
changing queue deliberately stops after one retry and remains visibly unknown.
`HUMAN_TODO.md` was read and left unchanged; no owner action is required for
this repair. Status/checkpoint files are left to the driver as instructed.
