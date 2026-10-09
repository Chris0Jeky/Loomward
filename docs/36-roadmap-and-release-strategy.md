# Roadmap and open-source strategy

## The first release should be valuable without autonomy

Ship a dependable Windows observation workbench: bounded native inventory, understandable sizes/coverage, virtual organisation, personal feedback, reviewable simulations and a narrow local integration surface. Do not make unattended moves, full process control or a replacement shell prerequisites for usefulness.

The v0.2 reference demonstrates these boundaries but is not that native release. Keep the original Windows validation, Rust compilation and fixture-lab issues at the front of the native critical path. The new interoperability/query work can progress in parallel without weakening those gates.

## Workstreams

**Native truth.** Verify toolchains, identity, volume capabilities, enumeration, journal reconciliation and metadata fidelity. Acceptance requires Windows evidence, not a cross-platform mock.

**Useful views.** Persist the catalogue, add paged IPC, connect the desktop workbench and preserve import/demo/live distinctions. Benchmark the actual Windows application.

**Personal learning.** Evaluate the existing student, implement scoped feature/label provenance, add a candidate embedding model and test selective teacher use. Do not promote a model without held-out evidence.

**Interoperability.** Validate the MCP reference with real hosts, select a maintained SDK for production, implement durable grant/view semantics and add one real read-only provider at a time.

**Resource coordination.** Port admission/lease contracts, authenticate workers, reconcile restart state and enforce only owned-worker policies initially.

**Supported effects.** Add one operation class with native handles, manifest approval, durable intent, fault injection and successful restoration. Do not bundle generic deletion, moves, uninstall and process killing into one first action release.

## Release levels

A source preview can contain synthetic UI and reference tests with explicit caveats. A developer alpha needs a reproducible Windows build and a fixture lab. A native observation beta needs measured indexing/query behaviour, installer/uninstaller checks and honest degraded states. A controlled-action beta needs operation-specific recovery evidence and a narrowly selected supported scope.

Each level has a different completion claim. A passing Python suite does not advance the project to native beta. A signed installer does not prove algorithmic correctness. A model benchmark does not grant filesystem authority.

## Open-source boundaries

Retain MIT for Loomward's original code and documents unless the owner chooses otherwise. Integrate with other applications through their documented interfaces and respect their licences. The inspected current Taskdeck source is proprietary; it is not a code dependency to copy into this project.

Keep provider adapters small, individually testable and optional. Publish a compatibility matrix and redacted fixtures rather than bundling credentials or private estate paths. An adapter should not force an account, network service or telemetry pipeline on users who only want local file inspection.

The name Loomward remains provisional. Repository search is not trademark clearance. No repository or remote issues were created this pass; the package retains dry-run-first publication helpers and issue bodies for an explicit later owner decision.

## Scope discipline

Defer kernel drivers, transparent filesystem virtualisation, general shell execution, automatic application uninstall, arbitrary process management, distributed catalogue authority and fleet management. These may be future research, but they would multiply the project's safety and maintenance burden before the core workflow is proven.

Prefer interoperating with a good specialised tool to rebuilding it. An Everything provider could supply search candidates through its documented SDK [V20], but must still apply Loomward's scope and evidence semantics. A backup provider can supply restore evidence without Loomward owning a new backup format. A process inspector can remain the expert escape hatch.

## Success review

At each milestone, ask whether the product reduced owner effort and improved a measurable outcome. Count useful decisions, successful retrieval and recovered workflows, not implemented menu items. Retire experimental modules that cost more than they help, while preserving their evidence and lessons in the repository.

## Source references

- [V20] Everything SDK: https://www.voidtools.com/support/everything/sdk/
