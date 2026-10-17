# loomward-desktop (Tauri 2 shell, LW-018)

The desktop adapter of docs/41 section 5.1: a Tauri 2 window around the built Svelte app
(`app/dist`) with exactly two IPC commands into the `ViewService`. It is excluded from the root
Cargo workspace and has its own `Cargo.lock`. First built and tested on Windows 11 on
2026-10-10 (`evidence/v3/native/`).

The shell hosts the engine service (`loomward_service::Service`) in a **personal session by
default**, with its state under `%LOCALAPPDATA%\Loomward Data\v3\personal`, beside the per-user
install directory `%LOCALAPPDATA%\Loomward` rather than inside it (#373). It scans
only folders the owner grants through the native folder picker, metadata only. `src/launch.rs` holds
the command line and the service it opens.

| Start | Service | State |
|---|---|---|
| `loomward-desktop.exe` | engine, personal session | `%LOCALAPPDATA%\Loomward Data\v3\personal` |
| `--dataset synthetic` | engine, synthetic session (registered lab roots only) | `%LOCALAPPDATA%\Loomward Data\v3\synthetic` |
| `--fixtures` | the contract examples (`FixtureService`): no state, no scanning | none |
| `--state-dir DIR` | overrides the state directory of either engine session | `DIR` |

**Why there is no `--allow-personal` here.** `loomward-serve` refuses `--dataset personal` without
`--allow-personal` (#143) because it is started from a command line, often by an agent or a script,
and takes roots from `--grant-root` with no person in the loop. The desktop takes no root argument
(`--grant-root` is an unknown argument): a personal session starts without any new root, and every
root comes from the owner choosing a folder in the Rust-owned native picker, which the webview cannot
open, drive or bypass. That choice, made per folder, is the deliberate opt-in. The service's own guard
stays: `Config.allow_personal` is set only by `launch::open` for a personal launch, with an empty
`grant_roots`. Stored grants from earlier sessions are rechecked against today's path rules before
any scan (#312). **Agents and tests must not start the default**: it opens the owner's real state.
They pass `--dataset synthetic` or `--fixtures` with a scratch `--state-dir`, as the self-test and the
WebView2 probe do.

The per-class state directories keep a synthetic session away from personal state, and the catalogue
refuses to open a store of the other class (`a_synthetic_session_cannot_read_personal_state`). The
grant rules refuse a folder that contains the state directory, so on the system drive the native
picker cannot grant the volume root `C:\`. Run the app unelevated: native identity opens are refused
when elevated, so every grant is refused with `identity_unavailable`.

**State, uninstall and one instance (#373).** The per-user installer installs into
`%LOCALAPPDATA%\Loomward`; the state lives in the sibling `%LOCALAPPDATA%\Loomward Data\v3`, out of
the uninstaller's reach (`release.yml` uninstalls silently and asserts the state survives). A default
start (no `--state-dir`) moves state that an earlier build left in `%LOCALAPPDATA%\Loomward\v3` there
once. It never merges into an existing `Loomward Data\v3`, and a move that fails refuses to start
rather than open an empty store. `launch::open` takes docs/41's exclusive `lock` file in the state
directory before the stores open and holds it while the service lives. It is an OS lock on an open
handle, so a crash leaves nothing stale. A second instance on the same state directory is refused
with `busy` and exits; it does not focus the first window (no single-instance plugin, so the shell
stays plugin-free). `loomward-serve` takes no such lock. A start that fails prints to a console when
there is one (a parent console, the debug build's own, or a redirected stderr) and otherwise shows a
native error box, so an installed build never exits silently.

## Build and run

```sh
npm.cmd --prefix app ci
npm.cmd --prefix app run build                      # app/dist is embedded at compile time
cargo build --manifest-path native/Cargo.toml
native/target/debug/loomward-desktop.exe --dataset synthetic --state-dir .loomward/desktop-state  # a demo window
native/target/debug/loomward-desktop.exe              # the owner's personal session (real state)
native/target/debug/loomward-desktop.exe --self-test  # headless; exit 1 on any failure
```

`cargo build` is the whole build: `tauri` is compiled with `custom-protocol`, so the window always
serves the embedded `app/dist` (there is no `devUrl`; for UI work use Vite with `loomward-serve`).
The Tauri CLI is pinned as an app devDependency (`@tauri-apps/cli` 2.12.1, no global install);
from the repository root, `app/node_modules/.bin/tauri.cmd info` reports the toolchain and config.
`cargo build` and `cargo test` do not bundle. The unsigned NSIS preview installer is built only by
`.github/workflows/release.yml` (`tauri build --bundles nsis`; see [RELEASING.md](../RELEASING.md)). It
bundles `contracts/v3/examples` beside the exe; an installed build reads them from there (for
`--fixtures` and the self-test) because it has no checkout. The self-test needs no checkout: it runs
the protocol checks against those examples, then opens the engine service as `main` does, synthetic
and personal, each in a scratch state directory that it removes.

## Checks

| Check | Command |
|---|---|
| Build, lint | `cargo build --manifest-path native/Cargo.toml`; `cargo clippy --manifest-path native/Cargo.toml --all-targets -- -D warnings`; `cargo fmt --manifest-path native/Cargo.toml --check` |
| Command layer, dialog guards, Tauri IPC + ACL (mock runtime) | `cargo test --manifest-path native/Cargo.toml` |
| The real Win32 disclosure box (needs an interactive desktop) | `cargo test --manifest-path native/Cargo.toml -- --ignored native_disclosure` |
| Self test | `native/target/debug/loomward-desktop.exe --self-test` |
| Real WebView2 over the engine service (synthetic, scratch state): raw IPC, ACL, CSP, navigation, heartbeat, renderer lag, teardown; then the personal default (scratch state): the real native picker and confirmation, declined (no grant) and then accepted, grant, scan and revoke a disposable `.loomward/` folder; a dialog still open at the end is cancelled | `py -3 native/tests/webview2_probe.py` (Playwright over CDP; writes to `<cargo target dir>/webview2-probe/` by default, the target dir `cargo metadata` reports, so `CARGO_TARGET_DIR` is honoured; `--out DIR` overrides it, and `--out evidence/v3/native` refreshes the committed evidence) |

CI runs the build, format, test, clippy and headless self-test checks in one cached
`windows-latest` desktop job, after building `app/dist`. The WebView2 probe is a local proving
check only. On the hosted runner (#282) the shell starts and six WebView2 processes run, but
WebView2 never opens a DevTools endpoint: with a fixed port nothing listens, and with
`--remote-debugging-port=0` no `DevToolsActivePort` file is written, so Playwright cannot attach.
Three different attempts (a load wait, diagnostics, a self-chosen port) changed nothing; the
runner's cause (policy, session or argument handling) could not be observed from the job. Run the
probe locally after any change to the shell, IPC, CSP or transport. It launches a real Win32 window,
closes it with `WM_CLOSE`, does not use Chromium's headless mode, and enforces the 50 ms P9 median
budget. The interactive disclosure test remains excluded.

## IPC surface

| Command | Arguments | Returns | Notes |
|---|---|---|---|
| `lw_call` | `Uint8Array` containing a UTF-8 JSON `RequestEnvelope` | `ResponseEnvelope` | The raw body is checked against 64 KiB before our JSON parsing; `RequestEnvelope::from_slice` refuses duplicate keys at every depth. Already-parsed JSON IPC is refused, but a JSON content type is parsed by Tauri before this command's Raw check: the raw size/duplicate guarantees do not protect that earlier parsing. At most four calls are admitted before blocking work is spawned; a full cap rejects with `busy:` before dispatch and the app retries once after 500 ms with the same bytes. `roots.request_grant` in a synthetic session and `grants.create_disclosure` in a personal one are refused here before the service (`synthetic_session_requires_lab_root`, `confinement_not_enforced`). |
| `lw_events` | `{ channel: Channel<EventEnvelope>, lastEpoch: string \| null, lastSeq: number \| null }` | `null` once `stream.hello` is on the channel | The service's stream is pumped into the channel, `stream.hello` heartbeat after 15 s of silence included. At four streams the oldest is evicted; drop guards release slots on pump panics. A page load or window close tears that window's streams down. A `lastSeq` without a valid `lastEpoch` resumes from an unknown epoch (`stream.lagged`, `epoch_changed`). This command uses Tauri's parsed arguments; it does not claim raw size or duplicate-key enforcement. |
| `lw_update` | `{ request: { action: "status" } \| { action: "set_prefs", check_on_start, channel } \| { action: "check", previous } \| { action: "install", version } }` | update status JSON | The opt-in update check (`src/updates.rs`, HUMAN_TODO q-30/q-33). No action takes a URL, path or file; the endpoints are compiled in. `may_request` lets a request through only for `check` (a click) or a start with check-on-start on (off by default), and never in a build without `plugins.updater.pubkey` ("updates are not configured in this build") or in `--fixtures` mode. `install` must name the checked offer's version and shows a native OK/Cancel box (Cancel is the default) before the Tauri updater downloads, verifies the signature and runs the installer. Prefs live in `<state dir>\updates.json`. |

`capabilities/default.json` grants the `main` window `allow-lw-call`, `allow-lw-events` and
`allow-lw-update` and nothing else: no `core:*` permission and no plugin permission (no fs, shell,
dialog, http or process plugin is registered; the updater plugin is, but only Rust calls it, and
its own `plugin:updater|*` commands are refused). `build.rs` declares the three commands to the
ACL, so an unlisted one is refused. The CSP in `tauri.conf.json` allows only same-origin scripts and styles and IPC.
Main-document navigation stays on the embedded origin and new windows are denied. Production
assets ignore `?transport=mock`; mock UI checks require a development build. The client turns
live events older than five seconds into one `stream.lagged` resync notice, discarding stale
backlog until fresh events arrive. On a same-epoch resume, events through the first hello's
`last_seq` are retained replay and bypass this age check; the resume tracker still deduplicates
them. Later heartbeats cannot widen that exemption.
The replay window is bounded by `oldest_replayable_seq` and the hello envelope's sequence. Negative,
non-integer or unsafe watermarks are ignored; future watermarks are clamped to the envelope's
sequence, including in the shared reconnect cursor tracker. Channel delivery remains fire-and-forget,
not an acknowledged or bounded renderer queue.
The WebView2 probe gives each run a private test profile under ignored `.loomward/webview2-profiles/`, so debug
options do not share the application's normal profile or another run's environment.

## Native dialogs

`DesktopDialogs` (`src/dialogs.rs`) implements `NativeDialogs` in Rust; the webview cannot open
either dialog. The folder picker (`rfd`) never opens in a synthetic session. It opens on the user's
profile folder (`USERPROFILE`) with nothing selected, never on the folder Windows remembers: a stray
Enter there picks the whole profile, which the grant rules refuse. With no usable profile it does
not open. Every pick is then confirmed in a main-window-owned `MessageBoxW` that names the folder
(letters of any script shown; controls, bidi and zero-width characters escaped) and whose default
button is Cancel; declining returns `cancelled_by_user` and creates no grant. Volumes that cannot
prove file identity (FAT/exFAT, #372, not merged when this landed) get no separate confirmation here
yet. Its result goes
through the service's grant-root validator (native selection permits volume roots), then refuses
whole profiles and credential/browser stores (including `.docker`, `.cargo`, `.npm`,
`.terraform.d` and `.gemini`); the service must still check state overlap,
dataset provenance and native identity when granting. Concurrent dialog requests decline.
The picker is owned by the main window and declines before opening if no owner is configured.
Sibling profile fencing applies only inside named `Users`, `profiles` or `Documents and Settings`
directories; a nonstandard profile location does not fence unrelated siblings.
The disclosure box is a main-window-owned Win32 `MessageBoxW` whose default button is Cancel,
listing every item with non-ASCII characters escaped as `\u{..}` and literal backslashes doubled.
More than four items, 2,048 characters or 20 conservative display lines (wrapping estimated at
80 characters per line, including fixed text) declines without truncation. This temporary budget
is not a guarantee for every display scale: personal disclosure is refused by both the adapter and the dialog
implementation until LW-111 provides scrolling confirmation and the confinement gate passes.

## The engine behind the shell

The shell forwards calls and events exactly as `loomward-serve` does (`dispatch` there, `dispatch_bytes`
here): one `RequestEnvelope::from_slice` ingress, the adapter's availability check, then
`ViewService::call` with the native dialogs in the `CallContext`; `subscribe` for events, the first
event within 5 s, then a pump that closes the stream on teardown. The engine yields `stream.hello`
first and after 15 s of silence, refuses `roots.request_grant` in synthetic sessions and every
teacher command itself (the shell's two refusals are a second wall), and retains each mutation's
outcome by `request_id`, so a caller must use a fresh id per call. `roots.request_grant` waits on a
person: the default mutation deadline is 5 s, so a caller sends `deadline_ms` (at most 60,000) and
treats `deadline_exceeded` as "outcome unknown", re-reading `roots.list` or waiting for
`roots.changed`. The app has no control that calls `roots.request_grant` yet, so a person cannot grant
a folder from the interface; the WebView2 probe calls it over real IPC.
