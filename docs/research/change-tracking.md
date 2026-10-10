# Non-elevated change tracking: LW-007 / LW-008 (L16a)

Measured on 2026-10-10, Windows 11 Pro 10.0.26300, x64 MSVC, Rust 1.97.1,
windows-sys 0.61.2. Starting source: `dcbd1c2950f32c158721a14a28a5edab68c3f36a`.
This is a disposable native spike, not implementation of either backlog issue.
The executable checks `TokenElevation` and refuses an elevated token. This establishes a
non-elevated execution, not whether the account belongs to Administrators or how a different
account's ACLs behave. No journal was created, resized or deleted; no reboot or elevation occurred.

## Verdict

**Unprivileged USN catch-up works on both G: and E: through directory handles.** Both volumes
are fixed NTFS. In the measured V3 records, file and parent IDs and reason flags survive, but
filenames do not: zero named records out of 60,000 on each volume. Reading through a lab
directory handle also returned sibling-lab records; a directory handle does not confine the
journal to that directory. Native grant scope and catalogue-ID filtering remain mandatory.
Whether records for ACL-inaccessible objects are filtered was **not tested**; do not claim that
this interface limits disclosure to files the user can enumerate.

**Use a 64 KiB recursive watcher per authorised root and reconciliation as the correctness
mechanism.** Immediate draining lost no name records at any measured buffer size. Deliberately
stalling until the burst finished overflowed 4 KiB from 1k operations, 64 KiB from 10k, and 1 MiB
at 100k. All observed overflows were successful zero-byte completions; error 1022 remains a
documented alternative and is covered by the spike's classifier test. A 1 MiB buffer postpones
overflow but cannot replace root invalidation.

## Reproduce and scope

From the worktree root:

```powershell
cargo run --release -p loomward-lab --example change_tracking -- --usn
cargo run --release -p loomward-lab --example change_tracking -- --run evidence/v3/change-tracking.json
cargo run --release -p loomward-lab --example change_tracking -- --catch-up evidence/v3/change-tracking.json
cargo test -p loomward-lab --example change_tracking
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

`--usn` opens only G:/E: volume and root-directory handles and reads at the current journal head.
`--run` creates private OS tempdirs (on the NTFS temp volume on this host), writes the existing
lab marker, watches recursively, mutates only synthetic files, and removes each owned tree.
`--catch-up` appends USN results to the existing JSON, creating marked `watch-usn-*` tempdirs
under `G:\loomward-lab\scale` and `E:\loomward-lab\scale`. Those parents must already exist.
It pins checked, non-reparse/non-offline lab ancestors, requires 20 GB available, rejects an
escaped tempdir, and denies deletion sharing on the lab root while running. Write sharing on
that root permits the explicitly authorised child renames. A sibling marked control has ten
synthetic files. All successful runs removed their own trees; existing scale fixtures were untouched.
Only aggregate counts are retained for non-target records; no unrelated names, IDs or paths
enter the evidence. The only named paths here are the authorised lab parents.

The watcher uses one overlapped request and an auto-reset event. It waits for arming before
mutating. Filters are `FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME`; content,
size, attributes and repeated-write notifications are outside this experiment. Each 1k/10k/100k
case means **mutation operations**, not records: floor(N/3) files are created, renamed and deleted,
with remainder creates. Renames have old/new name records, giving 1,333 / 13,333 / 133,333 expected
records. One file remains after each case. Bulk rename seeds 10,000 files before arming, then
renames their parent once. Setup and cleanup are excluded from timings.

Two consumer modes run once each: continuously drain, or stop consuming after arming until
generation ends. The latter forces queue pressure; it is not a claim about real burst rates.
Names are short fixed-length synthetic labels, which affect capacity. Latency is elapsed time
from immediately before a mutation syscall to processing its matching name record, including
the syscall, scheduling, and probe overhead. It is not the kernel's notification timestamp.
Matched-record percentiles after overflow say nothing about missing records. `first_overflow_ms`
is **detection time since watcher startup**, not the instant the kernel buffer filled.
Watcher CPU uses `GetThreadTimes`; total process CPU includes generation, timestamp maps and
decoding. CPU accounting is quantised (15.625 ms observed here), so a zero is below measurement
resolution, not proof of no cost. Wall time includes a 300 ms idle drain. This was not an isolated
host benchmark: estate workloads continued, and exploratory USN work overlapped some watcher
trials. Raw process CPU, wall time, byte counts, batches and latencies are in the JSON.

## Watcher results

All continuously drained cases matched all expected records, with no duplicates or overflow.

| Buffer KiB | Operations | Matched / expected | p50 / p95 / max ms | Watcher CPU s | Largest batch |
|---|---:|---:|---:|---:|---:|
| 4 | 1,000 | 1,333 / 1,333 | 0.298 / 0.833 / 1.315 | 0.000 | 2 |
| 4 | 10,000 | 13,333 / 13,333 | 0.304 / 0.827 / 73.044 | 0.109 | 10 |
| 4 | 100,000 | 133,333 / 133,333 | 0.315 / 0.796 / 84.620 | 1.563 | 10 |
| 64 | 1,000 | 1,333 / 1,333 | 0.503 / 6.283 / 12.275 | 0.000 | 2 |
| 64 | 10,000 | 13,333 / 13,333 | 0.436 / 0.979 / 8.245 | 0.203 | 6 |
| 64 | 100,000 | 133,333 / 133,333 | 0.312 / 0.960 / 135.741 | 1.313 | 20 |
| 1,024 | 1,000 | 1,333 / 1,333 | 0.309 / 0.786 / 2.797 | 0.031 | 10 |
| 1,024 | 10,000 | 13,333 / 13,333 | 0.323 / 0.883 / 4.504 | 0.078 | 9 |
| 1,024 | 100,000 | 133,333 / 133,333 | 0.318 / 0.942 / 76.380 | 1.781 | 16 |

For consumers stalled to burst end:

| Buffer KiB | 1k operations | 10k operations | 100k operations |
|---|---|---|---|
| 4 | 1 matched, one zero-byte overflow | 1 matched, one zero-byte overflow | 1 matched, one zero-byte overflow |
| 64 | all 1,333 matched | 1 matched, one zero-byte overflow | 1 matched, one zero-byte overflow |
| 1,024 | all 1,333 matched | all 13,333 matched | 1 matched, one zero-byte overflow |

The first outstanding request retained one name hint; later history was discarded on the
overflowing trials. This proves that earlier successful notifications cannot make a later
zero-byte completion safe to ignore. Overflow thresholds between these grid points were not
measured, and no `ERROR_NOTIFY_ENUM_DIR` occurred on this host.

Bulk rename of a directory containing 10k children yielded **two records**, old and new parent
name, at every buffer size: 25.718 / 6.804 / 9.348 ms for 4 / 64 / 1,024 KiB. All 10k children
remained. There was batching, not loss or coalescing of the measured file-name records. The bulk
rename did not emit per-child path changes. Repeated data writes may coalesce; this name-only
experiment does not test that separate filter.

## USN handles and errors

Both query and unprivileged read were attempted separately. On a failed query the read used
zero journal ID/cursor and is not a valid catch-up request; that diagnostic is recorded rather
than treated as a working path. Successful query/read combinations used the actual journal ID,
`StartUsn = NextUsn`, versions 2..3, and zero timeout/bytes-to-wait (nonblocking).

| Handle (same results on G: and E:) | Open | Query | Unprivileged read |
|---|---|---|---|
| `\\.\X:`, desired access 0 | success | 1 (`ERROR_INVALID_FUNCTION`) | 1 (`ERROR_INVALID_FUNCTION`) |
| `\\.\X:`, `GENERIC_READ` | 5 (`ERROR_ACCESS_DENIED`) | not called | not called |
| `X:\`, `FILE_LIST_DIRECTORY`, backup semantics | success | success, 64 bytes | success, 8-byte cursor at head |

Both existing journals advertised supported versions 2..4 and a 32 MiB target maximum size.
The lab requested 2..3; **all actual records returned were V3**, including on NTFS. Their 128-bit
ID high halves were zero on this host. Do not infer NTFS means V2, or generalise this lab's
64-bit ID restriction into the engine.

Release-build catch-up after 10k creates + 10k renames + 10k deletes, 64 KiB read buffers:

| Volume | Filter | Lab records | Calls | Elapsed ms | Records/s | CPU s |
|---|---|---:|---:|---:|---:|---:|
| G: | all reasons | 60,000 | 74 | 32.697 | 1,835,031 | 0.03125 |
| G: | rename reasons `0x3000` | 30,000 | 37 | 10.502 | 2,856,708 | < resolution |
| G: | close-only `0x80000000` | 30,000 | 37 | 8.392 | 3,575,046 | 0.015625 |
| E: | all reasons | 60,000 | 74 | 65.254 | 919,484 | 0.03125 |
| E: | rename reasons `0x3000` | 30,000 | 37 | 27.919 | 1,074,552 | 0.015625 |
| E: | close-only `0x80000000` | 30,000 | 37 | 23.557 | 1,273,518 | 0.015625 |

Throughput includes IOCTL calls, checked V2/V3 decoding, file-ID deduplication and aggregation;
it excludes generation and any catalogue database work. It is a warm, single trial, not a
sustained engine-throughput guarantee. Both volumes returned 10k distinct lab file IDs, no
filenames, and these reason values (10k records each): `0x100`, `0x1000`, `0x2000`, `0x80000100`,
`0x80000200`, `0x80002000`. The rename mask also matches close records containing the new-name
reason, hence 30k rather than 20k. Close-only aggregates reasons; neither filter is a complete
sequence of operations. `Timeout`, `BytesToWaitFor`, cursor/journal ID, reason mask, close-only,
and supported-major-version bounds are available in `READ_USN_JOURNAL_DATA_V1` [S2]. The defined
V3 layout also has timestamp, source/security IDs and attributes; their usefulness under this
unprivileged API was not measured. File/parent IDs, reasons, versions, name length and returned
cursor were decoded and observed.

The all-reasons read through the lab directory returned 25 non-target records on each volume
(including the sibling control creation). Close-only returned 12. These are aggregate counts
only; no names or IDs were exported. **Do not use the handle path as an authorisation filter.**
ACL-inaccessible-object visibility remains unknown; no ACLs or access grants were changed.

| Negative read control | G: | E: | Interpretation |
|---|---:|---:|---|
| `StartUsn = FirstUsn - 1` | 1181 | 1181 | `ERROR_JOURNAL_ENTRY_DELETED`: required history is unavailable |
| journal ID XOR 1 | 87 | 87 | `ERROR_INVALID_PARAMETER`: wrong ID is not necessarily error 1179 |
| requested versions 99..99 | 87 | 87 | unsupported format request |

These controls changed only read parameters. Existing journal identity was unchanged across
generation, and reopening a directory handle successfully read from the saved cursor. The
first fixture attempt denied write sharing on the parent and child rename failed with 32
(`ERROR_SHARING_VIOLATION`); the root pin now permits write sharing while still denying deletion.
Exploratory debug throughput is excluded from the table. Building over the actively running
release executable caused one `LNK1104`; the final build ran after it exited.

## Reboot, wrapping and lane recommendations

Directory notifications require a live watcher; their queued history does not survive process
exit or reboot [S3]. The NTFS journal is persistent, and cleanup logging can occur on remount
after an intervening restart [S2]. That permits catch-up in principle, not guaranteed retention:
old records are trimmed, and a recreated or restamped journal invalidates the old identity.
The oldest available record is `FirstUsn`; `LowestValidUsn` marks validity for the current journal
instance; check both [S4,S5]. The old general journal guide says administrator privileges are
required [S5]; the successful directory-handle unprivileged measurements here establish the
specific capability on this Windows build, not universal compatibility. No actual reboot,
forced wrap, journal recreation, or interrupted shutdown was performed. The below-`FirstUsn`
control observes the real gap error without modifying the journal.

For **L7**, store native volume/object identity with ID width, per-root completeness, and dirty
epochs. A relist clears only the dirty epoch it covered; a notification during that listing
keeps the directory dirty. Publish absence only from complete listings. Incomplete/denied/
cancelled listings preserve old rows and visible stale state. A moved directory is matched by
identity, reparented and its descendants' paths/depths updated without requiring per-child hints;
both parents and ancestor aggregates need reconciliation. Keep the root generation stale until
all relevant dirty listings and the repair rollup complete, as doc 41 section 7 requires.

For **L16**, arm the recursive watcher before scanning. Start at 64 KiB, rearm promptly and hand
bounded dirty-directory work to the writer; parsing or a slow database must not block draining.
Coalesce directory invalidations, not an assumed authoritative event history. If the dirty queue
fills, a record is malformed, either overflow form occurs, or a rename pair/parent cannot be
resolved, conservatively dirty the whole granted root. A renamed subtree needs identity-based
reconciliation; a missing pair stays a tombstone until complete listings resolve it. ReadDirectoryChangesW
does not report changes to the watched directory itself [S1], so validate root identity/grant
on restart and handle watcher failure/root disappearance as loss of coverage. The first request
fixes buffer capacity for that handle's lifetime; resizing needs a new handle. Network buffers
over 64 KiB are rejected [S1]. A larger local buffer is a later measured tuning choice, not part
of the required correctness design.

For **LW-007**, treat unprivileged USN as an optional dirty-ID accelerator. Establish a valid
bookmark before the baseline scan, persist `(native volume identity, journal ID, next USN)`
alongside the catalogue state it covers, and replay up to a captured head before declaring
catch-up complete. Never advance the durable cursor before publishing the corresponding dirty
state. Requery to detect truncation or ID changes during catch-up. Redacted names mean new,
deleted and renamed objects still require parent-directory relists; unknown IDs/parents require
a conservative root relist. Preserve 128-bit IDs for V3 and reject unsupported/malformed formats.
The spike's high-half-zero restriction is deliberately lab-only.

On startup/gap/error, show stale/repairing coverage, validate the active grant, re-establish the
watcher, and **fully relist each affected authorised root**, then capture a fresh bookmark. This
is not permission for a whole-volume scan or a repeated full-disk polling loop. Missing privileges,
unsupported filesystems/formats, errors 87/1181, journal-not-active/delete-in-progress, or a cursor
outside the current valid range all take that fallback. Root grant loss takes no-scan/stale state.
Catch-up is change metadata, never read-usage evidence, undo content, or an operation grant.

## Proof and outstanding boundaries

The original three spike tests cover both overflow forms and notification/USN decoder rejection.
Four fix-round regressions additionally cover pending-read cancellation and completion on failed
arming, shutdown/join before cleanup after an arming timeout, pinned-tree cleanup, and V2/V3
nonempty-name offsets outside the header on UTF-16 boundaries. Every submitted watcher request
is guarded until completion, including early returns; catch-up and control roots remain pinned
during content cleanup, then their empty roots are removed non-recursively after releasing the
pins. This is a root-pinning mitigation, not full identity-bound cleanup. Measurements were not
rerun for the fix round; the recorded evidence remains the original measurement.
Fix-round checks on Windows (2026-10-10): `cargo fmt --all --check`, `cargo test --workspace`
(107 passed), `cargo test -p loomward-lab --example change_tracking` (7 passed), and
`cargo clippy --workspace --all-targets -- -D warnings` all passed.
The original workspace format, test and clippy checks are recorded in the JSON validation receipt.
All 21 watcher trials and both volume catch-up trials completed and cleaned their disposable trees.
Production dirty epochs, database publication, restart state machine and engine integration are
not implemented here; LW-007 and LW-008 remain open. No UI/Python behavior changed. No commit,
push, release or journal mutation occurred. `HUMAN_TODO.md` is unchanged; q-5 (real-metadata
teacher scope) remains open and q-6 (firewall rule) is not yet needed, unrelated to this spike.

## Primary sources (accessed 2026-10-10)

- [S1: ReadDirectoryChangesW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-readdirectorychangesw): both overflow forms, root-self exclusion, buffer lifetime and network limit.
- [S2: READ_USN_JOURNAL_DATA_V1](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-read_usn_journal_data_v1): cursor, filters, formats and remount/close semantics.
- [S3: Change Journals](https://learn.microsoft.com/en-us/windows/win32/fileio/change-journals): live notifications versus journal recovery.
- [S4: USN_JOURNAL_DATA_V1](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_journal_data_v1): identity, valid ranges and retention size.
- [S5: Using the Change Journal Identifier](https://learn.microsoft.com/en-us/windows/win32/fileio/using-the-change-journal-identifier): recreation/restamping and identity integrity.
- [S6: Change Journal Records](https://learn.microsoft.com/en-us/windows/win32/fileio/change-journal-records): trimming, coalesced reasons and lack of undo information.
- [S7: USN_RECORD_V2](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v2): record-version dispatch and rename parent semantics; V3 is linked there.
- [S8: System error codes 1000–1299](https://learn.microsoft.com/en-us/windows/win32/debug/system-error-codes--1000-1299-): 1022, 1178, 1179 and 1181.
- Windows SDK `winioctl.h` 10.0.22000.0 declares `FSCTL_READ_UNPRIVILEGED_USN_JOURNAL` as `CTL_CODE(FILE_DEVICE_FILE_SYSTEM, 234, METHOD_NEITHER, FILE_ANY_ACCESS)`; the spike uses the windows-sys 0.61 constant. A dedicated Microsoft Learn page for this IOCTL was unavailable, so unprivileged behavior above is measured evidence, not an invented documentation guarantee.

## Evidence redaction and known measurement limits

The committed JSON carries no journal identity and no absolute USN positions. Journal IDs became
`id_present`. Cursor positions became spans: `retained_span_usn` and `catch_up_span_usn`. The
`--run` mode no longer probes volume journals; that probe is only `--usn`, which you invoke
explicitly. Catch-up reads stop at the end of the returned buffers, not at the captured target
USN. Records appended by other processes after the target can therefore inflate
`outside_lab_records_count_only` and the byte and call totals slightly. Lab-record counts are
exact, because they are matched by file ID.
