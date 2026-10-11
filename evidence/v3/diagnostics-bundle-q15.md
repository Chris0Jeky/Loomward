# Diagnostic bundles (LW-055, q-15): Loomward's first file write

Date: 2026-10-12. Branch `feat/diagnostics-bundle-q15` from `main` at `09f80b1` (state v7, 54 commands).
Platform: Windows 11 (build 26300), non-elevated, DESKTOP-IHKOOJS. Design, assumptions and the write gate:
[docs/42 ADR-V3-24](../../docs/42-v03-adrs.md). Wire rules: [semantics.md, "Diagnostic
bundles"](../../contracts/v3/semantics.md). Every fixture is synthetic; scanned folders are disposable temporary
folders under `target/test-roots`. Nothing was uploaded, and no file outside test temporaries was written.

## Changed

- **Contract (54 to 56 commands).** `diagnostics.preview` (Tauri and HTTP, every dataset class) and
  `diagnostics.save` (Tauri only), the one command whose `commands.json` `effects` is `owner_picked_export`.
  New `$defs`: `UnknownReason`, `DiagnosticUnknown`, `DiagnosticsBundle` and its parts, `DiagnosticsPreviewResult`,
  `DiagnosticsSave*`. `SessionInfoFeatures` (gaining `diagnostics_save`) and `HealthWarningCode` become named defs
  so the bundle references them. Four invalid examples: a root carrying a path, a save result carrying a path, a
  free-text unknown reason, and an unknown written as `null`. The example preview's `bytes` and `digest` are
  real (they are those of its canonical text), so the mock and the app check it as they check the engine.
- **Telemetry.** `loomward_telemetry::host()`: OS build (`RtlGetVersion`), active logical processors and
  installed RAM, `None` on failure (q-16 allows these in new evidence).
- **Service** (`crates/loomward-service/src/diagnostics.rs`): the bundle is built from typed fields only, with
  explicit unknowns; the preview's exact bytes are held in one slot; the save checks the digest before any
  dialog, takes the shared dialog turn, and writes through `write_export` (refusals listed in ADR-V3-24, then a
  temporary file renamed over the target). The device-name check moved from `backups.rs` to `paths.rs`;
  `roots.list`'s scan-state mapping became `scan_view`, shared with the bundle.
- **Native.** `DesktopDialogs::save_file` / `save_with`: the parented rfd save dialog from the profile folder,
  retitled, default name `loomward-diagnostics-<UTC yyyymmdd-hhmmss>.json`, JSON filter, every session, one
  dialog at a time. The self-test checks the hello flag, an engine preview and a stale-digest refusal.
- **App.** A "Diagnostic bundle" panel on Grants & health: the canonical text in a scrollable, focusable region,
  its byte count and digest, a check that the shown text is exactly those bytes, "Nothing is uploaded. Saving
  writes exactly what is shown to a file you choose.", and Save only where `features.diagnostics_save` is true
  and the check passed. The mock serves the contract example and has no save (like HTTP).

## Verified (Windows, this branch)

| Command | Result |
|---|---|
| `cargo fmt --all --check`; same with `--manifest-path native/Cargo.toml` | PASS |
| `py -3 scripts/verify.py` (Python, JS syntax and boundary, Rust format, `cargo test --workspace`, clippy) | PASS: Python 227 tests (1 symlink-privilege skip), 9 JS boundary and 80 cross-language fixtures, Rust 946 tests (service 176, of which 10 are new; protocol 36) |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `py -3 scripts/gen_contracts_ts.py --check` | PASS (up to date) |
| `npm.cmd --prefix app run check` / `run test` / `run build` | PASS: 0 errors, 0 warnings; vitest 38 files, 394 tests (3 new); build OK |
| `py -3 scripts/test_app.py` | PASS: 249 checks, including the mock panel and the desktop-like fake engine's cancel, named refusal and save |
| `py -3 scripts/test_app.py --a11y` | PASS |
| `py -3 scripts/test_app.py --live-serve` | PASS: the real engine's preview over HTTP matches its digest (recomputed in Python from the received bundle), `diagnostics.save` is `capability_unavailable`, the view offers no save |
| `cargo build`, `cargo test` (7 lib, 35 + 1 ignored integration), `cargo clippy --all-targets -- -D warnings` with `--manifest-path native/Cargo.toml` | PASS |
| `native/target/debug/loomward-desktop.exe --self-test` | PASS: 13 checks, including both engine sessions' bundle preview and stale-save refusal |
| `py -3 crates/loomward-service/tests/diagnostics_mutations.py` | PASS: 14 of 14 mutants killed, sources restored, targeted tests green again |
| `git diff --check` | PASS |

The service tests (`crates/loomward-service/src/diagnostics_tests.rs`) cover:

- **Hostile names.** A registered lab root whose own name carries a right-to-left override, quotes and a
  fullwidth `C:\Users`, holding a folder named with a fullwidth `</script>` and quotes, a nested `Users\…`
  folder, a zero-width space, a bidi override, an emoji and `&amp%2F;`. It is scanned to completion (the test
  first proves the catalogue holds the three marker files), then previewed in a synthetic session and, granted
  again, in a personal one. No marker, the root path (raw or JSON-escaped), or any volume key or display name
  appears in the bundle bytes.
- **Digest.** No preview, an older preview's digest and a foreign digest are each `preview_changed`, and the
  dialog never opens.
- **Exact bytes.** A save writes byte-for-byte the previewed text in both dataset classes, reports the byte
  count, returns exactly `outcome`, `bytes_written` and `refusal`, and leaves no temporary file.
- **Cancel and replay.** Cancel writes nothing; a re-sent `request_id` replays the outcome with one dialog and
  one file.
- **Replace.** An owner-confirmed replace overwrites a larger regular file whole.
- **Refused targets.** A relative path that reaches a real folder, `\\.\C:\…`, UNC, `\\?\UNC`,
  `\\?\GLOBALROOT`, `CON.json`, `com1 .json`, `NUL.json`, a stream name, a folder, a missing parent and (when
  this host allows a file symlink) a symlink are each `unsupported_target`, and the folder is unchanged.
- **Failed write.** Writing to a read-only target is `write_failed`, leaves the target unchanged and leaves no
  partial file.
- **Adapters.** HTTP is `capability_unavailable`; a second dialog is `busy`.
- **Example.** The contract example's digest is the canonical-text digest.

Mutants killed: digest match, absolute-only, namespace refusal, device-name refusal, stream-name refusal,
parent-is-directory, existing-target-regular, partial-file cleanup, shared dialog turn, write exact bytes,
canonical (pretty) text, a root path leaked into the bundle, the native dialog's title, the native one-at-a-time
lock. Logs: gitignored `.loomward/diagnostics-bundle/`.

## Failures and workarounds

- One full `cargo test --workspace` run failed two `loomward-windows` duplicate-reader tests
  (`core_final_cancel_keeps_verified_groups`, `core_final_deadline_keeps_verified_groups`, a 50 ms time budget
  under full parallel load). Both passed alone, and the next full run (inside `verify.py`) passed: the known
  wall-clock flake family (#320). This branch does not touch that crate.
- The first `--a11y` run found two `main .note` elements (the panel's sentence and Grants & health's own note);
  the panel's sentence now uses its own class.

## NOT verified

- **The real save dialog.** No automated test opens it. Not exercised: the overwrite prompt
  (`FOS_OVERWRITEPROMPT` is a documented Windows default that rfd 0.17 leaves untouched; this was read in rfd's
  source, not observed), parenting to the main window, the start folder, and the default name as displayed.
  This is a manual or `native/tests/webview2_probe.py` leg.
- **The rename on network or cloud-synced folders.** Never run on OneDrive or other synced folders, or on
  FAT/exFAT targets.
- **Linux and macOS.** Not built or run here: `os_family` would be `other`, and its host facts unknown.
- **Crash mid-write.** Not exercised: a crash between creating and renaming the temporary file would leave a
  hidden `.loomward-diagnostics-*.partial` beside the target.
