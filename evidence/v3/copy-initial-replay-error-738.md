# Initial journal-read failure during revocation (#738)

**Status: source repair and portable regressions prepared for hosted validation. Native execution remains unverified; C8 stays closed.**

This follow-up addresses [PR #809's initial-replay review finding](https://github.com/Chris0Jeky/Loomward/pull/809#discussion_r4331589136). The previous executor closed admission before its initial replay but registered the pending cause only after that read succeeded. A read error could therefore stop a worker without retaining the request for its commit, finish or the service reaper.

The source baseline is PR #809 head `45c5fd0d26cdc6b359845ab515d879ba5306072a`. The production executor baseline blob is `60ee88a23c4ab840fbbb2fc9bbd453c2a9c58b12`; its portable test module baseline is `2ddff44be71fb448871295ad7395ed614411389a`.

## Contract

A request is retained with its operation and cause before the first fallible replay. Its epoch starts unknown. Only an actual validated nonterminal observation supplies the epoch that makes a terminal annotation eligible. Existing pending entries must belong to the operation before another cause can be inserted.

A new request whose first observation succeeds as terminal is settled without adding history. An earlier request whose first observation failed cannot be treated as that fresh request on retry. If the first successful later observation is terminal, the available information cannot distinguish these two histories:

1. The operation was already terminal when the first request arrived.
2. The request arrived while nonterminal, but another Store committed before the failed read could be retried.

The executor annotates neither history without evidence. It also does not discard the request: retry returns a journal error, the original cause remains pending, and the live service keeps new copy admission closed with `copy_fence_pending`. A terminal receipt, a previously used Control or the Control's fence/rename flags cannot supply an invented epoch.

If a later validated replay is nonterminal, all retained same-operation requests without an epoch can be qualified from that observation before any are appended. Worker commit and finish resolve the queue before their own terminal decision. A qualified cause can be recorded or deduplicated while a distinct unknown cause remains retained; partial success must not clear the remaining pending flag.

This is conservative live-service retention. An unknown request first observed successfully only after terminal state may remain blocked for that service's lifetime. Repeating a terminal replay cannot prove the missing ordering. There is no durable request outbox, automatic history repair, automatic resume or new cleanup authority. Process/service teardown can still lose a request that never became durable; restarting is not evidence that it was acknowledged.

The existing inert-tail exception is preserved for a fresh request: a tail refused by every Store prevents all later effects, and acknowledgement still waits for an admitted call. That branch does not erase an earlier unresolved request. Other initial SQL/replay errors retain the request.

## Regressions

The executor tests use real SQLite, actual executor controls and production journal paths. They have no lab/elevation early return and invoke no native copy effect.

| Test | Required result |
| --- | --- |
| `a_first_replay_error_keeps_the_original_request_for_the_workers_commit` | The failed request remains retained; the real worker commit writes its original `Fenced` immediately before `Committed`, survives reopen and deduplicates on retry. |
| `a_first_read_error_cannot_classify_a_foreign_terminal_winner` | Both indistinguishable histories above remain pending and fail retry without adding rows, changing the receipt or inventing attribution. |
| `a_request_retained_before_replay_cannot_move_to_a_different_operation` | A valid second terminal operation cannot receive or consume the first operation's unknown cause. |
| `a_qualified_annotation_does_not_discard_a_distinct_unknown_cause` | Exactly one eligible annotation is durable while the distinct unknown request and retention hint remain. Repeated same/different-cause calls add no rows. |
| `an_unknown_request_cannot_acknowledge_an_admitted_step` | Metadata retry and the fencer cannot acknowledge before the admitted step returns; Store remains available, and terminal provenance remains unknown afterward. |
| `a_first_replay_error_survives_service_reaping_until_its_provenance_is_resolved` | The real service reaper retains a finished worker's request through an active read failure. Later nonterminal qualification drains and disposes the Control; a foreign terminal winner keeps it retained, preserves status and blocks new admission. |

The service test module is now inline under `crates/loomward-service/src/copy.rs`, retaining its original `pending_fence_tests` module name and Windows/test configuration. The external `copy_pending_fence_tests.rs` is deleted. This restores the existing architecture rule that the service copy module is the sole consumer of the Windows copy primitives. The architecture guard and its negative controls are unchanged.

## Failure fixture and execution record

The read-error fixture temporarily renames the synthetic database's `op_journal_head` table, calls the real `Store::replay` path, then restores the name before checking results. It deliberately creates an outer SQL read error. An ordinary foreign writer in WAL mode does not establish this failure because ordinary readers remain permitted.

A local Python/SQLite WAL probe confirmed that an existing connection's head query fails with `no such table: op_journal_head` during the rename and reads the unchanged head again after restoration. This checks the fixture mechanism only. It is not a Rust or Windows runtime result.

The three tests were preserved separately before the production repair. The initial hosted RED commit required formatting corrections before it could execute. Formatted tests-only head [`36579de81511b7d9b8ac907c9d6f6c31d838adb4`](https://github.com/Chris0Jeky/Loomward/commit/36579de81511b7d9b8ac907c9d6f6c31d838adb4) then ran all 14 tests in [Windows job 118968401260, run 39522083187](https://github.com/Chris0Jeky/Loomward/actions/runs/39522083187/job/118968401260): the three new tests failed as intended, the 11 existing tests passed, and none was ignored. The observed failures were the worker's missing `GrantRevoked` receipt, an incorrectly successful unknown-terminal retry, and an incorrectly accepted wrong-operation request. This is the actual runtime RED result; the full repair's formatting, compile, test and Clippy results remain unverified in this receipt.

Run the existing focused gates without elevation-dependent skips:

```console
cargo +stable test --locked -p loomward-windows --features copy-primitives pending_fence_tests
cargo +stable test --locked -p loomward-service copy::pending_fence_tests
```

Record exact-head formatting, the full workspace suite, the existing architecture negative controls, Windows all-features tests and Clippy results before treating the repair as qualified. These metadata/service regressions do not prove native rename, native recovery confirmation, process termination, power-loss durability or personal copying. The separate native qualification draft remains necessary; #738 and C8 retain their existing qualification boundaries.
