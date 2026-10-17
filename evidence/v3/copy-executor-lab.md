# C5 copy executor on lab roots: first measurements

Date: 2026-10-17. Host: **KRASPYON**, Windows 11 Home 10.0.26100, NTFS C:, a laptop shared with
about ten parallel agent lanes (numbers below are noisy upper-bound costs, not a benchmark).
Refs #415, LW-040 #46, #506; design: [doc 47](../../docs/47-copy-and-verify.md) sections 3, 4, 5, 7, 8 and 11.
Starting commit: `654a1a3` (origin/main with C1 #471/#511, C2 #503, C3 #474, C4 #508).

**Lab only.** The executor runs for synthetic sessions on registered lab roots. Personal sessions keep
answering `copy_effect_gated`; the C8 gate is not opened (AGENTS.md invariant 1), and the G5 power-cut and
G9 proven-restore runs have not happened. MCP has no copy command. HTTP still refuses personal approvals (G6).

## What runs

- `loomward-windows::effects::exec` (feature `copy-primitives`): `prepare` (fresh, read-only observation and
  the C4 preview), `run`, `cancel` (also abandon), `recover` and `status`. Every effect has its intent
  journaled first in `state.db` (C3), including the new `Planned` record for creation times before metadata
  preservation changes an identity (#506).
- `loomward-service` serves `copy.preview/approve/start/cancel/abandon/status` to synthetic sessions only;
  the session root gate (lab roots only) runs for every source and the destination. Service open runs
  recovery: only C3's decisions execute; nothing resumes by itself.
- `capabilities.effects` stays all false: the contract has no way to say "synthetic only", so serving is
  decided by dataset class.
- docs/47 Q1 is OPEN (agent-hq#461). Default `LOOMWARD_COPY_STAGING_REMOVAL = false`
  (`loomward-copy-staging-removal`): cancel and abandon stop, journal `staging_left`, end `cancelled` and
  report the staging folder's name in `CopyStatus.staging_folder`. With it on, Loomward removes only its own
  journaled staging by identity (children first, then the folder). Both settings are tested.

- Shutting the service down cancels and joins running copies (no worker outlives the service), so a copy
  interrupted by shutdown ends `cancelled` under the Q1 setting. A worker that cannot be spawned settles
  its started operation as cancelled (nothing was created). Neither path has its own test yet.

## Lab suite (`cargo test -p loomward-windows --all-features effects::exec`)

| Case | Result |
|---|---|
| Tree with sizes 0, 1, 512, 4097, 1 MiB + 1, a named stream, read-only, hidden, Unicode, a 200-unit name, a 12-level chain past 260 units, an empty folder | committed; **C1 `read_tree` + `compare` equal** on content and every section 6 class; source identities, signatures and bytes unchanged |
| Multi-item selection with the dated name taken | lands in `Loomward copy <date> (2)`; the existing folder is never written |
| Source edited mid-run | `needs_attention`, `source_changed` on the item, never published, staging kept; abandon then applies Q1 (left and named, or removed) |
| Cancel after staging created, identities, write, verified, each under both Q1 settings | `cancelled`; never published; staging left and named, or removed so the parent is empty |
| Crash (run stops with no further record) at staging created, identities, written, verified, before publish, after publish; then a fresh store | before publish: recovery waits (no resume), abandon cleans; after the rename: recovery re-hashes by ID against the source, commits, **C1 equal**; source unchanged in all six |
| Staged byte flipped before verification | removal on: one redo after identity-bound removal, committed with correct bytes; removal off: `needs_attention`, `verification_mismatch` |
| Preflight: system attribute; destination inside the source | refused at preview, approval impossible, nothing created |
| Source changed between approval and start | `source_changed` at admit (C4) |
| Insufficient space | **seam only**: no small lab volume here (a VHD needs elevation). ERROR_DISK_FULL and ERROR_HANDLE_DISK_FULL map to the resumable `insufficient_space` |
| Attach (#506) | by journaled file ID with the original or planned creation time only; a different object under the journaled name is refused; publish refuses the staging name or a case-only variant |

Service level (`cargo test -p loomward-service copy_tests`): preview, approve over HTTP and start on a
scanned lab root, committed with names and bytes equal (C1 agreement is the windows suite above), status survives a restart; start without approval, a reused
approval, a wrong digest, and a source changed after approval are refused; abandon of a `blocked`
(`source_in_use`) run applies both Q1 settings. Personal sessions: every copy command gated on both
adapters, the HTTP approval refused as `copy_personal_approval_http`.

## New measurements

- **H2 for named streams is negative.** A held named-stream handle (`FILE_SHARE_READ | FILE_SHARE_DELETE`)
  blocks renaming its ancestor folder with error 5; default-stream holds do not (as C2 measured). The
  executor therefore keeps default-stream holds through publish, releases stream holds just before the
  rename and re-hashes every named stream through the held file right after (doc 47 section 4, step 2).
  Pinned by `h2_named_stream_hold_blocks_publish_default_hold_does_not`.
- The Win32 relative rename returned **87** (not only 123) in the service lab run for the final name `a`;
  the native no-replace rename succeeded. C2's fallback now covers both codes. Why 87 rather than 123 was
  not isolated (a direct short-name test does not reproduce it).
- Paths past 260 units need the verbatim (`\\?\`) form, so the service canonicalizes granted roots. A path
  is never trusted for that: the source root opened for observation must equal the source grant's
  verified (volume serial, file ID), and the pinned destination's ancestry must contain the destination
  grant's verified root, checked at preview and again at run before staging (`destination_outside_grant`,
  `refused`). A junction swapped between the grant check and canonicalization is therefore refused before
  any write. Tested by constructing the mismatch, not by a live junction race.
- Review round 1 (#522) also added: the full named-stream list and a zero EA size are re-checked on every
  held file after the rename (a hold denies neither adding a stream nor FILE_WRITE_EA), with the re-hash
  handles kept until `committed`; a staging-create failure ends `refused` with its cause; the post-rename
  confirmation tells an unverifiable read (retried at the next open; abandon settles it as
  `needs_attention` with the cause, never touching the published copy) from a real change
  (`changed_after_verification`); and intended-but-unjournaled staging tokens are reported in
  `CopyStatus.leftover_staging_folders`. EA addition after verification is covered by code, not a test
  (writing an EA needs `NtSetEaFile`).

## Follow-ups from #526 (2026-10-17, KRASPYON, Windows 11 Home, NTFS temp volume)

- **H7 is positive on this host.** A child process maps a staged file writable (`PAGE_READWRITE`,
  `FILE_MAP_WRITE`), closes its file and section handles and keeps only the view; the executor's
  deny-write verification open then fails with error 32, and succeeds again once the child exits.
  Pinned by `effects::tests::h7_writable_mapped_view_blocks_the_deny_write_hold`. ReFS not measured.
- **Revocation fence and admission lock (doc 47 section 5).** `exec::fence` commits a `fenced` record at
  a new epoch, raises the run's in-memory fence, and waits up to 30 s for the run's admission lock.
  Every effect step (staging create, each item create, preallocation and each 1 MiB chunk write with
  its flush, each stream create, each metadata set, the rename) takes that lock and reads the fence
  first. Before the rename the run cancels (a journaled publish intent is withdrawn); after it the run
  confirms and commits, reported with `grant_revoked`. `roots.revoke` and `grants.revoke` fence every
  copy whose plan names the grant, join a running worker, and cancel a waiting one (blocked or
  interrupted; `needs_attention` stays for the owner). Tests: `a_revocation_before_the_rename_cancels_the_run`
  (at staging, after an identity batch, after the publish intent), `a_revocation_after_the_rename_commits_annotated`,
  `a_fence_waits_for_the_in_flight_step_only`, service `revoking_the_root_cancels_a_blocked_copy`.
- **Publish retry honours cancel.** The 10 x 3 s retry wait ends within 20 ms of a cancel or shutdown;
  the run waits `blocked` (`publish_in_use`) with its intent kept (`cancel_ends_the_publish_retry_wait_at_once`).
- **Folders are checked for named streams and EAs** at the pre-publish tree check, after the rename and
  in recovery's confirmation (`a_stream_planted_on_a_staged_folder_is_never_committed`; EA addition is
  covered by code, not a test).

## Throughput (release build, `throughput_lab`, two runs)

| Tree | Run 1 | Run 2 |
|---|---|---|
| 64 files x 4 MiB (256 MiB) | 1.12 s, 229.5 MiB/s, 57 files/s | 1.17 s, 218.8 MiB/s, 55 files/s |
| 2,000 files x 4 KiB (7.8 MiB) | 9.84 s, 203 files/s | 10.46 s, 191 files/s |

Each file pays a pinned deny-write source open, a create, an identity commit share, a flush, a metadata
set and flush, and a non-cached read-back; small files are dominated by those per-file costs and the
`state.db` FULL commits (four per batch of 256). Source bytes were freshly written, so source reads were
likely served from cache; the destination read-back was non-cached. Disk model, cache state and other
lanes' load were not controlled.

## NOT verified

- No resume: `blocked`, `needs_attention` and interrupted runs exit only by Abandon in C5.
- Emergency stop has no command yet (the fence takes `FenceCause::EmergencyStop`, unused). The revocation
  acknowledgement (position, `pending_in_flight_step`) is internal: `roots.revoke` does not report it.
  A step stuck past 30 s leaves the revocation unacknowledged and the worker unjoined. A fence landing
  mid create batch leaves that batch's created items unjournaled (rule 6 refuses their identity commit):
  reported as staging content, never adopted. Cleanup outside the run does not take the admission lock.
- Between the last post-publish check and `committed`, attributes and new named streams can still be
  added (a hold denies neither); only content is held.
- Process-kill crashes (crash points here stop the run in-process; the OS cache survives), the C6 fault and
  crash matrix, G5 power cuts and G9 restore.
- `publish_in_use` retries (10 x 3 s), `staging_in_use`, the 30-minute expiry over the wire, a real small
  volume for `insufficient_space`, ReFS, other hosts.
- Refusals carry `node_id: null`: an item refusal names a manifest entry, not a catalogue node.
