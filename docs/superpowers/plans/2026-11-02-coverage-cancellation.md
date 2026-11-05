# Coverage cancellation and worker accounting (#1108)

Base: GitHub main `3f5aaf2b9dd6a9c49d1c0471f4390deb6df5d10a`.

1. Pin the existing service API's missing worker reservation with a held synthetic
   read. Publish this tests-only revision for hosted Windows behavioral red evidence.
2. Extend native tree-read cancellation to observe the job and both root flags
   directly. Preserve durable admission and the ordered final root gate.
3. Run coverage in its existing job worker, holding its reading guard and one shared
   scan-worker reservation until actual I/O returns. Cancellation stays requested
   while an outstanding call drains; no helper read thread or premature terminal state.
4. Pin global coverage conflicts, permit exhaustion/release, phase cancellation,
   pending native I/O drain, result suppression, and explicit native refusal accounting.
5. Update the CLI caller and the coverage behavior note. Run all available checks;
   exact-head hosted Rust/Windows CI and fresh review remain required before merge.

No wire/schema changes, new effects, gate changes, dependencies, or progress contract
in this slice. Synchronous metadata calls still depend on their filesystem driver.
Rust tooling is absent locally: missing-tool failures are UNVERIFIED, never test red
or green. New cancellation APIs cannot be expressed by the pre-change signature;
those tests need staged implementation or a separately reported red limitation.

## Hosted red receipt

Tests-only PR #1131 head `c942f40412878e6edb025712c0e5e1f4ad35d3b7`,
run `41445039658`, Windows job `125275922496`: on 2026-11-02 at
12:49:38 UTC, `coverage_worker_is_accounted_until_the_held_read_returns`
failed with observed workers 0 versus expected 1 (`coverage read bypassed the
shared worker pool`). The standard-user service suite reported 684 passed,
1 failed, 2 ignored in 110.99 seconds. This was behavioral red with the old API,
not a compile error or an elevated skip. Production changes followed this receipt.


## Completion and subsequent follow-ups — 5 November 2026

This ownership/cancellation slice landed through
[#1137](https://github.com/Chris0Jeky/Loomward/pull/1137), whose exact head
`085f79fca49125b1290652bea42ca0c215dfde08` passed complete
[PR 41951591904](https://github.com/Chris0Jeky/Loomward/actions/runs/41951591904) and
[push 41951583002](https://github.com/Chris0Jeky/Loomward/actions/runs/41951583002) qualification,
9/9 jobs each. Its historical tests-first worker-reservation failure above remains preserved.

Separate follow-ups added observed phase progress through #1140/#1143 and completed final
directory-query cancellation/uncertainty through
[#1146](https://github.com/Chris0Jeky/Loomward/pull/1146). The final head
`c0ab2262bcfaf5c58fea09e181ca12637a80b72a` passed both full PR/push workflows
**41960178507 / 41960173043**, 9/9 each, including actual strict one-service/four-native passes
under a real standard-user NTFS token with no setup skips. Actual main
`1d5aaf0fd318bdeb692e9e0ec420f05266db96a9` has the exact qualified tree, and #1108/#1144 are
closed after scoped acceptance. See the [complete final-loop evidence](../../../evidence/v3/coverage-identity-cancellation-1144.md).

The original slice's no-progress-contract statement describes that slice only. The later phase
contract is separate. Cancellation still cannot guarantee interruption of a synchronous kernel
call; the held-return fixture and no-retrievable-cancelled-report guarantee retain their documented
limits. C8 remains closed.
