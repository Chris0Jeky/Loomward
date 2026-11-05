# Coverage phase progress (#1108)

## Behavior and scope

The Coverage job publishes four observed read passes: destination listing, source content reads,
destination content reads, and the final source recheck. The existing revisioned `job.state` stream
carries an optional `coverage_progress` object. Older jobs may omit it; present values must contain
exactly one known phase. No percentage, item total, path, or estimate of remaining work is added.

The callback runs on the owned reader before each pass, with cancellation checked again before
native I/O. Repeated phases do not advance the revision; cancellation and terminal states freeze the
last observed phase. Existing worker reservation, Reading guards, cancellation flags, pending-I/O
draining, root admission, and report-storage checks remain in the same call chain.

The UI names the phase and shows elapsed time against the selected read budget while explicitly
saying that work remaining is unknown. Cancellation calls it the last phase. Older services retain
the existing fallback. The mock transport follows the same phase order and cancellation freeze.

## Tests-first evidence, 2026-11-05

[PR #1140](https://github.com/Chris0Jeky/Loomward/pull/1140) first published only the protocol,
service, and Python contract regressions at `308dae0ac2e9f6d0e2dfbd4f6e163515e6c0fe4d`.
In [run 41951590108](https://github.com/Chris0Jeky/Loomward/actions/runs/41951590108),
[Linux Python 3.11 job 126996550467](https://github.com/Chris0Jeky/Loomward/actions/runs/41951590108/job/126996550467)
executed the new schema cases and rejected each of the four valid phases because the old closed Job
schema did not admit `coverage_progress`. This is the intended behavioral failure, not a missing
dependency or test compilation failure. No completed Windows negative run is claimed here.

App tests accompany implementation because their new typed Job fields do not compile against the
old DTO. A separate local Node check loaded the actual production TypeScript model: before the
change it failed on the first expected phase label; after the change all phase labels, elapsed/read
budget text, unknown-work wording, cancellation/terminal/fallback behavior, and the actual mock
transport's phase sequence and cancellation freeze passed. This focused check is not Svelte,
Vitest, or browser qualification.

## Regression coverage and local checks

The authored tests cover schema/DTO agreement; unknown phases, nulls and invented totals; ordered
native service events and strictly increasing revisions; final snapshots; cancellation at report
storage; repeated-phase suppression; cancellation/terminal freezing; cancellation immediately after
each phase callback; UI stale snapshots and older-service fallback; and mock cancellation behavior.

`python scripts/verify.py` passed 442 tests with 34 skips in the implementation worktree. Cargo is
absent, so that script explicitly reported Rust UNVERIFIED. Two of the skips are the new schema
tests because local `jsonschema` is absent; the hosted negative above did execute them.
`python scripts/gen_contracts_ts.py --check` and whitespace checks passed. A direct Node model/mock
check also passed after applying the patch to the integration-derived publication worktree.
Offline dependency installation could not find `zimmerframe-1.1.5`; full app and native gates are
therefore hosted on the exact PR head and recorded in its qualification update.

## Remaining limits

Phase progress does not bound cancellation latency within the existing directory identity-validation
loop. The worker remains owned while it drains and cancelled results are discarded, but adding finer
cancellation checks to that loop is separate work. No filesystem effects or copy capability are added.

## Integrated phase qualification, 5 November 2026

The phase implementation landed through [#1143](https://github.com/Chris0Jeky/Loomward/pull/1143)
as `5dfcd4c8b29d715a12608113ca5e7162f8bb6810`. Its tree
`0a72674d826ca4a5043f6c8405ac1b67c032ba3f` exactly matches qualified head
`0e7c94429920c488e9823cbd3fb6f42bbf3ed631`. Both
[PR run 41955056422](https://github.com/Chris0Jeky/Loomward/actions/runs/41955056422) and
[push run 41955048805](https://github.com/Chris0Jeky/Loomward/actions/runs/41955048805) passed
**9/9 jobs**, attempt 1.

The Linux and real standard-user Windows suites executed the phase schema/DTO, revision/freeze,
native phase-publication and callback-cancellation regressions. App unit/type/build and Linux
mock/live browser gates passed. Windows reported `TokenElevation=0`, NTFS, child exit 0 and
**707 service tests passed, 0 failed, 2 ignored**, followed by successful Clippy gates.

The final directory-identity loop limitation above is addressed separately in #1146; its negative
and positive qualification status is tracked in
[the #1144 evidence note](coverage-identity-cancellation-1144.md). This phase receipt alone does
not establish that later fix or a hard bound on a synchronous kernel call.


## Final cancellation follow-up and issue closeout — 5 November 2026

The remaining final directory-identity loop work is now qualified and merged through
[#1146](https://github.com/Chris0Jeky/Loomward/pull/1146), actual main
`1d5aaf0fd318bdeb692e9e0ec420f05266db96a9`. Exact head
`c0ab2262bcfaf5c58fea09e181ca12637a80b72a` passed
[PR 41960178507](https://github.com/Chris0Jeky/Loomward/actions/runs/41960178507) and
[push 41960173043](https://github.com/Chris0Jeky/Loomward/actions/runs/41960173043), 9/9 each.
Both real standard-user NTFS runs executed the strict one-service/four-native gates without
setup skips. The broader worker, retry, registry and phase acceptance is complete and #1108 is
closed. Full receipts and the query-return/kernel-call and internal-retention boundaries are in
[the final-loop evidence](coverage-identity-cancellation-1144.md#final-qualification-and-landing--5-november-2026).
The earlier phase-only qualification did not itself prove that later final-loop behavior.
