# External archive disk and agent storage tooling

Status: **design, proposed 2026-10-25** by the external-disk driver session. It adds no file effect and opens no
gate: AGENTS.md invariants 1 and 7 and the copy-and-verify gate of [47](47-copy-and-verify.md) section 11 stand as
written. It extends [46](46-find-and-ask.md) section 8.2 (the native MCP projection) with the separate grant design
that section asks for, and it reuses copy-and-verify for every write Loomward itself makes.

## 1. What the owner asked for

The owner attached an external disk as backup and archive space and will leave it attached most of the time. They
want Loomward to:

1. archive and move things between disks to bring order to their disk space;
2. let the external disk host components that are not storage-location sensitive;
3. give agents the tooling (MCP, CLI) to store, move and archive files safely, with no risk of corruption or deletion;
4. show clearly what is where and what takes the space;
5. then, and only then, run a supervised cleanup with the owner.

## 2. The disk, and how Loomward names it

Measured 2026-10-25 on the owner's desktop: a WD Elements 8 TB disk on the USB bus, GPT, one NTFS volume labelled
`Elements`, mounted as `H:` with about 5.6 TB free. Windows reports its drive type as **fixed**, as it does for most
USB hard disks, and the disk does not report removable media.

**Today Loomward refuses it.** `volumes::removable_device` treats every USB or SD bus as removable whatever the
drive type (`crates/loomward-windows/src/volumes.rs`), and the path rules refuse removable volumes as roots with
`network_or_removable_unsupported` (`crates/loomward-service/src/paths.rs`); docs/47 refuses them as copy
destinations (`destination_unsupported_volume`). So `H:` can be neither scanned, nor verified, nor copied to.

**Agent decision (reversible, lane E0).** A USB volume becomes an allowed **scan and grant root** when it is fixed
media (no removable-media flag, not SD) and its file system verifies identity (NTFS or ReFS). USB volumes with FAT or
exFAT stay refused as roots, as today (q-12's identity-unverified listing is unchanged for the volumes it covers). Reason: the owner asked Loomward to show and organise this disk, and scanning
it is a read; the identity model below already handles a disk that comes and goes. Reversal: restore the fence.
Copying **to** it stays behind the copy gate: docs/47's `destination_unsupported_volume` is lifted for such volumes
only in lab sessions (synthetic lab roots on the USB disk) until C8 opens.

Rules for a detachable root:

- **Never the drive letter.** Letters move between plugs. A root is its volume GUID and root file ID (the lab and
  grant identity already used); a copy destination is the pinned destination folder (docs/47 section 4). Another
  disk on `H:` tomorrow is `identity_changed` for the root and `destination_changed` for a copy, never a retarget.
- **Bus type, not drive type.** The app and the agent tools show a USB volume as "external, may be detached".
  Unknown bus type shows nothing extra (invariant 4).
- **Detached is offline, not empty.** A detached disk's catalogue rows stay visible and are marked offline; sizes
  are last-seen observations, never zero (docs/05, invariant 4). A scan or read of an offline root is refused, not
  run against whatever volume holds the letter. On reconnect the root is online again only if its identity matches.
- **Detach is a destination power loss.** Unplugging the disk mid-copy cuts power to its cache. The copy journal
  lives in `state.db` on the internal disk, so the executor's rules apply: staging is never adopted by name and a
  published copy is never removed (docs/47 sections 5 and 8). Real unplugs are evidence towards G5, not a substitute
  for the method the owner picks in `loomward-copy-power-cut-method`.

## 3. What already exists (reused, not rebuilt)

| Need | Existing piece |
|---|---|
| Fast inventory of each volume | native scan into the catalogue (`scan.start` on owner-granted roots; whole volumes need the owner's native grant dialog, q-7) |
| Where the space goes | `tree.slice` / `tree.children` sorted by size (Atlas), `stats.breakdown` by extension family, extension or age band, `clutter.developer` (build outputs and caches), duplicate groups (q-10), `volumes.list` capacity and free space |
| Tiers | `volumes.declare_tier`, `tiers.model`, `placement.simulate` (non-executable proposals) |
| A safe write | copy-and-verify (docs/47): handle-based copy into new staging, non-cached read-back, SHA-256 per stream, atomic no-replace publish, journal and recovery, revocation fences. Lab-only today |
| Comparing a copy made elsewhere | the C1 tree verifier, `Engine::compare_trees` (read-only, both roots through the q-10 admission gate, 200,000-entry cap) |
| Per-client disclosure | LW-067 client grants (`mcp:<name>`, roots and fields, revocable, epoch-bound cursors) and the LW-069 projection (`crates/loomward-service/src/clients.rs`). Issued in process only (`Service::grant_client`; Ask's `ask.grant` wraps it for `ask:`). `grants.*` are the teacher's disclosure grants, not these |

## 4. The agent surface

### 4.1 One native MCP server, read-only by construction

`loomward-mcp` is a stdio MCP server binary (new crate `crates/loomward-mcp`, LW-066/F12). It succeeds the frozen
Python reference for live catalogues; the Python reference is untouched.

- **Host.** It opens the personal service in process, as `loomward-serve` does, and calls it with the fixed
  principal `mcp:<name>` (`--client <name>`, set at launch, never by a payload). Every call goes through the
  service's client gate, so the owner's live grant is re-read on every call and a revocation holds from the next one.
- **The state lock.** The state directory has one exclusive owner (docs/41 section 3), shared through one helper.
  The server takes the lock lazily on a tool call and releases it after a short idle period
  (`--idle-release-secs`, default 10). While another process holds it, tools answer `busy`. The desktop, on launch,
  retries the lock for 15 seconds and then says which kind of process holds it ("Loomward's data is in use by an
  agent's Loomward tools; stop that agent or try again"), so an agent calling in a tight loop can always be stopped by
  the owner, and the owner can then revoke its grant. ponytail: a named-pipe endpoint in the desktop is the upgrade if
  agents and the open app must work at the same time.
- **References.** Results name nodes by opaque session references (`r1`, `r2`, ...), never a `NodeId`. A reference
  survives an idle release while the catalogue instance and generation are unchanged, and is re-checked against the
  live grant on use; a rescan, revocation or restart makes it unknown ("call find again").
- **Read-only.** The tool set reaches read commands only. Tests assert `state.db` and `ask.db` are unchanged by every
  tool and by every attempted hidden name (docs/46 F12). There is no copy, approve, start, cancel, scan, grant or
  delete tool, and none may be added without changing invariant 7 (G6 holds).

### 4.2 The grant design docs/46 section 8.2 asked for

Docs/46 limited MCP to `find` and `describe` until "a separate grant design". This is it:

- A client grant keeps its roots and its fields (`path`, `size`, `modified`, `attributes`). Every space tool is
  **scoped to the granted roots**: a node outside them answers as unknown, and totals are computed only from rows
  under them, by the catalogue, before anything is counted (the same rule as search). Nothing reveals that rows
  outside the grant exist.
- A new optional field, `volumes`, discloses the machine's volume list (label, bus type, capacity, free space,
  online state, declared tier). It is off unless the owner ticks it, because it is a machine-wide observation.
- Each output needs the field it discloses. Names and kinds are always part of a row, as in search (LW-069); paths
  and locations need `path`; byte totals need `size` (a grant without
  `size` gets no space tools); age bands and modified ranges need `modified`; attribute-derived warnings
  (placeholders, reparse points) need `attributes`. Without the field, that part is absent, not null.
- The owner issues, lists and revokes MCP client grants through new owner-only commands `clients.grant`,
  `clients.list` and `clients.revoke` (thin wrappers over `grant_client` / `revoke_client`, as `ask.grant` is for Ask),
  on the desktop (Tauri) adapter only, in an "Agent access" panel in the desktop app. An agent never issues its own grant; a shell-capable agent could
  bypass any local gate, so the grant is the owner's consent and scope, not a sandbox.
- What an MCP host sends to its model is outside Loomward (docs/28). The panel says so before a grant with `path`.

### 4.3 Tools

| Tool | Backed by | Needs | Notes |
|---|---|---|---|
| `find` | `search.query` | grant | as docs/46 |
| `describe` | `node.inspect` | grant | as docs/46 |
| `volumes` | `volumes.list` | `volumes` | bus type, capacity, free, tier, online |
| `roots` | `roots.list` | grant | only granted roots; coverage and freshness |
| `largest` | `tree.children` sorted by size | `size` | children of a granted folder, page cap 30 |
| `breakdown` | `stats.breakdown` | `size` (+ `modified` for age bands) | by extension family, extension or age band, 16 groups |
| `clutter` | `clutter.developer` | `size` | build outputs and caches under granted roots |
| `plan_archive` | composition of the reads above | `size`, `volumes` | pure; see 4.4 |

### 4.4 `plan_archive`: a plan, never an action

Input: up to 64 session references (folders or files from `find`/`largest`), a destination volume from `volumes`,
and an optional new folder name. Output, computed from the catalogue only:

- per item and in total: bytes, files, coverage and freshness (stale, partial or offline is said so); with
  `modified`, the last-modified range;
- the destination's free space after the copy manifest's reserve, `max(1 GiB, 1% of the volume)`
  (`loomward_operations::manifest`), and whether the plan fits;
- warnings the catalogue can already see: offline sources, a destination on the same physical disk as a source
  ("not a backup", docs/09), more than 200,000 entries in one item (the verifier's cap: split it); with `attributes`,
  cloud placeholders and reparse points;
- the destination identity (label, bus type, volume key) and the new-folder shape of docs/47 (Q3 defaulted to (a) on
  2026-10-24, no owner answer: a new folder only, never into an existing one);
- `execution`: always `"owner_approval_required"`, with the reason the copy cannot run yet for personal data
  (the copy gate, docs/47 section 11).

The plan is JSON the agent shows to the owner. It writes nothing: no draft in `state.db`, no file.

### 4.5 Comparing a copy made outside Loomward

`loomward-native verify-tree SOURCE DESTINATION --state-dir DIR` runs the C1 verifier once from the command line.

- Both folders explicit; neither a volume root nor a whole user profile (q-4, q-7); the state directory outside both.
  `--state-dir` is a separate scratch store, never the desktop's: the CLI records exactly the two folders as grants
  there, as `loomward-serve --grant-root` does (never a volume root), because `compare_trees` admits only an exact
  active grant and overlapping grants are refused. Both must be grantable roots (so `H:` needs lane E0 first).
- It reads metadata and content only and requests no write access. Content reads are **non-cached**
  (`FILE_FLAG_NO_BUFFERING`, sector-aligned, as docs/47's read-back), so a destination's bytes are read from the
  device, not from pages still in the system cache. The device's own cache stays outside that proof (docs/47
  section 4).
- Budgets: `--max-entries` (at most 200,000, the verifier's cap), `--budget-bytes`, `--budget-secs`. Exit 0 equal,
  1 differences (listed), 2 unknown (budget, cap, refusal or an unsupported class), which never counts as equal.
- Its result is **observation-time evidence** (docs/47 section 11, C1): it says the two trees matched when read. It
  is not a durable read-back receipt and not by itself permission to retire a source (section 6).

## 5. What may live on the external disk

Location-insensitive data can move to `H:` once copying is open; location-sensitive data stays. Agents propose,
the owner approves each plan.

| Good candidates | Stay on internal disks |
|---|---|
| finished projects and old repositories (archived as folders, not in use) | anything an application opens by absolute path while running (databases, VM disks in use, live repositories) |
| installers and downloaded archives | the Windows, program and user-profile system folders |
| photo, video and music libraries that are only browsed | credential, browser-profile and sync-client folders (invariant 2 already keeps agents out) |
| model weights and datasets not used this week (re-pointing a tool to a new path is the owner's step) | caches and build outputs (delete-and-rebuild is cheaper than archiving; `clutter` lists them for the owner) |
| game libraries through the launcher's own "move install folder" feature (the launcher moves them; Loomward only shows the space) | Loomward's own state directory |

## 6. Executing a plan

- **Personal data, copy gate open (later).** The owner opens the plan's sources in the desktop Copy view, picks the
  destination in the native dialog and approves; copy-and-verify does the rest. Until G5 and G9 pass and C8 opens,
  personal sessions keep answering `copy_effect_gated`.
- **Before the gate opens.** The owner's answer to `loomward-first-cleanup-copier` decides: wait, robocopy by an
  agent per approved plan, or the owner copies. robocopy is never wired into Loomward (docs/47 option D stays rejected
  for Loomward code). An agent's robocopy run copies into one new folder and may use only copy flags: `/E /COPY:DAT
  /DCOPY:DAT /XJ /R:1 /W:1 /NP /UNILOG:<log>` plus `/XC /XN /XO` (never replace an existing file). `/MOV`, `/MOVE`,
  `/MIR`, `/PURGE`, `/IS`, `/IT` and `/CREATE` are forbidden. The log lives outside the new folder. A robocopy exit
  code of 8 or more is a failed plan. A failed or interrupted run is never patched up: its folder is left and reported
  to the owner, and a retry copies into another new folder. Then `verify-tree` must exit 0.
- **Removing originals.** The owner's answer to `loomward-archive-originals` decides who removes them. Under every
  option, removal of a plan's originals needs a **second** `verify-tree` run that exits 0 after the owner safely
  removed the external disk and plugged it in again (the owner confirms the replug; it empties the system cache for
  that volume, while the device's own cache stays outside the proof, as in section 4.5), and the copy is never
  removed. The owner chose a Loomward Recycle Bin effect, which is not built: it needs its own LW issue and
  safety gate after the copy gate (invariant 1). Until it exists, nothing removes an original.
- **Journal for the interim path.** Each approved plan, its robocopy log summary and both verifier reports are kept
  in the gitignored `.loomward/archive/` of the primary checkout, never in Git (no real names in this repository).

## 7. Lanes (each one PR)

| Lane | Delivers | Effect | Acceptance |
|---|---|---|---|
| E0 | USB fixed-media NTFS/ReFS volumes as scan and grant roots (section 2), by splitting the shared `is_removable` predicate so the copy refusal for personal sessions stays; offline and identity-change states for detachable roots; lab copy destinations on such volumes | none (lab copies only, as today) | fence tests: USB fixed NTFS allowed, SD, removable media and USB FAT/exFAT refused as before; a root whose volume disappears reads offline with its last-seen sizes; a different volume at the same letter is `identity_changed`; a real `H:` read-only scan recorded with anonymised counts |
| E1 | this document | none | review |
| E2 | `loomward-mcp`: stdio transport (both MCP eras, ported from `python/loomward/mcp_stdio.py`), lazy host with idle release and the shared lock helper, principal, session references, `find` and `describe` | none | subprocess tests in both eras; busy while another process holds the lock; `state.db` unchanged by every tool and hidden name |
| E3 | service: space reads open to clients under the 4.2 rules (`volumes.list`, `roots.list`, `tree.children`, `stats.breakdown`, `clutter.developer`), the `volumes` field, projections | none | out-of-scope nodes unknown; totals exclude rows outside the grant; missing field refused or absent; revocation from the next call |
| E4 | `loomward-mcp` space tools, `plan_archive`, references that survive an idle release | none | tools match the service results; plan arithmetic and reserve tests; field gating; no writes |
| E5 | `loomward-native verify-tree`, non-cached content reads | read-only | equal, differing and unknown trees on lab fixtures; volume-root and overlap refusals; no write access requested; reads open with no buffering |
| E6 | owner commands `clients.grant/list/revoke`; app "Agent access" panel; USB volumes marked external | none | service and contract tests; Playwright and unit tests |
| E7 | ship `loomward-mcp` beside the desktop executable in the existing nightly bundle (Tauri `externalBin`, as the Ask broker); the desktop's 15-second lock retry and its message | none | bundle self-test finds and launches it with `--help`; lock-retry test |

## 8. Owner guide (once E0 to E7 ship in a nightly)

1. **See the space.** Open Loomward, grant `C:\`, `E:\`, `G:\` and `H:\` in the folder picker, and scan. Atlas shows
   the biggest folders; Clutter lists build outputs and caches you can delete yourself; volumes on USB are marked
   external, and a detached one shows offline with its last-seen sizes.
2. **Let an agent look.** In Agent access, create a grant for one agent name (for example `claude`): pick the roots
   and tick what it may see (`size` for space tools, `path` for where things are, `modified` for ages, `attributes` for file
   attributes, `volumes` for drive capacity). Revoke it any time; it stops at the agent's next call.
3. **Connect the agent.** Add the server to the agent host, for example Claude Code:
   `claude mcp add loomward -- "%LOCALAPPDATA%\Loomward\loomward-mcp.exe" --client claude`. The agent's tools and the
   Loomward window take turns on Loomward's data; if the window says an agent is using it, stop the agent or wait.
4. **Review plans.** The agent sends you `plan_archive` results: what, how big, where, and whether it fits. Nothing
   moves until you approve; how approved plans are executed follows your answers to the decisions below.

## 9. Owner decisions (answered 2026-10-25 in chat; HUMAN_TODO q-47, q-48)

- `loomward-copy-power-cut-method`: (a) Hyper-V VM. Hyper-V was already enabled; the VM and image live on `E:`.
- `loomward-copy-staging-removal`: allowed, but Loomward first assesses whether something can be done, helps the owner
  understand what happened and indexes or explains before deleting its own staging (docs/49 section 4).
  `LOOMWARD_COPY_STAGING_REMOVAL` stays false until that explain-first behaviour lands.
- `loomward-first-cleanup-copier`: (b) agent robocopy per approved plan, then `verify-tree` (section 6). Survey all
  disks first; mainly `C:` to `E:` and `H:`, `G:` also a candidate.
- `loomward-archive-originals`: (b) Recycle Bin after per-plan confirmation, with a human-readable, machine-learning
  summary of what is being removed (docs/49). The effect is not built and has its own gate (invariant 1).
- USB fixed NTFS/ReFS roots (section 2): confirmed by the owner.

Agent decisions recorded here, reversible: USB fixed-media NTFS/ReFS volumes become scan roots (section 2); the MCP
server holds the state lock only while busy, and the desktop retries before refusing (4.1); space tools need `size`,
are root-scoped, and each output needs the field it discloses; the machine's volume list needs the new `volumes`
field (4.2); the interim robocopy flag allowlist and the replug re-verification before any removal (section 6).
