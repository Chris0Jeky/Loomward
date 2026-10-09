# Provider runtime, event exchange and interconnectivity

Status: provider SDK and event-exchange design. The v0.2 package includes schemas/examples and tests for their shape. No external provider runtime, subscription bus or synchronisation service is implemented.

## A provider contributes evidence, not ambient authority

A provider should expose a narrow capability manifest: identity/version, contract versions, operations, scope types, data sensitivity, network requirement, freshness semantics and work budgets. A declaration is not a grant. The owner chooses which provider is enabled and which capabilities it may exercise.

Start with three classes. Native observers obtain local identities and measurements. Adapters translate one established application's documented read interface into Loomward observations. Extractors interpret explicitly permitted file contents in an isolated worker. Do not blend all three into a generic plugin that inherits the full filesystem and every configured credential.

The initial provider package format should be data plus an explicitly selected executable entry point, pinned to a reviewed build. A manifest must not smuggle command fragments, environment expansion or an arbitrary download/install step into configuration. The reference schemas deliberately have no executable loader.

## Lifecycle

Use states `unconfigured`, `disabled`, `starting`, `ready`, `degraded`, `stale`, `blocked`, `failed` and `stopped`. Each state has a reason code, observation time and optional next user action. Do not conflate “not installed” with “installed but permission denied.” A provider crash should change only that provider's health until independent evidence indicates a broader fault.

Start providers on demand where practical. Assign a resource budget and a cancellation contract. Health polling should back off when a provider is unavailable and stop when disabled. Avoid waking a sleeping HDD or mounting a disconnected network share merely to refresh a cosmetic badge.

An incompatible schema version is a blocked state, not permission to guess field meanings. The package should retain a redacted raw fixture for debugging only when the owner explicitly permits it and the fixture is bounded and scrubbed.

## Request/response contract

A read request carries a scope reference, projection, continuation cursor, maximum records and work deadline. The service resolves the authenticated view before forwarding the request. A provider cannot expand its scope by returning a different root. The response includes source epoch, coverage, observation time and bounded records.

A provider may return fewer records than requested and a cursor, or an explicit incomplete status. It must not call a truncated response complete. Unknown fields can be preserved only in a non-authoritative extension map with limits; control fields remain strictly versioned.

Use one adapter per credential audience. A Taskdeck token never becomes a Loomward token, and a console token never becomes a backup credential. Secrets stay in the owner's protected configuration/key store; they do not appear in proposal payloads, exported manifests, process command lines or model context.

## Event envelope

Adopt a CloudEvents-style envelope for interoperability where useful: `specversion`, `id`, `source`, `type`, optional subject/time, and a typed `data` payload. CloudEvents standardises event description; it is not a transport, authentication system, ordering guarantee or exactly-once delivery mechanism [V15]. Loomward adds provider epoch, sequence, scope reference, schema version and origin inside its own validated data contract.

Useful event families include `catalog.scope_dirty`, `catalog.generation_committed`, `provider.health_changed`, `proposal.prepared`, `proposal.invalidated`, `work.admission_decided`, `work.lease_expired` and `recovery.evidence_updated`. Avoid sending an event for every row to every subscriber. Coalesce a burst into a dirty-scope notification and let an authorised consumer query the current projection.

A proposal-approved event must never cause a consumer to execute a filesystem operation by itself. It can notify a review surface; the operation broker still requires its exact manifest grant and fresh preconditions. An event is a statement to verify, not an instruction to trust.

## Delivery, duplicates and ordering

The proposed local service uses transactional outbox rows for events that must survive a crash. A consumer keeps a bounded inbox keyed by provider identity, epoch and event ID. Acknowledgement means the event was durably accepted or projected, not that an external action finished. Retries may redeliver the same event.

Use per-source order only where it exists. Sequence 42 from provider A cannot be compared with sequence 41 from provider B. A reconnect with a missing range marks the projection uncertain and triggers bounded reconciliation. An older event arriving after a newer observation cannot overwrite the current state merely because it arrived last.

Do not claim end-to-end exactly-once behaviour. For future external writes, combine a durable intent, provider-supported idempotency and effect reconciliation. When the remote outcome is ambiguous after a timeout, report “outcome unknown” and inspect it before retrying a destructive command.

## Loop prevention

Cross-tool automation can create feedback loops: Loomward opens a task, Taskdeck emits a task event, an agent interprets it as a new storage request, and Loomward opens another task. Carry `correlation_id`, `causation_id`, origin and a bounded hop count. Use stable idempotency keys tied to the underlying decision, not fresh IDs on every delivery.

A “do not echo” marker is useful but insufficient. The consumer must compare the semantic work identity and maintain its inbox. Model-generated comments should not become new human preferences or fresh usage evidence simply because another service repeats them.

## Multi-device direction

Begin with one machine as authority for its live objects. Other devices can consume scoped summaries and propose questions. A disconnected machine cannot approve or act on the current native identity of another machine's file using a stale replica of the catalogue.

Synchronise selected user preferences and task references only after explicit conflict semantics exist. Concurrent collection edits might use a mergeable set; file moves cannot be reduced to a conflict-free set update. Preserve per-device volume identities, provider epochs and permission scopes. Avoid syncing the live SQLite database itself.

A phone companion should prioritise review, status and pausing owned optional work. High-risk file effects should remain a separately authorised native workflow until remote identity, authentication, liveness and recovery semantics have been demonstrated. A secure network tunnel is not, by itself, a complete action-authorisation design.

## Provider conformance suite

Require fixtures for missing fields, wrong units, stale timestamps, out-of-order events, epoch reset, duplicate events, scope mismatch, oversized records, malformed JSON, denied credentials, cancellation, timeout and redaction. A provider's first release should be read-only and validated against at least one real deployed instance, not just against its README.

For each adapter, record a tested version range and last verified payload fingerprint. Contract tests are replay evidence; they do not establish that a service is currently connected or healthy. The Connections page therefore presents adapter designs as unconnected rather than simulating green provider statuses.

## Source references

- [V15] CloudEvents project: https://cloudevents.io/
