# Pending terminal revocation: native qualification follow-up (#738)

**Status: two strict hosted attempts compiled, then failed the non-elevated-host prerequisite.
A standard-user CI invocation is now prepared; its runtime result and the native ordering remain
UNVERIFIED until actual execution is recorded.**
This follow-up does not close [#738](https://github.com/Chris0Jeky/Loomward/issues/738), open C8,
or change any production capability, staging-removal default or fixture guard.
The Windows native tests no longer run as the elevated hosted account: [#814](https://github.com/Chris0Jeky/Loomward/pull/814)
moved them under the disposable standard user, and the tests that pin an elevated-token refusal
(invariant 2) run in their own elevated step ([#1007](https://github.com/Chris0Jeky/Loomward/issues/1007)).
The other CI checks (rustfmt, Clippy, the Linux workspace tests) remain in place.

## Source and scope

The initial implementation snapshot used to prepare this fixture was
[PR #809](https://github.com/Chris0Jeky/Loomward/pull/809) at
`9192e97a259923ac021c3de71342b76f4ddc0d89`. Its native executor test file has Git blob
`8ae287622e2e2a36831e49bae5f8c8abf62ec91f`, unchanged in the later implementation candidate.
The first hosted attempt used the integration over
`105e7851b65344d8115a31fae646883ce36ec71c`, including the reviewed initial-read provenance
correction and inline service tests. Its observed setup failure is recorded below.

This follow-up incorporates the reviewed production settlement repair in
[PR #815](https://github.com/Chris0Jeky/Loomward/pull/815). Its production source has executor blob
`83b78734f0160fdd229dc9e5b6bb7a5e2bb0abc6` and metadata-test blob
`ae061613399a731f5b897ca82a0cf91349f5571b` after three formatter-only method-chain wraps.
It registers pending requests before publishing the fence and serializes retained requests with
the resident worker's cancellation settlement.
The native fixture and strict invocation in this follow-up are unchanged.

**Reviewed production source parent:**
[`66c3ec61dba0a47de3f4b8134a88a271e300cc95`](https://github.com/Chris0Jeky/Loomward/commit/66c3ec61dba0a47de3f4b8134a88a271e300cc95),
tree `d0eace0f30f0150880f95945e6069d8f78c3901c`.
That reviewed implementation passed all nine jobs in
[run 39579965994](https://github.com/Chris0Jeky/Loomward/actions/runs/39579965994) and merged at
`132e10e8c43a963658d03cbeaec34d8f20a6c0a8` with the identical source tree. Current main
`cd726e0581bfd9f18fe2d9e8f1931216cfb6d697` also contains the subsequent documentation checkpoint
and desktop broker prerequisite. The qualification overlay is reconciled with that main and changes
no production Rust file. This follow-up still requires its own exact-head CI receipt and actual
supported-host execution; the qualifying head must be recorded from that execution.

The new test promotes and strengthens the
[preserved native reproducer from PR #803](https://github.com/Chris0Jeky/Loomward/blob/40fceaca208ee407ef71a4504653f37e3c7e575a/evidence/v3/known-failure-738-native.patch)
in the existing [Windows executor fixture module](../../crates/loomward-windows/src/effects/exec_tests.rs).
The historical unapplied patch and its negative evidence remain unchanged. The metadata contract,
service retention, fault handling and old-reader policy belong to #809, the resident settlement
ordering repair belongs to #815, and their contract remains
[docs/47](../../docs/47-copy-and-verify.md).

Relative to current main, this follow-up adds one native regression, the Windows standard-user
invocation and its bounded runner script, and this qualification record. All original Windows test
commands remain in the script; Linux, formatting, Clippy and other workflow jobs retain their checks.
Required focused native invocations fail on setup refusal or missing test execution. These additions
do not alter production code or provide a durable outbox for a pending request lost when its
service/process ends.

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

## Hosted execution attempt — 2026-10-21

The strict fixture compiled and was selected, but failed its supported-host prerequisite before
executing the native rename/foreign-commit ordering. This is unsupported setup. It establishes
neither the annotation defect nor a passing native qualification. Keep #814 draft, #738 open and
C8 closed pending actual supported-host evidence.

| Receipt field | Observed value |
| --- | --- |
| PR head | `0f643206ffcef5807c119b11e6df85cf79100f16` |
| PR workflow run | [39577743320](https://github.com/Chris0Jeky/Loomward/actions/runs/39577743320) |
| Windows native job / complete log | [119147380529](https://github.com/Chris0Jeky/Loomward/actions/runs/39577743320/job/119147380529) |
| Actual CI checkout | `06ab2f85783ebfc126dc0ca748413fc940aa1a96` |
| Actual checkout tree | `694e84e816298f57d021a075231e4bec5ed931cc` |
| Actual checkout parents | `105e7851b65344d8115a31fae646883ce36ec71c`, `0f643206ffcef5807c119b11e6df85cf79100f16` |
| Toolchain reported in the log | `rustc 1.99.1 (58cf8f9c3 2026-10-13)`; Cargo version was not separately recorded |
| Hosted Windows formatting / fixture compilation | PASS / PASS |
| Executor metadata / service retention tests | 16 passed, 0 failed, 0 ignored / 5 passed, 0 failed, 0 ignored |
| Strict native test | 1 selected; 0 passed, 1 failed, 0 ignored, 283 filtered out; workflow step exit 1 |
| Failure classification | Unsupported elevated-host setup at `Lab::new().expect(...)` |
| Native rename/foreign-commit ordering | NOT EXERCISED |
| Later Windows scan, workspace, full-feature fixture and Clippy steps | SKIPPED after the strict setup failure |
| Other workflow jobs | 8 succeeded |
| Qualifying supported-host SHA and native receipt | UNVERIFIED |

The strict invocation was:

```console
cargo +stable test --locked -p loomward-windows --all-features effects::exec::tests::another_store_committing_a_pending_rename_keeps_the_annotation -- --exact --nocapture --test-threads=1
```

The log prints `skipped: elevated token (invariant 2)`, then the strict test panics with
`#738 qualification requires the supported non-elevated Windows lab`. The strict caller reports
a failure and cannot count the optional fixture's setup refusal as a passing early return.
Neither the native scenario nor the adjacent same-Store and process-exit scenarios obtained new
supported-host evidence from this attempt. No fixture guard or assertion was relaxed.

This receipt describes the actual checkout above. It predates the #815 integration, so it supplies
no runtime result for the reconciled follow-up. The existing passing metadata and service counts
also predate #815's two additional ordering regressions. A final integrated-head receipt and a
supported non-elevated host remain required.

## Integrated hosted execution attempt — 2026-10-21

The subsequent integrated attempt is also retained as unsupported setup, with no native pass.

| Receipt field | Observed value |
| --- | --- |
| PR head | `1c68ae2a952d71882b6fb6ad7d46d5cf414cd385` |
| PR workflow run | [39581627089](https://github.com/Chris0Jeky/Loomward/actions/runs/39581627089) |
| Windows native job / complete log | [119160359240](https://github.com/Chris0Jeky/Loomward/actions/runs/39581627089/job/119160359240) |
| Actual CI checkout | `5e2362bc9c2517aefd3a40938a4e548b225bfe7e` |
| Complete checkout tree | `e9cb1789534daced6423223dfdf199332601344b` |
| Executor metadata / service retention tests | 18 passed, 0 failed, 0 ignored / 5 passed, 0 failed, 0 ignored |
| Strict native test | 1 selected; 0 passed, 1 failed, 0 ignored, 285 filtered out |
| Failure classification | Elevated-token refusal at the strict lab prerequisite |
| Native rename/foreign-commit ordering | NOT EXERCISED |
| Later Windows scan, workspace, all-feature tests and Clippy | SKIPPED after the setup failure |
| Other workflow jobs | 8 succeeded |

The two additional metadata tests from #815 executed successfully in this attempt. That is evidence
for their metadata schedules, not for the strict native scenario that could not start.

## Hosted standard-user invocation — prepared 2026-10-22

[GitHub documents that its Windows VMs run as administrators with UAC disabled](https://docs.github.com/en/actions/reference/runners/github-hosted-runners#administrative-privileges).
The fixture requires a real non-elevated token. The
[qualification runner](../../scripts/test_windows_standard_user.ps1) therefore creates a temporary
standard local account on this repository's disposable GitHub-hosted Windows VM and launches the
unchanged tests with that account's credentials and loaded profile. It requests no elevation and
changes no UAC policy or fixture guard.

Before running Cargo, the child proves its expected SID, absence of Administrator membership,
`TokenElevation: 0`, a loaded profile and an NTFS temporary volume. Public Rust toolchain/registry
inputs are copied into the task directory; the runner account's credentials and profile are not
shared. The child receives read access to the checkout and write access to the build and disposable
fixture directories. Its process, temporary account/profile and access entries have bounded cleanup.

The wrapper runs all existing Windows metadata, service, scan, workspace and full-feature tests.
It also selects the strict foreign-commit test, the adjacent same-Store and process-exit scenarios,
and the second-root volume-space regression from #796 by exact name. Focused invocations require
actual passing test counts; the native witnesses reject setup-skip output. All failures and timeouts
propagate to CI. The full suites use the same standard-user context because they also select the
strict test. Formatting, Clippy, Linux, app, desktop and Python checks remain required.

This is a source-reviewed invocation prepared for hosted validation. No result from the new wrapper
is assumed here. A successful focused #796 run would establish that fixture on this host; it would
not by itself reproduce or diagnose the original shared-host intermittency.

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
| Final integrated follow-up commit SHA | UNVERIFIED; the historical hosted attempt is identified above |
| Windows version and NTFS fixture host | UNVERIFIED |
| Non-elevated token confirmed by strict test setup | UNVERIFIED |
| Rust/Cargo toolchain versions | UNVERIFIED |
| Native foreign-commit ordering, complete log and exit code | NOT EXERCISED; historical strict invocation failed unsupported setup |
| Same-Store pending-rename regression | NOT RUN in this follow-up |
| Volatile-pending process-exit regression | NOT RUN in this follow-up |
| Exact-head formatting, workspace and full-feature fixture gates | UNVERIFIED |
| Remaining failures or unsupported setup | Historical elevated-token prerequisite failure above; final integrated and supported-host results remain UNVERIFIED |

Keep host-specific paths in the private lab record; published evidence should use anonymized labels
and aggregate results. Link immutable CI/lab logs or a checked-in anonymized receipt to the exact SHA.
Until this table is completed with actual results, the draft advances the missing fixture and leaves
native qualification open.

This test is not power-cut durability, restore qualification, personal-copy approval or evidence that
a cause lost before any durable metadata survives process death. G5/G9 and the remaining C8 decisions
stay separate, and personal effects stay closed.
