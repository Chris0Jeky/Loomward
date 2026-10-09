# Local execution packs and ownership

No subagents ran during this authoring pass. The packs below support future parallel local sessions without pretending every lane can merge independently. Read the current source and exact backlog body before editing. Do not equate a dependency's reference implementation with its production completion.

## Coordinator contract

One coordinator owns `AGENTS.md`, `handoff/`, the capability matrix, central HTTP routing, `contracts/v2/INDEX.json`, shared schema version changes, dependency upgrades and integration commits. Workers may propose changes to these surfaces but must not independently overwrite them. Every worker uses an isolated branch/worktree and returns a commit plus an evidence receipt; only the coordinator integrates and publishes.

An interface change must identify old/new fields, callers, migration, downgrade behavior and the fixtures it changes. A passing local lane suite is not a whole-project pass. Preserve the v1 oracle and negative results. Keep state, personal metadata, credentials and real telemetry outside Git.

## Pack A: Windows-native truth, first critical path

**Issues:** LW-001, LW-002 and LW-064, then LW-003/LW-004 as their gates permit.

**Owns:** native build/fixture evidence and the identity/volume modules named by those issues. Do not modify Python planner semantics, MCP routing or UI architecture.

**First actions:** inventory installed toolchains, reproduce the reference suite on Windows, compile/format/test native code, and build disposable fixture coverage. Never download/install/elevate or inspect personal paths automatically. Record precisely which cases ran on Windows versus which were mocked.

**Stop condition:** blocked native prerequisites must be reported, not replaced by an optimistic status. Independent documentation and fixture design may continue.

## Pack B: Reference catalogue and useful UI

**Issues:** LW-068, with LW-069 only after native catalogue/grant dependencies; LW-096 follows the appropriate view gates.

**Owns:** bounded catalogue view models, browser query rendering and related tests. Central routes and schema updates are submitted as explicit coordinator patches.

**First actions:** preserve whole-state reference behavior while adding a paged path, test 50K synthetic records, race search changes, cancellation and cursor invalidation. Measure payload and actual browser responsiveness separately.

**Stop condition:** do not claim million-file or native performance from the in-memory experiment. Never convert truncated display paths into operands.

## Pack C: Protocol compatibility and grant design

**Issues:** LW-065; LW-067 with native prerequisites; then LW-066.

**Owns:** MCP subprocess/host compatibility fixtures and adapter implementation. Shared ToolService contracts belong to the coordinator review boundary.

**First actions:** run modern and legacy reference exchanges; check installed host/SDK versions; test name-disabled scope and no-effect invariants. Record unsupported clients by version. Draft the durable grant store before exposing multiple authenticated clients.

**Stop condition:** no HTTP listener, remote exposure, host auto-configuration, approve/apply tool or provider connection. A supported SDK API is not evidence of host interoperability.

## Pack D: Resource and allocation algorithms

**Issues:** LW-080 and LW-081 after native prerequisites; LW-078/079/077 in their own dependency order.

**Owns:** planner/search and resource lease/admission modules plus synthetic fixtures. No actual worker or process-control integration until the owning-worker gates pass.

**First actions:** preserve the original counterexample and all 51 cases; add large-instance distributions and cutoff tests. Port decimal/integer boundaries. Reproduce simultaneous lease admission and restart/expiry cases.

**Stop condition:** do not silently change the lexicographic objective or mark a budget-limited answer optimal. Do not call demand accounting OS enforcement.

## Pack E: Provider contracts and one real adapter

**Issues:** LW-071, LW-072 and LW-073 with prerequisites; then exactly one of LW-074, LW-075 or LW-076.

**Owns:** provider descriptor/state machines, synthetic recordings and one selected adapter. Do not duplicate sibling control APIs or migrate their state.

**First actions:** use unconfigured fixtures, then explicitly owner-configured endpoints only. Verify authentication, timestamps, coverage and errors against a current upstream contract. Preserve upstream licence boundaries.

**Stop condition:** an unavailable provider stays unavailable. Do not guess a replacement endpoint, copy a credential or infer a grant from a README.

## Pack F: Personal learning and thesis evaluation

**Issues:** LW-025/LW-026 and LW-093/LW-094 in dependency order; no personal data import without an explicit dataset choice.

**Owns:** grouped temporal evaluation, information-value experiments and learning provenance fixtures. Do not alter native authority or redefine an ML score as a permission.

**First actions:** compare the existing weighted student with simple explicit-rule and random-review baselines. Prevent derivative/duplicate leakage and teacher labels entering the human test set. Record accuracy, abstention, owner burden and resource cost together.

**Stop condition:** do not claim learned user preference from synthetic examples or a calibrated probability from a raw model score.

## Integration receipt

Every pack returns exact branch/base/head, owned file changes, named criteria completed or blocked, red/green evidence, full available-suite result, OS/toolchain, external side effects, unverified claims and the next bounded task. The coordinator re-runs the whole suite, updates the capability matrix, reviews generated contracts, and creates the final source manifest/bundle only after reconciling all lane outputs.
