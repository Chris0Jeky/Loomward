# L16 change tracking receipt

Measured 2026-10-10 on Windows, non-elevated native fixture runs, in
the `lw-l16` worktree, branch `feat/l16-change-tracking`, starting HEAD
`06750477fafb431afd0330fc721bb0bb2fb8c265`. The implementation was handed to the driver as a working-tree
change. The lane made no commit, push, release, elevation, journal mutation or real-root scan.

## Changed

- One recursive 64 KiB `ReadDirectoryChangesW` watcher per granted root; file-name,
  directory-name, size and last-write filters. The first request is armed before engine
  enumeration starts. The enumeration module's component-wise opens are reused, with
  asynchronous I/O only on the final component, no-recall flags, reparse/offline refusal,
  elevation refusal, expected identity checking and retained ancestor/root pins.
- Every pending request owns an RAII cancellation/completion guard. Cancellation waits for
  completion before releasing its buffer, OVERLAPPED, event, file or ancestor pins, including
  early exits. The next request is armed before delivering the preceding batch.
- Native callbacks feed a bounded 16-batch queue; a separate dispatcher does sink writes.
  Dirty parent directories coalesce into epochs, capped at 1,024 paths / 512 KiB of UTF-16
  names. Queue loss, malformed notifications, either overflow form, unresolved parents or
  unmatched renames require full-root reconciliation. Old/new rename parents both become dirty.
- Baseline and relists keep coverage repairing. An epoch is cleared only when a successful
  listing covers its captured value; notifications during that listing survive. Reconciliation
  uses targeted scopes and the existing N2 upserts-only behavior outside them. Reparenting
  discards obsolete in-flight tickets and forces a conservative full relist. A second drain
  catches changes during final publication before the job reports complete.
- MemorySink retains absence tombstones outside targeted scopes and repairs ancestor sums
  bottom-up after complete scoped reconciliation. Durable adapters must implement
  `watch_dirty` (default refuses watcher activation) and should implement
  `resolve_watch_directory` (unknown defaults to full relisting). Coverage still depends on
  each adapter's existing `finish_run` repair/publication contract.
- Watches survive between scans, mark idle changes stale, stop/join on grant revocation and
  engine teardown, and are re-established before a subsequent scan if coverage has failed.
  Three reconciliation passes and the original aggregate entry budget bound a run; unsettled
  roots remain partial/repairing rather than looping indefinitely.

## Verified

All requested gates passed on the final implementation:

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | PASS |
| `cargo test --workspace` | 250 passed, 0 failed, 0 ignored |
| `cargo test -p loomward-windows --all-features -- --nocapture` | 34 passed, 0 failed, 0 ignored |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `git diff --check` | PASS |

The native tests use canonical OS-temp roots. On elevated hosts they assert native refusal
instead of claiming that their fixture watch ran. This host executed the native cases.
The burst oracle compares native directory IDs, exact UTF-16 names, file IDs, attributes,
logical sizes and final logical/allocated/count sums against an independent fresh scan.
It covers 200 create/rename operations with 100 deletes, an existing file deletion and size
change, both with and without a moved subtree. It also injects a file creation during final
publication and checks idle invalidation afterward.

Eight temporary rule-removal checks each produced an actual failing test (exit 101), not
an import/compilation failure; every temporary edit was restored:

| Rule removed | Discriminating test |
| --- | --- |
| ERROR_NOTIFY_ENUM_DIR classification | `both_overflows_discard_history` |
| Successful zero-byte overflow classification | `both_overflows_discard_history` |
| Early-exit I/O cancellation/completion | `pending_read_early_exit_drains_before_storage_is_reused` |
| Dirty epoch retention | `overflow_and_rename_parents_have_epochs` |
| Delivery of mid-scan hints | `armed_before_scan_burst_reconciles_names_moves_and_sums_like_a_fresh_scan` |
| Native stop on revocation | `revocation_stops_native_io_and_late_hints_cannot_reactivate_it` |
| Ancestor repair rollup | `repair_rollup_preserves_tombstones_outside_targeted_scope` |
| Post-publication drain | `armed_before_scan_burst_reconciles_names_moves_and_sums_like_a_fresh_scan` |

### Fixture measurements

Final all-features run, 64 KiB buffer:

- Continuously drained: 50 individual creates, no root invalidations; latency
  p50 / p95 / max **0.317 / 0.470 / 0.742 ms**. Latency spans the mutation syscall through
  callback delivery, not the kernel notification timestamp; one warm, shared-host trial.
- Deliberately stalled consumer: 10,000 creates, **root invalidation detected**, only **2 name
  hints before invalidation**, detected **1,769.935 ms from burst start**. This includes generation
  and the deliberate stall; it is neither the time the kernel buffer filled nor an overflow
  threshold estimate. The public hint API coalesces overflow forms; this trial did not log which
  native completion form occurred. Both forms are covered by classifier and engine injection tests.
- Burst reconciliation equals the fresh-scan oracle in both subtree variants. Revocation tests
  verify no later hints, released pins (the root can be renamed), and a stopped native channel.

## LW-007 hypothesis

The earlier [change-tracking research](../../docs/research/change-tracking.md) measured
non-elevated USN catch-up through directory handles on two NTFS volumes. Names were redacted,
128-bit V3 identity width matters, and records escaped the directory subtree; ACL-inaccessible
record visibility remains unknown. This is measured compatibility on that host, not universal
support or subtree confinement. No USN accelerator is implemented or enabled by L16. A future
explicit opt-in must filter catalogue IDs, retain journal/volume identity, stop at the captured
target USN, publish dirtiness before advancing its durable cursor, and fully reconcile on gaps
or unsupported formats. It must not create, resize or delete a journal.

The watcher completion/overflow rules follow Microsoft's
[ReadDirectoryChangesW contract](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-readdirectorychangesw).

## Files changed and recommended commits

1. `crates/loomward-windows/src/watch.rs`
2. `crates/loomward-windows/src/enumerate/mod.rs`
3. `crates/loomward-windows/src/enumerate/native.rs`
4. `crates/loomward-engine/src/scan/watch.rs`
5. `crates/loomward-engine/src/scan/mod.rs`
6. `crates/loomward-engine/src/scan/sink.rs`
7. `crates/loomward-engine/src/lib.rs`
8. `evidence/v3/l16-change-tracking.md` (this receipt)

Recommended commit messages, if the driver splits the native and engine seams:

- `Add guarded recursive Windows change watchers`
- `Reconcile watched scan roots with dirty epochs`

Raw local gate logs are gitignored under `.loomward/l16-proof/`; they are not publication
artifacts. `HUMAN_TODO.md` is unchanged; its remaining teacher decisions (q-5 and q-8) do not
block this fixture-only observer lane.

## NOT verified / residual risk

No durable catalogue/service integration, hosted CI, real personal roots, sustained scale,
reboot recovery experiment, USN production catch-up, or elevated execution was proved here.
Targeted L7 runs still traverse the root and publish only upserts outside their scope; this
preserves correctness but is not a measured targeted-I/O optimization. Each scan currently starts
with a conservative full baseline. Under continuous churn a bounded run stays partial and needs
another explicit refresh. Ancestor/root pins deny deletion sharing during the grant's watcher
lifetime; revocation/teardown releases them. Fixtures establish the local native boundary, not
instantaneous filesystem snapshots or durable catalogue crash consistency.
