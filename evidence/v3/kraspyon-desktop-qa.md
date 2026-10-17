# First desktop-shell QA on the KRASPYON laptop

Date: 2026-10-17. Host label: KRASPYON laptop, Windows 11 Home 10.0.26100. Lane: `desktop-qa`.
Base: origin/main `ae8c8c4`. This evidence commit adds only this file; no code changed.

This is the first run of `loomward-desktop` (Tauri 2 shell, LW-018) on this machine. The earlier
native evidence in `evidence/v3/native/README.md` was taken on a Windows 11 Pro 10.0.26300 box. The
laptop shares CPU and disk with roughly ten other agent lanes, so timings are not controlled
benchmarks. Commands used `CARGO_BUILD_JOBS=4` and the lane target dir
`C:/lw/target/desktop-qa`. Raw logs, probe JSON and screenshots stay outside Git in
`C:/lw/evidence-raw/kraspyon-desktop-qa/` because they carry local paths.

## Toolchain

| Item | Value |
|---|---|
| OS | Windows 11 Home 10.0.26100 |
| rustc / cargo | 1.97.1 (stable-x86_64-pc-windows-msvc) |
| linker | MSVC link.exe 14.29 (VS 2019 BuildTools) |
| Node / npm | v24.19.0 / 11.12.0 |
| Python | 3.14.7 (`py -3`); Playwright importable (version not recorded) |
| WebView2 Runtime (HKLM WOW6432Node EdgeUpdate client, read-only) | 155.0.4283.45 at the first read; 155.0.4283.60 at the second read and as reported over CDP by the probe |

The WebView2 runtime updated during the session. The `Application` folder still holds both
`155.0.4283.45` and `155.0.4283.60`. WebView2 was present, so nothing was installed.
The HKCU and HKLM (non-WOW) EdgeUpdate client keys were absent. The runtime is machine-wide.

## Commands and results

| # | Command (run from `C:/lw/wt/desktop-qa`) | Exit | Result |
|---|---|---|---|
| 1 | `npm.cmd --prefix C:/lw/wt/desktop-qa/app ci` | 0 | 66 packages added, 0 vulnerabilities, 2m16s |
| 2 | `npm.cmd --prefix C:/lw/wt/desktop-qa/app run build` | 0 | vite 8.3.4, built in 9.68 s |
| 3 | `cargo fmt --manifest-path C:/lw/wt/desktop-qa/native/Cargo.toml --check` | 0 | clean (CI runs this too) |
| 4 | `cargo build --manifest-path C:/lw/wt/desktop-qa/native/Cargo.toml` | 0 | cold build, 14m19s, no warnings |
| 5 | `cargo test --manifest-path C:/lw/wt/desktop-qa/native/Cargo.toml` | 0 | 59 passed, 1 ignored (see detail below) |
| 6 | `cargo clippy --manifest-path C:/lw/wt/desktop-qa/native/Cargo.toml --all-targets -- -D warnings` | 0 | clean (clean rerun, 41.5 s) |
| 7 | `C:/lw/target/desktop-qa/debug/loomward-desktop.exe --self-test` | 0 | 14 PASS lines, then `self-test passed` |
| 8 | `py -3 native/tests/webview2_probe.py --exe C:/lw/target/desktop-qa/debug/loomward-desktop.exe --out C:/lw/evidence-raw/kraspyon-desktop-qa` | 0 | 24/24 checks passed; verdict VERIFIED; P9 median 40.2 ms |
| 9 | Manual launch, `--dataset synthetic --state-dir <scratch>`, measured and closed (below) | 0 | clean exit after WM_CLOSE; stderr has one expected line |

Test detail for step 5: lib unit tests 18 passed; `tests/desktop.rs` 35 passed, 1 ignored (the
interactive `native_disclosure` test, not run); `tests/updates_install_guards.rs` 2 passed;
`tests/updates_qualification.rs` 4 passed; doc-tests 0.

Note on step 5: the first `cargo test` run failed to link the `desktop` test binary
(`link.exe` exit 1104). That run had waited on a build-directory lock held by an earlier cargo run
from this same lane, which I had started in the background against the same target dir. The clean
rerun, with no other cargo process from this lane, passed. The cause is not established and the
failure was not reproduced.

## WebView2 probe (step 8)

Checks that passed (24/24), all over the real window and its IPC:

- The app loads from the embedded dist at `http://tauri.localhost`, and the Tauri transport is selected.
- The CSP refuses an injected inline script (`script-src-elem`).
- `session.hello` over real IPC is answered by `loomward-service/0.1.0`.
- `roots.request_grant` is refused in the synthetic session.
- Raw IPC: an exact 64 KiB body is accepted; oversized, already-parsed and duplicate-key bodies are refused.
- Navigation to a remote origin is denied; new windows are denied.
- Every non-`lw_` command is refused by the ACL (event, window, webview, app, path, dialog, fs, shell, `lw_delete`).
- `stream.hello` arrives first on the Channel, and the heartbeat repeats after 15.01 s of silence.
- A reload and a window close each tear down the streams. A stalled renderer gets `stream.lagged`.
- Personal default, scratch state. Declining the native confirmation grants nothing (outcome `cancelled_by_user`).
  Accepting grants one folder that the probe created itself under the worktree's gitignored `.loomward/`
  (3 synthetic files). The scan completes with 3 files indexed, and the revocation over IPC is `revoked`.
  No real folder was granted. No late dialogs remained.
- P9 (development fixture build, 2,500-node Atlas slice, mock transport): layout plus first draw runs
  41.9, 40.2, 26.5, 39.9 and 43.5 ms, median 40.2 ms against the 50 ms budget. The 2026-10-10 dev box
  measured a median of 17.5 ms. This laptop is slower, and it was under load from other lanes. The
  fixture build is not a release build.

## Manual launch on a synthetic session (step 9)

Launched as `loomward-desktop.exe --dataset synthetic --state-dir C:/lw/state/kraspyon-desktop-qa-synthetic`,
with a scratch WebView2 user-data folder and `--remote-debugging-port=0` (the probe's method, for this
run only). The state directory was empty, with no lab roots registered and no grants. The personal
default was never started.

| Measure | Value |
|---|---|
| DevTools endpoint reachable | 2.37 s after process start |
| App root rendered (`data-mode=tauri`) | 2.86 s after process start |
| Shell working set at 30.0 s | 41.9 MB (private 7.6 MB) |
| WebView2 descendants at 30.0 s | 6 processes, 420.1 MB working set in total |
| Errors | none. stderr: one line, `window close: closed 1 event stream(s) of webview "main"`. stdout empty |
| Close | WM_CLOSE via the probe's `close_window`; exit code 0 |

A single 30 s sample only. No soak, no second sample, and no release build was measured.
The screenshot `manual-synthetic-30s.png` is outside Git, in the raw folder.

## Changed / verified / NOT verified / residual risk

**Changed:** `evidence/v3/kraspyon-desktop-qa.md` (this file). Issue #504 filed for the probe's target-dir
behaviour. No product code, tests, docs or `HUMAN_TODO.md` changed.

**Verified:** steps 1-9 above, each with its exit code. The clippy and build results come from this
lane's target dir on this laptop. The build is a debug build.

**NOT verified:**

- A release build, the NSIS installer and `release.yml`.
- The ignored interactive `native_disclosure` test (it needs a Win32 MessageBox on an interactive desktop).
- A personal session with a real folder. The brief forbids it, and the probe used only its own disposable folder.
- Scanning a real lab root. No lab dataset was seeded or registered.
- Elevation: the lane ran unelevated. This is inferred from the grant succeeding, since the README says
  an elevated host refuses native identity.
- Hosted CI. Only the local equivalents of the `desktop` job were run.
- Hover, zoom and frame pacing in WebView2. Long-run memory, beyond one 30 s sample.
- The root workspace (`cargo test --workspace`). It is outside the desktop seam.

**Residual risk:**

- The P9 number is noisy and comes from a development fixture build, so it is not a release claim.
- The probe's personal leg drives real Win32 picker and confirmation dialogs on this laptop. Run it again
  with no other dialog open.
- The probe's default `--out` (`evidence/v3/native`) writes logs and screenshots into the repository. This
  lane avoided that on purpose. Anyone who runs it with the default must check the output before committing.

## Noticed, not done

1. Issue #504: the probe ignores `CARGO_TARGET_DIR` and does a second full build into `native/target`
   (3.8 GB in the worktree). The fix needs a choice among three options. That choice is for the owner or driver.
2. AGENTS.md's Tauri row says `cargo test` is "7 + 1 ignored". It is now 59 passed + 1 ignored across five
   test binaries. This is documentation drift, and AGENTS.md is not mine to edit.
3. `HUMAN_TODO.md` was not updated. The probe-target-dir choice in #504 is an owner or driver decision.
