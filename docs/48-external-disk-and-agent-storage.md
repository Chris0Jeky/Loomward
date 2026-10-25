# External archive disk and agent storage tooling

Status: **design, proposed 2026-10-25** by the external-disk driver session. It adds no file effect and opens no
gate: AGENTS.md invariants 1 and 7 and the copy-and-verify gate of [47](47-copy-and-verify.md) section 11 stand as
written. It extends [46](46-find-and-ask.md) section 8.2 (the native MCP projection) with the separate grant design
that section asks for, and it reuses copy-and-verify for every write.

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
`Elements`, mounted as `H:` with about 5.6 TB free. Windows reports its drive type as **fixed**, not removable, as it
does for most USB hard disks. Rules that follow:

- **Never the drive letter.** Letters move between plugs. A destination is the volume's GUID path and the pinned
  identity of the destination folder (docs/47 section 4: the destination parent is pinned like a root). A different
  disk on `H:` tomorrow is `destination_changed`, never a silent retarget.
- **Bus type, not drive type.** `volumes.list` already reports `device.bus_type`; the app and the agent tools show
  a USB volume as "external, may be detached" whatever Windows' drive type says. Unknown stays unknown (invariant 4).
- **Detach is a destination power loss.** Unplugging the disk mid-copy cuts power to its cache. The copy journal
  lives in `state.db` on the internal disk, so the executor's existing rules apply: an unreachable destination is
  `destination_changed` or a resumable refusal, staging is never adopted by name, and a published copy is never
  removed (docs/47 sections 5 and 8). Measuring this with real unplugs is evidence towards G5, not a substitute
  for the method the owner picks in `loomward-copy-power-cut-method`.
- **Offline is honest.** A detached disk's catalogue rows stay visible and are marked offline; their sizes are
  last-seen observations, never zero (invariant 4).

## 3. What already exists (reused, not rebuilt)

| Need | Existing piece |
|---|---|
| Fast inventory of each volume | native scan into the catalogue (`scan.start` on owner-granted roots; whole volumes need the owner's native grant dialog, q-7) |
| Where the space goes | `tree.slice` / `tree.children` sorted by size (Atlas), `stats.breakdown` by extension family, extension or age band, `clutter.developer` (build outputs and caches), duplicate groups (q-10), `volumes.list` capacity and free space |
| Tiers | `volumes.declare_tier`, `tiers.model`, `placement.simulate` (non-executable proposals) |
| A safe write | copy-and-verify (docs/47): handle-based copy into new staging, non-cached read-back, SHA-256 per stream, atomic no-replace publish, journal and recovery, revocation fences. Lab-only today |
| Proof of a copy made elsewhere | the C1 tree verifier, `Engine::compare_trees` (read-only, both roots through the q-10 admission gate) |
| Per-client disclosure | LW-067 client grants (`mcp:<name>`, roots and fields, revocable, epoch-bound cursors) and the LW-069 projection (`crates/loomward-service/src/clients.rs`) |

## 4. The agent surface

### 4.1 One native MCP server, read-only by construction

`loomward-mcp` is a stdio MCP server binary (new crate `crates/loomward-mcp`, LW-066/F12). It succeeds the frozen
Python reference for live catalogues; the Python reference is untouched.

- **Host.** It opens the personal service in process, as `loomward-serve` does, and calls it with the fixed
  principal `mcp:<name>` (`--client <name>`, set at launch, never by a payload). Every call goes through the
  service's client gate, so the owner's live grant is re-read on every call and a revocation holds from the next one.
- **The state lock.** The state directory has one exclusive owner (docs/41 section 3). The server opens the service
  lazily on the first tool call and drops it after an idle period (`--idle-release-secs`, default 120), releasing the
  lock so the desktop app can open. While the desktop holds the lock, tools answer `busy` with a plain message
  ("Loomward is open on this machine; ask the owner, or try again after it closes"). References are session-local
  and die with each release (docs/46 section 8.2). ponytail: a named-pipe endpoint in the desktop is the upgrade if
  agents and the open app must work at the same time.
- **Read-only.** The tool set reaches read commands only. Tests assert `state.db` and `ask.db` are unchanged by every
  tool and by every attempted hidden name (docs/46 F12). There is no copy, approve, start, cancel, scan, grant or
  delete tool, and none may be added without changing invariant 7 (G6 holds).

### 4.2 The grant design docs/46 section 8.2 asked for

Docs/46 limited MCP to `find` and `describe` until "a separate grant design". This is it:

- A client grant keeps its roots and its fields (`path`, `size`, `modified`, `attributes`). Every space tool is
  **scoped to the granted roots**: a node outside them answers as unknown, and totals are computed only from rows
  under them, by the catalogue, before anything is counted (the same rule as search).
- A new optional field, `volumes`, discloses the machine's volume list (label, bus type, capacity, free space,
  online state, declared tier). It is off unless the owner ticks it, because it is a machine-wide observation.
- Names and paths still need `path`; sizes need `size`. A grant without `size` gets no space tools at all (an
  aggregate of bytes is a size disclosure).
- The owner issues and revokes grants in the desktop app (an "Agent access" panel over `grants.create_disclosure`,
  `grants.list`, `grants.revoke`). An agent never issues its own grant; a shell-capable agent could bypass any local
  gate, so the grant is the owner's consent and scope, not a sandbox.
- What an MCP host sends to its model is outside Loomward (docs/28). The panel says so before a grant with `path`.

### 4.3 Tools

| Tool | Backed by | Needs | Notes |
|---|---|---|---|
| `find` | `search.query` | grant | as docs/46 |
| `describe` | `node.inspect` | grant | as docs/46 |
| `volumes` | `volumes.list` | `volumes` | bus type, capacity, free, tier, online |
| `roots` | `roots.list` | grant | only granted roots; coverage and freshness |
| `largest` | `tree.children` sorted by size | `size` | children of a granted folder, page cap 30 |
| `breakdown` | `stats.breakdown` | `size` | by extension family, extension or age band, 16 groups |
| `clutter` | `clutter.developer` | `size` | build outputs and caches under granted roots |
| `plan_archive` | composition of the reads above | `size`, `volumes` | pure; see 4.4 |

### 4.4 `plan_archive`: a plan, never an action

Input: up to 64 session references (folders or files from `find`/`largest`), a destination volume from `volumes`,
and an optional new folder name. Output, computed from the catalogue only:

- per item and in total: bytes, files, last-modified range, coverage and freshness (stale or partial is said so);
- the destination's free space after Loomward's reserve (the planner's 1% floor), and whether the plan fits;
- warnings the catalogue can already see: cloud placeholders, reparse points, items under another grant's scope,
  offline sources, a destination on the same physical disk as a source ("not a backup", docs/09);
- the destination identity (label, bus type, volume key) and the new-folder shape of docs/47 (Q3 answered (a) by
  default on 2026-10-24: a new folder only, never into an existing one);
- `execution`: always `"owner_approval_required"`, with the reason the copy cannot run yet for personal data
  (the copy gate, docs/47 section 11).

The plan is JSON the agent shows to the owner. It writes nothing: no draft in `state.db`, no file.

### 4.5 Verifying a copy made outside Loomward

`loomward-native verify-tree SOURCE DESTINATION --state-dir DIR` runs the C1 verifier once from the command line.
Both folders must be explicit, neither may be a volume root or a whole user profile (q-4, q-7), and the state
directory must lie outside both. It reads metadata and content only, requests no write access, and prints the
`loomward_operations::tree::Report` as JSON. Exit code 0 means equal, 1 means differences (listed), 2 means unknown
(budget, refusal or an unsupported class), which never counts as equal. It is the proof step of the interim cleanup
path (section 6), and later of G9's proven restore.

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
  agent per approved plan, or the owner copies; in the latter two, `verify-tree` must report equal before a plan is
  done. robocopy is never wired into Loomward (docs/47 option D stays rejected for Loomward code).
- **Removing originals.** The owner's answer to `loomward-archive-originals` decides: the owner deletes from a
  verified list, or a new Recycle Bin effect with its own gate. Until then nothing removes an original.
- **Journal for the interim path.** Each approved plan, its robocopy log summary and its verifier report are kept in
  the gitignored `.loomward/archive/` of the primary checkout, never in Git (no real names in this repository).

## 7. Lanes (each one PR)

| Lane | Delivers | Effect | Acceptance |
|---|---|---|---|
| E1 | this document | none | review |
| E2 | `loomward-mcp`: stdio transport (both MCP eras, ported from `python/loomward/mcp_stdio.py`), lazy host with idle release, principal, session references, `find` and `describe` | none | subprocess tests in both eras; busy while another process holds the lock; references die on release; `state.db` unchanged by every tool and hidden name |
| E3 | service: space reads open to clients under the 4.2 rules (`volumes.list`, `roots.list`, `tree.children`, `stats.breakdown`, `clutter.developer`), the `volumes` field, projections | none | out-of-scope nodes unknown; totals exclude rows outside the grant; no `size` means refused; revocation from the next call |
| E4 | `loomward-mcp` space tools and `plan_archive` | none | tools match the service results; plan arithmetic and reserve tests; no writes |
| E5 | `loomward-native verify-tree` | read-only | equal, differing and unknown trees on lab fixtures; volume-root and overlap refusals; no write access requested |
| E6 | app: "Agent access" panel (issue, list, revoke client grants) and a volumes overview that marks USB volumes external | none | Playwright and unit tests |
| E7 | ship `loomward-mcp` beside the desktop executable in the existing nightly bundle (Tauri `externalBin`, as the Ask broker) | none | bundle self-test finds and launches it with `--help` |

## 8. Owner guide (once E2 to E6 ship in a nightly)

1. **See the space.** Open Loomward, grant `C:\`, `E:\`, `G:\` and `H:\` in the folder picker, and scan. Atlas shows
   the biggest folders; Clutter lists build outputs and caches you can delete yourself; Tiers marks `H:` as external.
2. **Let an agent look.** In Agent access, create a grant for one agent name (for example `claude`): pick the roots
   and tick what it may see (`size` for space tools, `path` for names, `volumes` for drive capacity). Revoke it any
   time; it stops at the agent's next call.
3. **Connect the agent.** Add the server to the agent host, for example Claude Code:
   `claude mcp add loomward -- "%LOCALAPPDATA%\Loomward\loomward-mcp.exe" --client claude`. Close the Loomward window
   while the agent works, or the agent's tools answer busy.
4. **Review plans.** The agent sends you `plan_archive` results: what, how big, where, and whether it fits. Nothing
   moves until you approve; how approved plans are executed follows your answers to the two decisions below.

## 9. Owner decisions this design waits on

- `loomward-copy-power-cut-method` (docs/47 Q2) and `loomward-copy-staging-removal` (Q1), open since 2026-10-17:
  without Q2 the copy gate never opens for personal data.
- `loomward-first-cleanup-copier` (posed 2026-10-25): how the first cleanup copies before that gate opens.
- `loomward-archive-originals` (posed 2026-10-25): who removes originals after a verified copy.

Agent decisions recorded here, reversible: the MCP server holds the state lock only while busy (4.1); space tools
need `size` and are root-scoped, and the machine's volume list needs the new `volumes` field (4.2).
