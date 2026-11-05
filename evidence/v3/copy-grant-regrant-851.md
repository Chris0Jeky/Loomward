# Copy consent across root re-grant (#851)

Scope: the literal approval epoch in [issue #851, item 1](https://github.com/Chris0Jeky/Loomward/issues/851).
Base: `085f79fca49125b1290652bea42ca0c215dfde08` (PR #1137).
The patch adds regressions and comments; production behavior is unchanged.

## Current invariant

The service passes zero to both `approve` and `Approval::admit`, so the generic approval epoch comparison does not detect revocation. Admission instead depends on the stored preview node IDs and exact root-grant identities:

- `Inner::revoke` permanently retires the grant. A subsequent real `Inner::grant` allocates a new grant and root row; the old catalogue nodes retain their old root association.
- `copy_start` resolves those stored nodes again. Catalogue lookup requires their old grant to remain active, and the fresh native preview checks identity and manifest data.
- After preparation, `copy_start` holds `mutation` while rechecking the exact source and destination grants and registering the journal and worker. A concurrent revoke therefore either refuses admission or fences the registered run. The native operation's fencing epoch is separate from the generic approval epoch.

Source anchors: `crates/loomward-service/src/{copy.rs,lib.rs,handlers.rs}`, `crates/loomward-catalog/src/{query.rs,writer.rs}`, and `crates/loomward-operations/src/manifest.rs`.

## Added regression

`crates/loomward-service/src/copy_tests.rs` adds four Windows tests under `tests::copy_tests::lab_copy`:

- `grant_regrant_source_cannot_reuse_old_copy_consent`
- `grant_regrant_destination_cannot_reuse_old_copy_consent`
- `grant_regrant_approval_after_source_revocation_cannot_start`
- `grant_regrant_approval_after_destination_revocation_cannot_start`

Each test uses two registered synthetic roots, revokes only the selected grant, and re-grants the same physical root through `Inner::grant`. It requires strictly newer grant/root IDs, the old grant still revoked, a completed fresh scan, and a usable fresh preview with new node IDs. The source-only case requires the fresh manifest digest to equal the original digest, proving that refusal cannot depend on a digest change. The destination case requires a changed digest because the destination grant is part of the manifest.

The old consent must return `not_found` before creating any operation journal or worker. Both fixture trees must retain the same relative entries and file bytes. The two approval-after-revocation cases characterize the current cached-approval acknowledgment: it can succeed, but it cannot restore the retired grant or admit a copy after re-granting.

## Qualification

Local Rust compilation, rustfmt, and native execution are **UNVERIFIED**: Cargo and rustc are unavailable in this environment. The tests require a non-elevated Windows token and the normal synthetic lab setup. The fixture's only allowed early return emits `skipped: elevated token; the native scan refuses elevation (invariant 2)`; all other setup failures fail the test.

The Windows standard-user qualification harness now runs this focused command with `MinimumPassed 4` and `NoSetupSkip`:

```text
cargo test --locked -p loomward-service --lib grant_regrant_ -- --show-output --test-threads=1
```

The normal applicable native test and formatting gates remain required. This note records the assertions and source analysis; it does not claim hosted test results.
