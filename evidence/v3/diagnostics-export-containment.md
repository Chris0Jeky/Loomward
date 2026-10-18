# Diagnostic export directory containment

Lane: `diagexp-fix`, branch `fix/diagnostics-export-containment`. Base:
`3ad18703c2f5c47aa676d99e932e69e09986b584`. Windows 11, KRASPYON, 2026-10-18.
Synthetic disposable folders only. Produced by a Sol lane; committed and published by the driver in PR #640.

## Reproduction

The claim against `4162325` is also reproducible on this lane's newer base. Before changing
production code, added `diagnostic_export_stays_in_the_pinned_folder_after_a_junction_swap`.
It checks a target, renames its parent and installs a real directory junction at the original
pathname, then writes the held bytes. This is the gap occupied by replacement confirmation.

Command (working directory: the lane worktree):

```powershell
$env:CARGO_BUILD_JOBS='4'
# CARGO_TARGET_DIR was inherited as C:\lw\target\lane-diagexp-fix for every cargo command.
cargo test --manifest-path C:/lw/wt/diagexp-fix/Cargo.toml -p loomward-service diagnostic_export_stays_in_the_pinned_folder_after_a_junction_swap -- --nocapture
```

Original implementation: **0 passed, 1 failed**. With an existing report, the unrelated report
changed from `unrelated` to `previewed`. A separate initial run with a new report also failed:
the unrelated folder acquired the report. Both failures occurred before modifying the writer.

## Change

The Windows service retains an `ExportFolder` across confirmation. Its canonical local parent
is opened with the existing no-reparse handle walk. Target metadata, exclusive temporary-file
creation and rename use that pinned directory; failed-write cleanup uses the temporary file's
still-open handle. Directory pathnames are never resolved again during the write.

The native rename passes replacement consent directly to `ReplaceIfExists`, including atomic
no-replace semantics for a new target. Coordinate this adjacent final-component behavior with
the driver handling #437. No protocol, dialog, redaction, copy gate or product invariant changed.
The Windows primitive is opt-in through `diagnostics-export`; no dependency was added.

The regression covers new files, confirmed replacements and read-only replacement failures.
For the direct parent it requires a real successful junction swap. This host refuses an ancestor
rename while a descendant directory handle is open; those cases verify that refusal, successful
writing to the original directory and preservation of the unrelated folder. Every case checks
that no partial file survives. Existing diagnostics tests remain in place.

Native contracts: Microsoft's [NtCreateFile documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntcreatefile)
and [relative rename structure](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information).

## Verification

All commands use the lane worktree, `CARGO_BUILD_JOBS=4` and its inherited target directory.

```powershell
cargo test --manifest-path C:/lw/wt/diagexp-fix/Cargo.toml -p loomward-service diagnostics_tests -- --nocapture
```

**16 passed, 0 failed** after the fix, including the six-case regression. A subsequent casing
control also passed: selecting `REPORT.JSON` finds the existing `report.json`, requires replacement
consent and replaces that one file. No implementation change was needed for casing.

The initial full two-crate command was:

```powershell
cargo test --manifest-path C:/lw/wt/diagexp-fix/Cargo.toml -p loomward-service -p loomward-windows --all-features
```

The service passed **344 unit tests and 1 integration test**, with **1 existing manual 1 GiB probe
ignored**. Windows finished **193 passed, 1 failed, 1 ignored** (673.09 s). The failure was the
copy-ownership test's exact feature-list expectation, updated to retain both explicit features
and the same ownership checks. The existing ignored test is `effects::exec::tests::throughput_lab`.
The corrected ownership check and the new native component guard were then run directly:

```powershell
cargo test -p loomward-windows --all-features --lib -- diagnostics::component_names_are_single_children duplicate_reader::tests::copy_primitives_are_wired_only_into_the_service_copy_module --exact
```

**2 passed, 0 failed**. Together with the full run's unaffected tests, all **195 non-ignored Windows
tests** were exercised successfully; the long recovery/200,001-entry fixtures were not repeated
for this test-only expectation correction.

The three affected mutation checks were selected from the existing harness by the gitignored
`.loomward/diagnostics-containment/check_mutations.py` runner:

```powershell
py -3 C:/lw/wt/diagexp-fix/.loomward/diagnostics-containment/check_mutations.py
```

**3 killed mutations; sources restored byte-for-byte; their baseline tests green again**:
`stream-name-refusal`, `parent-is-directory`, `partial-file-cleanup`. An initial stream-guard
mutation survived because native metadata independently refused the stream; it now targets
the native component guard under its direct test. The existing end-to-end unsupported-target
test is unchanged. The directory guard uses its existing direct helper test because the native
handle walk also refuses non-directories; cleanup now targets the native handle operation.

Final checks:

```powershell
cargo test -p loomward-service
cargo fmt --all --check
cargo clippy -p loomward-service -p loomward-windows --all-targets --all-features -- -D warnings
git -C C:/lw/wt/diagexp-fix diff --check
```

- Service: **345 unit tests and 1 integration test passed, 0 failed, 1 existing manual probe
  ignored**; all **17 diagnostics tests** passed. Unit run: 48.09 s.
- Formatting, scoped all-target/all-feature Clippy and whitespace checks: **PASS**, no warnings.
- The lane made no commit, push, PR, merge or deployment; the driver committed it for PR #640. The lane's mutation runner lived in ignored lane state and is not in the repository; the committed mutations are in `crates/loomward-service/tests/diagnostics_mutations.py`.

Raw local logs and the runner remain gitignored under `.loomward/diagnostics-containment/` and
`.loomward/diagnostics-bundle/`. Ignored Python caches and local `target/test-roots/` fixtures
are also left in place for the driver; no `cargo clean` or manual target deletion was performed.

## Not verified and remaining scope

The workspace suite belongs to the driver and was not run. No app, native GUI, hosted CI or
physical save/replace dialog was exercised. `HUMAN_TODO.md` q-45's manual dialog observation
remains open and unchanged. The non-Windows pathname fallback is unchanged and still has its
previous race; this is a Windows containment fix, not a portable containment claim.

An abrupt process exit can still leave a diagnostic partial file in the pinned folder, as
already documented by ADR-V3-24. The driver owns review, integration with #437, commit and push.
