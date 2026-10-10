# Scan pipeline follow-ups — #180

Windows, 10 October 2026. Measured before commit on `fix/scan-followups-180`, based on
`befd890614f2fae864a1acabfed657792ec1445d`. Commands ran from the worktree's repository root.
Source hashes, gate results and every mutation's command and failing test are in
[scan-followups-180.json](bench/scan-followups-180.json). These measurements preceded the commit
and push published in PR #262; they describe that measurement pass, not later review fixes.

## Changed

- **F4:** [pipeline.rs:246](../../crates/loomward-engine/src/scan/pipeline.rs#L246) releases listed
  handles when their unique volume/ID can be reopened. The native
  [reopen:484](../../crates/loomward-windows/src/enumerate/native.rs#L484) verifies the root anchor,
  uses `OpenFileById`, and revalidates attributes, ID width and volume. Queued/active handles
  remain charged, including handles reopened during reconciliation. Reopen failure preserves
  partial coverage and prior observations. Outstanding I/O keeps its handle until it returns.
- **F5:** [pipeline.rs:71](../../crates/loomward-engine/src/scan/pipeline.rs#L71) adds
  `duplicate_identity`. Same-listing aliases and sightings under another parent increment it;
  ordinary parent relists do not. A later clean listing cannot erase the run's partial evidence.
- **F6:** [pipeline.rs:1022](../../crates/loomward-engine/src/scan/pipeline.rs#L1022) documents the
  lock order and worker-join prerequisite. [Scheduling:606](../../crates/loomward-engine/src/scan/pipeline.rs#L606)
  queues each name-only slot once per reconciliation round. Latest listing counters replace old
  counters, and [watch reconciliation:426](../../crates/loomward-engine/src/scan/watch.rs#L426)
  likewise reports the latest pass. `enumeration_attempts` retains the entry limit across every
  attempt. Deliberate outside-scope upserts are partial, without being counted as I/O failures.
- **F7:** [semantics §7:150](../../contracts/v3/semantics.md#L150) names both the 1,024-event
  limit and the 60-second age cutoff. Checked against `events.rs`: publication retains at most
  1,024 events; replay and live subscriber eligibility exclude events aged 60 seconds or more.
  The wording matches the code. Event code is unchanged.
- **Further issue items:** the name-only scheduling path uses the freshly opened handle.
  [Panic cleanup:1089](../../crates/loomward-engine/src/scan/pipeline.rs#L1089) terminates a poisoned
  run, stops its writer, finishes incomplete and releases the byte reservation. Poison recovery
  is for failure cleanup only. The lab's report test initializer includes the new fields.

## Handle measurement and arena effect

Before the change, a synthetic temporary directory was opened with read/write/delete sharing.
Windows rename and `RemoveDirectoryW` succeeded, but `FileStandardInfo.DeletePending` remained
true on the retained handle. Recreating the removed name also succeeded while the old handle
was open: name availability alone does not disprove delete-pending object lifetime.

Measured Rust layouts at the fixed **16 MiB arena reservation**:

| Estimate | Before | After |
|---|---:|---:|
| Persistent cost per listed native directory | 344 bytes, including retained handle/task estimate | 344 bytes, including identity and counters |
| Arena upper slot count | 48,770 | 48,770 |
| Extra cost per live native handle/task | Already included above | 56 bytes |

The new all-unlisted upper bound is 41,943 slots before names; actual capacity depends on the
frontier and held names. Releasing native handles **does not restore the historical ~116k estimate**:
the new identity/counter storage offsets the released cost. These are accounting/layout estimates,
not a scale or kernel-memory measurement.

A separate small synthetic oracle makes reclamation observable with deliberately large owned
handles: a 4 MiB byte budget / 1 MiB arena traverses all 95 descendant directories when handles
are released. Mutating away release stops at 14 descendants, partial with a limit. This is not a
native throughput benchmark. Existing 10M evidence is unchanged; no scale runs were performed.

## Verified

| Command | Final result |
|---|---|
| `cargo fmt --all --check` | PASS, exit 0 |
| `cargo test --workspace` | PASS, 513 passed, 0 failed, 0 ignored |
| `cargo test -p loomward-windows -p loomward-engine --all-features` | PASS, 198 passed, 0 failed, 0 ignored |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, exit 0 |
| `git diff --check` | PASS, exit 0 |

Twelve new tests cover handle release, capacity reclamation, reopening after rename, rejected
identities, reopen budgeting, duplicate evidence, relist counters, queue coalescence, fresh
name-only handles, watcher attempt limits and panic cleanup. **17 mutations were killed** by runtime
test failures, covering every new test; compilation failures do not count. Fixed source bytes
were restored after mutations. The panic regression bounds its observation at two seconds so
the old writer deadlock produces an assertion failure. Existing admission, revocation, cancellation
ordering, active-I/O lifetime and watcher tests pass in the full suites.

The first workspace compile identified the lab initializer missing the new report field;
that integration defect was corrected before the final gates. Raw task logs stay ignored under
`target/scan180/`; the linked JSON is the public receipt.

## NOT verified / residual risk

No M or 10M benchmark, hosted CI, cold-cache qualification, physical ReFS or unsupported-filesystem
reopen qualification. No throughput claim. Name-only/non-unique identities and sources without
identity reopen support retain their handles; dirty reopen failures stay partial. Renaming or
replacing the granted root prevents reopening through a stale anchor.

No issue item was declined. Scale benchmarks were skipped under the shared-host constraint.

## PR #262 review follow-up

Windows, 10 October 2026, from `5ce026d04716283bc6b11dacff719055496a2321`.
Cancelled runs aggregate partial observations unless the arena is poisoned. Native handle release
requires a verified identity and an NTFS/ReFS flag from the opened directory's volume query;
FAT, exFAT and unknown filesystems retain handles. A released slot losing its identity stays
partial instead of panicking, and reconciliation stops before the next refresh after cancellation.

Four regression tests failed before their fixes: `cancelled_multidirectory_scan_preserves_partial_totals`,
`handle_release_requires_ntfs_or_refs_and_verified_identity`,
`released_slot_losing_identity_stays_partial_without_panicking`, and
`cancellation_stops_reconciliation_before_the_next_refresh`. All pass after the fixes.
Eight mutations were killed by runtime test failures: skip error aggregation, aggregate a poisoned
arena, release on every filesystem, allow exFAT release, disallow ReFS release, panic on a lost
identity, omit its failure count, and continue reconciliation after cancellation. Fixed source bytes
were restored before the final gates. Local logs and receipts stay ignored under `target/scan262/`.

All required gates passed, exit 0: `cargo fmt --all --check`; `cargo test --workspace`
(576 passed, 0 failed, 1 existing ignored test: `verbatim_device_namespace_state_paths_are_refused`,
tracked in #246); `cargo test -p loomward-windows -p loomward-engine --all-features`
(207 passed, 0 failed, 0 ignored); `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
`git diff --check`. Filesystem names in the new release test are synthetic, not physical FAT/exFAT/ReFS
qualification. No hosted CI, scale benchmark or push was performed for this follow-up. The four
separately tracked LOW findings and `HUMAN_TODO.md` are unchanged.
