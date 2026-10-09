# Expansion architecture decisions

These are project decisions, not assertions that every target subsystem exists.

## ADR-V2-01: Keep semantic meaning, residency and authority independent

**Decision.** Maintain separate representations for collection membership, physical replica placement and capability/approval. The learner supplies evidence and preferences. The planner produces constrained alternatives. An independently reviewed executor would later apply an exact grant.

**Alternatives.** Direct LLM file tools simplify demonstrations but couple interpretation errors to effects. Pure extension rules are cheap but fail the personal-learning objective. A universal knowledge graph used as the authority database would make inferred edges too easy to confuse with permissions.

**Consequence.** More types and explicit transitions are required, but virtual organisation is useful before physical automation exists. A category correction cannot accidentally grant a move. The reference has no native action executor.

## ADR-V2-02: Use a modular native application, not early microservices

**Decision.** Aim for a per-user Rust core with clearly separated modules, a local relational state store and bounded sidecar workers. Isolate untrusted/extraction/model work where warranted; keep authority and catalogue transactions close together.

**Alternatives.** Microservices add authentication, partial failure and distributed transaction costs before the product needs remote scale. One giant privileged process reduces deployment work but expands the blast radius and makes independent grants difficult.

**Consequence.** The first production target needs explicit IPC and provider supervision, not Kubernetes, a message broker cluster or a kernel driver. The Python reference remains an executable specification rather than the final performance architecture.

## ADR-V2-03: Prefer relational evidence projections before a graph database

**Decision.** Store typed entities, edges, versions and provenance in a local relational catalogue, with bounded traversal and materialised query views. Use indexes appropriate to observed query patterns.

**Alternatives.** A graph database can help complex traversal but adds deployment and maintenance costs. Flat JSON snapshots are easy to transport but expensive to query and difficult to migrate consistently at scale.

**Consequence.** The new in-memory catalogue demonstrates bounded views, not a full persistent graph. A later graph database must win a measured workload comparison and preserve the same identity, scope and evidence contracts.

## ADR-V2-04: MCP is an adapter, never an authority bus

**Decision.** Put protocol-independent ToolService contracts behind transport adapters. Start with fixed scoped snapshot reads and simulation. Keep client metadata, tool annotations, roots and model output outside the grant boundary. Treat the current and legacy protocol eras separately.

**Alternatives.** Exposing a generic shell or filesystem tool is flexible but defeats the controlled interaction model. Binding the internal architecture directly to one MCP SDK makes protocol churn harder to isolate.

**Consequence.** Production should use a reviewed maintained SDK and real-host compatibility evidence. This pass supplies a narrow handwritten stdio reference and rejects unsupported functions. A future proposal tool may prepare a review item but not approve or execute it.

## ADR-V2-05: Return the best feasible plan with an honest proof boundary

**Decision.** Use a heuristic portfolio to provide an incumbent, then spend an explicit search budget on small cases. Optimise shortfall before transfer/disruption/heat, preserving all hard constraints.

**Alternatives.** A single greedy order is fast but demonstrably misses feasible targets. Unbounded exact optimisation can exhaust interactive budgets. A learned allocator without a checked feasible projection could violate physical limits.

**Consequence.** Search completeness and objective scope must be fields in the result. Zero shortfall is not proof of minimum transfer. More advanced solvers can improve the incumbent but cannot rewrite source/recovery assumptions.

## ADR-V2-06: Coordinate demand before manipulating other processes

**Decision.** First budget Loomward-owned and explicitly cooperating workloads. Account for concurrent reservations in one authority. Add native enforcement only after worker identity and failure handling are tested.

**Alternatives.** Aggressive working-set trimming can improve a number while worsening responsiveness. Independently competing schedulers each overestimate headroom. A global autonomous process killer would create high-cost failures.

**Consequence.** The reference lease broker has no OS effects or authentication. Estate Console and other owners retain their process-control authority. Application-specific cooperative mechanisms are preferable to generic forceful controls.

## ADR-V2-07: Make interfaces disclose evidence, limits and alternatives

**Decision.** Show source, freshness, coverage, before/after, uncertainty, recovery and granted capabilities alongside recommendations. Separate review notes from action approvals. Preserve a useful non-chat interface.

**Alternatives.** A chat-only product hides state and makes repeated comparisons difficult. A dashboard of unexplained confidence numbers looks precise but can mislead. Frequent modal questions can cost more attention than the automation saves.

**Consequence.** The Decision desk and Connections page are implemented interaction references. Durable evidence linking, screen-reader validation and native large-data views remain separate gates.

## ADR-V2-08: Keep the product independent of the owner's estate

**Decision.** Loomward works without Taskdeck, Estate Console, Agent Harness, a model account or a cloud service. Adapters are optional and use explicit source/grant contracts. Do not copy proprietary sibling implementations.

**Alternatives.** Tight coupling accelerates one personal setup but makes an open-source project difficult to adopt and can create circular control or licensing problems.

**Consequence.** The source package includes synthetic examples and documentation-level integration designs, not private configurations or active connections. Ecosystem-specific work can be added without becoming the product's foundation.
