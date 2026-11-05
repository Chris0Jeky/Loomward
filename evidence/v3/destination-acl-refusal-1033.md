# Destination ACL refusal evidence (#1033)

## Failure and bounded repair

The inherited-only destination ACL guard correctly refused explicit/protected permissions, but its generic
I/O error lost the cause. Staging-root metadata verification discarded the error entirely. A single-file or
multiple-selection copy has no source-root metadata and reached the guard later, where the pre-publication
check mislabeled the refusal `changed_after_verification`.

The repair gives the existing guard a typed error and preserves `destination_acl_not_inherited` through
directory verification, the final pre-publication check, journal replay and the wire contract. It is
non-resumable. Unknown errors stay unknown, including an error with identical text but a different type.
Post-publication ACL changes keep their existing `changed_after_verification` meaning. The guard is not
relaxed, permissions are not changed by production code, and personal copying remains gated off.

The app describes staging at the time of refusal, since later abandonment may remove it. Verification
mismatch wording no longer claims that every metadata mismatch had two attempts: directory metadata is
checked once, and a file redo depends on the existing identity-bound removal policy.

## Tests-first native receipt

At exact checkpoint `997e57c6075e91f43dfba942d927d5da980d1aa6`,
[run 41949145867](https://github.com/Chris0Jeky/Loomward/actions/runs/41949145867),
[Windows job 126988654721](https://github.com/Chris0Jeky/Loomward/actions/runs/41949145867/job/126988654721)
proved a non-elevated standard-user token on NTFS and ran all four focused regressions. None was skipped.
All four failed at the exact refusal assertion, before the implementation was published:

| Selection / injection | Observed old refusal | Required refusal |
|---|---|---|
| Whole selected folder / staging root | null | `destination_acl_not_inherited`, no item |
| Whole selected folder / nested directory | null | `destination_acl_not_inherited`, nested item |
| Single file / staging root | `changed_after_verification` | `destination_acl_not_inherited`, no item |
| Multiple selections / staging root | `changed_after_verification` | `destination_acl_not_inherited`, no item |

The focused result was **0 passed, 4 failed, 0 ignored**, and child exit 101. Each case injected an actual
explicit ACE with `icacls` after content verification and confirmed that the injection ran. The final tests
also require retained staged payload, no published destination, unchanged source identity/signature/bytes,
and the same refusal after reopening state. Those later assertions require the passing implementation run;
the negative receipt does not claim they executed after its failed assertion.

The standard-user harness requires all four focused tests to execute and rejects setup skips before its
existing full workspace and all-features gates.

## Contract and app receipts

At the same checkpoint, [Linux native job 126988654705](https://github.com/Chris0Jeky/Loomward/actions/runs/41949145867/job/126988654705)
failed `destination_acl_refusal_survives_the_status_wire_contract` because the old schema rejected the new
code. It also exposed the separate inherited ChangesNewSince conditional-object closure check, repaired in
#1137 without weakening the checker or changing the accepted response contract.

The app regression first failed for the intended missing explanation at
`47654e9d45f4754a193f99611c704039e67aee09`, job `126981646667`: one failed and 1,074 passed.
The earlier initial fixture mistakenly expected Cancel in `needs_attention`; it was corrected to the existing
Abandon action before this behavioral receipt. The expanded checkpoint also tests abandonment followed by
`cancelled`, with the historical refusal retained and no staging folder.

## Review and validation boundary

An independent review traced native metadata/recheck/finish/recovery, journal transitions, service status,
schema/DTOs and UI rendering. It found the missing single/multiple-selection path and the misleading
post-abandon text. Both findings were fixed and re-reviewed without another identified blocker.

The local Linux Python suite passes 440 tests with 32 dependency/platform skips; generated TypeScript
contracts and whitespace checks are current. Cargo, Windows and offline app dependencies are unavailable
in that workspace. The production revision therefore requires exact-head hosted native/app/Python checks,
including the full real standard-user Windows suite, recorded on [PR #1138](https://github.com/Chris0Jeky/Loomward/pull/1138).
This evidence does not open C8, establish physical power-loss safety or authorize original-file effects.
