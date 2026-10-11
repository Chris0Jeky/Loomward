# Pending-publication window: references bound between staging and publication (#356)

Measured on 2026-10-11, Windows 11 (build 26300), rustc/cargo 1.97.1, Python 3.14.3, Node 24.13.1,
branch `fix/pending-publication-window-356` on base `2944542`. Native Rust engine only; the Python
reference is unchanged. All fixtures are synthetic (fixture roots, synthetic identities).

## The window

The catalogue writer stages reference intents (`st.pending_reference`) from a snapshot of
`st.object_ref`, in `state.db` transactions that commit **before** the catalogue transaction that
publishes the listing and its witness token (`writer.rs`, "Precious writes commit before the
catalogue transaction"). Any reference that another connection binds after that snapshot and
before the catalogue commit has no intent. After publication and reconciliation it stays
`resolved` to the pre-publication row: it neither follows a hard link to the surviving row nor
retires with a dropped row.

## Reproduction (before the fix)

A one-shot test seam, `PublicationWindow` (`crates/loomward-catalog/src/writer.rs`, behind the new
`test-hooks` feature and `cfg(test)`), pauses the writer after the staging commits and before the
catalogue transaction. Only commands that stage intents (`DirListing`, `ListingDone`, `EndRun`)
pause there.

| Writer | Path into the window | Result before the fix |
|---|---|---|
| `collections.update_members` add | Writes `st.object_ref` directly in its own `BEGIN IMMEDIATE` transaction on the service connection | **Real.** Service test: adds of file row 1 (hard-linked to row 2) and row 3, made while the writer was paused in a rescan that drops rows 1 and 3, ended as `[(1,"resolved",false,1),(2,"resolved",false,3)]` (`id,state,retired,row_id`). Both are bound to deleted rows; the member for row 1 showed `unresolved` although its identity survives as row 2. Catalogue test: the same, with refs 5 and 6 left on rows 1 and 3. |
| `feedback.record` (learning label) | Binds through `WriteCommand::ObjectReference` on the writer thread | **Cannot happen.** The command queues behind the paused publication (the queue's byte charge rises; no `object_ref` row appears while paused). After release it runs against the published catalogue: the dropped row answers `NotFound`, with no reference and no label. The test passed before the fix and still passes. |

So the defect is confined to `collections.update_members`, the only reference writer outside
the writer thread (`grep` for `INSERT INTO st.object_ref` in non-test code: the writer and
`collections.rs` only).

## Chosen fix: option (a), re-stage at publication

Option (a) keeps every guarantee and needs no wire or schema change, so (b) (a retryable refusal
plus contract text) was not needed.

Collections binds inside `BEGIN IMMEDIATE`, which takes the write lock on every attached file,
`main` included. So once the writer's catalogue transaction holds `main`'s write lock, no
out-of-thread reference write can start, and none can still be uncommitted. (SQLite commits every
file's frames before it releases any lock.) The writer therefore:

1. reads the `object_ref` AUTOINCREMENT mark (`sqlite_sequence`) before staging; any insert
   raises it, even after a deletion;
2. stages as before; the test seam pauses here;
3. opens the catalogue transaction and takes `main`'s write lock with a value-preserving
   `UPDATE revision SET catalog_rev=catalog_rev`, then re-reads the mark (`reference_fence`);
4. if the mark moved, rolls back the empty transaction and deletes this attempt's
   **unwitnessed** intents (`publication_token IS NOT` the witness; a witnessed row is never
   touched). It then recomputes `batchable` and stages again over every reference. Otherwise it
   publishes as before.

Guarantees kept:

- **Single publication.** The retry happens before any catalogue write, so there is still one
  catalogue commit per batch.
- **Revision fences.** The lock statement leaves `catalog_rev` unchanged and the savepoint still
  adds 1. A rolled-back attempt changes no revision.
- **Crash safety (#173).** Intents still commit before the witness. A crash at any point of a
  retry leaves either no witness, so recovery discards the intents, or the witness of the final
  attempt, so recovery confirms them.
- **FULL sync.** `full` is now sticky across attempts, so `main` is FULL whenever any attempt
  staged intents, and NORMAL is always restored.
- **Idempotency by `request_id`.** Unchanged; it lives at service ingress, and collections still
  binds and adds in one transaction.
- **Failure handling.** A failure to take the lock or read the mark fails that command
  unpublished (its intents are discarded by reconciliation), as a savepoint failure did before.
  Only a failure to open the transaction still closes the writer.

## Tests

| Test | What it pins |
|---|---|
| `writer::publication_tests::a_reference_bound_inside_the_publication_window_is_restaged_before_publication` | Live publication, run twice: once with the window binding committed before release, once with it still open, so the writer meets the fence while the binding holds both write locks. Ref 5 ends `(resolved, row 2)` and ref 6 `(unresolved, retired)`; membership follows the hard link. |
| `writer::publication_tests::crash_after_a_window_restage_recovers_like_any_publication` | Crash injection extended to the retry path. `BeforeCatalog` (moved after the fence, so after the re-stage): durable intents `[(3,None),(4,None),(5,Some(2)),(6,None)]`, which recovery discards; refs 5 and 6 keep their still-current rows; the slice recovers. `AfterCatalog`: recovery confirms the re-staged token, giving ref 5 on row 2 and ref 6 retired. |
| `collections_tests::collections_add_inside_the_publication_window_follows_the_publication` | Real handler. Bindings are `[(1,resolved,false,2),(2,unresolved,true,3)]`; the members page shows row 1's member resolved as `report-001.txt`. |
| `learning_tests::feedback_inside_the_publication_window_queues_behind_the_publication` | Real handler. Feedback queues behind the publication; nothing is bound in the window; the result is `NotFound` with zero references and labels. |

Existing crash, recovery, sync and batching tests in `publication_tests.rs` pass unchanged.
`work()` is now a `cfg(test)` wrapper over `serve()`, so their call sites did not change.

## Mutation results

Each guard was reverted, the four tests were run, and the original file was restored. The
restore was byte-checked with `cmp`.

| Mutation | Result |
|---|---|
| M1: the fence always passes | 3 FAIL: both catalogue tests and the collections test. Feedback passes, as designed. |
| M2: the fence reads the mark without taking `main`'s write lock | 1 FAIL: the catalogue late-commit variant, which caught it in 5 of 5 repeated runs. The variant sleeps 200 ms so that the writer reaches the fence first; the assertions hold in either order, but only the waiting order catches M2. |
| M3: the retry keeps the stale intents | 2 FAIL: both catalogue tests. The re-stage collides, so the listing fails. The collections test passes because no intents were staged before its window. |
| Pre-fix (seam only, no fence) | Collections and both catalogue tests FAIL with the bindings quoted above. Feedback passes. |

## Gates (this branch, Windows)

- `cargo fmt --all --check`: PASS.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS. Also PASS without
  `--all-features`, and for `-p loomward-catalog --lib` without the feature, so production builds
  carry no seam.
- `cargo test --workspace`: 841 passed, 0 failed.
- `cargo test -p loomward-catalog --all-features`: 155 passed, 0 failed.
- `npm.cmd --prefix app run build`: PASS.
- `py -3 scripts/test_app.py --live-serve`: ALL PASS.
- `py -3 scripts/verify.py`: the Python, JavaScript, Rust format and Rust lint legs PASS. Its Rust
  unit leg failed one timing assertion in each of two runs, and each test passes in isolation:
  - run 1: `meaning_tests::meaning_work_budget_is_an_honest_prefix`, `DeadlineExceeded` on a
    read that takes about 1 s of its 2 s deadline in isolation (3 of 3 passes);
  - run 2: `publication_tests::reparent_bounds_the_whole_subtree_and_keeps_the_batch_and_end_run_usable`,
    `max_transaction_commands == 4`, which depends on the writer's 250 ms batch window
    (4 of 4 passes).

  Other lanes were compiling and testing on the same host at the time. Neither timed region
  contains the fence: it runs before `transaction_start`.
- `git diff --check`: clean.

## Not verified

- **Other writers to the same `state.db`.** Another process, or future code that binds a reference
  outside the writer thread without taking `main`'s write lock, is not covered. The fence's doc
  comment states this requirement for any future out-of-thread reference writer.
- **Livelock.** A continuous stream of new references, one landing in every window, would keep
  re-staging. Each retry needs a fresh human-rate collections transaction, so this is not
  expected; it is not bounded in code.
- **Interleavings outside the fixtures.** The window was exercised for `DirListing` (service and
  catalogue) and `ListingDone` (catalogue). `EndRun` staging (`retire_pending`) takes the same
  path but has no window test of its own.
- **Real disks.** No real-disk scan, no elevated run and no hosted CI. The proof is local only.
