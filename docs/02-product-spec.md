# Product specification: a learned workspace companion

Status: design target, not a declaration of implemented capabilities. Read `16-implementation-status.md` for the actual build. The requested ambition is preserved; delivery is staged by evidence and risk rather than by screen count.

## Outcome
Loomward helps a Windows user understand their digital workspace, maintain a useful organisation, keep appropriate working data on appropriate storage, and control resource-intensive work. It should learn personal preferences without making the user surrender the ability to explain, preview, veto, recover or stop a change.

The organising unit is not always a file. A software repository, photo shoot, CAD assembly, game installation, model bundle, virtual machine, music project or export batch may have internal dependencies. A sophisticated organiser understands these as groups before it proposes physical placement.

Success is not “more files moved” or “more RAM freed.” Success is quicker retrieval, fewer unwanted interruptions, fewer storage emergencies, acceptable foreground responsiveness, reliable recovery and a declining need to correct repeated classification mistakes. Inactivity, size and unfamiliarity are not proofs of worthlessness.

## Product boundaries
The persistent catalogue is a map, not a replacement filesystem. Windows remains responsible for memory management and storage integrity. Explorer remains the default shell. Backup providers remain responsible for their established backup formats. The optional LLM supplies bounded advisory results, not commands. A separately reviewed execution layer must own all future effects.

The first public-quality wedge should be **an excellent read-only storage explorer with a teachable organising desk**. This can be useful before the most difficult features exist. Desktop replacement, arbitrary process control, transparent storage virtualization and autonomous cleanup are not prerequisites for finding product value.

## Six product areas
| Area | Core user question | Intended mature capability | First safe increment |
|---|---|---|---|
| Observe | What exists, and how reliable is this picture? | Incremental multi-volume index, physical/logical accounting, growth attribution | Explicit-root metadata snapshot and coverage |
| Understand | What belongs together? | Personal collections, groups, semantic search and explanations | Supervised virtual-label suggestions |
| Place | Which device should hold it? | Learned access forecasts, constrained bidirectional tier placement | Non-executable capacity simulation |
| Protect | Can I recover it? | Verified independent backups, restore drills and operation undo | Explicit unknown/unconfigured states |
| Present | What do I need on my desktop now? | Virtual workspace views, shortcut health and context-aware layouts | A separate companion dashboard |
| Govern | What is making the machine slow? | Process trees, workload groups, reversible policy controls | Read-only process/memory telemetry |

## Principal journeys
**First run.** The app opens without elevation and without scanning the entire machine. The user chooses demo or one folder. The scope screen distinguishes metadata, optional content reads, usage observation and external/model requests. A small root is recommended by the UI as a test scope, but the app does not choose a personal folder silently.

**Understand a full disk.** The user sees total capacity, OS-reported available space, indexed logical bytes, known allocated bytes and unknown/unattributed space as separate numbers. A partial scan never claims to explain all used volume space. Growth is a comparison between compatible index generations, not a subtraction of unrelated scans.

**Teach organisation.** A recommendation card says which virtual collection fits, what metadata informed the suggestion, whether the model is abstaining and which alternatives exist. “Label Finance” changes application state only. “Move into the Finance folder” is a separate future operation with destination and dependency checks. A rejection can mean wrong label, wrong timing, keep current structure, or insufficient evidence; these must not become the same training signal.

**Make room.** The user supplies a target such as “keep 90 GiB available.” The app offers alternative plans, including the no-change plan. Each shows predicted benefit, copy cost, source-retention delay, recall cost, required devices, backup conditions and residual shortfall. It never hides infeasibility by violating a pin or moving active work.

**Restore working material.** The mature app can stage a pinned working set onto a faster device before an explicit work session. “Archive” is not permanent disappearance. Recall is cancelled or deferred when the necessary device is absent. Shortcuts and logical references must continue to make sense.

**Investigate pressure.** The process view separates available physical memory, resident working sets, private commit, CPU utilization and sample freshness. It explains the difference between reducing background work and merely pushing pages out of RAM. A future action card identifies the exact process instance, expected effect, reversible settings and risks.

## Requirements and release conditions
- Every estimate carries units, freshness and evidence quality. Unknown is a supported value, not zero.
- Every learning event has item identity, revision, provenance and optional retraction. Human corrections outrank weak teacher labels.
- Every future mutation is a typed operation with a deterministic eligibility decision, exact plan digest, fresh preconditions, bounded grant and recovery record.
- The app must not infer approval from silence, a previous unrelated approval, a model score, accepting a label, or selecting a disk.
- No destructive “optimise all” button. Batch approval shows the concrete changes and their aggregate impact.
- Source files remain ordinary Windows files. Virtual organisation must remain useful without physical moves.
- Content processing is optional, bounded and isolated. Archives, office documents, media codecs and thumbnails are untrusted input surfaces.
- Existing development workflows are respected. Do not move repository internals, package installations, live databases, virtual disks, mapped model files or application-managed stores without a dedicated provider.
- Resource policies must also constrain Loomward itself. Continuous LLM residency is not required for the core product.
- Offline/removable devices have explicit unavailable states. The UI must not call them empty or deleted.
- Exported diagnostics, feedback and snapshots may contain filenames or project names. They are private by default.

## Non-goals for the first release
No kernel driver, filesystem filter, global shell replacement, cloud account service, antivirus engine, registry cleaner, guaranteed app leak repair, automatic permanent deletion, live application relocation, duplicate hard-link replacement, or self-modifying security policy. A later proposal must justify each additional privilege and permanent background component.

## Definition of a useful prototype
A user can launch it, scan a deliberately selected folder, inspect honest totals, explicitly test duplicates, save labels, observe that the student changes, simulate a constrained disk plan and inspect processes without any mutation controls. The repository must include reproducible tests, documented limits and a continuation plan. That is the scope of the current reference implementation; Windows-native execution remains a later gate.
