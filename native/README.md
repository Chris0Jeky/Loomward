# loomward-desktop (Tauri 2 shell, LW-018)

The desktop adapter of docs/41 section 5.1: a Tauri 2 window around the built Svelte app
(`app/dist`) with exactly two IPC commands into the `ViewService`. It is excluded from the root
Cargo workspace and has its own `Cargo.lock`. First built and tested on Windows 11 on
2026-10-10 (`evidence/v3/native/`).

Until lane L8 lands the engine service, the shell answers from `loomward_http::fixture::FixtureService`
(the contract examples, synthetic only). `--dataset personal --allow-personal` is refused at start,
exactly as by `loomward-serve` (#143). The fixture reads `contracts/v3/examples` through a path
compiled into the binary, so a build runs only from the checkout it was built in.

## Build and run

```sh
npm.cmd --prefix app ci
npm.cmd --prefix app run build                      # app/dist is embedded at compile time
cargo build --manifest-path native/Cargo.toml
native/target/debug/loomward-desktop.exe            # the window
native/target/debug/loomward-desktop.exe --self-test  # headless; exit 1 on any failure
```

`cargo build` is the whole build: `tauri` is compiled with `custom-protocol`, so the window always
serves the embedded `app/dist` (there is no `devUrl`; for UI work use Vite with `loomward-serve`).
The Tauri CLI is pinned as an app devDependency (`@tauri-apps/cli` 2.12.1, no global install);
from the repository root, `app/node_modules/.bin/tauri.cmd info` reports the toolchain and config.
Bundling is off (`bundle.active: false`); no installer is built.

## Checks

| Check | Command |
|---|---|
| Build, lint | `cargo build --manifest-path native/Cargo.toml`; `cargo clippy --manifest-path native/Cargo.toml --all-targets -- -D warnings`; `cargo fmt --manifest-path native/Cargo.toml --check` |
| Command layer, dialog guards, Tauri IPC + ACL (mock runtime) | `cargo test --manifest-path native/Cargo.toml` |
| The real Win32 disclosure box (needs an interactive desktop) | `cargo test --manifest-path native/Cargo.toml -- --ignored native_disclosure` |
| Self test | `native/target/debug/loomward-desktop.exe --self-test` |
| Real WebView2: raw IPC, ACL, CSP, navigation, heartbeat, renderer lag, teardown | `py -3 native/tests/webview2_probe.py` (Playwright over CDP; writes `evidence/v3/native/`) |

## IPC surface

| Command | Arguments | Returns | Notes |
|---|---|---|---|
| `lw_call` | `Uint8Array` containing a UTF-8 JSON `RequestEnvelope` | `ResponseEnvelope` | The raw body is checked against 64 KiB before JSON parsing; `RequestEnvelope::from_slice` refuses duplicate keys at every depth. Already-parsed JSON IPC is refused. At most four calls are admitted before blocking work is spawned. `roots.request_grant` in a synthetic session and `grants.create_disclosure` in a personal one are refused here before the service (`synthetic_session_requires_lab_root`, `confinement_not_enforced`). |
| `lw_events` | `{ channel: Channel<EventEnvelope>, lastEpoch: string \| null, lastSeq: number \| null }` | `null` once `stream.hello` is on the channel | The service's stream is pumped into the channel, `stream.hello` heartbeat after 15 s of silence included. At four streams the oldest is evicted; drop guards release slots on pump panics. A page load or window close tears that window's streams down. A `lastSeq` without a valid `lastEpoch` resumes from an unknown epoch (`stream.lagged`, `epoch_changed`). This command uses Tauri's parsed arguments; it does not claim raw size or duplicate-key enforcement. |

`capabilities/default.json` grants the `main` window `allow-lw-call` and `allow-lw-events` and
nothing else: no `core:*` permission, and no plugin is registered (no fs, shell, dialog, http,
process or updater). `build.rs` declares the two commands to the ACL, so an unlisted one is
refused. The CSP in `tauri.conf.json` allows only same-origin scripts and styles and IPC.
Main-document navigation stays on the embedded origin and new windows are denied. Production
assets ignore `?transport=mock`; mock UI checks require a development build. The client turns
events queued for over five seconds into one `stream.lagged` resync notice, discarding stale
backlog until fresh events arrive. Channel delivery remains fire-and-forget, not an acknowledged
or bounded renderer queue.
The WebView2 probe gives each run a private test profile under its output directory, so debug
options do not share the application's normal profile or another run's environment.

## Native dialogs

`DesktopDialogs` (`src/dialogs.rs`) implements `NativeDialogs` in Rust; the webview cannot open
either dialog. The folder picker (`rfd`) never opens in a synthetic session. Its result goes
through the service's grant-root validator (native selection permits volume roots), then refuses
whole profiles and credential/browser stores; the service must still check state overlap,
dataset provenance and native identity when granting. Concurrent dialog requests decline.
The disclosure box is a main-window-owned Win32 `MessageBoxW` whose default button is Cancel,
listing every item with non-ASCII characters escaped as `\u{..}`. Text over 2,048 characters
declines without truncation. Personal disclosure is refused by both the adapter and the dialog
implementation until LW-111 provides scrolling confirmation and the confinement gate passes.

## For lane L8 (the real service)

Replace `FixtureService::new(dataset)` in `src/main.rs` with the engine service; nothing else in
the shell changes. The service must yield `stream.hello` first and after 15 s of silence on every
stream (the shell forwards, it does not beat), close its streams on `EventStream::close`, refuse
`roots.request_grant` in synthetic sessions and personal disclosure itself (the shell's guards are
a second wall), and call `ctx.dialogs` only from inside `call`. Personal mode then needs the
`--dataset personal --allow-personal` start path to construct that service.
