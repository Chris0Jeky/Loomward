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

C5 has the per-run admission lock and a fence epoch read at every effect step, the rename included
(`crates/loomward-windows/src/effects/exec.rs`, `admit`; docs/47 section 5; #544). A revocation or an identity
change fences a running copy (#564). CP-16 is the point immediately before the rename call, where the rename
step takes the admission lock and reads the epoch. The revocation cases of section 10 are only partly covered;
see "Not covered here".

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

## Journal append faults (#535)

The kill matrix stops the process; it never makes a journal write fail while the process lives on. docs/47
section 8 lists `state.db` full or locked ("stop before the effect; no effect without its intent"). A test-only
seam now fails a chosen `Store::append` as SQLite would, and the executor runs on in this process.

**Seam.** `loomward-operations` feature `fault-injection` (off everywhere except the dev-dependency of
`loomward-windows`; production builds contain no hook, no field and no error path for it). A hook sees every
append and its stage and may fail it:

| Stage | Where inside the append | After the error |
|---|---|---|
| `Row` | after validation, before the journal row is inserted | rolled back: nothing stored |
| `Head` | after the row, before the head anchor (#539) | rolled back: row and head together, never a row without its head |
| `Ack` | after the commit | the record is durable while the caller is told it failed (a lost acknowledgement) |

Two kinds: `Full` (SQLITE_FULL, "database or disk is full") and `Busy` (SQLITE_BUSY, "database is locked").
`crates/loomward-operations/tests/append_faults.rs` also drives the two real SQLite errors with no seam: a second
connection holding the write lock past the 5 s busy timeout, and `PRAGMA max_page_count` capping the database.
Both leave the journal replaying (head equals tail) and accepting the next append.

**Matrix** (`a_failed_journal_append_stops_the_executor_and_recovers_under_the_oracle`, in-process, no kill). For
every journal append each scenario's step makes (Run, Refused, AbandonVerified, AbandonPublish, Confirm; run, or
recover then Abandon), under every variant of kind x stage, the Nth append fails. The same oracle then applies,
plus these checks:

1. The fault surfaces as that database error (`Fault::Journal` naming `DiskFull` or `DatabaseBusy`), never
   swallowed, never turned into another outcome.
2. No append is attempted after the failed one, and the destination parent is identical when the step returns and
   at the moment of the failure (no effect after a failed record; write-ahead).
3. The journal holds exactly the rows before the failed append (`Ack`: plus that one); it replays; the receipt
   (`status`) agrees with it and passes the oracle before any recovery.
4. Recovery on open, then the owner's Abandon, then the oracle again: the same end states as a kill at that record.

Record classes failed: CP-01, 02, 03, 04, 10, 11, 13, 15, 17, 19, 20, 21, 22 (every "before <record>" boundary
of the kill map; the run asserts each). CP-05 to CP-09, 12, 14, 16 and 18 are effect boundaries or "after"
labels and have no append of their own.

Default mode (this PR): 43 faults, 1 variant per append, rotating through the six; all 8 `crash_tests` passed in 51 s
on a quiet host. Full: 360 faults, every append under all six variants, both Q1 policies, 0 source changes, 0
false commits, 24 commits (the `Ack` variants at `committed`, each C1-equal to the source). Faults per class: CP-01
24, CP-02 12, CP-03 12, CP-04 12, CP-10 24, CP-11 24, CP-13 24, CP-15 36, CP-17 12, CP-19 132, CP-20 24, CP-21 12,
CP-22 12. Outcomes (state after recovery -> after Abandon): cancelled -> cancelled 184, copying -> cancelled 88,
committed -> committed 24, preparing -> cancelled 24, publishing -> cancelled 12, staged -> cancelled 12,
cancelling -> needs_attention 8, copying -> needs_attention 4, refused -> refused 4. The `needs_attention`
endings are the designed answer for a failed write mid-cleanup or after a create batch (an unjournaled
create is never adopted).

A lost acknowledgement at `committed` leaves a published, C1-equal copy and a `committed` receipt while the run
returned an error: a false failure, never a false completion. Callers must treat `Fault::Journal` as "read the
journal", not "nothing happened".

**The oracle was shown to fail.** (a) With the `create_batch` append's error ignored (`let _ = ...`), the run
continued to a committed copy and the matrix failed at the first lost-acknowledgement variant: "the failed append
was swallowed or misreported". (b) With the staging folder created after a failed `staging_intent` append, it
failed with "an effect ran after its record failed" and the extra `~lw-staging-...` entry. Both mutations were
reverted.

## Elevation

Hosted runners are elevated and the copy primitives refuse elevated tokens. Each C6 test first proves the
refusal (`identity::observe` fails), then skips with the reason, like `effects::tests::skip_elevated`.

## Default mode (CI)

Without `LOOMWARD_CRASH_SWEEP=full` the matrix kills each scenario once per crash-point class it reaches
(Run, Refused and Confirm with removal off, AbandonVerified under both policies, AbandonPublish with removal
on). It also kills at the first and last of four labels, with Run under both policies: the
effect-before-journal gaps `staging folder made` and `created item`, and the folder metadata sets
`directory metadata set` and `staging metadata set`, which change journaled creation times one at a time.
That is 41 kills covering all 22 classes, including two `needs_attention` endings from unjournaled creates,
plus 4 random kills. At `04fb4ca` it passed in 195 s, 0 source changes and 0 false commits, with the laptop
under heavy parallel load (the first 29-kill selection took 42 s on a quieter host). A child that does not
exit within 120 s fails the run instead of hanging it.

**Folder-metadata kill points came after the full run.** They were added in `04fb4ca` from the Codex review of
#532, so the full run at `dcf6077` has no kill between partial folder metadata sets. They are covered by the
default-mode run above. The full re-run at `04fb4ca` was refused at preview with `insufficient_space`: C: had
9.1 GB free, below the 1% reserve, because of other lanes' build directories. It was owed.

**Full re-run, 2026-10-17 14:34 to 14:46 UTC, 11 min 25 s (684 s of tests), 8 passed** with the journal append
matrix above in the same command. The kill matrix is now **242 kills** (the folder-metadata points raised Run
from 70 to 73 boundaries per policy): 0 source changes, 0 false commits, 30 commits, each C1-equal to the
source; CP-10 58 (was 52), every class CP-01 to CP-22 killed at least once. Random: 240 kills, seed
`0x2026101700c6`, 0 source changes, 0 false commits, 53 commits. Total 482 kills and 360 append faults.

The preview refuses with `insufficient_space` when C: has less free than the reserve (the larger of 1 GiB and
1% of the volume). On this 1 TB host, shared with other lanes' build directories, that refusal was seen
at 7.5 GB and at 9.1 GB free.

## Not covered here

- Real power cuts (G5) and the proven restore (G9): open, owner method pending (Q2).
- Resume after a crash: C5 has none.
- Revocation at each boundary: `exec_tests.rs` fences at staging creation, identity commit, before and after
  the rename, and mid-file at the 1 MiB chunk. Not fenced there: the `Writing`, `Written` and `Verified`
  boundaries, and mid-file offsets other than 1 MiB. The publish barrier test and the publish TOCTOU regression
  (docs/47 section 10) are not written.
- A kill between `started` and the plan record (the service writes both): `recover` handles it as
  cancelled-before-plan, but no kill exercises it here.
- A failed `set_plan` write (the service writes the plan after `started`, outside the executor), a failed
  `Store::start`, and an ambiguous commit where the database has the record but the connection is gone before
  the executor is told (modelled only as the `Ack` stage, in the same process).
- Journal faults during several appends in a row (the matrix fails exactly one append per run, then recovers).
