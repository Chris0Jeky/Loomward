# Loomward interoperability and efficiency expansion implementation plan

> For agentic workers: use the executing-plans workflow for these bounded reference subprojects. Preserve test-first evidence and read the specification before continuing.

**Goal:** Extend the existing prototype with tested optimisation and interoperability foundations and a durable architecture/design expansion.
**Architecture:** Immutable scoped catalogue and pure simulators sit behind typed adapter contracts. MCP is stdio only and cannot gain action authority. UI additions reveal the same restrictions.
**Tech Stack:** Python >=3.11, SQLite, standard-library JSON-RPC transport, existing vanilla JS/CSS, optional installed Chromium/Playwright.
**Spec:** `docs/superpowers/specs/2026-10-09-expansion-design.md`.

## Global constraints
- No user-file mutation, process control, automatic network/provider/model setup or remote publication.
- No synthetic values presented as live host observations.
- Existing Rust/Tauri remains uncompiled and separate from Python v2 semantics.
- Scope filtering precedes all summaries and lookup responses.
- The old planner and experiment remain available as a baseline.

## Review focus
Cursor tampering and replay across grants/generations, tested in catalogue tasks. Modern requests accidentally inherit legacy metadata, tested in MCP tasks. CPU/GPU/RAM reservations race, tested in lease tasks. Budget exhaustion creates false optimality, tested in planner tasks. Imported or synthetic interfaces claim live execution, tested in UI tasks.

## Task 1: Bounded placement optimisation
Files: `python/loomward/planner_v2.py`, `tests/test_planner_v2.py`, `experiments/v2_benchmarks.py`.
Consumes: v1 scenario semantics and validated v1 result. Produces: `plan_tiers_v2(scenario: dict, *, node_budget: int=50000) -> dict`.
- [ ] Write a test reproducing the cold-small-group counterexample; assert v1 shortfall 10 and v2 shortfall 0, and no mutation.
- [ ] Run `PYTHONPATH=python python -m unittest discover -s tests -p test_planner_v2.py`; record the red result.
- [ ] Implement bounded search and a safe deterministic portfolio fallback without changing v1.
- [ ] Cover exact exhaustion, cutoff, infeasible capacity, input constraints, deterministic tie breaks and unchanged input; run the full suite.

## Task 2: Scoped immutable catalogue
Files: `python/loomward/catalog.py`, `tests/test_catalog.py`.
Consumes: initial `inventory` snapshot format. Produces: `Catalog(snapshot: dict, *, prefix: str='', disclose_names: bool=False)`, `.summary()`, `.search(query='', extension='', limit=25, cursor=None)`, `.explain(item_ref)` and `.close()`.
- [ ] Write tests for scope-before-aggregate, prefix component boundaries, excluded sensitive names, cursor tamper/query mismatch, deterministic pagination and invalid paths/numbers.
- [ ] Run the focused tests and retain the red output.
- [ ] Implement a construction-time scoped SQLite index with HMAC handles/cursors, bounded SQL work and output.
- [ ] Test pagination has no omissions/duplicates on equal-sized records and no off-scope ID oracle; run the full suite.

## Task 3: Resource planner and lease reference
Files: `python/loomward/scheduler.py`, `tests/test_scheduler.py`.
Produces: `admit_workloads(scenario: dict) -> dict` and `LeaseBroker(capacity, clock=...)` with `acquire`, `release`, `snapshot`.
- [ ] Test every resource dimension, unknown capacity, reservations, ageing, TTL/replay and owner/token mismatch before implementation.
- [ ] Record red output, then implement bounded pure admission and lock-protected ephemeral leases.
- [ ] Use concurrent requests against a single-slot broker to prove only one grant; run the full suite.

## Task 4: Tool service and stdio MCP reference
Files: `python/loomward/interop.py`, `python/loomward/mcp_stdio.py`, `scripts/run_mcp.py`, `tests/test_interop.py`, `tests/test_mcp_stdio.py`.
Consumes: catalogue, v2 planner. Produces: `ToolService(catalog).call(name, arguments)` and protocol handler plus CLI.
- [ ] Write scoped-tool tests and newline-subprocess tests before implementation.
- [ ] Record red output. Implement four fixed tools, two fixed resources, launch-only data grants and bounded framing.
- [ ] Test modern missing/unsupported metadata, legacy initialize/notification, notification silence, JSON duplicate keys, unknown methods, oversized input, schema errors and dangerous tool absence.
- [ ] Run the complete suite. Mark interoperability with actual external MCP clients unverified.

## Task 5: Interface and API additions
Files: `ui/expansion.js`, `ui/styles.css`, `ui/index.html`, `ui/app.js`, `python/loomward/server.py`, `scripts/build_preview.py`, `scripts/test_ui.py` and new UI/API tests.
Consumes: current page registry and App routing; pure v2 planner/scheduler.
- [ ] Add failing API and browser checks for three new pages and explicit simulation/connection states.
- [ ] Implement additive pages, exportable contract drafts, a simulation-only decision workflow and resource budget controls.
- [ ] Keep imports scoped; forbid arbitrary process/file operations; exercise narrow viewport, keyboard focus and no browser exceptions.

## Task 6: Architecture, research and handoff
Files: `docs/24-*` onward, `docs/adr/`, `schemas/`, `integrations/`, `backlog/`, `handoff/`, `evidence/v2/`.
- [ ] Record primary sources with dates, versions, supported claims and limitations.
- [ ] Publish product thesis and falsifiable evaluation plan, domain contracts, provider/MCP boundaries, estate integration proposals, use cases, efficiency and failure labs.
- [ ] Append new issue IDs without altering old semantics or inventing remote issue numbers.
- [ ] Verify fixtures/schemas/backlog links and source-artifact consistency.

## Task 7: Evidence and packaging
- [ ] Run reference, syntax, protocol, UI and synthetic benchmarks. Preserve failure/correction history.
- [ ] Record self-review and unresolved native/client/provider gates.
- [ ] Rebuild preview and consolidated report; update checkpoint, README and agent prompt.
- [ ] Commit locally; make a complete Git bundle and checksummed source ZIP.
- [ ] Extract to a clean relocated checkout, rerun tests and verify archive/manifest/bundle consistency.

## Pre-flight interfaces
Planner uses v1 validation but v2 output metadata must be separate. Catalogue accepts only a snapshot, never a path to scan. Tool service consumes immutable catalogue and pure planner. UI simulation uses the same native-free API; standalone surfaces must distinguish their local illustrative simulation from the Python solver. Lease reference is not advertised as a process controller or an MCP tool.
