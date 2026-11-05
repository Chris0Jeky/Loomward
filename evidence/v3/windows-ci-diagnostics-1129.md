# Windows qualification diagnostics retention

## Purpose and boundary

The historical Windows failure in [#1129](https://github.com/Chris0Jeky/Loomward/issues/1129)
still has no retrievable ordinary Actions log. This change cannot recover that log or classify the failure.
It preserves future synthetic qualification evidence outside the disposable account workspace: only
`stdout.log`, `stderr.log` and `status.json`, uploaded for 14 days after an attempted standard-user step.
No account configuration, profile, cache or workspace is uploaded.

Existing test commands, cleanup operations and the 35-minute child / 75-minute job bounds are unchanged.
Status-writing errors cannot interrupt cleanup and cannot make a failed qualification pass. Loss of the runner
before the artifact upload can still prevent retention.

## Negative receipt, 2026-11-05

Commit `6c2fe7056dd096a94b7627a7f8ee94ea5061f0ac` temporarily printed a distinct marker on each child
stream and exited with 73 immediately after the real standard-user identity and non-elevated token checks.
The native tests were deliberately not reached in this receipt. The three-line probe was removed afterward.

- [Run 41948078975](https://github.com/Chris0Jeky/Loomward/actions/runs/41948078975), push at that exact commit.
- [Windows native job 126985148106](https://github.com/Chris0Jeky/Loomward/actions/runs/41948078975/job/126985148106)
  failed; its log records child exit 73. The enclosing Actions PowerShell step reports a nonzero exit of 1.
- Artifact `windows-standard-user-41948078975-1`, ID `13380400952`, uploaded successfully after the failure.
- Downloaded ZIP: 682 bytes; SHA-256
  `50bcdb2d1b03caa1f437c035ad945a26388f3c73fcbce5097c51641c2bc28982`.
- The ZIP contained exactly `stdout.log`, `stderr.log` and `status.json`. Its bytes, both markers and every
  status field below were verified after download, independently of the job's console rendering.

`stdout.log` contained `LW-CI-DIAGNOSTICS-NEGATIVE-STDOUT`; `stderr.log` contained
`LW-CI-DIAGNOSTICS-NEGATIVE-STDERR`. The retained status was:

```json
{
  "schemaVersion": 1,
  "runId": "41948078975",
  "runAttempt": "1",
  "workflowSha": "6c2fe7056dd096a94b7627a7f8ee94ea5061f0ac",
  "startedUtc": "2026-11-05T01:29:45.8352644Z",
  "updatedUtc": "2026-11-05T01:31:05.4169785Z",
  "phase": "finished",
  "childExitCode": 73,
  "timedOut": false,
  "parentError": null,
  "cleanupCompleted": true,
  "cleanupFailed": false
}
```

The ordinary log also records no remaining temporary-account processes, verified removal of temporary ACL
grants, and detachment of the filter instance owned by the qualification. `cleanupCompleted` means every
cleanup block was attempted; `cleanupFailed` separately records whether one reported failure.

## Qualification limits

This receipt proves that a failing child stays failed while its output survives normal cleanup. It does not
qualify the skipped native suite. The production revision requires its own exact-head standard-user run;
that run and the other applicable gates are recorded on [PR #1139](https://github.com/Chris0Jeky/Loomward/pull/1139).
The Linux failure on the negative checkpoint was the inherited unavailable-root fixture expectation, repaired
separately in #1137; it was not caused by this Windows-only diagnostic path. #1129 remains open.

## Production PR receipt, 2026-11-05

The following completed receipts supersede the pending production-qualification statement above;
the negative exit-73 receipt and the historical #1129 failure remain part of the record.

[PR #1139](https://github.com/Chris0Jeky/Loomward/pull/1139) head
`370018042df0762f9a62b4910471a25933f342d1` passed
[PR run 41951881611](https://github.com/Chris0Jeky/Loomward/actions/runs/41951881611), attempt 1.
[Windows native job 126997724134](https://github.com/Chris0Jeky/Loomward/actions/runs/41951881611/job/126997724134)
checked out PR merge `c687cb15396f9d4863e4cfe9ab2f2e3ceb968e3d`, combining that head with
`085f79fca49125b1290652bea42ca0c215dfde08`. The standard-user step ran from 02:13:00 to
02:41:59 UTC; its child exited 0, and the log records Administrator membership false,
TokenElevation 0 and NTFS. Both later Clippy gates passed; the job completed at 02:43:17 UTC.

- Retained artifact: [windows-standard-user-41951881611-1, ID 13384060805](https://github.com/Chris0Jeky/Loomward/actions/runs/41951881611/artifacts/13384060805).
- Downloaded ZIP: 122,742 bytes; SHA-256
  `74e47468688b5c62903ad7253fe2f0d48da4224843035ba3bf6cdd2bda51921e`.
- The coordinating review downloaded and hash-verified the ZIP. This evidence update independently
  rehashed and opened that saved ZIP; its digest matches the Actions-reported digest. It contains
  exactly `stdout.log`, `stderr.log` and `status.json`.

The downloaded status records:

```json
{
  "schemaVersion": 1,
  "runId": "41951881611",
  "runAttempt": "1",
  "workflowSha": "c687cb15396f9d4863e4cfe9ab2f2e3ceb968e3d",
  "startedUtc": "2026-11-05T02:13:00.7061169Z",
  "updatedUtc": "2026-11-05T02:41:59.6779699Z",
  "phase": "finished",
  "childExitCode": 0,
  "timedOut": false,
  "parentError": null,
  "cleanupCompleted": true,
  "cleanupFailed": false
}
```

## Failed duplicate push receipt, 2026-11-05

The same head's [push run 41951875642](https://github.com/Chris0Jeky/Loomward/actions/runs/41951875642),
attempt 1, failed. Its
[Windows native job 126997824360](https://github.com/Chris0Jeky/Loomward/actions/runs/41951875642/job/126997824360)
started at 02:06:46 UTC and completed with `failure` at 02:59:48 UTC: 53 minutes 2 seconds.
Formatting and the elevated-token refusal step passed. The final job metadata still records the
standard-user step as `in_progress`, started at 02:13:25 UTC with no completion timestamp. Diagnostics
upload, both later Clippy gates and post-job cleanup remain `pending` in that metadata.

The completed-job log endpoint returned HTTP 404 `BlobNotFound` at 03:00:16 UTC and again at
03:01:41 UTC after a delayed recheck. At that recheck the run retained only its two browser artifacts;
there was no `windows-standard-user-41951875642-1` artifact. The coordinating review confirmed another
HTTP 404 for the ordinary log at 03:03:31 UTC, with no Windows diagnostic artifact available. No child
exit code or test result can be recovered from this attempt's available evidence.

The exact source revision sets a 75-minute native job timeout, so the 53-minute failure did not reach
that configured limit. Its 35-minute child deadline starts only after `Start-Process` returns; the
available metadata does not prove that point was reached or that the deadline fired. At the initial
03:01:41 UTC review, the incomplete metadata and unavailable log left the failure mode unclassified;
abrupt runner loss or a hard process/runner hang were possibilities, not established causes. The
successful PR receipt is separate evidence and does not replace this failed duplicate. The later
annotation recovery below establishes a failure mode for both jobs without recovering either log.

### Recovered runner annotations, 03:05 UTC

The coordinating review expanded "Open job annotations" in the public GitHub Actions UI at
03:05 UTC on 2026-11-05 and directly observed the same annotation on both exact job pages:

- Historical #1129 failure: [run 41389074739, job 125096338660](https://github.com/Chris0Jeky/Loomward/actions/runs/41389074739/job/125096338660).
- New duplicate push failure: [run 41951875642, job 126997824360](https://github.com/Chris0Jeky/Loomward/actions/runs/41951875642/job/126997824360).

> The hosted runner lost communication with the server.

The annotation then lists possible workflow-related triggers: termination of the runner process,
CPU or memory starvation, or blocked network access. It does not identify which, if any, occurred.
This direct UI observation updates the current failure-mode classification for both jobs to
**runner communication loss**. The underlying trigger, the last running test, and whether the child
deadline fired remain unknown; the annotation is not evidence that a particular test or timeout
failed. The earlier metadata-only uncertainty and HTTP 404 observations remain historical evidence.
#1129 stays open for cause investigation, recurrence and the diagnostic gap when runner execution
cannot reach the artifact upload.

## Integrated qualification and merge, 2026-11-05

The diagnostics implementation was also qualified with the other maintenance changes in
[PR #1143](https://github.com/Chris0Jeky/Loomward/pull/1143), head
`0e7c94429920c488e9823cbd3fb6f42bbf3ed631`, tree
`0a72674d826ca4a5043f6c8405ac1b67c032ba3f`. Both attempt-1 runs completed with all 9 jobs passing:

- [PR run 41955056422](https://github.com/Chris0Jeky/Loomward/actions/runs/41955056422), including
  [Windows native job 127008004347](https://github.com/Chris0Jeky/Loomward/actions/runs/41955056422/job/127008004347).
- [Push run 41955048805](https://github.com/Chris0Jeky/Loomward/actions/runs/41955048805), including
  [Windows native job 127008058011](https://github.com/Chris0Jeky/Loomward/actions/runs/41955048805/job/127008058011).

An independent integration review downloaded and inspected the PR run's
[windows-standard-user-41955056422-1 artifact, ID 13384203677](https://github.com/Chris0Jeky/Loomward/actions/runs/41955056422/artifacts/13384203677).
The ZIP is 123,998 bytes; its Actions-reported SHA-256 is
`a870cdcd654ddfd9a653b2bd47e1c4d8d7decfd37c01062dd69915b10828e392` (not independently recomputed
for that inspection). It contains exactly `stdout.log` (679,065 bytes), `stderr.log` (empty) and
`status.json` (386 bytes). The stdout records Administrator membership false, TokenElevation 0 and NTFS.
The retained status identifies run `41955056422`, attempt `1`, PR merge checkout
`a877bffb4254b4d62d76157ef1b962a95d7880c3`, started at `2026-11-05T02:31:24.7973120Z` and updated
at `2026-11-05T03:00:19.8493763Z`: phase `finished`, child exit 0, `timedOut: false`,
`parentError: null`, `cleanupCompleted: true` and `cleanupFailed: false`.

The actual [merge commit 5dfcd4c8b29d715a12608113ca5e7162f8bb6810](https://github.com/Chris0Jeky/Loomward/commit/5dfcd4c8b29d715a12608113ca5e7162f8bb6810)
has tree `0a72674d826ca4a5043f6c8405ac1b67c032ba3f`, verified equal to the qualified head's tree.
These receipts qualify ordinary child-failure capture and normal successful qualification/cleanup.
They cannot guarantee a later artifact upload when runner execution is lost. Both failures above have
observed runner communication loss with an unknown underlying trigger. #1129 remains open.


## Final integrated positive receipts — 5 November 2026

The final maintenance head `c0ab2262bcfaf5c58fea09e181ca12637a80b72a` also completed both entire
workflows successfully: [PR 41960178507](https://github.com/Chris0Jeky/Loomward/actions/runs/41960178507)
and [push 41960173043](https://github.com/Chris0Jeky/Loomward/actions/runs/41960173043), 9/9 each.
The Windows jobs are **127024899379 / 127024835838**. Both artifact statuses are finished,
child exit 0, timedOut false, parentError null, cleanupCompleted true and cleanupFailed false.
Both stdout files record TokenElevation 0 and NTFS.

The independently downloaded ZIP digests match the API:
PR **13386861576**, **124,836 bytes**, SHA-256
`3ef787326c4d54d1b00cfe5bfe2c8eb3f2c1b13ec908404afe35b904d71b9506`;
push **13386069142**, **124,805 bytes**, SHA-256
`e3d56c03dc1dc62f03cfa2782d24676847c3dc987d0e8f70d94a691a500dacd1`.
Each archive contains exactly stdout, empty stderr and status. The PR status names checkout
`a10ad95d17f3f15d5facd0bbbd2c710d0667af3d`; the push status names the source head.
Detailed counts and the expected crash-test child negative control are recorded in
[the final Coverage receipt](coverage-identity-cancellation-1144.md#final-qualification-and-landing--5-november-2026).

These positive receipts confirm ordinary completion/upload and cleanup. They do not explain either
earlier runner communication loss or establish artifact retention after a runner disappears.
#1129 remains open for its documented residual cause and execution-evidence questions.
