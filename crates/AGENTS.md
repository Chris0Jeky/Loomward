# crates/ — Rust workspace (seam notes; the root `AGENTS.md` governs)

- Inner loop `cargo test -p <crate>`; final `cargo fmt --all --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo test --workspace`; add `--all-features` for the `loomward-windows` fixture lab.
- Under heavy host load a few timing-bound `loomward-service` tests can time out: re-run them alone before
  reporting a failure.
- Code used only under `#[cfg(windows)]` must be gated itself (Linux clippy fails on unused items); prefer
  `if cfg!(windows)` and `std::path::MAIN_SEPARATOR`.
- Hosted Windows runners are elevated: standard-user tests skip on `running_elevated()`.
