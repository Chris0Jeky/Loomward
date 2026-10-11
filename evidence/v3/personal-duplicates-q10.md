# Personal duplicate content reads opened (q-10)

2026-10-11. Windows 11, `x86_64-pc-windows-msvc`, non-elevated token. Branch
`feat/personal-duplicates-q10`, base `30e4d1d`. Owner decision HUMAN_TODO q-10: open duplicate content
reads on personal roots, local and read-only, confirmed in stages (same size, partial hash, full hash,
byte comparison). All fixture files and names below are synthetic and live in disposable folders under
`target/test-roots` or the system temp folder. No real user folder was read or scanned.

## What changed

- **Engine** (`crates/loomward-engine/src/scan/mod.rs:220`): duplicate admission no longer refuses
  personal grants with `content_read_gated`. The grant, the engine and the durable state store must all
  name one dataset class (`check_root` at `:230`, durable class at `:282`), so neither session reads the
  other class's roots. Every other admission step is unchanged: the per-root gate, the revoked fence,
  one active matching durable row, and the final revocation gate (`:201`).
- **Service** (`crates/loomward-service/src/handlers.rs:226`, `duplicates.rs:20`): `duplicates.inspect`
  runs the same session root gate as `scan.start`, extracted unchanged from it: a synthetic session its
  registered lab roots, a personal session its own owner grants that still pass today's profile,
  profiles-directory and store fences. It runs before the engine, the catalogue query or any open.
- **App** (`app/src/views/duplicates/View.svelte:23`, `:61`): the view offers Start inspection for the
  session's own class of root and says "Reads file contents on this machine to confirm duplicates;
  nothing is deleted or moved.", plus the stages, budgets and what is skipped. The store's special
  gated state is gone; a refusal is shown as the service's message.
- **Contract** (`contracts/v3/semantics.md:370`): personal content reads are open, with their limits and
  refusal reasons. No DTO, schema, example or generated TypeScript changed. `Capabilities.effects.content_read`
  stays `false`, as it already was during synthetic inspection: no command returns file content.

The reader (`loomward-windows::duplicate_reader`) is unchanged apart from doc comments.

## Guards kept, each with a test and a mutation

Each mutant was applied alone, its test run, and the source restored byte for byte (checked) before the
next. Every one failed by an assertion, never by a compile error. 17 of 17 killed, plus one app mutant.

| Guard | Test that pins it | Mutation | Result |
|---|---|---|---|
| Pinned, revalidated identity before each read | `native_modified_during_read_is_rejected` (windows) | drop write/change time from the handle signature | killed |
| A row without a 128-bit identity is never read | `core_unverified_identity_is_never_read` (windows, new) | drop the `FileId128` check | killed |
| Non-elevated | `native_opens_refuse_elevation_consistently` (windows) | drop the reader's first `non_elevated()` | killed |
| No-recall: placeholders skipped, never hydrated | `native_refuses_special_and_placeholder_classes` (windows) | drop the placeholder attribute refusal | killed |
| Reparse points not followed | same test | drop the reparse attribute refusal | killed |
| Byte budget | `core_entry_and_time_budgets_are_partial` (windows) | ignore the byte reservation | killed |
| Time budget | same test | ignore the deadline | killed |
| Reads stop on cancellation | `pending_content_read_is_cancelled_and_drained` (windows) | ignore the cancel flag in the meter | killed |
| Candidate cap before any I/O (#358) | `duplicate_candidate_budget_refuses_before_root_io` (engine) | skip the cap | killed |
| Revocation redacts the report (#318, #351) | `duplicates_personal_session_confirms_owner_duplicates_and_revocation_redacts` (service, new) | keep a terminal report on revocation | killed |
| Revocation during the read refuses the whole report | `duplicate_grant_returns_evidence_for_its_own_class` (engine, extended) | drop the final revocation gate | killed (it survived before this test was extended) |
| Excluded and shielded entries stay out | `duplicates_select_exact_sizes_visible_names_and_scan_identity_with_a_cap` (catalogue) | drop the shield and flag filters | killed |
| Session root gate (origin) for inspection | `personal_scan_start_refuses_unknown_lab_and_revoked_roots`, `scan_start_is_refused_outside_registered_lab_roots`, `personal_scan_start_refuses_a_stored_grant_todays_fences_refuse` (service, extended) | remove the gate call from `duplicates.inspect` | killed (3 tests) |
| Protected profile and store fences | `personal_scan_start_refuses_a_stored_grant_todays_fences_refuse` | skip the personal fence | killed |
| Engine class binding | `duplicate_admission_binds_grant_engine_and_store_to_one_class` (engine, new) | drop `check_root` | killed |
| Durable class binding | same test | drop the durable class comparison | killed |
| The opening itself | the service end-to-end test | restore `content_read_gated` in the engine | killed |
| App offers inspection on owner grants | `scripts/test_app.py --live-serve`, personal leg | `canStart` back to lab roots only | killed: Start inspection stays disabled |

## Proving commands (all exit 0, on the final source)

| Command | Result |
|---|---|
| `cargo fmt --all --check` | PASS |
| `cargo test --workspace --no-fail-fast` | 853 passed, 0 failed, 0 ignored |
| `cargo test -p loomward-windows --all-features` | 94 passed, 0 failed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, no warnings |
| `py -3 scripts/verify.py` | PASS: 227 Python tests (1 symlink-privilege skip), JS syntax, 9 JS boundary assertions and 80 fixtures, Rust format, 853 Rust tests, Rust lint |
| `npm.cmd --prefix app run check` | 376 files, 0 errors, 0 warnings |
| `npm.cmd --prefix app run test` | 34 files, 360 tests passed |
| `npm.cmd --prefix app run build` | PASS, 194 modules |
| `py -3 scripts/test_app.py` | ALL PASS, 216 printed checks |
| `py -3 scripts/test_app.py --live-serve` | ALL PASS, 85 printed checks |
| `git diff --check origin/main...HEAD` | PASS |

The live personal leg starts a personal `loomward-serve` granted one disposable folder (five files of
0x01 bytes; one same-size pair), scans it from the app, presses Start inspection and sees the pair as
a verified group: 60 bytes read across the three stages, nothing skipped, no stop, file contents and
modification times unchanged. It then revokes the root and finds the completed job with no report.
The service test does the same through the service with a nested pair (`a.txt`, `sub\copy.txt`), a
same-size different file and a unique size: one group, at most 36 bytes read, files unchanged, report
redacted after revocation, a new inspection refused.

## NOT verified

- FAT/exFAT: the q-12 lane (refusing rows on identity-unverified volumes with `identity_unverified`) was
  not on `main` when this finished. Nothing here bypasses identity revalidation: candidates need a
  16-byte scan identity, the reader needs `FileId128` quality and the full handle signature before and
  after every pass. Whether a FAT or exFAT volume reports `FileId128` was not measured (#285).
- A real cloud placeholder, a stalled filesystem driver and a personal root on a real user folder were
  not exercised; placeholders use the attribute refusal seam, as before.
- The Tauri desktop shell (`native/`, now hosting the engine on `main` after #371) was not built or
  driven; its `cargo test`, clippy and `--self-test` were not run. Hosted CI and an independent review
  were not run here.

## Residual risk

Equality is point-in-time default-stream evidence, not a snapshot and not permission to act. Revocation
redacts current reports; it cannot retract a copy a client already received. Metadata calls and
cancelled-I/O draining still depend on the filesystem driver returning. The time budget bounds work
admission and waits, not a hard real-time stop.
