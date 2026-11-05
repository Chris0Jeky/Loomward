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
