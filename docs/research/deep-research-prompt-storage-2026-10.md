# Deep-research prompt: storage order, safe archiving, categorisation and external media (2026-10)

Written 2026-10-25 for a ChatGPT Pro deep-research run requested by the owner. Paste everything below the line.
Bring the report back into `docs/research/` and the backlog. The report counts as research input, not as product
authority (AGENTS.md "Where the ChatGPT material stands").

---

You are a principal engineer and research lead. Produce an exhaustive, source-cited deep-research report for
**Loomward**, an open-source (AGPL-3.0), local-first Windows 11 workspace, storage and resource companion. It has a
Rust native engine, a Tauri 2 desktop shell with a Svelte UI, a SQLite catalogue, and an in-process service with
typed commands. Be ambitious. Go deep on each area below, then go wide on what I have not thought of. Prefer primary
sources: Microsoft Learn and Windows SDK headers, file-system and storage specifications, vendor documentation,
source code of comparable tools, papers, and reproducible benchmarks. Date every claim that can go stale. Mark what
is measured, what is documented and what is your inference.

## Who it is for and what exists

- One power user on a Windows 11 Pro desktop with 32 GB RAM:
  - a 2 TB NVMe system disk, about 1% free;
  - a 1 TB NVMe;
  - a 2 TB SATA HDD;
  - a newly attached 8 TB USB 3 external HDD, NTFS, that serves as backup and archive and stays attached most of
    the time.
  - A second, lighter laptop runs Windows 11 Home.
- The user runs AI coding agents (Claude Code, Codex and others) all day. The agents produce huge build caches.
  Local LLMs run through LM Studio; the main model today is Qwen 27B.
- Loomward already has:
  - a native NTFS metadata scanner and catalogue, designed to beat WinDirStat-class tools;
  - treemap, breakdown and developer-clutter views;
  - duplicate detection by local hashing;
  - a small learned "student" model trained on the owner's own labels, with an optional LLM teacher used as weak
    supervision;
  - per-client disclosure grants;
  - a read-only MCP surface (in progress);
  - copy-and-verify, its first file effect. It copies with handles into new staging, reads back without caching,
    hashes every stream with SHA-256, publishes with an atomic no-replace rename, and keeps a write-ahead journal
    with recovery. It is lab-only until real power-cut proofs pass on a Hyper-V VM.
- Core ruling: **meaning, residency and permission are separate systems.**
- Product invariants:
  - no silent reach: no automatic cloud calls, no whole-disk scans without the owner's pick, no elevation;
  - honest unknowns;
  - a learned score is never an approval;
  - MCP is read-only;
  - no delete of owner data exists until its own safety gate opens, and then only with the owner's per-plan
    confirmation.

## Part A — Integrity of copy, move and archive on Windows (go very deep)

1. **Durability.** What exactly makes a write durable on NTFS and ReFS, on USB 3 mass storage (BOT and UAS) and on
   NVMe and SATA?
   - Compare `FlushFileBuffers`, `FILE_FLAG_WRITE_THROUGH` and `FILE_FLAG_NO_BUFFERING`. Cover FUA, the device
     write cache, and Windows' "quick removal" versus "better performance" policies.
   - What does a safe removal guarantee, and what does a surprise removal lose?
   - How do USB bridge chipsets behave on cache flushes? Collect known lies and quirks, with sources.
2. **Power-loss testing on Windows.** Cover Hyper-V hard stop (Turn Off) versus host crash, and VHDX write-cache
   semantics. Ask whether a VM power-off actually loses the guest's unflushed data, given the host cache.
   - Survey fault-injection tools, including Windows equivalents of dm-flakey, dm-log-writes and CrashMonkey. Cover
     disk filter drivers, the Driver Verifier fault injector, and ALICE/BOB-style application crash-consistency
     checkers.
   - Design the strongest practical harness for a single developer, with cycle counts and oracles.
3. **Verified-copy prior art.**
   - Compare robocopy (exact flag semantics, exit codes, `/J`, `/COPY:DATSOU`, `/DCOPY`, the backup-mode
     pitfalls), FastCopy, TeraCopy, rclone, rsync on Windows and Explorer's copy engine (`CopyFile2`,
     `COPYFILE2_EXTENDED_PARAMETERS`, offloaded data transfer).
   - Which ones verify after a non-cached read, and which only re-hash from cache?
4. **Hashing and integrity.**
   - Compare SHA-256, BLAKE3 and xxh3 for throughput on USB HDDs versus CPU cost, and say when the choice matters.
   - Cover Merkle manifests for large trees and incremental re-verification.
   - Cover periodic scrubbing, bit-rot detection and PAR2 or erasure-coded parity for cold archives.
   - Cover ReFS integrity streams on an external disk: are they worth it?
5. **File-system edge cases** a mover must handle or refuse:
   - alternate data streams (keep the mark of the web), sparse files, EFS, compression, hard links, junctions and
     symlinks;
   - case-sensitive directories, long paths over 260 characters, and reserved names;
   - cloud placeholders (OneDrive, Cloud Files), files open in Lightroom or an IDE, volume shadow copies;
   - ACL inheritance when copying to an external disk.
6. **Moves and deletion.** How should "move" be built safely?
   - Compare a copy, re-verify and then Recycle Bin flow with alternatives. Cover Recycle Bin semantics per volume,
     `IFileOperation` flags, and what happens with files above the bin's size limit.
   - Cover undo windows and two-phase "tombstone" approaches.
   - Explain how to show the user what they are about to remove in human terms.

## Part B — Where is the space going, fast and correctly

1. **Enumeration.** Fastest correct enumeration on NTFS:
   - raw MFT reading (as WizTree and Everything do) versus `FSCTL_ENUM_USN_DATA` versus `NtQueryDirectoryFile`;
   - the elevation needs of each, and how to stay non-elevated;
   - incremental updates through the USN change journal, and its failure modes (journal wrap, deletion,
     disconnected volumes).
2. **Size truth.** Logical size versus allocated size, compression, sparse files, hard-link double counting, and
   system-protected areas (`pagefile`, `hiberfil`, System Volume Information, WinSxS, the Windows Search index,
   Docker/WSL VHDX growth).
3. **"Cold" detection without NTFS last-access times** (often disabled). Candidate signals: Prefetch, SRUM, Jump
   Lists/Recent, the Windows Search index, application MRUs, git activity and build timestamps. Cover the privacy
   implications of each.
4. **Developer clutter at scale.** Cargo/target, node_modules, pnpm and npm caches, NuGet, pip/uv, Gradle, Docker,
   WSL, Hugging Face and model caches, LM Studio models, IDE caches, agent worktrees.
   - Safe reclaim rules and tool-specific "clean" commands.
   - Relocation via environment variables versus junctions, and the risks of each.
5. **Visualisation.** Treemaps (squarified, cushion), sunbursts, timelines and "what changed this week" views.
   Which works best for multi-terabyte, multi-volume understanding at a glance?

## Part C — The categoriser: machine learning first, LLM second (high priority)

The owner wants a cheap, fast categoriser that explains, at a glance and in human-readable form, what a folder or
file set is: "Lightroom catalogue backups from 2023", "abandoned Rust experiment with 40 GB build output",
"duplicate camera imports". Explanations must go beyond file names.

1. **Classical ML first.**
   - Features: path tokens, extensions, magic bytes, size and time distributions, sibling and structure signatures,
     project markers (`Cargo.toml`, `.git`, `package.json`, `.lrcat`), EXIF and XMP fields.
   - Models: gradient-boosted trees, linear models on hashed n-grams, small embedding models (e5, bge, gte families
     at the smallest sizes) with nearest-neighbour labelling.
   - Weak supervision (Snorkel-style labelling functions, with the LLM as one noisy source), active learning,
     calibration, and per-user personalisation from very few labels.
2. **Small LLMs and SLMs as the second stage, only where ML is unsure.**
   - Survey the best small models available on Windows as of 2026 for structured classification and short
     summaries: Qwen small variants, Gemma, Phi, SmolLM, Liquid LFM-class and others.
   - The owner mentioned a model they called "Laya". Identify what that is, if it exists, and evaluate it.
   - Compare quality per GB and per millisecond against a 27B baseline. Cover quantisation (GGUF Q4 to Q8), runtimes
     (llama.cpp, LM Studio, ONNX Runtime with DirectML, NPUs), structured output and grammars, and batching for
     throughput.
   - Propose a benchmark: the dataset shape, the labels and the metrics.
3. **Hosted fallback.** A hosted model may be the fallback, with a per-request preview of exactly what is sent.
   Cover minimising disclosure (names only, redaction, aggregation) and caching.
4. **Agent orchestration.** Cover cost and latency budgets, and how to put an agent such as Hermes in front of this
   without it becoming a permission system.

## Part D — External-disk media workflow: RAW photos with Lightroom (seed a big design)

Goal: browsing and choosing files on the external disk is instant, clear and pleasant, and the only waiting is the
transfer itself. Example: browse years of RAW photos on the 8 TB disk, pick the shoots to edit, and Lightroom
Classic gets them locally while a background transfer job runs.

1. **Previews.** Fast extraction of embedded JPEG previews and thumbnails from RAW files (CR3, NEF, ARW, RAF, DNG)
   with LibRaw, exiftool and the Windows Imaging Component and codecs, without full decodes. Cover a persistent
   local thumbnail and preview cache with an eviction policy, and perceptual hashes for near-duplicate shoots.
2. **Lightroom Classic integration.** Catalogues, smart previews (DNG proxies), previews.lrdata, folder relinking,
   "move to external" patterns, XMP sidecars, catalogue-safe moves.
   - What can an external tool do safely, and what must go through Lightroom?
3. **Placeholders and hydration.** Could the Windows Cloud Files API (cfapi) or the Projected File System (ProjFS)
   present archived content in place as placeholders that hydrate from the external disk on demand?
   - Cover its pitfalls with Lightroom and antivirus, and offline behaviour when the disk is unplugged.
   - Compare it with junction-based or library-based approaches.
4. **Transfers.** A transfer queue design: prioritisation, throughput tuning for USB 3 HDDs (queue depth, I/O size,
   sequential ordering by on-disk extent), verification, resume after unplug, and notifications.
5. **Prior art.** Photo Mechanic ingest, Lightroom smart previews, Capture One sessions, digiKam, Apple Photos and
   iCloud optimise-storage, Synology Photos, and DAM tools. What makes them fast?

## Part E — Agent-safe storage tooling

1. **MCP state of the art (2026).** Tool design for file operations; the plan / approve / execute split;
   capability-scoped grants; read-only projections; audit logs.
   - Prompt injection through file names and metadata, and mitigations.
2. **Comparable tools.** How other agent frameworks expose file management safely, and documented failures (agents
   deleting or overwriting data).
3. **Journalled, resumable operations.** Patterns an agent can drive while a human keeps the irreversible decisions.

## Part F — Go wide: what have we not thought of?

Find at least 15 non-obvious opportunities or risks, each ranked by value and cost. Examples of the kind wanted:

- the conflict between "archive" and "backup" on one external disk (3-2-1, a second copy, cloud cold storage);
- SMART and health monitoring of the USB disk, and spin-down and USB selective-suspend side effects;
- BitLocker To Go;
- ReFS block cloning, NTFS compression or Compact OS for cold internal data;
- Storage Spaces tiering;
- deduplication of photos and videos;
- energy and noise;
- disk-letter drift;
- multi-machine catalogues (desktop plus laptop);
- licence-compatible libraries.

## Deliverable format

1. An executive summary: the top 10 recommendations, each with expected impact.
2. For each Part: the findings with citations, a comparison table, a recommended design, and the risks.
3. A proposed overall architecture for the next six months, phased, each phase with acceptance tests.
4. An experiment plan: the measurements to run on this exact hardware class, with the commands or code sketches.
5. An annotated bibliography with the dates of the sources.
6. Open questions only the owner can answer.
