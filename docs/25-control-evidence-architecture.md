# Architecture: a modular core with separate trust boundaries

Status: target architecture. Implemented v0.2 pieces are explicitly identified below. Rust remains the intended native core; Python is the executable reference and optional ML/provider worker language. The browser interface is dependency-free; the separate Tauri scaffold remains uncompiled.

## Choose a modular monolith, not a fleet of services

The domain core should be one versioned Rust workspace with cohesive modules, a single catalogue writer and typed internal messages. Separate processes are justified for untrusted parsers, optional model runtimes, external provider adapters and elevated operations. They are not justified merely because a box appears in an architecture diagram.

This keeps local startup, packaging, upgrades and recovery manageable. A one-person project should not require a message broker, graph database, vector database, scheduler cluster and multiple HTTP servers before it can display a folder. Interfaces can remain modular without becoming network services.

## Five planes

| Plane | Owns | May consume | Must not own |
|---|---|---|---|
| Evidence | Identity observations, coverage, freshness, provenance | Provider records and native scans | User approval or inferred intent |
| Understanding | Personal preferences, groups, collections, forecasts | Scoped evidence and feedback | Raw privileged handles |
| Decision | Alternatives, constraints, simulations, review state | Evidence snapshots and model proposals | Execution credentials |
| Coordination | Work requests, budgets, leases, provider health | Observations and explicit work intent | Arbitrary process control |
| Effects | Approved, supported native operations and receipts | Exact approved manifest and fresh native state | Natural-language instruction parsing |

The UI is a view over these planes, not a sixth authority store. MCP is another view/adaptor. A user click may request a narrowly defined state transition, but the backend validates it. No DOM field is an execution grant.

## Target deployment

```text
Desktop workbench                 Agent host / local automation
        | typed IPC                         | scoped MCP
        +-------------- query / proposal façade -------------+
                                  |
                    application service and policy
                    /             |               \
              catalogue     decision records    work coordinator
                 |                |                    |
            writer actor      review service      owned workers
                 |
          native observation providers

Optional isolated components:
- content extraction worker: content-read grant, byte/time/memory budget
- teacher worker: scoped features, optional explicitly permitted network
- external adapter: one provider, one credential audience, bounded envelope
- future operation broker: short-lived supported manifest, no model interface
```

## Native observer

The observer discovers file/volume identities, performs bounded enumeration and tracks changes. The catalogue stores both records and coverage. A watcher buffer overflow or journal discontinuity marks the affected scope dirty; a reconciliation scan restores a known generation. Microsoft documents that directory notifications can overflow and that change journals may discard or coalesce history [V13, V14]. Design for discontinuity from the beginning instead of assuming every event arrives.

Do not make a full scan and an event stream two unrelated truth sources. Define an epoch with a baseline boundary, buffer subsequent changes, commit the baseline and reconcile overlap by object identity. On unsupported filesystems, retain a slower enumeration provider with visible capability differences. An imported snapshot has no live epoch and must not inherit these assurances.

## Catalogue writer and query projections

A single writer serialises catalogue mutations in bounded transactions. Readers consume versioned projections and cursors. Large rescans build a replacement generation without making half a directory disappear from an existing reader's view. Incremental dirty sets coalesce repeated changes by identity and reason; they are not permanent audit entries for every filesystem notification.

Use SQLite for the initial persistent native catalogue. The current v0.2 `Catalog` is an immutable in-memory scoped reference, not that persistent implementation. SQLite's query-planning documentation explains how index order affects searching and sorting; WAL permits useful reader/writer concurrency but does not eliminate writer contention or make the database a distributed store [V10, V11]. Keep the native database on local storage rather than treating a synced/network folder as a multi-host database.

## Application façade

Expose operations such as `summary`, `search_page`, `explain_object`, `simulate_placement` and `prepare_proposal`. They return structured evidence and authority limits. They do not expose `execute_sql`, `run_shell`, `read_any_path` or a generic RPC-to-command mapping. The same domain operation can serve desktop IPC and MCP without duplicating policy.

The current tool service implements the first four in a snapshot/simulation context. The legacy UI `/api/state` still returns a full snapshot; it has not been migrated to the paged catalogue. That migration is an explicit next step, not a performance benefit this pass silently claims.

## Understanding workers

Feature extraction, inference and training receive a versioned feature batch and the relevant consent scope. A result carries model ID, training-profile revision, feature schema and abstention status. Results are committed only if their input revision remains current. If the owner changes a label during inference, stale output is retained only as historical evidence, not applied to current membership.

Do not let an inference worker fetch additional content because a prompt asks it to. It must request a capability through the service, which can return a refusal or an owner-review request. A failed local model never silently triggers a cloud fallback.

## Decision and effect separation

A proposal is a document containing alternatives, assumptions and expected effects. Approval is a separate record bound to the exact canonical manifest, policy version, scope, expiry and relevant identities. Execution is a state machine that revalidates those conditions. Completion is a verified receipt; the existence of an approval does not prove execution succeeded.

A future operation broker should accept a small typed manifest, not a model conversation. For cross-volume relocation it needs independent checks for source identity, target identity, metadata fidelity, copy verification, destination durability, source retirement and recovery policy. User-visible progress must distinguish “copied,” “verified,” “source retired” and “space observed as available.” None of these effects is implemented in v0.2.

## Cancellation, backpressure and fairness

Every expensive worker needs a work ID, cancellation checkpoint, deadline, estimated resource vector and bounded output. Cancellation is a request until the worker acknowledges that it has reached a safe stopping point. A coordinator restart cannot assume an unacknowledged job stopped.

Queues are separated by purpose: interactive queries, catalogue reconciliation, optional content inspection, inference and maintenance. A fairness policy must prevent a flood of embedding tasks from starving metadata reconciliation. Per-volume I/O admission is more useful than a single global disk percentage. The reference scheduler demonstrates vector admission and lease idempotency, not a complete fairness policy or OS enforcement.

## Failure containment

An external provider failure marks that provider stale or unavailable. It must not erase independent evidence or block the entire app. A malformed extractor response is rejected without changing a file's category. A full database pauses intake and preserves operation/recovery records ahead of optional metrics. An unavailable model leaves search and inspection usable.

State transitions need explicit degraded states. “No data” can mean empty scope, unconfigured provider, refused permission, stale snapshot, unsupported filesystem, exhausted budget or an error. The UI should preserve those distinctions rather than painting a generic green zero.

## Migration order

First verify and compile the existing native foundation on Windows. Then implement identity/capabilities and the disposable fixture lab. Add persistent catalogue projections and typed IPC. Port the reference algorithms with shared fixture tests. Connect the desktop workbench to the native façade. Only then add optional external adapters and owned-worker enforcement. Physical file effects remain behind their original independent gates.

The reference is executable design evidence. It is not a reason to embed a large Python utility daemon permanently if the measured native architecture can be simpler.
