# FAT and exFAT identity-unverified listing (#219)

Measured 2026-10-11 on DESKTOP-IHKOOJS (Windows 11 Pro build 26300, x86_64-pc-windows-msvc,
rustc/cargo 1.97.1, Python 3.14.3, Node 24.13.1), non-elevated, in worktree branch
`feat/fat-exfat-unverified-219` based on `2944542`. Owner decision: HUMAN_TODO q-12. Synthetic
fixtures only; no real file names or paths are recorded here.

## What changed

- **Source** (`loomward-windows`): only NTFS and ReFS verify identity (`verifies_identity`). Any other
  filesystem opens only when the source is built `with_identity_unverified()`; otherwise it is
  refused `IdentityUnverified`. The decision reads the filesystem name the opened handle reports,
  never which ID query failed, so an NTFS or ReFS identity failure is still refused. Unverified
  directories report `IdBasis::Unverified`, drop every listed ID, are never released for reopening,
  and Find binds its pins by path. The duplicate reader refuses an unverified root.
- **Catalogue**: `root_grant.identity_mode` (state schema v5; idempotent migration, existing grants
  `verified`). A root observed on a non-NTFS/ReFS filesystem is granted `unverified` and its root
  row carries no file ID. Row flag bit 10 `identity_unverified`: quality `unverified`, no file ID
  allowed, never a duplicate candidate (counted as unknown, so coverage is partial).
- **Engine**: `GrantedRoot` allows unverified listing for synthetic fixtures or when the grant record
  says so; scans of an unverified root are never watched; duplicate admission refuses
  `identity_unverified`.
- **Service**: every grant records the volume's real filesystem (this also fixes live NTFS grants,
  which previously recorded none and so lost durable references). A personal unverified root needs
  `--allow-identity-unverified` (CLI) or the picker's one-line native confirmation ("This volume
  cannot verify file identity; Loomward will list it read-only and never act on it"); declining is
  `cancelled_by_user`. The scan sink flags every row of an unverified run. Placement simulation and
  candidates refuse unverified rows (`identity_unverified`).
- **Contract**: `IdentityObservation.quality` gains `unverified`; `EntryFlag` gains
  `identity_unverified` (node flags bound 13); schema, DTOs, generated TS, `semantics.md` and
  `examples/dtos/identity-observation-unverified.json` together. ADR-V3-05 addendum in docs/42.

## Real-volume leg: UNVERIFIED

`Get-Volume` (read-only) on this host lists eight volumes, all NTFS; no FAT, FAT32 or exFAT volume is
mounted, so no real-hardware scan was made and nothing was written to `.loomward/scan-roots.log`.
The mode is proven only through the injected filesystem seam (`NativeSource::with_filesystem`,
fixtures/test builds) on NTFS temp directories. Whether a real FAT or exFAT volume reaches
`Extended`, `Both` or `Find`, and what IDs it reports, stays unmeasured; the mode does not depend on
that, because it keys on the filesystem name and drops every ID.

## Tests added

| Seam | Test |
|---|---|
| windows | `only_ntfs_and_refs_verify_identity`, `unverified_filesystems_open_only_under_an_unverified_grant`, `unverified_listing_keeps_metadata_and_drops_every_id` (Extended, Both, Find), `find_binds_unverified_pins_by_path_but_never_on_ntfs`, `unverified_relist_refuses_when_containment_cannot_be_proven`, `identity_unverified_root_is_never_a_content_read_input` |
| engine | `scan_source_allows_identity_unverified_only_when_the_root_does`, `identity_unverified_roots_are_never_watched`, `duplicate_admission_refuses_an_identity_unverified_root`, grant-mode assertions in `granted_root_comes_only_from_an_active_durable_grant`, and the #269 oracle `native_relist_containment` now also runs unverified (moved out refused `ContainmentUnproven`, move within root and renamed root allowed) |
| catalogue | `identity_unverified_rows_are_stored_exposed_and_never_duplicate_candidates`, `v4_state_store_grants_migrate_as_identity_verified`; the flag-bound contract test moved to bit 11 |
| service | `identity_unverified_rows_are_flagged_and_never_carry_a_file_id`, `an_unverified_run_flags_every_published_row`, `identity_unverified_cli_grants_need_explicit_confirmation`, `picker_confirms_identity_unverified_volumes_explicitly`, `placement_refuses_identity_unverified_rows`, `a_live_scanned_ntfs_grant_can_be_labelled_and_collected` |
| http / desktop | `identity_unverified_grants_need_an_explicit_flag`, `identity_unverified_confirmation_is_explicit_and_personal_only` |

The live test grants a real NTFS lab root (synthetic session) and a real personal folder through
`--grant-root`, scans both through `scan.start`, and shows `durable_reference: available`; the
synthetic file is labelled through `feedback.record` and both files join a collection. In the
personal session `feedback.record` is still refused, now by its own owner gate
(`learning_personal_gated`), not by the filesystem.

## Mutations (each guard reverted alone, targeted tests rerun, source restored)

25 mutants, 25 killed: the open-time refusal; a silent NTFS fallback; the listing ID strip; Find's
path binding; the duplicate reader's root refusal; engine content-read admission; the grant-mode
read in `granted_root`; the scan source allowance; the watch skip; the catalogue row-ID refusal,
quality mapping, candidate exclusion and unknown count, grant-mode derivation, root-row branch and
migration; the sink flag, ID drop and run-mode capture; the CLI confirmation; the picker
confirmation; placement refusal; the desktop prompt and its personal-only check; recording the
grant's filesystem.

## Gates (exact outcomes)

- `cargo fmt --all --check`: clean. `git diff --check origin/main...HEAD`: clean.
- `cargo test --workspace`: 855 passed, 0 failed (apps 24, backup 16, catalog 155, core 30,
  engine 177, http 68, lab 27, learn 18, protocol 36, service 141, shell 27, telemetry 34,
  windows 99, doc 3).
- `cargo test -p loomward-windows --all-features`: 99 passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: clean.
- `py -3 scripts/verify.py`: Python 227 tests OK (1 skip), JS syntax, 9 JS boundary and
  80 cross-language fixtures, Rust format, unit tests and lint all PASS.
- Native: `cargo build`, `cargo test` (3 + 22, 1 ignored), `cargo clippy --all-targets -- -D warnings`
  with `--manifest-path native/Cargo.toml`; `loomward-desktop.exe --self-test` passed.
- App: `npm.cmd --prefix app run check` 0 errors 0 warnings, `run test` 331 passed, `run build` OK;
  `py -3 scripts/test_app.py --live-serve` ALL PASS.

Under host load from parallel lanes, three wall-clock tests failed once each in full parallel runs
(`native_scan_job_reports_exact_final_progress` 1 s and
`completed_scan_releases_pins_and_refreshes_full_baseline` 2 s in the engine,
`meaning_work_budget_is_an_honest_prefix` in the service). Each passed alone (about 0.15 s) and in
the next full run; the counts above are a clean full run.

## Not verified

- Any real FAT, FAT32 or exFAT volume (none mounted); removable volumes stay refused at grant time.
- The desktop picker's native MessageBox on screen (the prompt is driven by an injected stand-in).
- A personal unverified grant through the HTTP `--grant-root` startup path end to end (the CLI flag
  is parsed and the grant refusal and acceptance are tested through `Inner::grant`).
- Grants made before this change keep a NULL filesystem until they are revoked and granted again.
