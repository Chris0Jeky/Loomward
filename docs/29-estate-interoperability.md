# Interoperability with the owner's engineering estate

Status: integration designs based on read-only README inspection through the connected GitHub tool on 9 October 2026. No service was queried on the owner's machine. No repository code was copied, modified or published. The review receipts in `evidence/v2/estate-review.json` preserve the inspected blob IDs.

## Estate Console: consume resource evidence, do not duplicate its controls

The inspected README describes a local Python backend, phone PWA and desktop cockpit, cached observations, host/process views and a read-only `estate_host` MCP tool. Its control boundary is already more specific than “run a command”: owner controls and desktop-only process actions have their own checks and audit semantics.

Loomward should begin as an optional consumer of host-resource evidence. It can display the source, observed time, coverage and freshness. Its coordinator can use that information as an input, but should not issue the console's kill or pilot controls. Two controllers that each decide to pause, resume or restart work can oscillate and obscure ownership.

Proposed first adapter: explicitly configured read-only host snapshot, bounded timeout, schema-versioned translation, stale-data handling and redacted fixture tests. A missing source reports unconfigured/unavailable rather than zero capacity. The Connections page names this design and truthfully says Not connected.

## Taskdeck: turn an unresolved decision into work

The inspected README describes read/propose/manage capabilities and an MCP surface that does not expose approve/apply. This matches the separation Loomward needs. A storage review can produce a Taskdeck capture such as “Verify independent backup for Project Atlas before considering archival placement.” The capture should reference an opaque Loomward decision, not attach a whole private catalogue.

A task's status is context. Completing a Taskdeck card must not approve a file move in Loomward. The operation broker must still require the exact current manifest and native preconditions. Avoid bidirectional state synchronisation until the mapping of capture, proposal, approval and execution is explicit.

Licensing boundary: the inspected Taskdeck README identifies its current source as private proprietary beta, not an open-source dependency. Loomward remains MIT and integrates through a documented protocol boundary. No private Taskdeck implementation is included. Provider names in a design do not imply endorsement or a redistribution right.

## Agent Harness: honour activity and ownership evidence

The inspected README documents guarded worktree management and cooperative leases. The claimant label is self-declared, not an authenticated principal. Missing, malformed or stale evidence should lead Loomward to preserve or mark a worktree unknown, not infer that it is safe to remove.

Proposed first adapter: ingest a scoped, read-only worktree activity observation with its epoch, validity interval and source. Present “active work” as a placement veto. Do not reproduce the harness's closeout/removal logic in Loomward or invoke its apply path. Independent authorities over the same worktree would increase race and recovery complexity.

## Generic AI runtimes and asset tools

A runtime may voluntarily declare a model or project lease. The lease says that the resource is in use, with an expiry and renewal contract. It should not expose complete prompts, command lines or unrelated files. A model loader could later cooperate with Loomward to release a model it owns when idle; this is separate from killing another application's process.

Asset generators and build tools can mark output as reproducible, costly to regenerate, ephemeral or owner-reviewed. Treat those as attributed claims. A reproducible label requires enough retained inputs, versions and commands to actually reproduce the artefact; it is not permission to delete the only output automatically.

## System-of-record boundaries

Loomward owns local object observations, its personal organisation profile, its resource-admission policy and its decision records. Taskdeck owns its work items. Estate Console owns its control surface. Agent Harness owns its worktree-operation protocol. A backup provider owns its repository format and integrity machinery. GitHub owns repository/issue state.

Exchange references and attributed evidence instead of replicating every database. Keep a projection only as long as needed, with a clear source. When the source is unavailable, the projection is stale; it does not become a new authority.

## First integration demonstrations

Use synthetic fixtures first. Demonstration A imports a redacted host snapshot and shows why an embedding job is deferred. Demonstration B exports a review-only work-item draft with a stable correlation ID. Demonstration C imports an active worktree lease and makes a placement candidate ineligible. None needs a real provider write.

Then validate one adapter against a real read-only instance with the owner's explicit configuration. Record actual payload shape, version and freshness. Do not equate a README contract with a deployed API guarantee. Only after that evidence should the UI offer a genuine Connected state.
