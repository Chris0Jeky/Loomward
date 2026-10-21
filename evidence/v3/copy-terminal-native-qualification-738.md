# Pending terminal revocation: native qualification follow-up (#738)

**Status: drafted test and execution instructions; native execution is UNVERIFIED.**
This follow-up does not close [#738](https://github.com/Chris0Jeky/Loomward/issues/738), open C8,
or change any production capability, staging-removal default, fixture guard or CI gate.

## Source and scope

The implementation baseline is [PR #809](https://github.com/Chris0Jeky/Loomward/pull/809) at
`9192e97a259923ac021c3de71342b76f4ddc0d89`. Its native executor test file has Git blob
`8ae287622e2e2a36831e49bae5f8c8abf62ec91f`.

The new test promotes and strengthens the
[preserved native reproducer from PR #803](https://github.com/Chris0Jeky/Loomward/blob/40fceaca208ee407ef71a4504653f37e3c7e575a/evidence/v3/known-failure-738-native.patch)
in the existing [Windows executor fixture module](../../crates/loomward-windows/src/effects/exec_tests.rs).
The historical unapplied patch and its negative evidence remain unchanged. The metadata contract,
service retention, fault handling and old-reader policy belong to #809 and
[docs/47](../../docs/47-copy-and-verify.md).

This PR adds one native regression and this qualification record. It does not alter production code
or provide a durable outbox for a pending request lost when its service/process ends.

## Ordering the fixture must establish

1. A real synthetic copy previews, approves, creates and verifies the existing `tree_fixture`, then
   parks at `Point::CheckedPublish` inside the admitted native rename step.
2. A revocation observes `Publishing`, retains `GrantRevoked` in the resident Control, and reports
   `PendingInFlightStep` with a zero wait. The cause is not yet in the durable journal.
3. The rename is released. The worker parks at `Point::AfterPublish`, after the Guard and admission
   locks have been released and before the resident confirmation/commit.
4. Another Store and Control perform the real native recovery/confirmation path and reach
   `Committed`. The test captures that durable journal prefix and verifies that the original cause
   is still pending only in the resident Control.
5. The resident worker resumes. Exactly one `RevokedAfterPublish { cause: GrantRevoked }` follows
   the captured prefix. The native copy remains committed and verified, with its original epoch,
   digest, staging identity, publish identity and item records.

The two fixture gates have bounded receives and release-on-drop senders. A failed handoff assertion
releases parked workers before the thread scope joins them. A refusal by the foreign recovery is a
setup/qualification failure: it does not establish the terminal-winner scenario.

## Assertions required for qualification

The regression also requires all of the following:

- Both Store outcomes are `Committed` with all items verified. The resident/reopened receipt has
  the existing `grant_revoked` reason, and its pending Control is drained and remains fenced.
- All rows through the foreign commit are byte-for-byte unchanged. Exactly one metadata row is
  appended at the original epoch; no repeated commit, ordinary fence or advancing row is accepted.
- The independent C1 comparison agrees on the source and published copy. The source snapshot is
  unchanged, and the destination contains exactly the single published `top` folder.
- No native effect checkpoint is reached after the foreign commit. The resident worker's actual
  warmed Guard refuses harmless effect callbacks for preparing, copying, publishing and cleanup
  states after the annotation.
- Reopening, recovery and explicit abandon preserve the receipt and tree, including with the
  staging-removal option enabled only on the test Control. Starting the settled operation again is
  refused. A new terminal `EmergencyStop` request remains `Settled` and adds no invented cause.
- Retrying the original drained Control appends nothing. Final rows, source/destination snapshots
  and C1 comparison remain unchanged after those terminal calls.

These assertions are test requirements. None has been reported as a native execution result here.

## Supported execution

Use an isolated checkout of the exact follow-up head on the existing supported **non-elevated
Windows/NTFS synthetic fixture host**. The fixture creates its own disposable temporary source,
destination and state directories. It exercises the existing native identity, long-path, named-stream,
read-only/hidden metadata and verification requirements. No owner directory is a fixture grant.

The test deliberately uses `Lab::new().expect(...)`. An elevated token fails setup; the test is not
ignored and cannot report a passing early return. Normal hosted Windows runners may have an elevated
token, so a successful unrelated or metadata-only hosted job cannot supply this native qualification.
An elevated-host failure must be recorded as unsupported setup, without changing the fixture guard
or treating it as a reproduction of the annotation defect.

First capture the exact checked-out SHA and the installed toolchain:

```console
git rev-parse HEAD
rustc +stable --version
cargo +stable --version
```

Run the single native ordering and retain the complete output and exit code:

```console
cargo +stable test --locked -p loomward-windows --features copy-primitives effects::exec::tests::another_store_committing_a_pending_rename_keeps_the_annotation -- --exact --nocapture --test-threads=1
```

The result must name and execute **one test**, with no ignored, skipped or filtered-in substitute.
A setup panic, timeout, sharing/access refusal, mismatched tree, missing annotation, additional row
or later effect is a failure requiring investigation. Do not relax an assertion to produce a receipt.

Then run the adjacent same-Store and volatile-pending process-exit regressions on that host:

```console
cargo +stable test --locked -p loomward-windows --features copy-primitives effects::exec::tests::a_pending_fence_during_rename_commits_annotated -- --exact --nocapture --test-threads=1
cargo +stable test --locked -p loomward-windows --features copy-primitives effects::exec::tests::a_process_dying_with_a_pending_fence_reopens_without_effects -- --exact --nocapture --test-threads=1
```

For those existing fixtures, their elevated-token early return is not execution evidence. The host
used for qualification must satisfy the non-elevated prerequisite established by the new strict test.
Complete the existing repository Rust gates at the same final head:

```console
cargo +stable fmt --all -- --check
cargo +stable test --locked --workspace
cargo +stable clippy --locked --workspace --all-targets -- -D warnings
cargo +stable test --locked -p loomward-windows --all-features
cargo +stable clippy --locked -p loomward-windows --all-targets --all-features -- -D warnings
```

The portable journal, executor metadata and service retention regressions from #809 remain required.
Their successful execution covers their own boundaries; it does not establish that this native
rename/foreign-confirmation ordering ran. Formatting-only or integration changes after a run require
the qualifying receipt to identify and validate the actual final head.

## Receipt to complete after execution

| Receipt field | Current value |
| --- | --- |
| Final follow-up commit SHA | UNVERIFIED |
| Windows version and NTFS fixture host | UNVERIFIED |
| Non-elevated token confirmed by strict test setup | UNVERIFIED |
| Rust/Cargo toolchain versions | UNVERIFIED |
| Native foreign-commit test, complete log and exit code | NOT RUN |
| Same-Store pending-rename regression | NOT RUN in this follow-up |
| Volatile-pending process-exit regression | NOT RUN in this follow-up |
| Exact-head formatting, workspace and full-feature fixture gates | UNVERIFIED |
| Remaining failures or unsupported setup | Record verbatim outcomes when measured |

Keep host-specific paths in the private lab record; published evidence should use anonymized labels
and aggregate results. Link immutable CI/lab logs or a checked-in anonymized receipt to the exact SHA.
Until this table is completed with actual results, the draft advances the missing fixture and leaves
native qualification open.

This test is not power-cut durability, restore qualification, personal-copy approval or evidence that
a cause lost before any durable metadata survives process death. G5/G9 and the remaining C8 decisions
stay separate, and personal effects stay closed.
