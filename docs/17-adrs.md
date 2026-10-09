# Architecture decision records

## ADR-001: Separate meaning from placement
Decision: virtual collections and learned semantics are independent from physical paths and lifecycle. Reason: most classification learning can be useful without moving data. Cost: a logical-item/location mapping and additional UI clarity are required. Revisit only if a specific application integration demands a tightly coupled library model.

## ADR-002: LLM proposes, deterministic authority decides
Decision: the teacher cannot emit executable commands, grants or arbitrary destinations. Reason: plausible reasoning and schema validity do not establish permission or filesystem safety. Cost: a narrower interface and more engineered adapters. This is an invariant, not a temporary workaround.

## ADR-003: No mutation endpoints in the reference
Decision: absent rather than merely disabled executors. Reason: an initial environment cannot verify Windows file/process safety. Cost: the reference demonstrates decisions without applying them. Revisit only through an approved implementation milestone and native evidence gate.

## ADR-004: Runnable Python oracle plus Rust source foundation
Decision: use the available Python runtime for executable tests and a Rust source workspace for the intended native path. Reason: rustc/cargo were absent and toolchain retrieval failed in this environment. Cost: port parity and two temporary implementations. Retire redundant reference paths after native behavior is tested, not before.

## ADR-005: Local SQLite and a single writer
Decision: avoid a server database and live network-synchronised catalogue. Reason: a single-machine product needs low operational overhead and predictable local transactions. Cost: native query/schema work and an export/sync design if multi-device support is later justified. [R22]

## ADR-006: Transparent baseline before sophisticated heat optimisation
Decision: weighted label student and constrained greedy placement first. Reason: measurable baselines reveal whether added complexity helps. Cost: the initial classifier and optimiser are deliberately limited. Revisit after human-labelled and placement evaluation, not because “ML” requires a large model.

## ADR-007: Integrate backups, do not invent cryptography/storage format
Decision: an evidence-bearing provider interface, initially evaluating restic. Reason: protection requires more than copying bytes or a green badge. Cost: provider compatibility and restore-fidelity testing. Revisit provider choice independently from the rest of the architecture. [R16,R26,R27]

## ADR-008: Tauri target; web UI kept transport-independent
Decision: use the current web UI as a reusable shell with future narrowly scoped native IPC. Reason: rapid usable prototype without giving the renderer arbitrary OS powers. Cost: WebView2/platform packaging and accessibility validation. C#/WinUI remains an alternative shell if native requirements warrant it. [R23–R25]

## ADR-009: Process policies begin with owned workloads
Decision: budget the companion's own workers before managing arbitrary applications. Reason: cooperative ownership is safer and makes effects measurable. Cost: narrower early value than a complete task-manager replacement. Arbitrary process control remains a separate gate. [R17–R20]

## ADR-010: Default private publication helper
Decision: source is intended for open release, but repository creation helpers default to private until explicit public confirmation. Reason: working name, unresolved native dependencies and potential local diagnostic files need a review. Cost: one explicit visibility step. No remote repository has been created by this package.

## ADR-011: Observation snapshots are not execution manifests
Decision: display/export formats cannot authorise effects. Reason: path hashes, stale observations, JSON integer precision and imported data do not prove current native identity. Cost: a separate immutable plan/precondition format. Do not relax this to save an API round trip.

## ADR-012: No silent success or silent demo fallback
Decision: failed capabilities, partial coverage, offline devices and unreachable sessions remain explicit. Reason: polished incorrect state is more dangerous than an honest unavailable panel. Cost: more UI states and tests. This applies equally to model confidence, backup health and build claims.
