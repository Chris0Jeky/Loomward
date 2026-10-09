# Loomward expansion design, v0.2

Date: 2026-10-09. Status: implementation brief for this continuation, not production approval.

## Intent and preservation
Extend the existing learning, storage and resource companion rather than replace it. The owner requested deeper architecture, interface, thesis, interoperability, interconnectivity, MCP, use cases and efficiency. Preserve the initial commit, its 89-test baseline, uncompiled Rust foundation, initial negative planner experiment and every earlier issue. Build in an isolated clone on `expansion/interop-v2`. User-file/process effects, installation, live integrations and remote publication remain disabled.

## Direction
Loomward is a local, evidence-backed workspace coordinator, not another universal agent orchestrator. Distinguish observation, semantic knowledge, proposal, approval, scheduling, execution and recovery. Its mature work is to minimise friction and resource contention subject to privacy, recovery and user preferences. MCP is an adapter for scoped reads and simulations, not the internal service bus and not a grant system.

## Alternatives
1. Documentation-only expansion: preserves ambition but leaves critical claims untested. Retain detailed docs but add bounded executable work.
2. Broad autonomous agent platform: duplicates estate-console and Taskdeck while expanding authority. Reject.
3. Observation-first interoperability workbench: selected. Implement scoped queries, a narrow MCP surface, resource-admission simulation and a better bounded allocator. Keep native integration as a separately verified next gate.

## Subprojects in this pass
A. Placement v2: preserve `plan_tiers` v1. Add `plan_tiers_v2(scenario, *, node_budget=50000)` with a deterministic portfolio incumbent and bounded exhaustive assignment search for at most 12 eligible groups and 4 targets. Objective is lexicographic: shortfall, transfer bytes, touched groups, byte-weighted heat. Report search exhaustion and the precise shortfall bound, never unconditional optimality. UI live simulation may use v2; standalone v1 stays labelled.
B. Scoped catalogue: an immutable SQLite snapshot index, bounded to 200000 records; scopes are fixed at construction, paths are relative and component-checked, sensitive records excluded before aggregation. Query pages use a stable indexed keyset, a signed cursor bound to query/generation/grant, SQL VM budget and bounded result sizes. No full scan of user files is exposed by this component. IDs are opaque presentation references, never native authority.
C. Resource admission: deterministic multidimensional admission planner over supplied CPU slots, memory MiB, GPU MiB and I/O MiB/s budgets. Account for existing reservations once, unknown capacity blocks demands in that dimension, priority/age never overrides capacity. Add a lock-protected in-process lease reference with TTL, owner/token checks and replay semantics. Neither changes processes.
D. Interoperability: protocol-neutral tool service plus a stdio MCP reference implementing read-only tools/resources and bounded simulation. Support modern 2026-07-28 and legacy 2025-11-25 separately. Current revision requires request `_meta` version and client capabilities and resultType; legacy initialize establishes only that process's protocol mode, not filesystem scope. Four tools: `workspace_summary`, `catalog_search`, `evidence_explain`, `placement_simulate`. Scopes/metadata disclosure are launch-time options; request arguments cannot expand them. No content reads, approvals, shell, file writes, network service or arbitrary provider execution.
E. Interface: preserve all seven existing pages; add a decision desk, connections/MCP contract lab, and resource budget lab. Mark design cards and synthetic simulations, never display external providers as connected. Keyboard/accessibility and exportable settings help users review without a terminal. Do not wire previews to real capabilities.
F. Durable design: product and research thesis; plane/component architecture; evidence and event contracts; provider lifecycle/security; integrations with estate-console, Taskdeck and agent-harness; use-case atlas; efficiency strategy; research/benchmark protocol; adversarial failure cases; staged roadmap and new dependency-linked issues.

## Implementation constraints
Python >=3.11 standard library for core additions. No required package installation, external service or model. Existing optional Playwright/psutil are reused only if installed. No changes to existing Rust semantics in this pass; its planner remains v1 until parity is implemented and compiled. Numeric outputs in new agent-facing contracts use decimal strings for byte counts. Immutable snapshots provide consistency, not freshness. Metadata disclosure is a separate grant and does not make sending it to an external model private.

## Review focus
Cross-scope leakage through aggregates, cursors, item identifiers and resources; malformed/untrusted snapshot records; stateless-versus-legacy protocol confusion; planner search truncation disguised as optimality; lease replay/expiry and aggregate resource overcommit; preview-only actions disguised as execution; imported metadata authorising new reads; proprietary sibling-source code copied into an open-source project.

## Acceptance
Original suite remains green. New units and adversarial tests cover each reference boundary. Browser checks exercise every new page and persistence/export behaviour. Stdio subprocess tests read actual newline-delimited JSON-RPC, including invalid/overlarge messages and both version eras. Reproducible experiments compare the allocator against the preserved tiny-case oracle and catalogue paging against result-equivalent bounded baselines. Deliver updated source, Git bundle, standalone preview, a consolidated expansion report, source manifests and an exact handoff. Windows, Rust, real model, real MCP-host and live-provider results are UNVERIFIED unless explicitly exercised.

## Execution ruling
Inline execution with recorded self-review because no subagent-dispatch tool is available. The owner explicitly requested continued deliverables and implementation in this pass; proceed without repeated design-confirmation questions. Keep implementation, experiments, prospective architecture and unverified support clearly separate. Read-only sibling README inspection informs integration proposals; do not copy their implementation or configure their runtimes.
