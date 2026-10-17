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
- Paths past 260 units need the verbatim (`\\?\`) form; the service canonicalizes granted roots and binds
  the observed root to the grant's verified identity before anything else.

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
- In-run revocation and emergency stop: no admission lock or fence wiring yet; a fence record makes the
  executor's next journal append fail (stale epoch), but effects between appends are not fenced.
- Process-kill crashes (crash points here stop the run in-process; the OS cache survives), the C6 fault and
  crash matrix, G5 power cuts and G9 restore.
- `publish_in_use` retries (10 x 3 s), `staging_in_use`, the 30-minute expiry over the wire, a real small
  volume for `insufficient_space`, ReFS, other hosts.
- Refusals carry `node_id: null`: an item refusal names a manifest entry, not a catalogue node.
