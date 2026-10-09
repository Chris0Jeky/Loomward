# Loomward first prototype implementation plan

**Goal:** Deliver a runnable observation-and-learning workbench plus a native Rust foundation and a staged production roadmap.

**Architecture:** Typed inventory and proposal records feed independent learning, tier-planning and review modules. A loopback-only UI exposes bounded read capabilities and application-state writes. There is no filesystem/process action executor in this build.

**Tech stack:** Python 3.11+ standard library, SQLite, optional psutil, vanilla JavaScript/CSS; Rust 2021 workspace with serde and sha2, with standard-library CLI/scanner code; a separately gated Tauri 2 shell.

**Spec:** `docs/00-design-brief.md`, followed by the detailed product/architecture/safety/ML specifications.

## Global constraints
- Every screen identifies synthetic versus observed data.
- No ordinary-file or process mutations.
- Hashing requires explicit consent and has a byte budget.
- LLM use requires explicit metadata consent, a literal loopback address and a schema-conforming result. No default cloud requests.
- Raw model scores are not calibrated probabilities and never authorise actions.
- Scanner results expose exclusions, errors, limits, unknown physical allocation and identity limitations.
- Native compilation and Windows behavior remain unverified until executed on a suitable local host.

## Review focus
Untrusted filenames in HTML; symlink/reparse/cloud-placeholder traversal; stale or duplicate feedback; out-of-distribution model inputs; impossible tier plans and arithmetic errors; HTTP origin/authentication boundaries; inaccurate claims of space reclaimed.

## Task 1: Data contracts and scanner
Files: `python/loomward/inventory.py`, `schemas/`, `tests/test_inventory.py`.
Interface: `scan(root, max_entries, max_depth) -> dict`; output schema version 1, file records, counters and explicit coverage.
- [ ] Write cases for byte totals, unknowns, exclusion, missing roots, bounds and symlinks.
- [ ] Observe failing tests before implementation.
- [ ] Implement metadata-only iterative traversal; never follow links/reparse points.
- [ ] Run the complete suite and record results.

## Task 2: Duplicate inspection
Files: `python/loomward/duplicates.py`, `tests/test_duplicates.py`.
Interface: `find_duplicates(root, inventory, byte_budget) -> dict`.
- [ ] Test exact equality, same-size different data, hard links, changed records, symlink replacement, budget and mismatched roots.
- [ ] Implement size grouping, bounded SHA-256 and byte comparison. Mark read budget exhaustion; no deletion endpoints.
- [ ] Run scanner/duplicate tests together.

## Task 3: Constrained tier simulation
Files: `python/loomward/planner.py`, `tests/test_planner.py`.
Interface: `plan_tiers(scenario) -> dict`; each proposal has a stable group ID, source/target volume, logical bytes and reasons.
- [ ] Test source pressure, target reserves, pinned/active/protected groups, unknown heat, offline destinations, cooldown, invalid values, no feasible solution, atomic groups and budget.
- [ ] Implement deterministic greedy baseline with explicit unmet demand, projected capacities and rejection reasons. It is not a learned policy or globally optimal solver.
- [ ] Run all tests.

## Task 4: Learning and teacher boundary
Files: `python/loomward/learning.py`, `teacher.py`, `tests/test_learning.py`, `test_teacher.py`.
Interfaces: `Student.fit(events)`, `Student.predict(features)`, `validate_teacher(...)`, `request_teacher(...)`.
- [ ] Test learning from examples, novel inputs, weak versus human provenance, retractions, conflicting versions, label vocabulary and malformed/untrusted teacher messages.
- [ ] Implement a weighted multinomial Naive Bayes reference student with OOD/low-support abstention and active-review scoring.
- [ ] Implement optional loopback-only structured teacher request, no redirects/proxies and no raw-content field.
- [ ] Run learner/teacher tests with synthetic examples; real-model and personal-data evaluation remain deferred.

## Task 5: Application state, telemetry and HTTP surface
Files: `store.py`, `telemetry.py`, `server.py`, `tests/test_store.py`, `test_server.py`.
- [ ] Test event idempotency/conflict, transactional revisions, token/origin/host rejection, body limits, unsupported operations and no arbitrary paths.
- [ ] Implement SQLite application-state persistence and capability-scoped endpoints. Use optional psutil for read-only process observation.
- [ ] Keep API errors explicit; no fake success responses or placeholder actions.

## Task 6: User interface
Files: `ui/index.html`, `ui/app.js`, `ui/styles.css`, `scripts/test_ui.py`.
- [ ] Cover navigation, search, imported snapshots, review choices, simulation inputs, accessibility basics and malicious display strings.
- [ ] Implement overview, storage explorer/map, learning/review, tier simulator, processes and protection roadmap screens.
- [ ] Exercise with a real browser; save screenshots and evidence.

## Task 7: Rust foundation and Tauri scaffold
Files: `Cargo.toml`, `crates/loomward-core/`, `crates/loomward-cli/`, `native/`.
- [ ] Write native unit/integration tests against the shared fixture semantics.
- [ ] Write scanner, typed fail-closed policy, pure transition simulator, planner and read-only CLI sources. Native duplicate hashing is deferred to LW-012.
- [ ] Add packaging scaffold without privileged capabilities or action commands.
- [ ] Explicitly mark unexecuted cargo and Windows verification. Local worker must compile/test before enabling integration.

## Task 8: Continuity and release handoff
Files: `docs/`, `backlog/`, `scripts/`, `handoff/`, `evidence/`, `README.md`, `AGENTS.md`.
- [ ] Preserve research with primary-source references and a claim ledger.
- [ ] Write ordered issues, milestones, acceptance criteria and ownership boundaries.
- [ ] Provide non-destructive environment/bootstrap and opt-in private publishing helpers.
- [ ] Run tests, audit feature claims, generate a checksummed source archive and standalone UI preview.

## Execution notes
Inline execution in this session. No independent subagent tool is available. Review is a recorded self-review, not represented as an independent review. Keep the work in the new isolated `loomward` directory; do not mutate any existing repository.

## Final task disposition
Tasks 1 through 6 have runnable reference implementations and recorded tests. Task 7 has meaningful source and written tests but remains UNVERIFIED until a native toolchain runs; native duplicate/process/database/IPC providers are deliberately deferred. Task 8 produces the docs, local issues and handoff, not remote publication. The definitive capability matrix is docs/16-implementation-status.md. Unticked steps above preserve the original plan rather than imply that written native tests ran.
