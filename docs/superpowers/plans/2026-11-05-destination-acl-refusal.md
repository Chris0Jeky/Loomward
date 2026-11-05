# Destination ACL refusal implementation plan

**Goal:** Resolve #1033 by preserving the actual destination ACL refusal through native copying, durable status, the wire contract and the app explanation.

**Architecture:** Keep the inherited-only destination ACL guard. Give that guard a typed error and map it to the non-resumable `destination_acl_not_inherited` refusal. Preserve classified errors when setting the staging root's metadata as well as nested directory metadata. Unknown errors remain unknown.

**Spec:** `docs/47-copy-and-verify.md`, sections 6 and 7; GitHub issue #1033.

**Constraints:** C8 remains closed. Only disposable synthetic lab roots are exercised. Do not change ACLs in production, relax the guard, add permissions or change the journal format.

## Test and implementation steps

- [x] Add native regressions injecting explicit destination ACEs before directory verification, separately for the staging root and a nested directory. Assert the named refusal, no publication, retained staged bytes, unchanged source, and persistence after reopening state.
- [x] Pin schema/DTO acceptance of the refusal and the app's non-resumable, actionable explanation. Preserve the generic fallback for unknown errors.
- [x] Publish the regression checkpoint on a draft branch and retain the exact-head negative results. Cargo and offline npm dependencies are unavailable in the current Linux workspace; hosted Windows is the native proof.
- [x] Implement the typed error, journal/DTO/schema code and UI explanation. Regenerate TypeScript contracts. Map root metadata errors without swallowing their existing codes; a metadata mismatch remains `verification_mismatch`.
- [x] Run the full applicable hosted native/app/Python gates and obtain an independent source review before merging. Record Windows, Linux, exact revisions and any remaining limitations.

## Review focus

Root-level versus item-level refusals must both survive journal replay. Post-publish ACL changes must retain their existing `changed_after_verification` meaning. Matching an error message string must never classify an unrelated error as an ACL refusal. The UI must not claim that nothing was written after staging already exists. Older app/engine combinations and old journals must retain conservative refusal handling.

Completed on 5 November through #1138/#1143. The actual four-case Windows negative and positive
receipts, full qualification, fresh source review and main merge are recorded in
[the evidence note](../../../evidence/v3/destination-acl-refusal-1033.md). The expanded cases include
single-file and multiple-selection pre-publication paths as well as the original folder selections.
