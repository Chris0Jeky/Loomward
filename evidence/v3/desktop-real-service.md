# The desktop shell hosts the engine service (11 October 2026)

Measured on Windows 11 (DESKTOP-IHKOOJS), without elevation, on branch `feat/desktop-real-service`
(stacked on #362). Until this change the Tauri shell (`native/`) answered from the contract examples
(`FixtureService`), so an installed app showed synthetic data and could never scan.

## What changed

- `native/src/launch.rs` (new): the command line and the service the window hosts. The default is
  the engine service (`loomward_service::Service`) in a **personal** session, with state under
  `%LOCALAPPDATA%\Loomward\v3\personal` (docs/41 section 3). `--dataset synthetic` opens the engine
  in a synthetic session (`...\v3\synthetic`), `--fixtures` the contract examples (no state, no
  scanning), and `--state-dir DIR` overrides either engine session's state. `%LOCALAPPDATA%` is
  injected into `launch::open`, so tests pass a temporary directory.
- `native/src/main.rs`: parses with `Launch`, opens the service with `launch::open`, and keeps the
  Rust-owned `DesktopDialogs` (folder picker; disclosure confirmation) as the service's
  `NativeDialogs`, exactly as before.
- `native/src/selftest.rs`: no checkout needed. It reads the contract examples bundled beside an
  installed exe (else the compiled-in checkout path), and two new checks open the engine service
  as `main` does, synthetic and personal, each in a scratch state directory that it removes.
- `lw_call` and `lw_events` are unchanged: the same two commands, ACL, CSP and stream adapter.

## The personal default and its opt-in

`loomward-serve` refuses `--dataset personal` without `--allow-personal` (#143) because it is
started from a command line, often by an agent or a script, and takes roots from `--grant-root`
with no person in the loop. The desktop takes no root argument (`--grant-root` and
`--allow-personal` are unknown arguments). A personal session starts with no new root; every root
comes from the owner choosing a folder in the native picker, which the webview cannot open, drive
or bypass. That per-folder choice is the deliberate opt-in. The service's own guard is unchanged:
`Config.allow_personal` is set only by `launch::open` for a personal launch, with an empty
`grant_roots`; stored grants are rechecked against today's path rules before any scan (#312).
Agents and tests never start the default, because it opens the owner's state: they pass
`--dataset synthetic` or `--fixtures` with a scratch `--state-dir`.

## Tests (`cargo test --manifest-path native/Cargo.toml`: 30 passed, 1 ignored)

| Test | Proves |
|---|---|
| `launch_defaults_to_a_personal_engine_session_and_takes_no_root_argument` | default personal engine; `--fixtures` is synthetic only and keeps no state; `--grant-root`, `--allow-personal` and a bad `--dataset` are refused |
| `default_state_dir_is_per_user_and_per_dataset_class` | `<LOCALAPPDATA>\Loomward\v3\<class>`, disjoint per class; a missing or relative `LOCALAPPDATA` is refused |
| `the_default_launch_opens_the_engine_service_in_the_injected_state_dir` | the default opens `loomward-service` (engine_version) in the injected directory, offers the picker, and the adapter still refuses personal disclosure |
| `a_synthetic_session_cannot_read_personal_state` | after a personal picker grant, the default synthetic session sees no root and refuses the picker; pointed at the personal store, a synthetic session refuses to open (`state store: ...`) |
| `engine_events_revocation_cancellation_and_budgets_flow_through_the_adapter` | through `Desktop::open_stream`: `budgets.set`, a picker grant and its `roots.changed`, `scan.start` and its `job.state` `completed` (3 files), cancel calls on the finished job, `roots.revoke` and its `roots.changed` `revoked`, then a refused scan |
| `bundled_examples_are_found_only_when_installed_beside_the_exe` | moved from `main.rs` to `launch.rs` |
| `self_test_passes` | the self-test, including the two engine checks |

The grant legs use `identity_refused` (as `native/tests/desktop.rs` already did): on an elevated
host such as hosted CI they assert `identity_unavailable` instead of granting.

### Mutations (each restored afterwards)

| Mutation | Caught by |
|---|---|
| state directory drops the dataset class | `default_state_dir_...`, `a_synthetic_session_cannot_read_personal_state`, `the_default_launch_...` |
| `--fixtures` accepts `--dataset personal` | `launch_defaults_...` |
| default dataset synthetic | `launch_defaults_...`, `a_synthetic_session_...`, `the_default_launch_...`, `engine_events_...` |
| a relative `LOCALAPPDATA` accepted | `default_state_dir_...` |
| `--grant-root` accepted | `launch_defaults_...` |
| `examples_dir` always the compiled-in checkout path | the installed self-test below: `FAIL fixture service loads` |

### Installed self-test without a checkout

The debug exe was copied into a scratch directory with `contracts/v3/examples` beside it, and the
checkout's `contracts/v3/examples` was renamed away for the run: `--self-test` passed (12 checks,
exit 0, also through `Start-Process -Wait -PassThru -NoNewWindow` as the release workflow now runs
it). With the bundled copy removed too, it failed as it should (`FAIL fixture service loads`, exit 1).

## WebView2 probe (`py -3 native/tests/webview2_probe.py --out <scratch>`)

23/23 passed, WebView2 154.0.4258.62, exit 0. The synthetic leg runs over the engine service with a
scratch state directory: the embedded origin, the tauri transport, the CSP, `session.hello` answered
by `loomward-service/0.1.0`, the synthetic picker refusal, raw IPC bounds and duplicates, denied
navigation and popups, the ACL refusing every non-`lw_` command, `stream.hello` first and again after
15.01 s of silence, teardown on reload and on window close, production ignoring the mock transport,
`stream.lagged` after a stalled renderer, and a clean exit. The personal leg (`--state-dir` scratch,
default dataset) passed: hello offers the picker; the real native picker granted the typed disposable
folder (`granted_via: desktop_picker`); the scan completed with 3 indexed files; revocation over real
IPC left the root `revoked`; the window closed. P9 (`--fixtures` development build): median 8.7 ms
for layout and first draw of 2,500 nodes (budget 50 ms). The personal leg alone also passed three
consecutive runs (5/5 each) after its control wait was fixed.

The personal leg starts the default with a scratch `--state-dir`, creates a disposable folder with
three synthetic files under the gitignored `.loomward/`, and drives the real native picker with
Win32 messages: it types the folder into the dialog's Folder box (`Edit` 1152) and presses
Select Folder (`IDOK`). It accepts the grant only if the granted root's display name is that
folder's random name; anything else is revoked unscanned, and an undriveable picker is cancelled.

**Incident during development, fixed in the probe.** In an earlier run the driver could not find
the dialog's controls and left the picker open; the dialog was then accepted by something other
than the probe (not identified; possibly a person at the desktop), granting the picker's default
location, a real owner folder. That first version of the probe scanned whatever was granted: the
metadata scan ran for about 10 s in the scratch state directory, was revoked, and the state
directory was deleted. No file was changed and nothing reached Git. The probe now refuses to scan
any grant that is not its own disposable folder and closes a picker it cannot drive.

## Gates

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo test --workspace` | exit 0; 837 passed, 0 failed (no root crate changed) |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | exit 0 |
| `cargo fmt --manifest-path native/Cargo.toml --check` | exit 0 |
| `cargo build --manifest-path native/Cargo.toml` | exit 0 |
| `cargo test --manifest-path native/Cargo.toml` | exit 0; 2 (lib) + 28 (`desktop.rs`) passed, 1 ignored (the interactive MessageBox test) |
| `cargo clippy --manifest-path native/Cargo.toml --all-targets -- -D warnings` | exit 0 |
| `native/target/debug/loomward-desktop.exe --self-test` | 12 PASS, `self-test passed`, exit 0 |
| `py -3 native/tests/webview2_probe.py` | 23/23, exit 0 (above) |
| `py -3 scripts/verify.py` | exit 0; Python 227 tests OK (1 skipped), JS syntax and boundary, 80 fixtures, Rust format, tests and lint PASS |
| `npm.cmd --prefix app run build`, `py -3 scripts/test_app.py --live-serve` | exit 0; 80 PASS, 0 FAIL, `ALL PASS (live serve)` |
| `git diff --check` | clean |

## Not verified

- The release workflow's new installed `--self-test` step on the hosted runner (the workflow was not
  dispatched). It was proved locally with Windows PowerShell 5.1, not `pwsh`.
- The personal picker leg on an elevated host: there the grant is refused with
  `identity_unavailable` and the probe reports that leg as UNVERIFIED.
- A person granting a folder from the interface: **the app has no control that calls
  `roots.request_grant`.** Until the app gains one (sending `deadline_ms`, at most 60,000, because
  the default 5 s mutation deadline answers `deadline_exceeded` while the owner is still choosing),
  an installed app shows an empty personal session. The preview notes say so.
- Two instances on one state directory (docs/41's lock file is not implemented).
