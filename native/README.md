# Native shell scaffold

This directory is **uncompiled source**, separate from the root Cargo workspace. It hosts only the synthetic UI. It has no live scanner, Python service, teacher, process adapter or mutation IPC bridge. No Tauri plugins receive filesystem/shell/process/network permissions, and bundling is disabled.

First validate the root Rust workspace. Then install the platform prerequisites in the official Tauri documentation, including the Windows native development dependencies and WebView2 environment. From `native/`, try `cargo run`. Treat all build, CSP, asset-loading and WebView2 behavior as unverified until tested. Use the browser/Python reference for the working end-to-end prototype now.

Do not solve an IPC integration problem by enabling broad filesystem or shell capabilities. Implement typed commands through the catalogue service and preserve the explicit scope/grant model. Resolve and commit a separate Cargo.lock for this crate before a reproducible native release. Branding identifiers and installer configuration are provisional.
