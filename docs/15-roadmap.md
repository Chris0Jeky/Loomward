# Roadmap and acceptance milestones

This roadmap separates a broad product vision from executable increments. Effort bands are planning estimates for an experienced developer with AI assistance, not commitments. Native API debugging, metadata fidelity, reliability and field feedback can dominate implementation time.

| Milestone | Deliverable | Exit gate | Rough effort after handoff |
|---|---|---|---|
| M0: reference workbench | Runnable Python/UI, models, simulation, docs and backlog | Current recorded tests; source archive | This package |
| M1: Windows observation alpha | Native compile, stable identity adapter, local catalogue, coverage-aware scan/watch, integrated UI | Windows fixture tests and measured performance | Several focused weeks |
| M2: teachable organisation | Virtual collections, feedback lifecycle, evaluated richer student, optional teacher review | Human-labelled holdout and no capability escalation | Several further weeks; can overlap M1 |
| M3: protection and copy-only | Backup adapter, fidelity/restore evidence, journalled copying on disposable data | Crash/recovery and restore matrix | Substantial reliability work |
| M4: approved placement | Exact consent, safe supported-file moves, limited bidirectional working-set staging | Native safety gate, field shadowing, independent review | Not scheduled before M3 evidence |
| M5: desktop/application providers | Shortcut health, sessions, app inventory, provider-specific cleanup advice | Shell/provider compatibility and accessible UX | Incremental releases |
| M6: resource controls | Own-worker budgets then reversible process policies | Exact-instance/rights/rollback tests | Independent gated workstream |

## Critical path
1. Run the reference and verify its evidence locally before changing architecture.
2. Compile and format the Rust foundation; establish actual Windows CI. Fix draft-source errors rather than disabling tests.
3. Build native object identity, volume capability and coverage adapters. Until then, the system is an observer, not an executor.
4. Introduce a persistent incremental catalogue and UI pagination. Prove correctness and performance on representative data.
5. Improve virtual organisation with genuine user feedback and grouped temporal evaluation.
6. Implement backup/restore evidence and a copy-only transaction on disposable data.
7. Only then design the exact first allowed move class, approval rules and recovery protocol.

Desktop graphics, custom icons and a conversational command surface must not pull unsafe execution ahead of this path. Conversely, the absence of an executor should not stop useful read-only releases.

## Parallel workstreams
A catalogue owner handles schema/identity/watchers; a learning owner handles datasets/student/teacher/evaluation; a UI owner handles view models/accessibility without widening IPC; a safety owner handles transaction and recovery tests; a provider owner handles backup/process/application adapters. The integration owner owns protocol changes, versioning and status claims. Parallel agents should receive exact file ownership and merge dependency boundaries.

## First ten local decisions
Confirm project name/licence/visibility; record Windows build and native toolchain; identify candidate test roots; inventory disks/filesystems and whether any are removable or cloud-backed; choose a maximum observation scope; decide whether content reads are allowed; select an optional local model and its memory budget; specify which processes the app may observe; set desired reserve/cooldown policies as draft preferences; and identify an independent backup target for future move experiments. Unknown answers block only the affected feature, not the entire read-only prototype.

## Design risks to retire early
Native identity under reparse races; no accidental cloud hydration; honest physical-size accounting; scalable query/UI memory; stale approvals; group dependency discovery; teacher/human label contamination; model memory overhead; backup restore fidelity; and confusion between “virtual organisation” and “physical relocation.” Each risk maps to backlog acceptance tests rather than a vague future cleanup task.

## Release discipline
Every checkpoint updates `handoff/CHECKPOINT.json`, implementation status and verification evidence. Finish a small end-to-end slice before adding another broad subsystem. A red native test is a local task, not grounds for marking the suite skipped and declaring completion. The package's current status must remain visible until new evidence replaces it.
