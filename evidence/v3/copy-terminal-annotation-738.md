# Draft: preserve a pending revocation when another Store finishes a copy

**Status: design and known-failure reproducer. Issue #738 remains open. This document does not change the journal format or enable copy effects.**

This follow-up addresses the remaining cross-Store annotation gap from [#738](https://github.com/Chris0Jeky/Loomward/issues/738). The separately reviewed [#751](https://github.com/Chris0Jeky/Loomward/issues/751) improvement adds the missing crash boundaries around a pending `Fenced` append and avoids an unnecessary replay on an ordinary commit. It does not make a pending cause visible to another Store.

The diagnosis uses the copy candidate at `7a0f89a10249d72dbfbf5d835ad30f4d3ad9877a`, plus that separate #751 delta. The production executor blob for the diagnosis is `c06c8767b2830e21785b5fe8526e3d29b13db638`. The native reproducer patch is based on #799 head `412d9bd22e9883ea8fc8bc28e5a498bbcfff4138` and test-file blob `8ae287622e2e2a36831e49bae5f8c8abf62ec91f`, preserving the separately revised handoff fixture. The cross-Store reproducer is deliberately kept outside normal test gates until a reviewed repair makes its assertion pass.

## Reproduced ordering

| Step | Actor | Durable journal | Pending cause |
| --- | --- | --- | --- |
| 1 | Store A's worker enters an admitted rename step and holds its SQLite Guard transaction. | `Publishing` | None |
| 2 | A fence raises the resident control and reaches its bounded wait. | `Publishing` | `GrantRevoked`, in memory; reply is `PendingInFlightStep` |
| 3 | The admitted step returns and releases the SQLite write lock. The resident worker has not yet journaled the cause. | `Publishing` | Still pending |
| 4 | Store B, with its own mutex and Control, finishes confirmation and commits first. | `Committed` | Only A can see the pending cause |
| 5 | A runs `journal_fence_locked`. Its replay is terminal, so `take().filter(...)` consumes and discards the cause. | `Committed`, without a fence record | None |

The executable Linux metadata reproducer ran the actual `Journal::while_in`, `Journal::commit`, `Control` admission/fence functions, `journal_fence_locked`, and `status` functions with two real Store instances and SQLite. Its effect callback only models a parked admitted step. It produced:

```text
#738 observed: state=Committed, refusal=None, pending_cause=None
```

The intended assertion fails because the receipt lacks `Some((GrantRevoked, None))`. The same run passes the terminal-state guard, both #751 regressions, and the child entrypoint: **four tests pass and this one known-failure test fails; none is ignored**. That count is not a count of native Windows scenarios.

## Why the existing records cannot repair the terminal winner

`Record::Fenced` explicitly rejects every terminal state in `crates/loomward-operations/src/journal.rs`. `Committed` cannot be repeated, and no `Transition` can leave a terminal state. Treating a pending cause as if it had become durable, or allowing a finished operation to advance, would violate the present contract.

Writing a pending fence before the Guard releases its transaction would cover a cause already present at that particular boundary. It would also change the Guard's current role, require append-failure and lock-order handling, and still leave the interval in which a cause arrives after Guard release but before another Store commits. It cannot recover a cause lost in a process crash before any durable intent exists.

The bounded correction needs an explicit journal metadata contract. It must preserve the original terminal state, verified receipt, and effect restrictions.

## Proposed metadata contract for review

The tentative record name is `RevokedAfterPublish { cause: FenceCause }`. The name and format require review before implementation.

- It is legal only for an already `Committed` operation with its publish identity retained. It records a revocation that was already pending while the operation was nonterminal.
- It changes no operation state, fence epoch, item identity, verification result, cleanup authority, or effect eligibility. The journal sequence and chain advance as for any durable metadata append.
- `Fenced` remains invalid on every terminal state. A terminal record does not create a path back into publishing, copying, or cleanup.
- The executor must distinguish an eligible pending cause from a new revoke first observed after the operation finished. The latter remains `Ack::Settled`; it must not manufacture a historical race or annotation.
- An eligible pending cause needs its operation and observed nonterminal epoch recorded in the resident control. A later request must not overwrite that cause merely because its own first replay sees a terminal operation.
- A retry deduplicates the same operation and cause, including an existing equivalent `Fenced` record. Different causes must retain their identity; they must not overwrite one another. The proposed bound is at most one annotation per operation and cause.
- The status reader exposes the same existing `grant_revoked` annotation used for a pre-terminal fence. The committed state and verification counts remain unchanged. The receipt precedence rule when another refusal is already present must be pinned by a test; earlier journal reasons must remain intact.

This proposal adds a record type. Older readers currently refuse unknown record kinds, so rollback/read compatibility must be explicitly reviewed. It does not silently broaden `Fenced`, rewrite historical rows, or relax unknown-record refusal.

## Bounded implementation shape

1. Define the metadata record and its replay rules in `crates/loomward-operations/src/journal.rs`. Keep terminal transition and effect admission rules unchanged.
2. Add a Store operation that checks the head and tail, validates the terminal receipt, checks for an equivalent annotation, and appends the row and head in one `BEGIN IMMEDIATE` transaction. Two Store instances must not race a separate read against their writes. Return whether the annotation was appended, already present, or ineligible.
3. Preserve the eligible pending cause until that operation succeeds or a fresh replay proves an equivalent annotation durable. A row/head/write error retains the pending cause and reports a retryable failure. A lost acknowledgement must be recognized on retry without a second row.
4. Route only the terminal-winner case in `journal_fence_locked` through the new metadata operation. Keep the existing nonterminal `Fenced` path and its #751 crash boundaries. Handle the same eligible pending cause when a later `fence_waiting` call first sees `Committed`.
5. Wait for any resident admitted step without holding the Store mutex before acknowledging the pending request. A metadata append supplies no acknowledgement shortcut around quiescence.
6. Extend the existing status reader and journal crash/fault fixtures. Pin the receipt annotation independently of the operation's terminal state and verification result.

The likely production paths are `crates/loomward-operations/src/journal.rs`, `crates/loomward-operations/src/store.rs`, and `crates/loomward-windows/src/effects/exec.rs`. The corresponding Operations, executor, crash, and service tests must exercise the public receipt and retry paths. Update the revocation section of `docs/47-copy-and-verify.md` only when the record contract is accepted.

## Acceptance matrix

| Scenario | Required durable/result behavior | Required safety check |
| --- | --- | --- |
| Same Store commits with a cause already pending. | Existing `Fenced` then `Committed`, annotated. | #751 crash boundaries and confirmation decision remain covered. |
| Another Store commits before the pending cause is written. | Exactly one durable matching annotation; state stays `Committed` after reopen. | Original confirmation and item verification remain unchanged. |
| A brand-new revoke is first observed after an already-settled operation. | `Ack::Settled`, with no invented earlier pending request. | No new effect or transition. |
| A pending request is retried after the foreign commit. | The eligible old cause is retained and recorded. | A later request does not overwrite or erase it. |
| Two Stores try to annotate the same cause. | One effective annotation under the SQLite transaction; retries converge. | No duplicate row caused by a read/write race. |
| Different causes or an existing equivalent fence are present. | Preserve cause identity and deduplicate only the defined equivalent. | Preserve earlier journal reasons and receipt precedence. |
| The new row insert or head write fails. | Neither becomes partially durable; cause remains pending; failure stays retryable. | Head/tail equality and rollback behavior stay enforced. |
| The annotation commits but its acknowledgement is lost. | Reopen/retry discovers it and does not append it again. | No success claim based only on volatile state. |
| A resident step is still admitted when a terminal winner is observed. | Pending reply until the required quiescence condition is met. | No Store mutex is held while waiting; status remains available. |
| The journal head or tail is damaged. | Annotation is refused through existing validation. | No repair by rewriting or re-anchoring damaged history. |
| `Fenced`, `Committed`, or an advancing transition is attempted after terminal state. | Existing illegal-record rejection remains. | A publishing/copying/cleanup effect callback never runs. |
| Crash after the foreign commit but before a new annotation. | A reopened journal remains `Committed`; a still-live eligible request may retry. | Do not claim that a lost volatile cause can be reconstructed without a specified durable source. |
| Crash after the annotation becomes durable. | Reopen preserves both the committed receipt and its annotation. | No automatic effect or cleanup. |
| Process dies with a pending cause before it is durable. | Retain the existing no-automatic-effects recovery requirement. | Native restart test must still refuse every effect; attribution durability in this earlier window is a separate requirement. |
| Older/newer binaries read the new record. | The accepted compatibility policy is explicit and tested. | No silent skip of an unknown record or relaxation of replay validation. |

## Reproducer delivery and evidence limits

`known-failure-738-native.patch` adds one native executor test when deliberately applied to an isolated checkout. It is **unapplied known-failure material**, not an ignored test or a passing gate. It uses the real Windows rename and another Store's real recovery/confirmation path. The fixture stops unless that foreign recovery actually reaches `Committed`; setup refusal is not a reproduction of the annotation bug.

Run that reproducer only in the existing supported, non-elevated synthetic Windows lab:

```console
cargo test -p loomward-windows --features copy-primitives effects::exec::tests::another_store_committing_a_pending_rename_keeps_the_annotation -- --exact --nocapture
```

The native reproducer has **not run in the Linux review environment**. Local proof covers production metadata/admission code and real SQLite, with an explicit synthetic effect callback. It does not prove Windows rename, Windows process termination, native source/destination verification, power-loss durability, or end-to-end personal copying.

Completion requires the known-failure assertion to turn green with the reviewed contract, the acceptance matrix above, and the existing product validation gates. C8 stays closed and personal effects remain disabled during this work. No passing, ignored, skipped, or unrun test is a substitute for those gates.
