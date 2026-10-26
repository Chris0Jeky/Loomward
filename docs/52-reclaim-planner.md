# Reclaim planner: a read-only plan for a nearly full system disk

Status: **design, proposed 2026-10-25** by the research lane ([51](51-research-programme.md), recommendation R1 of the
2026-10-25 deep-research report). It adds **no file effect, no process effect and no execute path**. Every number it
shows is an observation or an explicit unknown (invariant 4); every action it names is text for the owner to run
in the owning tool, never something Loomward runs (invariant 1). It composes existing reads: developer clutter
(LW-119, `clutter.developer`), placement relief (LW-108, `loomward-engine::placement`), volumes and tiers.

## 1. The problem

The owner's system NVMe (`C:`, about 1.86 TB) holds about 20 GB free, within a few GB of the 1% reserve that Loomward's own
native fixtures and copy manifest refuse to cross (`max(1 GiB, 1% of the volume)`). Builds, model downloads, VHDX
growth and copy staging each need more than that. Archiving owner documents to `H:` waits on the copy gate
(docs/47 section 11) and on the owner's per-plan review (docs/48 section 6). Generated and re-downloadable data is
the fastest relief that never touches unique owner content: build outputs, package caches, model stores, container
and WSL disks, and stale agent worktrees.

The planner answers one question honestly: **"Which generated or re-obtainable things on this volume could the
owner remove or relocate, how much would that really free, how sure are we, and which tool owns the cleanup?"**

## 2. What exists and is reused

| Need | Existing piece | Gap the planner fills |
|---|---|---|
| Project build outputs and dependency trees | `clutter.developer` (LW-119): Cargo `target`, `node_modules`, `.venv`/`venv`/`__pycache__`, .NET `bin`/`obj`, Gradle `build`/`.gradle`, beside a recorded manifest, with logical and allocated bytes, coverage and an uncertainty note | only inside project roots; `regeneration_cost` is always `unknown` (LW-087); no machine-level stores |
| Hard-link-aware relief | placement "verified relief" (LW-108): counts allocation only when every identity-matched link name is inside the group (`placement/mod.rs`, `linkcount.rs`) | not offered per clutter candidate |
| Placeholders | `linkcount::is_placeholder` (offline, recall-on-open, recall-on-data-access) | none |
| Volume capacity, bus type, tier | `volumes.list`, `volumes.declare_tier`, `tiers.model` | none |
| Stale worktrees | `ClutterTopic::StaleWorktrees` is reported as not assessed (LW-076 lease evidence) | still not assessed |
| Model stores | `ClutterTopic::ModelHostLifecycle` not assessed (LW-086) | still not assessed |

## 3. Candidate classes (rules version 2 proposal)

Each class is a **recogniser over catalogue rows** (names and structure only, as rules version 1 is), plus a fixed
statement of which tool owns the data and what its owner-run command is. Recognisers never read file contents.

| Class | Recogniser (names only) | Owning tool and owner-run step | Relocation the owner may choose |
|---|---|---|---|
| Project build output | rules v1 (Cargo, .NET, Gradle) | `cargo clean` (preview: `cargo clean --dry-run`), `dotnet clean`, `gradlew clean` | `CARGO_TARGET_DIR` on another volume |
| Shared Cargo target dirs | a folder holding `debug/` or `release/` plus `.rustc_info.json` / `CACHEDIR.TAG`, outside any project | `cargo clean --target-dir <dir>` | the same variable pointing at `E:` or `G:` |
| Dependency installs | `node_modules` beside `package.json` (v1) | reinstall from the lockfile | none |
| Package caches | npm `_cacache`; pnpm `store/v*`; NuGet `packages` under `.nuget`; pip `http-v2`/`wheels` under a `pip` cache; uv cache (`CACHEDIR.TAG` + `archive-v*`); Gradle `caches/` under a Gradle home; Cargo `registry/cache` and `git/db` under a Cargo home | `npm cache verify`; `pnpm store prune`; `dotnet nuget locals all --list` then `--clear`; `pip cache info` / `pip cache purge`; `uv cache prune`; Gradle's own retention; `cargo cache` tools are third-party, so for Cargo the step is "delete `registry/cache` by hand" marked **owner judgement** | `npm_config_cache`, `PNPM_STORE_DIR`, `NUGET_PACKAGES`, `PIP_CACHE_DIR`, `UV_CACHE_DIR`, `GRADLE_USER_HOME`, `CARGO_HOME` |
| Model stores | Hugging Face `hub/models--*` with `blobs/` and `snapshots/`; LM Studio `models/<publisher>/<model>/*.gguf`; Ollama `models/blobs/sha256-*` | the host's own model manager (LM Studio "My Models", `ollama rm`, `huggingface-cli delete-cache`) | `HF_HOME` / `HF_HUB_CACHE`; LM Studio models directory setting; `OLLAMA_MODELS` |
| Virtual disks | `ext4.vhdx` under a WSL distro folder; `docker_data.vhdx` / `Docker.vhdx` | `docker system df -v` then targeted `docker ... prune`; WSL compaction by its documented offline procedure | Docker Desktop disk-image location; `wsl --manage <distro> --move` |
| Agent worktrees | a directory with a `.git` **file**: a linked worktree, a submodule or a separate-git-dir checkout; names alone cannot tell which, so the owner step checks with `git worktree list` and `git submodule status` | check branch, HEAD and dirty state in the owning repository before removing anything | none |
| Recycle Bin contents | `$Recycle.Bin` on the volume (per-volume, system-managed) | empty the Recycle Bin (owner, Explorer) | none |
| System-managed space | `hiberfil.sys`, `pagefile.sys`, `swapfile.sys`, `WinSxS`, `SoftwareDistribution` | informational only: Disk Cleanup / `DISM /Online /Cleanup-Image /AnalyzeComponentStore`; never summed as reclaimable | none |

Excluded by invariant 2 and never recognised: browser profiles and their caches, credential stores, sync-client
folders, Loomward's own state directory.

## 4. How a candidate's size is stated

Every candidate carries three numbers, each nullable, and a reason when null:

- **logical bytes** and **allocated bytes**: the catalogue's observed aggregates, as `clutter.developer` reports
  today; null when the subtree is partial, stale, mid-scan or has unknown allocation.
- **estimated reclaimable bytes**: placement's verified-relief rule applied to the candidate as the deletion set.
  Allocation counts only for identities whose observed link count equals the names seen inside the candidate.
  When the link-count pass has not run, has hit its 200,000-name cap, or any link count exceeds the names seen,
  the value is null with `reason: "hard links outside the candidate, or not observed"`. A pnpm store or a Hugging
  Face cache is the common case: its blobs are linked from elsewhere, so naive sums overstate relief.

Two special rules:

- **Virtual disks** report the container's allocated bytes as "container size", and their reclaimable bytes as
  **unknown**: guest free space is not observable from the host without the owning tool.
- **The Recycle Bin trap.** Sending files to the Recycle Bin on `C:` frees nothing on `C:` until the bin is emptied:
  the bin lives on the same volume. The planner says so whenever it shows a recycle-type step for a candidate on the
  volume under pressure, and lists the bin's current size (from `SHQueryRecycleBinW`, a read-only metadata query, only when
  the granted root is the volume root) as a candidate of its own. This interacts with the owner's chosen archive-originals effect
  (q-48: Recycle Bin after per-plan confirmation): originals recycled from `C:` keep occupying `C:` until emptied.

## 5. Ranking and output

The plan groups candidates by **what the owner does next**, then ranks inside each group by estimated reclaimable
bytes (unknowns last, never ranked as zero):

1. **Tool-owned cleanup**: generated or cache data whose owning tool offers a supported cleanup command.
2. **Relocate**: active caches, model stores and target directories better placed on `E:` or `G:` (docs/07 tiers).
   Changing an environment variable or a tool setting is a global configuration change, so it is always the owner's
   step (invariant 2), shown as the exact variable and value to set.
3. **Archive candidates**: things that are not generated (old projects, installers, media) go to the archive plan
   (docs/48 `plan_archive`), never to this planner's cleanup list.
4. **Informational**: system-managed space, the Recycle Bin, virtual disks with unknown relief.

Each candidate renders as facts, never as a verdict (docs/49 section 3): "Cargo build output, 38.2 GB allocated,
37.9 GB estimated reclaimable, beside `Cargo.toml`; rebuild with Cargo; last write 212 days ago; no reclaim action has
been approved." The plan's footer always says how much of the volume the plan **does not** explain
(volume used minus the sum of covered roots), shown as "unaccounted", not hidden.

### Proposed contract (view service, read-only)

`reclaim.plan { volume_key, limit (1..200) } -> ReclaimPlan` with: `volume` (label, bus type, capacity, free,
reserve), `covered_roots` (granted roots on that volume with coverage), `groups[]` (the four above) of
`ReclaimCandidate { node_id, class, provider, kind, logical_bytes?, allocated_bytes?, reclaimable_bytes?,
reclaimable_reason?, coverage, owner_step: Text, relocation?: { variable_or_setting, suggested_target_volume },
evidence: [fact], uncertainty }`, `unaccounted_bytes?`, `rules_version`, `partial`, `truncated`, `budget_hit`.
`owner_step` is display text; there is no field, command or endpoint that executes it. For agents, a read-only MCP
tool `reclaim_plan` would mirror it under docs/48 section 4.2's field rules (`size` and `volumes` required); the MCP
crate is owned by the external-disk lane, so this is proposed there as an issue, not built here.

## 6. Coverage: which roots the planner sees

A volume-root grant reports the Recycle Bin and system-managed space first and still returns them when the walk runs out of budget (`budget_hit`).

The planner reads the catalogue, so it sees only owner-granted, scanned roots (invariant 2). Two reads touch the live
volume, both metadata only and both limited to granted roots: placement's link-count pass, and the Recycle Bin size,
which is read only when the granted root is the volume root (the bin is per volume; otherwise its size is unknown).
It never stats a
well-known location the owner has not granted. To make the first plan useful, the desktop shows a **checklist of
well-known store locations** (for example `%USERPROFILE%\.cargo`, `%LOCALAPPDATA%\npm-cache`,
`%USERPROFILE%\.cache\huggingface`, `%USERPROFILE%\.lmstudio\models`, `%LOCALAPPDATA%\Docker`,
`%LOCALAPPDATA%\Packages\<distro>\LocalState`), each granted one at a time through the native folder picker. None is
a whole user profile, a volume root or a credential or browser store (q-4, q-7). The checklist is text until the
owner picks; nothing is probed for existence in advance.

## 7. Validation

- **Synthetic fixtures** (`fixtures/reclaim/`, synthetic names only): one tree per class, plus hard-link groups that
  cross a candidate boundary, a placeholder inside a cache, a partial subtree, a `.git`-file worktree, and a decoy
  for each class (a folder named `target` with no `Cargo.toml`, a `models` folder of documents, a `node_modules`
  inside `target`). Acceptance: each decoy is not a candidate; cross-boundary links give null reclaimable bytes;
  partial subtrees give null sizes, never zero; output is byte-stable for the same catalogue.
- **Tool agreement** (lab, owner machine, read-only commands only, run by an agent, not by Loomward):
  `cargo clean --dry-run`, `dotnet nuget locals all --list`, `pip cache info`, `uv cache dir`, `docker system df -v`.
  Not `npm cache verify`: it garbage-collects the cache, so npm is compared on Loomward's own sizes only. Acceptance:
  where a command reports sizes (`pip cache info`, `docker system df -v`, and `cargo clean --dry-run` if the installed
  cargo prints a total), Loomward's allocated bytes agree within 2%, or the
  planner reports unknown; path-only commands (`uv cache dir`, `dotnet nuget locals all --list`) only confirm which
  folder is the cache. Results recorded with anonymised counts only.
- **No effect.** A test asserts the reclaim read requests no write, delete or rename access, and that `state.db` is
  unchanged apart from the read's own bookkeeping.

## 8. Lanes

| Lane | Delivers | Effect | Acceptance |
|---|---|---|---|
| R1a | this design | none | review |
| R1b | rules version 2 recognisers for machine-level stores in `loomward-catalog` (shared Cargo targets, package caches, model stores, virtual disks, `.git`-file worktrees), with the decoy fixtures | none | section 7 synthetic tests |
| R1c | per-candidate reclaimable bytes from the placement relief rule; the Recycle Bin size read | none | cross-boundary link fixtures; bin read is metadata only |
| R1d | `reclaim.plan` in the view service and protocol; the app's Reclaim view with the four groups and the unaccounted line | none | contract tests; Playwright; no execute affordance anywhere |
| R1e | tool-agreement run on the owner's machine | none (agent runs read-only tool commands) | section 7 record |
| R1f | proposed to the external-disk lane: MCP `reclaim_plan` tool | none | as docs/48 E4 |

## 9. What this design deliberately does not do

- It does not delete, clean, compact, move or relink anything, and it does not run any tool's cleanup command.
- It does not change environment variables or tool settings; it names them.
- It does not infer that anything is safe to delete from a learned score or a name (invariant 3); "generated" is a
  reading of names beside a manifest, stated with its uncertainty.
- It does not read Windows Recent, Jump Lists, Prefetch or SRUM for "last used" evidence; coldness stays on file
  times and, inside granted project roots, Git activity (see docs/51, coldness vector).
