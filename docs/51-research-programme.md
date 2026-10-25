# Research programme: reconciling the 2026-10-25 storage report with Loomward, and the next six months

Status: **proposed 2026-10-25** by the research lane (Opus 5.5 high driver, GPT-6.1 Sol high peer analysis).
Input: [the deep-research report](research/deep-research-report-storage-2026-10.md) the owner ran in ChatGPT Pro from
[the research prompt](research/deep-research-prompt-storage-2026-10.md). The report is research input, not product
authority (AGENTS.md "Where the ChatGPT material stands"); every claim it marks **[T]** is still unmeasured here.
This document opens no gate, adds no effect and changes no invariant. Owner decisions bind as recorded in HUMAN_TODO
q-47 and q-48 (2026-10-25).

## 1. The verdict in one paragraph

The report agrees with Loomward's ruling (meaning, residency and permission are separate) and with nearly all of the
copy-and-verify design (docs/47): most of its integrity section is **already built or already gated**. Its real
contributions are six **extensions**: (1) a read-only **reclaim planner** for the nearly full `C:`, which is the most
urgent product win and is designed in [52](52-reclaim-planner.md); (2) a **receipt vocabulary** that never says
"durable" for what is only "read back", tied to a **storage-path identity**; (3) the warning that **an archive that
becomes the only copy is not a backup**; (4) a **staged retirement** protocol (re-verify after remount, then Recycle
Bin, never straight delete); (5) a concrete **categoriser cascade** (labelling functions, linear and tree students,
tiny embeddings, then a small model, then hosted) with a model tournament; and (6) a **preview catalogue with
working sets** for the RAW and Lightroom direction. Two report recommendations need care against owner decisions:
its claim that Hyper-V alone is insufficient for copy graduation (the owner chose Hyper-V for G5; section 4.1 keeps
that and stops receipts claiming more than it proves), and its retirement-area step before the Recycle Bin (the owner
chose the Recycle Bin; section 4.4 shows the two compose). No recommendation requires breaking an invariant; two
would if taken literally (raw MFT enumeration and an elevated service, which the report itself rejects).

## 2. Reconciliation

Labels: **DONE** (built; code cited), **PLANNED** (lane, gate or LW item cited), **NEW** (minted in section 7),
**CONFLICT** (with the invariant or owner decision named). "Partial" means part is built. Code citations were
checked against `main` at 40bb8d1 on 2026-10-25 (read, not run).

### 2.1 The report's top ten

| # | Recommendation | Label | Where |
|---|---|---|---|
| 1 | Treat the 1%-free system NVMe as the first incident; read-only reclaim planner over Cargo targets, worktrees, Docker/WSL, package caches, model stores; never delete automatically | **NEW** (extends LW-119) | docs/52; LW-131. `clutter.developer` (LW-119 rules v1) already covers project build outputs and dependency trees with allocated bytes and coverage |
| 2 | Do not graduate copy-and-verify on Hyper-V alone; add real device power removal | **PLANNED, reconciled** | G5 = owner's Hyper-V choice (q-47 Q2); section 4.1: G5 stays Hyper-V, receipts never claim "power-loss qualified" without the physical campaign (LW-133, LW-134) |
| 3 | An always-attached 8 TB disk is not both archive and backup once originals go | **NEW** | section 4.3; LW-136; owner question Q2 (section 9) |
| 4 | Move as a multi-stage protocol: verified copy, later re-verification, retirement, separately confirmed purge | **PLANNED, extended** | docs/48 section 6 already requires a second `verify-tree` after safe removal and replug; the Recycle Bin effect is unbuilt and ungated (invariant 1); LW-135; section 4.4 |
| 5 | Keep non-elevated enumeration; do not chase raw-MFT speed | **DONE** | handle-relative native enumeration (LW-101, measured #111/#118); USN accelerator disabled (`engine/src/scan/watch.rs`); the elevated helper is a separate, gated design (LW-115) |
| 6 | Replace one classifier with a calibrated evidence ensemble | **PLANNED, extended** | docs/49 cascade (I1-I5); LW-138, LW-139; today's student is weighted multinomial naive Bayes with abstention (`loomward-learn/src/student.rs`) |
| 7 | Benchmark Laya and small models against the 27B baseline, for decisions not prose | **NEW** | LW-140; docs/49 I3. Laya is identified by the report as the Convai Innovations decision-model family (docs/49 section 5 asked) |
| 8 | Photo experience: persistent preview catalogue and explicit working sets, not filesystem virtualisation | **NEW** | docs/50 seed; LW-142, LW-143; CfAPI/ProjFS deferred (section 6) |
| 9 | Keep MCP read-only even after file effects mature | **DONE** | invariant 7; `loomward-mcp` tools are `find` and `describe` only (`crates/loomward-mcp/src/tools.rs`); G6; docs/48 section 4.1 |
| 10 | Physical volume identity, health, offline state and evidence freshness as domain objects; never the drive letter | **Partial DONE, NEW** | identity by volume GUID and root file ID (LW-004; `root_grant.volume_guid`); bus type (`volumes.rs`); offline roots planned (docs/48 E0); health counters and sector geometry: LW-137 |

### 2.2 Integrity of copy, move and archive

| Recommendation | Label | Where |
|---|---|---|
| `FlushFileBuffers` is not durability; non-cached read-back proves readability, not media persistence | **DONE** (stated) | docs/47 section 4; `effects.rs` `write_and_flush`, `noncached_hash`; comments in `exec.rs` say "not power loss" |
| Receipt vocabulary: copied, flushed, independently read back, published, power-loss qualified, archive verified at date | **NEW** | today one `items_verified` field (`loomward-service/src/copy.rs`); LW-132; ADR draft R1 |
| `published_unverified` as a named state after publish | **Partial DONE** | docs/47 section 7 outcome `published_unverified` on `changed_after_verification`; the journal state is `Publishing` (`operations/src/journal.rs`); LW-132 names it in receipts |
| Quick removal vs better performance; safe removal is an ordering barrier, not proof | **PLANNED** | docs/48 section 2 ("detach is a destination power loss"); record the removal policy in LW-137 |
| Qualify the actual enclosure path (VID/PID, bridge, firmware, disk, Windows build, removal policy, protocol, geometry) | **NEW** | LW-137 records the tuple; LW-134 attaches campaign evidence to it |
| Keep the 8 TB archive NTFS, not ReFS | **DONE** (agreed) | docs/48 E0 accepts NTFS/ReFS roots; nothing proposes reformatting `H:` |
| Test pyramid: state-machine traces, process death, Hyper-V, USB unplug, real power removal, soak | **Partial** | process death: C6 crash matrix with named points `exec.rs` `enum Point` (test builds only), `evidence/v3/copy-faults.md`; Hyper-V: G5 (LW-133); unplug and power: LW-134; soak: docs/45 bar item 7. 100,000-trace pure fault injection: **NEW**, inside LW-133 |
| Named fault points and injected Win32 errors | **DONE** | docs/47 section 10 `fault-points`; `exec.rs` `Flow::Fail` |
| Robocopy is transport, not verified publication; `/B` and `/COPY:DATSOU` are traps | **DONE** (agreed) | docs/48 section 6 flag allowlist (`/COPY:DAT`, no `/B`, no `/MIR`, `/PURGE`, `/MOV`); robocopy is never wired into Loomward |
| FastCopy and TeraCopy as comparators | **PLANNED** | LW-056 (fair benchmarks against specialist tools) |
| SHA-256 for receipts; BLAKE3 only if measured; XXH3 for candidate bucketing | **DONE / NEW (measure)** | SHA-256 per stream today; LW-144 measures whether hashing ever limits an HDD |
| Canonical Merkle tree manifest with versioned collation | **Partial** | canonical manifest digest exists (`operations/src/manifest.rs`, ADR-V3-04 byte strings); a per-directory tree root is **NEW**, inside LW-136 |
| Scrubbing as an ordinary product concept (`last_content_verified_at`, overdue not failed) | **NEW** | LW-136 |
| PAR2 parity for a cold-archive lane | **Deferred** | section 6; a second copy (LW-136, Q2) comes first |
| ADS preserved and hashed; `Zone.Identifier` survives | **DONE** (generic) | docs/47 section 6: up to 64 named streams, each SHA-256; no `Zone.Identifier`-specific fixture yet (fold into LW-132's fixtures) |
| Sparse: preserve | **CONFLICT-free difference** | Loomward v1 **refuses** sparse, compressed, encrypted and integrity-stream files before any write (docs/47 section 6, `effects.rs` `supported_attributes`). The report's own rule ("refuse if not proven") supports refusal; widening needs fixtures and review |
| EFS refused by default | **DONE** | `effects.rs` + test |
| Hard links: detect by identity, report external links unknown | **DONE** | link counts by `NumberOfLinks` (`identity.rs`); preview counts names with link count > 1; each name copies as an independent file (docs/47 section 6) |
| Junctions and symlinks never followed | **DONE** | `FILE_OPEN_REPARSE_POINT`; reparse points refused as a whole group |
| Case-sensitive directories preflighted | **DONE** | `FileCaseSensitiveInfo` on the destination; `case_collision` refusal |
| Long paths and reserved names | **DONE** | `name_unrepresentable`, `path_too_long` (docs/47 section 7) |
| Cloud placeholders opened no-recall and refused | **DONE** | `FILE_FLAG_OPEN_NO_RECALL`; `cloud_placeholder` refusal |
| Open Lightroom/IDE files fail closed | **DONE** | deny-write holds; `source_in_use` |
| No VSS in the normal path | **DONE** (agreed) | not used |
| Destination-inherited ACLs, reported | **DONE** | docs/47 section 6; H5 |
| Retirement rename into a per-volume area before purge; Recycle Bin not always reversible; human-readable confirmation | **NEW, reconciled** | section 4.4; LW-135; the explanation is docs/49's ML summary (owner, q-48) |

### 2.3 Space truth, enumeration and developer clutter

| Recommendation | Label | Where |
|---|---|---|
| Handle-relative enumeration as primary; Windows Search only auxiliary | **DONE** | LW-101; Everything as optional backend is LW-114 |
| Watcher plus reconciliation; never "no event" = "no change" | **DONE** | `ReadDirectoryChangesW` hints only (`windows/src/watch.rs`); overflow relists (`RootDirty`) |
| Explicit coverage states including `offline`, `watch_overflow_reconcile_required`, `root_identity_changed`, `volume_missing` | **Partial** | `CoverageState` has complete, partial, stale, unscanned, denied, excluded, cancelled, unknown; offline and identity-changed roots are docs/48 E0 (other lane) |
| Three size truths: logical, allocated, reclaimable-for-this-set | **DONE / NEW** | logical and allocated per file (ADR-V3-21, three byte quantities); hard-link-aware "verified relief" exists in placement (`engine/src/placement/mod.rs`); per-clutter-candidate reclaimable bytes: LW-131 |
| WinSxS via DISM, not directory sums | **NEW** (reporting rule) | docs/52 section 3, informational class |
| Docker/WSL VHDX: show container allocation separately; compaction is the tool's | **NEW** | docs/52 virtual-disk class; reclaimable stays unknown |
| Breakdown including "unknown / unaccounted space" | **NEW** | docs/52 section 5 "unaccounted" line |
| Coldness as an evidence vector; "no activity evidence since", not "unused since" | **Partial PLANNED, extended** | LW-028 (explicit usage observation with coverage), LW-029 (heat model); the vector's fields go into the evidence packet (LW-138). Jump Lists, MRUs, Prefetch, SRUM stay out (section 4.6) |
| Developer clutter as a first-class ontology, deletion delegated to the owning tool | **Partial DONE, NEW** | rules v1 (Cargo, npm, Python, .NET, Gradle); machine-level stores (package caches, model stores, VHDX, worktrees): LW-131 rules v2; regeneration cost LW-087; model hosts LW-086; worktree leases LW-076 |
| Prefer environment-variable relocation to junctions | **NEW** (planner output) | docs/52 "relocate" group; the variable is the owner's step (invariant 2) |
| Treemap plus change overlay and evidence drawer; "what changed this week" | **PLANNED** | Atlas (LW-105 done, app views); growth and low-space monitoring LW-121; forecasts LW-060 |

### 2.4 The categoriser

| Recommendation | Label | Where |
|---|---|---|
| Multi-axis labels (meaning, lifecycle, recreatability), never `approved_to_delete` | **NEW** | LW-138 taxonomy v1; invariant 3 already forbids score-as-approval |
| Deterministic evidence packets per folder | **NEW** | LW-138; schema draft lands with this programme (section 8) |
| Four students: hashed n-gram linear, structural GBDT, tiny embedding prototypes, labelling functions | **Partial PLANNED** | embeddings: LW-024; the linear and tree students and labelling functions: LW-139 |
| LLM teacher as one labelling function with measured error | **DONE** (in shape) | teacher labels are a separate data class (`Source::Teacher`, invariant 3); teacher confined until LW-111 canaries pass |
| Calibration on held-out owner labels; selective risk vs coverage | **Partial DONE** | grouped temporal splits, macro-F1, multiclass Brier, top-label ECE, reliability bins, group bootstrap, selective coverage at 0.5/0.8/0.9/0.98 and an isotonic **candidate** already exist (`loomward-learn/src/evaluation.rs`, LW-025 still marked planned). Not done: owner labels in volume, temperature scaling, a promotion gate |
| Evidence-grounded explanations (templates, not prose) | **PLANNED** | docs/49 section 3 ("every summary shows what it rests on"); LW-138 renders facts, LW-140 measures faithfulness |
| Small-model tournament with a promotion bar (97% of 27B macro-F1 on the uncertain subset, faster, 99.5% complete structured output, no unsupported claims) | **NEW** | LW-140 |
| Quantisation sweep Q4 to Q8 | **NEW** | LW-140 |
| llama.cpp / LM Studio as endpoint; ONNX/Windows ML for non-generative students; Foundry Local as an adapter | **PLANNED / NEW** | the Ask provider interface speaks the OpenAI-compatible API (LW-117, docs/46); ONNX for embeddings is LW-116's runtime choice; Foundry Local adapter deferred |
| Benchmark on group and time splits; OOD set; 1,000 owner-reviewed examples before claiming calibration | **Partial DONE, owner-bound** | splits exist; the 1,000 labels are owner time (expensive operation X4) |
| Hosted fallback: literal payload preview, disclosure levels, cache by payload digest | **PLANNED, extended** | docs/49 I4, q-5 scope (names only); Ask records body SHA-256 before send (`ask/src/turn.rs`) but shows no preview; LW-141 |
| Agent may propose, explain and simulate, never approve or mint capability | **DONE** | invariants 1, 3, 7; `plan_archive` returns `owner_approval_required` (docs/48 4.4) |

### 2.5 External media and Lightroom

| Recommendation | Label | Where |
|---|---|---|
| Embedded-preview extraction order (embedded JPEG, LibRaw, WIC, ExifTool, full decode last) | **NEW** | LW-142. LibRaw is LGPL-2.1/CDDL C code: a dependency decision for an AGPL project (legal-ip check before adoption); WIC is in Windows and needs none |
| Two-tier local cache keyed by volume identity and file ID, budgeted, never on `C:` while full | **NEW** | LW-142; docs/50 section 4 |
| Perceptual similarity as discovery only, never deletion proof | **PLANNED** | LW-059 |
| Lightroom: archive-in-place default, working-set copy, Lightroom-managed move; refuse effects on a locked `.lrcat`; RAW+XMP as a sidecar bundle | **NEW** | LW-143 |
| No CfAPI/ProjFS in six months | **Agreed** | section 6; docs/50 section 4 asked; answer: defer |
| Transfer queue with priorities P0-P4 over the verified-copy primitive | **NEW** | LW-143; needs the copy gate (LW-124) for personal data |
| I/O size and queue-depth sweep on the USB HDD | **NEW** | LW-144 |

### 2.6 Agent-safe tooling

| Recommendation | Label | Where |
|---|---|---|
| MCP read-only; native owner UI mints digest-bound, expiring approval | **DONE** | single-use 30-minute approval bound to the manifest digest (`operations/src/manifest.rs`); HTTP refuses personal approvals |
| Revalidation narrows, never expands | **DONE** | docs/47: `source_changed`, `destination_changed`, `digest_mismatch` refusals |
| File names and metadata are hostile input; typed untrusted-evidence payloads; escape bidi and control characters in confirmations | **Partial, NEW tests** | names are data in every projection; a dedicated hostile-name suite across MCP, Ask and the native confirmation: LW-145 |
| Machine-readable blocked reasons from the journal for agents to explain | **Partial DONE** | refusal codes (docs/47 section 7) exist; an MCP `operation_status` read is deferred until copy is personal (G6 holds) |
| Plan preview in MCP, still no execution | **PLANNED** | `plan_archive` (docs/48 E4, other lane) |

### 2.7 Wider risks and opportunities

| Item | Label | Where |
|---|---|---|
| SMART/health may vanish behind USB bridges: unknown, never "healthy" | **NEW** | LW-137 |
| BitLocker To Go for the archive | **Owner** | not Loomward work; flagged in the survey note for the owner (section 9 notes) |
| 512e/4Kn geometry drives every no-buffering alignment | **DONE** | `effects.rs` `physical_sector` via `StorageAccessAlignmentProperty`; recorded per path in LW-137 |
| USB selective suspend and spin-up latency | **NEW** (measure) | LW-144 records wake latency; no OS policy change (invariant 2) |
| The thumbnail cache becomes clutter | **NEW** | LW-142 hard quota, visible accounting |
| Local model proliferation | **PLANNED** | LW-086, docs/52 model-store class |
| Catalogue sync across machines | **PLANNED** | LW-128, LW-092 (no live SQLite in a sync folder) |
| Restore drills over backup receipts | **PLANNED** | G9 proven restore (docs/47 section 10), LW-036; archive drills LW-136 |
| Model download supply chain: source, licence, digest, consent | **PLANNED** | LW-116 (consented download); LW-140 records digests |
| SBOM and licence inventory | **PLANNED** | LW-057 |
| SQLite catalogue recovery evidence | **PLANNED** | ADR-V3-22 (precious vs derived state), state copy scrubbing |
| Storage Spaces, ReFS block cloning, compression | **Deferred** | section 6 |

## 3. Where the report is stale, wrong or overreaching

1. **It underestimates what is built.** The copy engine already has handle-bound staging, per-stream SHA-256,
   sector-aligned non-cached read-back, no-replace rename, deny-write holds, reparse/EFS/sparse/placeholder refusals,
   single-use digest-bound approval and test-only named fault points. The grouped temporal evaluation, Brier, ECE,
   bootstrap and selective-coverage reporting also exist. Its phase "Integrity proof foundation" is mostly G5, G9 and
   receipt wording, not new engineering.
2. **"Preserve sparse" is the wrong default for v1.** Loomward refuses unsupported classes before any write
   (docs/05, docs/47 section 6); expanding a sparse VHDX silently is exactly the failure both documents fear.
3. **Its counts are not Loomward's gates.** G5 needs at least 20 hard power-offs by the owner's method (docs/47
   section 10); the report's 2,000 Hyper-V and 300 physical cuts are a qualification target, not the gate. Section
   4.1 adopts them as a separate, higher label.
4. **It treats the 27B baseline as known.** The owner's exact model, revision and quantisation are not recorded
   (owner question Q4).
5. **Model release facts are unverified here** (Qwen3.5 sizes, Gemma 4 E2B/E4B, LFM2.5, Laya). Treat them as [T]
   until LW-140 reads each model card and records the digest of what it downloads.
6. **It does not know the Recycle Bin trap.** Recycling originals on `C:` frees nothing on `C:` until the bin is
   emptied, which matters for the owner's chosen effect on a full disk (docs/52 section 4).
7. **`fileciteturn…` and `citeturn…` markers do not resolve.** Primary sources must be re-read before any claim
   reaches a doc as fact.

## 4. Extensions to the plan

### 4.1 Power-loss: keep G5 on Hyper-V, add a qualification ladder

The owner chose Hyper-V for G5 (q-47 Q2). That stands: a Hyper-V `TurnOff` loses the guest's caches and proves the
journal, staging and publication invariants under sudden guest death. What it cannot prove is that a USB bridge or
HDD honours a flush when its own power goes. So receipts get a ladder (ADR draft R1):

| Label on a receipt | Evidence required |
|---|---|
| read back | the run's own non-cached read-back passed |
| crash-qualified (copy engine) | C6 matrix and G5: Hyper-V hard-offs at every crash point plus random times, zero invariant violations (LW-133; G5's floor is 20, the target 2,000) |
| power-loss qualified (this storage path) | LW-134 physical campaign on disposable media behind the **same** bridge and firmware, attached to the storage-path tuple of LW-137 |
| archive verified at *date* | a later independent scrub matched the manifest (LW-136) |

C8 needs only G5 as defined. Without LW-134, no receipt ever says "power-loss qualified", and the UI says
"not power-loss tested on this disk" for copies to a USB path.

### 4.2 Re-verification before any source leaves

docs/48 section 6 already requires a second equal `verify-tree` after the owner safely removed and replugged the
disk. This programme keeps that rule for every retirement (LW-135) and adds: the second verification must be at
least one remount later **and** after the copy's receipt, it reads every stream non-cached, and its result expires
(a source change, a destination identity change or 14 days invalidates it).

### 4.3 Archive is not backup

Once an original leaves `C:`, `E:` or `G:`, the copy on `H:` is the primary and only copy. An always-attached disk
shares the host's failures, theft and ransomware reach. So:

- `plan_archive` (docs/48 4.4, other lane) and the retirement review (LW-135) say **"after this, H: holds the only
  copy"** whenever the catalogue knows no second copy (proposed to the external-disk lane as an issue).
- LW-136 tracks copy count per archived object, the date of its last full content verification, and a rolling scrub
  budget (an 8 TB disk at 120-180 MB/s needs 12-19 hours for one full pass, so scrub a bounded slice per idle week).
- Owner question Q2 asks about a second, rotated offline disk.

### 4.4 Staged retirement, composed with the owner's Recycle Bin choice

The owner chose: originals go to the Recycle Bin after per-plan confirmation, with a human-readable ML summary
(q-48). The report's retirement area and that choice compose as one protocol (LW-135, ADR draft R2):

1. verified copy (copy gate, or the interim robocopy path plus `verify-tree`);
2. re-verification after remount (4.2);
3. the review: per group, the docs/49 summary (what it is, size, age, the verified copy's location and last
   verification, hard links outside the selection that will not free space, "H: will hold the only copy");
4. on confirmation: Recycle Bin through the Shell (`IFileOperation` with recycle and undo flags) **only where the
   Shell will recycle**; an item the Shell would delete permanently (too large for the bin, or a volume without one)
   stops at `requires_permanent_delete_confirmation` and is never deleted by this effect;
5. the review says plainly that recycling on `C:` frees no space on `C:` until the bin is emptied, and shows the bin.

The report's rename-into-a-retirement-area is not adopted for v1: the Recycle Bin is already the owner's undo area,
and a second hidden area on a full disk would cost space and add a delete path. Reversal: add the area if the bin's
size limit or Shell behaviour makes recycling unreliable in LW-135's tests.

### 4.5 The coldness evidence vector

Coldness becomes evidence fields in the packet (LW-138), never one timestamp: last content change, last metadata
change, last Git commit and dirty state (granted project roots only), last build regeneration (newest output vs newest
source), coverage and confidence. NTFS last-access time is reported as "system-managed, not evidence". The UI says
"no activity evidence seen since March", never "unused since March".

### 4.6 Privacy budget for activity evidence

Jump Lists, Recent, application MRUs, Prefetch and SRUM stay **out** (invariant 2 and the owner's posture). Revisit
only through an explicit owner decision; none is posed now.

### 4.7 The developer-clutter ontology

Rules version 2 (docs/52 section 3) extends v1 from project outputs to machine-level stores, each tied to its owning
tool's documented command. Loomward never runs those commands: it names them.

### 4.8 The categoriser cascade and benchmark

docs/49's four stages stand. This programme fills them in:

- **Stage 1 facts and labelling functions** (LW-138): the evidence packet; rules vote or abstain (for example
  `Cargo.toml` + dominant `target/` votes `generated_build`; `.lrcat` + dated `Backups/` votes `catalogue_backup`).
- **Stage 2 students** (LW-139, LW-024): hashed path n-gram linear model and a structural tree model over packet
  numbers, combined with the labelling-function votes by a weak-supervision label model; tiny embedding prototypes
  (BGE-small/E5-small class, about 33M parameters, 384 dimensions) from LW-024. Every output can abstain.
- **Stage 3 small model** (LW-140): only on abstention or for the one-line summary; Laya for typed decisions,
  a 0.8-4B generative model for the sentence.
- **Stage 4 hosted** (LW-141): q-5's preconditions, a literal payload preview, disclosure levels never escalated
  silently, a cache keyed by the digest of provider, model, prompt version, schema and disclosed payload.

Benchmark rules (LW-140): group and time splits as `evaluation.rs` does; an OOD set of new projects and shoots;
metrics as the report lists; the promotion bar in section 2.4; no vendor benchmark substitutes for the owner corpus.

### 4.9 The preview catalogue and working sets

Browse from local derivatives, touch originals only for original bytes (LW-142, LW-143). Acceptance target: once
previews are cached, browsing a shoot performs **zero** reads on the external disk.

### 4.10 Volume health and identity as domain objects

LW-137: a `StoragePath` record per volume (volume GUID, disk model and firmware, serial kept out of Git, bus type,
USB VID/PID and bridge where discoverable, BOT/UAS, logical and physical sector size, removal policy, write-cache
state, Windows build) and a `HealthObservation` (temperature, errors, power-on hours, wear, start/stop counts) where
`MSFT_StorageReliabilityCounter` reports them; an empty result is `unsupported_or_hidden`, never healthy. The
drive letter is presentation only. `crates/loomward-windows/src/volumes.rs` is owned by the external-disk lane this
wave, so LW-137 starts after E0 lands.

## 5. The six-month programme (November 2026 to April 2027)

Cheap and read-only first. Phases overlap; each has an acceptance test.

| Phase | Weeks | Work | Acceptance | Cost class |
|---|---|---|---|---|
| **P0 Foundations on paper** | 0-2 | this doc and docs/52; evidence-packet schema v1 and synthetic packets; benchmark corpus format; Hyper-V campaign scripts in dry-run; receipt vocabulary ADR | merged with proving checks; schema validator tests; scripts' dry-run self-test | cheap |
| **P1 Reclaim and integrity proof** | 1-8 | LW-131 reclaim planner R1b-R1d; LW-132 receipt vocabulary; LW-133 Hyper-V campaign (G5) and G9 proven restore with the copy lanes | reclaim plan on synthetic fixtures exact, decoys excluded, unknowns never zero; tool-agreement within 2% on the owner's machine (R1e); G5: at least 20 clean hard-offs across every crash point, target 2,000; no false verified receipt | cheap code; **X1** VM campaign |
| **P2 Meaning v2** | 4-12 | LW-138 packets, taxonomy and coldness vector; LW-139 students and labelling functions; LW-024 embeddings | grouped temporal split; macro-F1 and selective risk at coverage on synthetic plus owner labels; explanation facts trace to catalogue rows; abstention visible | cheap code; **X4** owner labels |
| **P3 Tiny-model tournament** | 8-14 | LW-140 runner and matrix | the promotion bar of 2.4 on the uncertain subset, or no promotion; every claim in a summary traceable to a packet fact | **X3** downloads |
| **P4 Owner-safe retirement and custody** | 10-20 | LW-135 Recycle Bin effect gate; LW-136 copy count, scrub, restore drills; LW-137 storage path and health | LW-135's own gate (fixtures, crash points, second lens, owner sign-off); a restore drill on `H:` lab data; health shows unknown honestly behind the WD bridge | effect: gated; **X6** scrub time |
| **P5 Media atlas and working sets** | 14-26 | LW-142 previews and cache; LW-143 working sets and transfer queue; LW-144 I/O sweep; LW-059 similarity | 100k-item cached library scrolls with zero external reads; cold 1,000-RAW import benchmark recorded; unplug and resume; no full decode when an adequate embedded preview exists | **X7**, **X8** |
| **Throughout** | | LW-145 hostile-name suite | no MCP or Ask path reaches a write; bidi and control characters escaped in the native confirmation | cheap |

Not scheduled in this window: CfAPI/ProjFS placeholders, Storage Spaces, ReFS migration, PAR2, live multi-machine
SQLite, automatic deletion of anything, an elevated product service.

## 6. Deferred, with the reason

| Item | Why not now | Revisit when |
|---|---|---|
| CfAPI / ProjFS placeholders over the archive | a filesystem-availability component with hydration, antivirus and unplug traps; ProjFS assumes a fast store | working sets prove that path-transparent hydration is actually needed |
| PAR2 parity | repair redundancy, not a backup | after a second copy exists |
| Storage Spaces, ReFS block cloning | one archive disk; no redundancy to repair from | multiple suitable fixed disks |
| Raw MFT / USN enumeration in the product | crosses the elevation boundary (invariant 2) | only through LW-115's separate gate |
| Foundry Local adapter | the OpenAI-compatible endpoint already covers LM Studio | if the owner adopts Foundry Local |
| A custom storage minifilter for crash ordering | driver engineering | a remaining ambiguity blocks release after LW-133 and LW-134 |

## 7. New backlog items (minted in `backlog/issues.json`, mirrored as GitHub issues)

| LW | Title | Phase | Depends on |
|---|---|---|---|
| LW-131 | Read-only reclaim planner for a nearly full volume | P1 | LW-119, LW-108, LW-009 |
| LW-132 | Truthful receipt vocabulary and storage-path qualification | P1 | LW-124, LW-040 |
| LW-133 | Hyper-V crash campaign for the copy gate (G5) | P1 | LW-041, LW-040 |
| LW-134 | Physical power-loss and unplug qualification on disposable media | P4 | LW-133, LW-137 |
| LW-135 | Gate the Recycle Bin retirement effect for verified archive originals | P4 | LW-124, LW-037, LW-138 |
| LW-136 | Archive custody: copy count, rolling scrub and restore drills | P4 | LW-124, LW-036, LW-084 |
| LW-137 | Storage-path identity and volume health as domain objects | P4 | LW-004 |
| LW-138 | Evidence packets, multi-axis taxonomy and the coldness vector | P2 | LW-110, LW-025 |
| LW-139 | Classical student ensemble with labelling functions | P2 | LW-138, LW-025 |
| LW-140 | Small-model tournament for categorisation and summaries | P3 | LW-138, LW-117 |
| LW-141 | Hosted categoriser fallback with a literal payload preview | P3 | LW-117, LW-111, LW-140 |
| LW-142 | RAW preview catalogue with a budgeted local cache | P5 | LW-099, LW-015 |
| LW-143 | Photo working sets and a prioritised transfer queue | P5 | LW-124, LW-142 |
| LW-144 | Measure hashing and I/O costs per storage class | P5 | LW-064 |
| LW-145 | Hostile names and metadata cannot create effects | P0-P5 | LW-118, LW-066 |

Bodies: `backlog/issues/LW-131.md` to `LW-145.md`.

## 8. Cheap advances shipped with this programme

| PR | Delivers | Effect |
|---|---|---|
| this one | docs/51, docs/52, LW-131 to LW-145 | none |
| evidence-packet schema | `schemas/evidence-packet-v1.json`, synthetic packets, validator test | none |
| reclaim fixtures | synthetic `fixtures/reclaim/` trees and their expected candidates (docs/52 section 7) | none |
| Hyper-V campaign skeleton | host and guest scripts with a dry-run self-test; nothing creates a VM | none |

## 9. Owner questions (batched; at most five)

1. **Q1 Hyper-V lab setup** (unblocks X1). The campaign needs a Windows guest image (about 20 GB download or an ISO
   you already have) and Hyper-V cmdlets, which need membership of "Hyper-V Administrators" or an elevated shell once.
   Recommended: you add your account to that group; agents build the VM on `E:` and run the campaign.
2. **Q2 A second copy** (decides 4.3). Once originals leave the internal disks, `H:` holds the only copy.
   Options: (a) a second disk rotated offline (recommended); (b) cold cloud storage with a per-upload preview;
   (c) accept a single copy for now, with a "sole copy" warning in every plan.
3. **Q3 Physical power-loss hardware** (decides LW-134). (a) Not now: copies to USB disks say "not power-loss tested on
   this disk" (recommended for now); (b) buy a disposable disk in the same WD enclosure model and a programmable USB
   power switch.
4. **Q4 Model tournament** (unblocks X3). Approve about 20-25 GB of model downloads to `E:` through LM Studio, and say
   which 27B model, revision and quantisation you run today and which GPU and VRAM the desktop has. Recommended:
   approve.
5. **Q5 Labelling time** (unblocks X4). Calibration claims need about 1,000 of your labels over a few sessions,
   using a draft taxonomy agents prepare from the categories you care about. Recommended: yes, 20-30 minutes a session.

## 10. Expensive operations waiting for the owner's go-ahead

Ordered by value per cost. None starts without a go-ahead.

| # | Operation | Disk | RAM | Wall time | Owner actions | Ready? |
|---|---|---|---|---|---|---|
| X1 | Hyper-V crash campaign (G5) | VM VHDX about 40 GB plus image about 20 GB, on `E:` | 4-8 GB while running | VM build 1-2 h; 20 hard-offs about 1 h; 2,000 about 3-4 days unattended | Q1 | after the campaign skeleton PR and C6; ready in P1 |
| X2 | Disk survey scans of `C:`, `E:`, `G:`, `H:` (owner asked to survey first) | state database 1-4 GB on `E:`, never `C:` | 1-3 GB | `C:` 10-30 min, `H:` over USB longer | native grants for each root (q-7); owned by the external-disk lane after E0 | after E0 lands |
| X3 | Model downloads for the tournament | 20-25 GB on `E:` | up to 6-8 GB per model loaded | downloads 1-2 h; runs a few hours per model | Q4 | after LW-140's runner |
| X4 | Owner labelling corpus (about 1,000 labels) | negligible | negligible | 5-8 h of owner time over weeks | Q5 | after LW-138 |
| X5 | Reclaim tool-agreement run (read-only tool commands) | none | small | under 1 h | none beyond the grants of X2 | after LW-131 R1b-R1c |
| X6 | Rolling scrub of `H:` archive data | none | small | 12-19 h for a full pass; sliced weekly | none (read-only), but the disk spins | after LW-136 |
| X7 | RAW corpus and preview cache benchmark | cache 10-50 GB on `E:` or `G:` | 1-2 GB | hours | point at your own RAW folders (real data stays out of Git) | after LW-142 |
| X8 | Transfer and hash sweep (LW-144) | 32-128 GiB generated corpus per storage class (not `C:`); up to 300 GB on `H:` for the transfer matrix | small | a day of disk time | none beyond lab folders | after LW-144 |
| X9 | Physical power-cut and unplug campaign | a disposable disk | small | days | Q3 (b): buy hardware | after LW-133, LW-137 |
| X10 | 24-72 h soak (docs/45 item 7) | lab | moderate | 3 days | none | with P1 |

## 11. ADR drafts

These are drafts for review; each becomes an ADR in docs/42's series when its LW item lands.

- **R1 Receipt vocabulary.** A receipt states the highest label its evidence supports (4.1) and never the word
  "durable". Consequence: the copy view, the CLI and agents show the same ladder; a USB destination without LW-134
  evidence says "not power-loss tested on this disk".
- **R2 Staged retirement.** Retirement is a separate effect after copy, with remount re-verification, a per-plan
  ML-summarised review, Shell recycling only where the Shell recycles, and no permanent delete path in v1 (4.4).
- **R3 Archive custody.** Archived objects carry copy count, manifest root and `last_content_verified_at`; a plan
  that would leave one copy says so; scrubs are bounded and overdue is not failure (4.3).
- **R4 Categoriser cascade.** Facts and labelling functions, then classical students, then a small model only on
  abstention or for the sentence, then hosted under q-5; labels are multi-axis and never approval (4.8).
- **R5 Preview catalogue.** Derivatives keyed by volume identity and file ID, budgeted, never on a volume under
  pressure; originals touched only on request; no placeholders (4.9, section 6).
- **R6 Storage path.** A storage path is the tuple of 4.10; qualification evidence attaches to the tuple, and a
  different enclosure invalidates it.

## 12. How this lane works

The Opus driver owns judgement, synthesis, review and merges under the T2 gate; GPT-6.1 Sol high gives an independent
analysis and takes decision-complete implementation slices; Haiku workers do extraction, inventory and named checks.
Files owned by the external-disk lane this wave (`crates/loomward-mcp`, parts of `loomward-service`, `loomward-cli`,
`loomward-windows/src/volumes.rs`, the Agent access view, `native/` bundling, docs/48) are changed only through
issues. Status files (`docs/16`, `handoff/CHECKPOINT.json`) are left to the next checkpoint PR.
