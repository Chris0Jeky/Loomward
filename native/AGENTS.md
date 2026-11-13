# native/ — Tauri 2 desktop shell (seam notes; the root `AGENTS.md` governs)

- Excluded from the root workspace: always pass `--manifest-path native/Cargo.toml`.
- Gate: stage the Ask sidecar with `py -3 scripts/build_broker.py`, then `cargo build`, `cargo test` and
  `cargo clippy --all-targets -- -D warnings` (each with the manifest path), then
  `native/target/debug/loomward-desktop.exe --self-test`. `native/tests/webview2_probe.py` drives the real window.
- IPC is `lw_call`/`lw_events` only; file dialogs stay Rust-side and a chosen path never crosses IPC.
