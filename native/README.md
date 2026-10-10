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
| Real WebView2: IPC, ACL, CSP, heartbeat, teardown, P9 | `py -3 native/tests/webview2_probe.py` (Playwright over CDP; writes `evidence/v3/native/`) |

## IPC surface

| Command | Arguments | Returns | Notes |
|---|---|---|---|
| `lw_call` | `{ request: RequestEnvelope }` | `ResponseEnvelope` | Over 64 KiB rejects the invoke; everything else enters through `RequestEnvelope::from_slice` and answers with an envelope. `roots.request_grant` in a synthetic session and `grants.create_disclosure` in a personal one are refused here before the service (`synthetic_session_requires_lab_root`, `confinement_not_enforced`). |
| `lw_events` | `{ channel: Channel<EventEnvelope>, lastEpoch: string \| null, lastSeq: number \| null }` | `null` once `stream.hello` is on the channel | The service's stream is pumped into the channel, `stream.hello` heartbeat after 15 s of silence included. At most four streams per window. A page load or window close tears that window's streams down. A `lastSeq` without a valid `lastEpoch` resumes from an unknown epoch (`stream.lagged`, `epoch_changed`). |

`capabilities/default.json` grants the `main` window `allow-lw-call` and `allow-lw-events` and
nothing else: no `core:*` permission, and no plugin is registered (no fs, shell, dialog, http,
process or updater). `build.rs` declares the two commands to the ACL, so an unlisted one is
refused. The CSP in `tauri.conf.json` allows only same-origin scripts and styles and IPC.

## Native dialogs

`DesktopDialogs` (`src/dialogs.rs`) implements `NativeDialogs` in Rust; the webview cannot open
either dialog. The folder picker (`rfd`) never opens in a synthetic session. The disclosure box is
a task-modal Win32 `MessageBoxW` whose default button is Cancel, listing recipient, model, fields
and every `DisclosureSummary` item with hidden characters written as `\u{..}`. It is unreachable
today: personal disclosure is refused with `confinement_not_enforced` until LW-111.

## For lane L8 (the real service)

Replace `FixtureService::new(dataset)` in `src/main.rs` with the engine service; nothing else in
the shell changes. The service must yield `stream.hello` first and after 15 s of silence on every
stream (the shell forwards, it does not beat), close its streams on `EventStream::close`, refuse
`roots.request_grant` in synthetic sessions and personal disclosure itself (the shell's guards are
a second wall), and call `ctx.dialogs` only from inside `call`. Personal mode then needs the
`--dataset personal --allow-personal` start path to construct that service.
