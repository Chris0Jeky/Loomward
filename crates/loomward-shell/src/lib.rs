//! Read-only desktop context and inert shortcut inspection. No shell resolution or effects.
#[cfg(all(test, windows))]
#[path = "../tests/fixtures/junction.rs"]
mod junction_fixture;
pub mod known_folders;
pub mod shortcuts;
