# Backups stage 1: evidence import

Date: 2026-10-11. Branch `feat/backups-evidence-import`. Merged with `origin/main` at `f5ac35c`, which
includes #372 and #374. Design and limits: [docs/42 ADR-V3-23](../../docs/42-v03-adrs.md). Wire
rules: [semantics.md, "Backups"](../../contracts/v3/semantics.md). All fixtures and receipts are
synthetic. No backup tool was run, and no real repository, configuration or credential was read.

## Changed

- **Contract.** Three new commands:
  - `backups.list` (Tauri and HTTP);
  - `backups.import` (Tauri only: the Rust-owned open-file dialog);
  - `backups.forget` (Tauri and HTTP).

  The schema gains new `$defs`, and each command has examples. Two new invalid examples cover an
  executable drill and a URL used as a label. `semantics.md` gains a section, `contracts.gen.ts` is
  regenerated, and `NativeDialogs::pick_file` is a default method that never opens a dialog.
- **`loomward-backup`, folding in #349:**
  - Only the bound snapshot is validated strictly. The list needs nothing more than unique digest IDs,
    and the 256-record count cap is gone because the byte budget bounds the list.
  - An empty, plain-text or non-summary check document now refuses as `IntegrityNotChecked` instead
    of returning an error.
  - Repository labels and snapshot-time text are restricted to fixed character sets.
  - `CON .txt` is refused.
  - `verified_recovery` is renamed `reported_recovery_evidence`.
- **State.** State schema is now v6 and adds the `backup_evidence` table. v5 belongs to #372 and is
  merged here, and both v4 and v5 stores migrate.
- **Service** (`crates/loomward-service/src/backups.rs`):
  - Personal sessions only.
  - Volume IDs are checked before any dialog opens.
  - One dialog at a time, sharing the folder picker's lock and cooldown (`dialog_turn`).
  - A pick is refused by name, before anything is opened, when it is a network, device or verbatim
    path, a device name, a link, a reparse point or a cloud placeholder.
  - At most 256 KiB is read, read-only, and the file handle closes before parsing.
  - For a check document, the service extracts its last non-empty line.
  - Only normalised evidence is stored, one record per label, and an existing label is replaced.
  - Ages and the staleness and future-time refusals are recomputed on every read.
  - Topology comes from volume identity.
- **Native:**
  - `DesktopDialogs::pick_file`: personal sessions only, retitled, with a JSON/text filter.
  - The adapter refuses `backups.import` in a synthetic session before it reaches the service.
  - A self-test line covers this.
- **App:**
  - A `Backups` view (`app/src/views/backups/`, rail order 65) with a pure `model.ts`.
  - The mock serves the contract fixture.
  - The page shows the honesty note: "A green backup job is not verified recovery."

## Design choices

- **Different volumes are reported as `unknown` topology** (basis `different_volumes_device_unknown`),
  not as an independent target. `volumes.list` holds no physical device identity: the device basis
  is `unavailable`. Two partitions can share one disk, so calling them independent would be a false
  claim (invariant 9). Only the same volume is derived positively.
- **Stage 1 never claims a full-data integrity check or an exit code.** `integrity_read_all_data` is
  always false; the stage-2 observer will record check arguments. Because stage 1 has no manifest,
  credential or restore observation, `reported_recovery_evidence` is never true.
- **Observation time is the picked file's modification time**, not the import time.
- **Proposed drill metadata is fixed** to ACL and timestamps until the owner can choose (stage 3).
  Freshness is a fixed 7-day window.
- **The request field is `relative_name`, not `path`.** The value is a lexical manifest name: it is
  never resolved or opened. `no_request_accepts_a_path` still passes.

## Verified (Windows, the merged head before this document was added)

C: briefly reached 0 bytes free during one workspace run, which produced spurious `DiskFull`
failures. The incremental caches in this worktree were removed, and every gate below was re-run
after space was freed, with `CARGO_INCREMENTAL=0`. The logs are in the gitignored
`.loomward/backups-import/`.

| Command | Result |
|---|---|
| `cargo fmt --all --check` | 0 |
| `cargo test --workspace --no-fail-fast` | 0: 906 passed, 0 failed, 0 ignored (service 159, catalog `open` 10, backup `receipts` 19) |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 0 |
| native `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings` | 0, 0 (5 unit tests; 33 passed and 1 ignored in `desktop`), 0 |
| `native/target/debug/loomward-desktop.exe --self-test` | 0, "self-test passed" |
| `py -3 scripts/verify.py` | 0 (227 Python tests, 1 skip; JS; fixtures; Rust format, tests and lint) |
| `npm.cmd --prefix app run check`, `test`, `build` | 0 each; Vitest 36 files, 367 tests |
| `py -3 scripts/test_app.py`, `--a11y`, `--live-serve` | ALL PASS each; the live leg includes `live_backups` |
| `py -3 scripts/gen_contracts_ts.py --check` | up to date |
| `git diff --check` | 0 |

**Mutation proofs.**

- `py -3 crates/loomward-backup/tests/mutations.py`: 24 mutants killed (5 new for #349), the source
  was restored and every test exercised.
- `py -3 crates/loomward-service/tests/backups_mutations.py`: all 17 mutants killed and the sources
  restored. The killed mutants cover:
  - the synthetic wall;
  - the namespace refusal;
  - the device-name refusal;
  - the regular-file check before opening;
  - the byte budget;
  - check-summary extraction;
  - the "no full-data claim" rule;
  - storing raw bytes;
  - topology identity;
  - read-time staleness;
  - read-time future refusal;
  - the repository limit;
  - forget's `not_found`;
  - the shared dialog turn;
  - the v6 migration;
  - the native adapter wall;
  - the native personal-only file picker.

## NOT verified

- **Symlinks.** The symlinked-receipt refusal is skipped on this host, which lacks the symlink
  privilege (OS error 1314). Its guard is the same `is_file` check that refuses a directory.
- **The real dialog.** The real rfd open-file dialog was never shown: native tests use stand-ins,
  and nobody clicked through the real WebView2 window.
- **Real files.** No real restic output was parsed, and no real cloud placeholder or OneDrive file
  was picked.
- **Restic formats.** The receipt formats are synthetic and follow the restic documentation.
- **Client-side validation.** The form validation in the UI is a convenience and has no mutation
  check. The service is the guard.

## Residual risk

- **Owner-declared binding.** Stage 1 evidence is bound by the owner's own declaration (label,
  snapshot ID and files). It can refuse recovery but never establish it.
- **Pre-release stores.** A state store written by this branch before it merges is v6. Only test
  and development stores exist.
