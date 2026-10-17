# Backup evidence follow-ups (#349)

Lane: `fix/349-backup-evidence`, Windows 11, 2026-10-17. Starting HEAD `00bb610`;
the backup and service paths are unchanged between it and the inspected `origin/main`
(`f39b75d`). Synthetic receipts only. No restic invocation, backup effect or personal scan.

## Reconciliation against main

The original normaliser changes already landed in `e77ad6c`, through PR #383 (`7b755f6`):

| Issue item | Current evidence |
| --- | --- |
| Large snapshot lists and unrelated malformed roots | `only_the_bound_snapshot_is_validated_strictly`: the byte cap remains, the snapshot-count cap is removed, and only the bound snapshot's roots are validated. The UI already recommends `restic snapshots <id> --json` for an oversized export. |
| Empty/plain-text integrity input loses snapshot evidence | `unreadable_integrity_documents_refuse_without_losing_the_snapshot` pins `IntegrityNotChecked`. |
| Repository/time label hygiene | `labels_and_snapshot_time_are_restricted_charsets` pins both allowlists. |
| Recovery naming | The public Rust and wire field is `reported_recovery_evidence`, with the caller-reported caveat on `RecoveryEvidence`. |
| `CON .txt` | `traversal_and_windows_ambiguous_names_are_refused` covers the trimmed device stem. |
| Summary extraction | `summary_document` and the service test `integrity_output_with_status_lines_is_extracted_and_never_claims_a_full_data_read` retain the last nonempty line. The wiring rule is in `contracts/v3/semantics.md`, Backups. |

The five requested mutation cases were still absent. This lane adds mutations for
`read_all_data`, the inclusive credential expiry boundary, the strict maximum-age
comparison, per-file metadata and path completeness, using the existing discriminating tests.

## Remaining service fixes

- The shared `paths::reserved_device` check now includes Windows superscript device
  aliases, including case and a space before the extension. Both receipt reads and
  diagnostic-save validation use this existing helper.
- Receipt observation seconds use checked nanosecond conversion before any evidence
  transaction. An unrepresentable snapshot or integrity mtime returns the existing
  typed `unreadable` refusal and preserves previously stored evidence.
- A direct service test opens the synthetic v1 catalogue/state fixtures, imports
  backup evidence after migration, and reopens it. No migration or schema was changed.
- The receipt reader comment now states that its post-open metadata follows links,
  that the pre-open check can race a swap, and that parents are not inspected.

## Red/green evidence

Before the implementation change, the service backup tests produced **10 passed,
2 failed**. The superscript case returned `unreadable` instead of the lexical
`not_a_regular_file` refusal. A synthetic receipt with an OS-set mtime of 2300-01-01
panicked at the old multiplication and surfaced `InternalError`. The regression
also exercises an integrity receipt with that mtime and checks that refusal does
not replace the previous record.

Commands ran from `C:/lw/wt/backup-349`. Cargo uses the inherited shared target pool
and `CARGO_BUILD_JOBS=4`; the full crate test run also uses `RUST_TEST_THREADS=4`.
No workspace test is run here.

| Command | Result |
| --- | --- |
| `cargo test --manifest-path C:/lw/wt/backup-349/Cargo.toml -p loomward-backup -p loomward-service --lib backups_tests` before implementation | 10 passed, 2 failed as described above |
| `cargo test --manifest-path C:/lw/wt/backup-349/Cargo.toml -p loomward-service --lib backups_tests` after implementation | 13 passed, 0 failed |
| `cargo test --manifest-path C:/lw/wt/backup-349/Cargo.toml -p loomward-backup -p loomward-service` | 348 passed, 0 failed, 1 existing ignored manual 1 GiB streaming-memory probe: backup 20 receipt + 9 fuzz; service 318 unit + 1 integration |
| `py -3 C:/lw/wt/backup-349/crates/loomward-backup/tests/mutations.py` | PASS: 30 mutations caught, including all five new cases; every receipt test exercised; exact source bytes restored and the 20-test receipt suite green again |
| `cargo clippy --manifest-path C:/lw/wt/backup-349/Cargo.toml -p loomward-backup -p loomward-service --all-targets -- -D warnings` | PASS, no warnings |
| `cargo fmt --manifest-path C:/lw/wt/backup-349/Cargo.toml --all --check` | PASS |
| `git -C C:/lw/wt/backup-349 diff --check` | PASS |

The normaliser source has no final diff. Ignored mutation receipts/logs are under
`.loomward/lw084/`; synthetic test scratch is under `target/`. They remain in the
worktree for the driver. No commit, push or PR was made by this Sol lane.

## NOT verified and open items

- Real restic, the native picker, cloud placeholders and native restore fidelity
  remain unverified. The fixtures establish parser/service behavior only.
- Owner-declared binding and mtime-based freshness are existing stage-1 limits in
  ADR-V3-23, not native recovery evidence. They need the separately gated observer
  and restore stages; this lane opens neither.
- The local swap/parent-link race remains. The comment is corrected as the issue
  permits; anchored no-follow I/O is separate work.
- A distinct UTF-16 BOM diagnostic is still open. The current import-refusal DTO
  exposes only `invalid_json`, with no diagnostic text; adding an encoding refusal
  needs the driver's shared-contract ownership. Export UTF-8 without a BOM. In
  Windows PowerShell 5.1, avoid `>` for these receipts. After capturing the output
  in `$receipt`, the following writes it without a BOM; no encoding is silently
  repaired by this lane:

  ```powershell
  [IO.File]::WriteAllText($destination, ($receipt -join "`n"), (New-Object Text.UTF8Encoding($false)))
  ```
- `HUMAN_TODO.md` q-14 already delegates the backup design to the driver. It is
  unchanged; no new owner answer is inferred. Keep #349 open for the encoding
  diagnostic and the explicitly retained stage-1 limitations.
