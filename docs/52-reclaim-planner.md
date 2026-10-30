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
| Shared Cargo target dirs | a folder holding `debug/` or `release/` plus `.rustc_info.json` / `CACHEDIR.TAG`, outside any project | `cargo clean --target-dir <dir>` | the same variable pointing at a faster declared tier (docs/07) |
| Dependency installs | `node_modules` beside `package.json` (v1) | reinstall from the lockfile | none |
| Package caches | npm `_cacache`; pnpm `store/v*`; NuGet `packages` under `.nuget`; pip `http-v2`/`wheels` under a `pip` cache; uv cache (`CACHEDIR.TAG` + `archive-v*`); Gradle `caches/` under a Gradle home; Cargo `registry/cache` and `git/db` under a Cargo home | `npm cache verify`; `pnpm store prune`; `dotnet nuget locals all --list` then `--clear`; `pip cache info` / `pip cache purge`; `uv cache prune`; Gradle's own retention; `cargo cache` tools are third-party, so for Cargo the step is "delete `registry/cache` by hand" marked **owner judgement** | `npm_config_cache`, `PNPM_STORE_DIR`, `NUGET_PACKAGES`, `PIP_CACHE_DIR`, `UV_CACHE_DIR`, `GRADLE_USER_HOME`, `CARGO_HOME` |
| Model stores | Hugging Face `hub/models--*` with `blobs/` and `snapshots/`; LM Studio `models/<publisher>/<model>/*.gguf`; Ollama `models/blobs/sha256-*` | the host's own model manager (LM Studio "My Models", `ollama rm`, `huggingface-cli delete-cache`) | `HF_HOME` / `HF_HUB_CACHE`; LM Studio models directory setting; `OLLAMA_MODELS` |
| Creative-AI model folders and game libraries (rules v3) | ComfyUI: each child of a `models` directory whose parent holds `comfy` and `custom_nodes` (`checkpoints`, `loras`, ...); A1111 / Forge: `models` holding `Stable-diffusion`, beside `webui.py`; Steam: each directory directly under `steamapps/common` where `steamapps` holds an `appmanifest_*.acf` file (names only, manifests are never read) | review in the tool by hand: custom checkpoints and trained LoRAs may be unique, so a store is never called disposable; Steam: Properties, Installed Files, Move install folder (whether a game is re-downloadable, or mods and saves survive an uninstall, is not established by names) | `extra_model_paths.yaml`; `--ckpt-dir` / `--lora-dir`; another Steam library folder (Steam Settings, Storage). All three plan under Relocate |
| Virtual disks | `ext4.vhdx` under a WSL distro folder; `docker_data.vhdx` / `Docker.vhdx` | `docker system df -v` then targeted `docker ... prune`; WSL compaction by its documented offline procedure | Docker Desktop disk-image location; `wsl --manage <distro> --move` |
| Agent worktrees | a directory with a `.git` **file**: a linked worktree, a submodule or a separate-git-dir checkout; names alone cannot tell which, so the owner step checks with `git worktree list` and `git submodule status` | check branch, HEAD and dirty state in the owning repository before removing anything | none |
| Windows leftovers (rules v4) | names only. `Windows.old`, `$WINDOWS.~BT`, `$WINDOWS.~WS` directly under the granted root (`windows-old`); `Windows\MEMORY.DMP`, `Windows\Minidump`, `Windows\LiveKernelReports` (`memory-dump`); `ProgramData\Microsoft\Windows\WER\ReportArchive` and `ReportQueue` (`error-reports`); `Windows\Temp` and `AppData\Local\Temp` (`temp`); `AppData\Local\CrashDumps` (`crash-dumps`); `AppData\Local\D3DSCache`, `NVIDIA\DXCache`, `NVIDIA\GLCache`, `AMD\DxCache` under `AppData\Local` (`shader-cache`); `AMD` or `NVIDIA` directly under the root holding an installer payload (`driver-extract`, below); `Windows\SoftwareDistribution\DeliveryOptimization` and a `Cache` directory under `NetworkService\AppData\Local\Microsoft\Windows\DeliveryOptimization` (`delivery-optimization`). Root-relative paths are matched from the granted root; `AppData` paths by the parent chain wherever `AppData` sits under the grant (a user-profile or volume-root grant), so a `Temp` outside `AppData\Local` or a `Windows.old` below the root is not recognised. All new Windows names compare ASCII case-insensitively (the v3 names keep their exact comparison) | Windows' own tool, named per rule: Settings > System > Storage > Temporary files ("Previous Windows installation(s)", Storage Sense); Disk Cleanup ("System error memory dump files", "Windows Error Reports and Feedback Diagnostics", "DirectX Shader Cache", "Delivery Optimization Files"); by hand after review for `crash-dumps` and `driver-extract`. Class `windows_leftover`, plans under Tool-owned cleanup; relief is the usual verified-relief rule, so unobserved sizes stay unknown | none |
| Installer caches (rules v4) | `ProgramData\Package Cache` and `Windows\Installer` directly under the granted root's `ProgramData` or `Windows` directory | informational (`system_managed`): never remove by hand, they back repair and uninstall; never summed as reclaimable | none |
| Recycle Bin contents | `$Recycle.Bin` on the volume (per-volume, system-managed) | empty the Recycle Bin (owner, Explorer) | none |
| System-managed space | `hiberfil.sys`, `pagefile.sys`, `swapfile.sys`, `WinSxS`, `SoftwareDistribution` | informational only: Disk Cleanup / `DISM /Online /Cleanup-Image /AnalyzeComponentStore`; never summed as reclaimable | none |

**Driver extracts (rules v4).** Names cannot show that a grant is a volume root, so `driver-extract` does not rely on one:
an `AMD` or `NVIDIA` directory directly under the granted root is recognised only when it holds an installer payload,
meaning a `Setup.exe` file, a `Packages`, `Display.Driver` or `DisplayDriver` child directory, or a child directory
holding a `Setup.exe` (`C:\AMD\<package>\Setup.exe`, `C:\NVIDIA\DisplayDriver\...`). An `AMD` or `NVIDIA` directory with
none of these, or below the root (`Program Files\NVIDIA Corporation`), is not recognised. A recognised root-level store
shields its subtree from the walk, so nothing inside `Windows.old`, `Windows\Temp` or a driver extract is reported twice.
`Windows\SoftwareDistribution\DeliveryOptimization` sits inside the already-listed informational `SoftwareDistribution`;
that parent is never counted as reclaimable, so the two do not double count.

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
2. **Relocate**: active caches, model stores and target directories better placed on a faster declared or hinted tier (docs/07 tiers).
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

`reclaim.plan { volume_id, limit (1..200) } -> ReclaimPlan` with: `volume` (label, bus type, capacity, free,
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

### 6.1 Observing relief (lane R1g, #939)

R1d leaves the link-count pass out of `reclaim.plan`: a plan answers under an owner read's deadline of a few seconds,
while a `target` or `node_modules` candidate holds 10^4 to 10^6 names, and `linkcount::observe` opens each one (metadata
only, at most 200,000 per candidate). Inline, the pass would almost always stop at the deadline, and every candidate
would stay "not observed". Relief is therefore observed by a separate, owner-started job, and the plan reads its stored
result.

- **Command.** `reclaim.observe { volume_id } -> JobResult`. It is owner-only (Tauri, HTTP) with effects `none`, has the job
  lifecycle of `scan.start` (`jobs.get`, progress events, cancel), and refuses the same volumes `reclaim.plan` refuses. No client
  or MCP access in this lane.
- **What it reads.** The job recomputes the plan's candidates from the catalogue exactly as `reclaim.plan` does, with the
  same `without_repairs()` reads and the same outermost roots. For each candidate it lists the catalogue's names under the
  candidate (bounded at `MAX_FILES_PER_GROUP`), builds each name's path under the granted root's current path, and runs
  `linkcount::observe` with the listing's identity as `expected`. A name whose handle identity differs from the listing,
  a placeholder or a reparse point is `Unknown`, so a path that now resolves somewhere else cannot lend its link count.
  Opens are metadata only, non-elevated (`observe` refuses an elevated token) and limited to granted roots
  (invariant 2). No content is read, and nothing is written outside `state.db`.
- **Budget.** The job has a wall-clock budget (default 10 minutes) and stops cleanly on cancel. A candidate the pass did
  not finish keeps `stop` set, so its relief stays unknown, never zero. Candidates are observed largest allocation first, so an
  interrupted job has still covered the ones that matter most.
- **Storage.** One `state.db` row per candidate:
  `(volume, root, candidate node, root generation, observed_at, unique_allocated_bytes?, reason?)`. A stored result
  applies only while the root's generation and the candidate's node incarnation are unchanged, and while the root's
  freshness is not stale. Otherwise the plan reports "relief not yet observed for this scan". Forgetting or revoking a
  root deletes its rows.
- **Plan.** `reclaim.plan` fills `ReclaimInput.files` from the matching rows; unmatched candidates stay
  `RELIEF_UNOBSERVED`. The plan stays a pure read within its deadline.
- **Age (#1029).** The bindings above track the catalogue, not the filesystem: a hard link made to a file under the
  candidate after the pass moves nothing in the catalogue, so a stored figure can overstate relief while it is still
  bound. Each candidate whose `reclaimable_bytes` comes from a stored verdict therefore carries `observed_at`, the time
  of that pass (null wherever the bytes are unknown), and the Reclaim view says "verified 3 hours ago" beside the
  figure. There is deliberately no maximum age after which the figure is dropped: that is a policy choice, left open.
  Placement reports the same time as `relief_observed_at` on a candidate group whose `estimated_relief_bytes` came from
  a stored verdict (null for a live link-count pass or unknown relief); the Tiers view words it like the Reclaim view.
- **Not chosen.** An inline pass for small candidates is cheap, but it helps only small caches and makes plan latency
  depend on disk state. Placement's `PlacementInput::files` has the same deadline problem; it can read the same rows later.
- **Acceptance.** Synthetic lab trees only. Hard links inside a candidate count once. Links crossing the candidate
  boundary give unknown. A renamed or replaced name (identity mismatch) gives unknown. Cancel and budget stops leave the
  unfinished candidates unknown. A rescan (new generation) invalidates the rows. `state.db` changes only in the new table,
  and the catalogue stays byte-identical. The job refuses an elevated token.

### 6.2 Inspect a selected set (#1002)

The Reclaim view lets the owner select up to 12 current candidates on one volume, enter a target
in whole bytes, and explicitly observe that set. This is read-only inspection. It adds no automatic
subset search, execution control or permission to remove anything.

The optional `selection` property on `reclaim.observe` and `reclaim.plan` contains sealed node IDs.
Old requests retain their behavior. The service derives paths from current catalogue records and
active grants; duplicate IDs, wrong-volume/non-candidate nodes and ancestor/descendant selections
are refused. One union pass has global limits of 200,000 names, 200,000 directories and ten minutes.
It uses the existing non-elevated, no-recall identity checks and `ReliefTally` link closure. Two
individually unknown candidates can yield a known union when their shared hard links close together;
an external link, mismatch, partial coverage or stopped pass keeps the whole union unknown.

Only one latest aggregate per volume and its member bindings are retained, without a name/object
graph. The publication statement checks the catalogue instance, volume, active grants, root
generations, node incarnations and subtree revisions together. Reads also require fresh, complete
roots with no scan in flight. Any changed member expires the result; revoke/forget removes it.

The view shows the recorded observation's age, exact byte estimate, reason when unknown, and exact
target comparison. Changing the selection clears the old result immediately. Recorded allocation
is neither freed space nor deletion approval. Recycle Bin moves and same-volume renames do not
immediately recover it. The existing directory-check/open race and physical-model limitations remain.

The native regression and `reclaim_selection_fixture` use two disposable candidates with three
names for two NTFS objects: individually unknown, jointly 20,480 allocated bytes counted once.
The production HTTP/Svelte qualification compares that complete case with PR1001's independent
Python oracle in the test process only. Loomward never starts the Python experiment.

### 6.3 Relief check: what a cleanup actually freed (#1013)

The plan and `plan_archive` estimate relief before anything happens. The relief check closes the loop
afterwards, read-only: the owner records a volume sample before a cleanup and another after it, and the Reclaim
view shows the observed change beside the plan's estimate. Loomward never runs the cleanup.

- **Command.** `reclaim.sample { volume_id, role: before | after, estimate_bytes? } -> ReclaimRelief`. Owner-only
  (Tauri, HTTP), effects `none`: it reads the volume's metadata and writes one row to `state.db`. `reclaim.plan`
  gains an optional `relief` block (absent until a before sample exists), so the view shows the stored check on load.
  Chosen over a separate `reclaim.outcome` read because the outcome is a pure function of two stored rows, and the plan
  is already the one read the view makes.
- **A sample** is free bytes, total bytes, the Recycle Bin size where readable, the volume's identity (the catalogue's
  volume key and the volume GUID recorded with the granted root) and the time. The bin is read exactly as the plan
  reads it: only through a volume-root grant, metadata only; otherwise it is unknown, never zero. Free or total space
  that cannot be read is `null` in the sample. A legacy grant with no recorded volume GUID cannot detect a
  foreign disk on its letter: the volume then reads as present, and the second sample's free space typically
  reads unknown (so `unreadable`) rather than comparing. A volume that is offline, or whose letter now holds another disk
  (#910), is refused and nothing is recorded.
- **Storage.** `st.reclaim_sample`, at most one `before` and one `after` per volume (primary key `(volume_id, role)`);
  a row also carries the catalogue instance and is ignored when that differs. Recording a new `before` deletes the
  old `after` and starts a new check. When the last active grant on a volume is revoked, forgotten or loses its
  identity, and in the startup sweep, the volume's samples are deleted with the other derived reclaim state. `estimate_bytes` is the plan's
  estimate as the owner saw it (the recorded union estimate of the current selection, when there is one), kept
  beside the before sample; it is refused with `after`, and an `after` without a `before` is refused.
- **Outcome.** A pure comparison of the two samples. Different volume identities (key or GUID) never compare
  (`different_volume`). Free space unreadable on either side gives `unreadable`, not zero. Otherwise
  `compared` carries `direction` (`more_free`, `less_free`, `unchanged`) and the unsigned `change_bytes`: a fall
  in free space is reported as a fall, not as a negative or zero relief.
- **Wording.** The view says "free space changed by X between T1 and T2; the plan estimated Y; the Recycle Bin held
  Z at each sample and keeps taking space until emptied". It states that this is the whole volume's change: other
  writes, an unemptied bin, hard links elsewhere and restore points all affect it, so it is not proof of what the
  cleanup freed, and not a calibration of the estimate.
- **Acceptance.** Unit tests for the arithmetic and the identity rule (different GUID, different key, negative
  change, unreadable free space, unreadable bin, a before sample alone, the unsigned extremes); service tests for the
  stored round trip, the refusals, and that only `st.reclaim_sample` changes; contract tests for the nullable fields.
  Not exercised: a readable Recycle Bin (it needs a volume-root grant on a real volume).

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
| R1g | `reclaim.observe` job: bounded link-count observation stored per candidate and root generation (section 6.1, #939) | none (metadata opens on granted roots) | section 6.1 acceptance |
| R1h | selected-set inspection and exact target comparison (section 6.2, #1002) | none (metadata opens on granted roots) | native union, invalidation, budgets and real HTTP/Svelte journey |
| R1i | relief check: before and after volume samples beside the plan's estimate (section 6.3, #1013) | none (volume metadata only; one `state.db` row per sample) | identity rule, unknown-not-zero and negative-change tests; the view's two buttons |

## 9. What this design deliberately does not do

- It does not delete, clean, compact, move or relink anything, and it does not run any tool's cleanup command.
- It does not change environment variables or tool settings; it names them.
- It does not infer that anything is safe to delete from a learned score or a name (invariant 3); "generated" is a
  reading of names beside a manifest, stated with its uncertainty.
- It does not read Windows Recent, Jump Lists, Prefetch or SRUM for "last used" evidence; coldness stays on file
  times and, inside granted project roots, Git activity (see docs/51, coldness vector).
