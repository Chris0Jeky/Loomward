# C6 fault and crash matrix for the copy executor

Date: 2026-10-17. Host: **KRASPYON**, Windows 11 Home 10.0.26100, NTFS C:, not elevated, a laptop shared with
about ten parallel agent lanes. Commit: `dcf6077` (branch `test/415-c6-copy-crash-matrix` on origin/main
`8a3dc6f`, which has the C5 executor #522). Refs #415, LW-041 #47; design: [doc 47](../../docs/47-copy-and-verify.md)
sections 5, 8, 10, 11 (row C6) and 12.

**Lab only, crash consistency only.** A process kill keeps the OS cache, so none of this is power-loss
durability (G5) or a proven restore (G9); both remain open and are not this lane. The C8 gate stays shut.

## How it runs

- Kill points live in `loomward-windows::effects::exec` and compile only into test builds (`#[cfg(test)]`;
  production reads no environment variable). Each journal record is a point before and after it commits, as
  are the staging folder create, each item create, each source chunk handed to the writer (so mid-write at
  0, 1, 2 and 3 MiB), each metadata set, each verification (before and after), the rename step, the rename's
  return and each post-publish confirmation read.
- The harness (`effects/exec_crash_tests.rs`) re-runs the test binary as a child for one step: a run, recovery
  alone, or recovery followed by the owner's Abandon. A clean child enumerates the boundaries; then one child
  per boundary N calls `TerminateProcess` on itself at exactly the Nth (no unwinding, no destructor). The
  harness checks that the child died at the label the clean run had at N.
- Then the parent reopens `state.db`, runs `recover`, checks the oracle, runs Abandon (`cancel`) when the
  operation is still open, and checks the oracle again.
- **Resume: not applicable.** C5 has no resume: a stopped run can only be abandoned (doc 47 section 11, C5
  seam). So the matrix is crash point x {recover on open, abandon after recover}. When resume lands, it becomes a
  third column.

### Oracle (after every recovery and every abandon)

1. The journal replays, and the receipt (`status`) agrees with it.
2. The source is unchanged: identity, signature and bytes of every object (snapshot), and the C1 verifier
   (`read_tree` + `tree::compare`) finds it equal to its pre-copy reading.
3. No receipt says `committed` unless C1 finds the published folder equal to the source.
4. Recovery and abandon create nothing: every path in the destination parent existed at the kill.
5. Q1 removal off (the default): nothing that existed is gone or replaced, so staging is never deleted. Q1
   removal on: every removed path's file ID is one the journal recorded (staging folder or an item identity).
   The same check runs on the child's own effects.
6. The destination parent holds only the pre-existing entry, journaled staging tokens and the final name.
   A pre-existing file there and a folder beside the lab roots are byte- and identity-unchanged.
7. After Abandon the state is terminal or `needs_attention`. When `cancelled`, the parent holds only the
   pre-existing entry, the staging folder left for the owner (removal off, and only then) and reported
   never-journaled leftovers.

The oracle was shown to fail. With cleanup ignoring the Q1-off policy, it failed at the first abandon
("removal off, yet ~lw-staging-... is gone"). With large default-stream chunks corrupted and the read-back
digest check removed, it failed on the first clean run ("FALSE COMMIT"). Both mutations were reverted.

## Crash points

docs/47 section 10 names CP-01 to CP-15 without listing them. This lane fixes the numbering from section 5's
journal boundaries, and adds section 12's open items for lane C2 and the C5 records as CP-16 to CP-22. An
unclassified label fails the run, so the map cannot fall behind the executor.

| CP | Boundary | Labels |
|---|---|---|
| CP-01 | approval consumed, before the staging intent | `before staging_intent` |
| CP-02 | staging intent, before its identity record (folder may exist, empty, unjournaled) | `after staging_intent`, `staging folder made`, `before staging_created` |
| CP-03 | staging identity, before the create batch | `after staging_created`, `before create_batch` |
| CP-04 | create batch, before its identity commit (empty files unjournaled) | `after create_batch`, `created item`, `before identities` |
| CP-05 | identity commit of a create batch (doc 47 section 12), before any byte | `after identities` |
| CP-06 to CP-09 | mid-write at offsets 0, 1 MiB, 2 MiB, 3 MiB | `write@<offset>` |
| CP-10 | written, metadata set, mid-verify, before the verified record | `metadata set`, `verify item`, `verified item`, `before verified` |
| CP-11 | verified, staged, before the publish intent | `after verified`, `... transition:staged`, `before publish_intent` |
| CP-12 | publish intent, before the rename step | `after publish_intent` |
| CP-13 | rename, before `committed` | `renamed`, `before committed` |
| CP-14 | `committed` | `after committed` |
| CP-15 | mid-abandon: cancelling and cancelled | `... transition:cancelling`, `... transition:cancelled` |
| CP-16 | the rename step (stream holds released, rename not issued) | `rename` |
| CP-17 | `publish_withdrawn` | `... publish_withdrawn` |
| CP-18 | post-publish confirmation (recovery and abandon), per item | `confirm item` |
| CP-19 | each cleanup removal and the staging folder removal | `... item_removed`, `... staging_removed` |
| CP-20 | C5 `planned` creation-time record (files, then folders) | `... planned` |
| CP-21 | C5 `staging_left` (Q1 off) | `... staging_left` |
| CP-22 | C5 refused at `preparing` | `... transition:refused` |
| CP-F | refusal records reached by fault scenarios | `item_failed`, `transition:blocked`, `transition:needs_attention` |

C5 has no admission lock or fence epoch read at the rename (revocation is not wired into the executor).
CP-16 is the point immediately before the rename call: the place that read will take. The revocation
cases of section 10 (publish barrier, revoke at each boundary) wait for that wiring.

## Scenarios

The fixed tree holds a 3 MiB + 1 file and files of 0, 1 and 4097 bytes, plus a named stream, read-only and
hidden files, a nested folder and an empty folder. Every scenario uses it with a pre-existing entry in the
destination parent.

| Scenario | Phase 1 (this process) | Child step |
|---|---|---|
| Run | none | `run` from `preparing` |
| Refused | destination denies folder creation (icacls) | `run`, refused at `preparing` |
| AbandonVerified | run stopped after the first verified batch | `recover`, then Abandon |
| AbandonPublish | run stopped with the publish intent journaled | `recover`, then Abandon (`publish_withdrawn`) |
| Confirm | run stopped between rename and `committed` | `recover` (post-publish confirmation) |

## Full run: `LOOMWARD_CRASH_SWEEP=full`

`cargo test -p loomward-windows --features copy-primitives crash_tests -- --nocapture --test-threads=2`,
2026-10-17 09:19 to 09:27 UTC, 7 min 54 s, 7 passed.

**Matrix: 236 kills, every boundary of every scenario under both Q1 policies.** 0 source changes, 0 false
commits; 30 commits, each C1-equal to the source.

| Scenario | Removal off | Removal on |
|---|---|---|
| Run | 70 boundaries, 70 kills | 70, 70 |
| Refused | 4, 4 | 4, 4 |
| AbandonVerified | 6, 6 | 26, 26 |
| AbandonPublish | 6, 6 | 26, 26 |
| Confirm | 12, 12 | 12, 12 |

Kills per class: CP-01 4, CP-02 8, CP-03 4, CP-04 24, CP-05 2, CP-06 16, CP-07 2, CP-08 2, CP-09 2, CP-10 52,
CP-11 10, CP-12 2, CP-13 6, CP-14 4, CP-15 12, CP-16 2, CP-17 4, CP-18 20, CP-19 44, CP-20 8, CP-21 4, CP-22 4.
Every class CP-01 to CP-22 was killed at least once (the run asserts it).

Outcomes (state after recovery -> after Abandon): copying -> cancelled 109, cancelled -> cancelled 62,
committed -> committed 30, copying -> needs_attention 11, preparing -> cancelled 10, publishing -> cancelled
6, staged -> cancelled 4, cancelling -> needs_attention 2, refused -> refused 2.

The `needs_attention` endings are the doc's designed answer, not failures. A kill inside a create batch leaves
empty files under manifest names with no journaled identity. They are never adopted or removed, so with
removal on the staging folder cannot be emptied and the run waits for the owner (section 5 table, row 3).
A kill during a removal is the other cause.

**Random: 240 kills, seed `0x2026101700c6`.** The seed drew 48 random trees, each with 1 to 8 files in up to
three nested folders. Sizes sit at sector and chunk edges or are random below 300 KB, and files are sometimes
read-only, hidden, carry a named stream or have a Unicode name. Each tree got a random scenario (Run,
AbandonVerified, AbandonPublish, Confirm) and a random Q1 policy, then 5 kills at uniformly random
boundaries. 0 source changes, 0 false commits; 48 commits, each C1-equal. Outcomes: cancelled -> cancelled
92, copying -> cancelled 67, committed -> committed 48, cancelling -> needs_attention 9, copying ->
needs_attention 8, publishing -> cancelled 8, staged -> cancelled 6, preparing -> cancelled 2.
Reproduce with `LOOMWARD_CRASH_SEED=<decimal seed>` (default `0x2026101700c6`).

**Total: 476 kills, zero source changes, zero false-complete receipts.**

## Injected faults (live, both Q1 policies where it matters, same oracle)

| Fault | How | Result |
|---|---|---|
| Disk full on the destination | the C5 seam: `Flow::Fail(112)` at `Point::Writing`, where a failed preallocation or write returns it. No small lab volume: creating one needs elevation | `blocked`, `insufficient_space` on the item, staging kept; Abandon `cancelled`, staging left (off) or removed (on) |
| Sharing violation | a source file held open for write by another handle (real) | `blocked`, `source_in_use`, never published; Abandon `cancelled` |
| Access denied, staging create | icacls deny "create folder" on the destination (real) | `refused` at `preparing`, `access_denied`, nothing created |
| Access denied, item create | icacls inherit-only deny of file and folder creation (real) | `blocked`, `access_denied` on the item, staging kept; Abandon `cancelled` |
| Source changed mid-copy, then a kill | source rewritten after the identity commit (real) | never committed, never published; Abandon `cancelled` |
| Source changed between rename and `committed` | kill at CP-13, then the source rewritten (real) | recovery does not commit; Abandon ends `needs_attention` and the published copy is kept byte-identical |

## Elevation

Hosted runners are elevated and the copy primitives refuse elevated tokens. Each C6 test first proves the
refusal (`identity::observe` fails), then skips with the reason, like `effects::tests::skip_elevated`.

## Default mode (CI)

Without `LOOMWARD_CRASH_SWEEP=full` the matrix kills each scenario once per crash-point class it reaches
(Run, Refused and Confirm with removal off, AbandonVerified under both policies, AbandonPublish with removal
on). It also kills at the first and last `staging folder made` and `created item`, the effect-before-journal
gaps, with Run under both policies. That is 35 kills covering all 22 classes, including two
`needs_attention` endings from unjournaled creates, plus 4 random kills. Measured: 42 s for the earlier
29-kill selection, and 184 s for the 35-kill selection with the laptop under heavy parallel load. A child that
does not exit within 120 s fails the run instead of hanging it.

The preview refuses with `insufficient_space` when C: has less free than the reserve (the larger of 1 GiB and
1% of the volume). On this 1 TB host, shared with other lanes' build directories, that refusal was seen
once at 7.5 GB free, before this lane's own build cache was cleaned.

## Not covered here

- Real power cuts (G5) and the proven restore (G9): open, owner method pending (Q2).
- Resume after a crash: C5 has none.
- Revocation at each boundary, the publish barrier and the publish TOCTOU regression: they need the admission
  lock and fence wiring, which C5 does not have.
- A kill between `started` and the plan record (the service writes both): `recover` handles it as
  cancelled-before-plan, but no kill exercises it here.
- Journal write failure injection (`state.db` full or locked).
