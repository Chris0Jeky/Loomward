# Domain model, evidence records and exact interchange

## Object identity is not a path

The target domain distinguishes an object, a path binding, a storage replica, a logical group and a collection membership. A Windows-native identity will require volume identity and handle-derived file identity with a defined lifecycle. A path is an observed name that may change or be reused. A hash identifies observed content, not ownership, recoverability or permission to replace a file.

The current snapshot catalogue deliberately uses process-local opaque `it_…` references. They are HMAC-derived under a fresh view secret. They are not stable IDs across server restarts and cannot be passed to a native executor. Their only purpose is to look up an item in the same scoped snapshot. The generation is derived from ordered in-scope records and keyed by the fresh view secret. It is deliberately different across view instances, not a public metadata fingerprint or globally canonical filesystem digest. This avoids exposing a reusable hash for guessing undisclosed names.

## Proposed durable entities

| Entity | Key information | Important separation |
|---|---|---|
| Object | Native identity, type, observed attributes | One object can have several path bindings |
| Path binding | Volume, parent, name, observation interval | A reused path need not identify the previous object |
| Replica | Object/content relationship, location, state | A replica is not automatically an independent backup |
| Group | Members, dependency reason, completeness | Semantic membership does not imply atomic movement |
| Observation | Provider, epoch, time, coverage, payload digest | Observation is not inference |
| Belief | Predicate, model/evidence, uncertainty, revision | Belief is not fact or policy |
| Preference | Human decision, context, revision, retraction | Teacher labels remain separate |
| Proposal | Alternatives, effects, assumptions, missing evidence | Proposal is not approval |
| Grant | Principal, scope, operations, expiry, manifest digest | Grant is not execution success |
| Receipt | Actual transition, identities, evidence, outcome | A transport success is not an effect receipt |
| Work request | Owner, demand estimate, priority, TTL | Admission is not process enforcement |

Represent the early graph in relational tables with explicit typed edges. A general graph database is unnecessary until measured query workloads justify it. Do not build an all-purpose ontology before shipping useful questions such as “why is this group pinned?” and “which evidence made this candidate eligible?”

## Evidence envelope

A production observation should carry `provider_id`, `provider_version`, `provider_epoch`, `observation_id`, `observed_at`, `received_at`, `scope_ref`, `coverage`, `freshness_policy`, `schema_version`, `payload_digest` and `origin`. Origins include native observation, supplied snapshot, human assertion, model inference and synthetic fixture. These types must survive exports.

Observation time and ingestion time are separate. A successful HTTP response can deliver a week-old cached record. An epoch change invalidates sequence comparisons across provider restarts. A digest detects an accidental payload mismatch; it does not authenticate the provider. Authenticated transport and verified provider identity remain separate requirements.

Unknown is a typed state rather than a magic zero or empty string. For example, a missing allocated-size measurement should be `unknown` with a reason, not `0`. Counts over a partial catalogue must say what they count. A query result should include its scope/generation and whether it hit a work or output limit.

## Projection before disclosure

Scope filtering occurs before counts, sums, ranking, cache keys and response construction. An unauthorised record must not influence a “top ten” result or aggregate total returned to a narrower caller. The v0.2 catalogue filters component-bound prefixes and sensitive/excluded flags before it builds the scoped database and generation digest.

Name disclosure is an additional launch-time choice, independent of summary access. Even summary counts may be sensitive, so selecting a snapshot grants disclosure of its scoped aggregate; the interface must explain that. This reference is not a multiuser access-control database. Anyone using the same launched process receives the same owner-selected view.

In a future service, derive each view from an authenticated grant. Every cache and cursor must be bound to the effective view, not merely to a query string. Revocation invalidates view caches and continuation tokens. A cursor is not a reusable authorisation token for another scope.

## Byte values and numbers

Use decimal strings for authority-bearing byte quantities on v2 interchange. This avoids silent JavaScript rounding when native counters exceed its safe integer range. The catalogue accepts each size up to signed 64-bit maximum and sums using Python integers; it returns the total as a decimal string. The placement reference intentionally accepts a narrower safe-integer domain after converting canonical decimal strings. Its JSON Schema describes that limit and runtime validation enforces it.

Use ordinary integers for bounded counts, priorities and resource estimates where their ranges are explicit. Reject booleans as numbers, non-finite floats, negative sizes, duplicate IDs and duplicate JSON keys. Do not accept an `execute` field and then ignore it; rejecting unknown control fields is easier to reason about.

A production native domain should use unit-specific types for bytes, monotonic durations, timestamps and percentages. Converting a MiB budget to bytes is an explicit checked operation, not an implicit arithmetic convention scattered through adapters.

## Pagination and bounded queries

The reference orders by negative size and opaque item reference, with a matching compound index. A signed keyset cursor binds the last row, query, extension, generation, scope and disclosure mode. A cursor cannot be reused with a different query or catalogue instance. The response has at most 100 records and a bounded encoded item payload. Display strings may be truncated, with an explicit flag; truncated paths must never be used as operands.

Literal substring search uses a bounded SQLite query, not full-text semantic search. Some searches can still inspect many records. The VM work budget returns an explicit error instead of an empty result. The current query planner, not a marketing claim, determines whether an index eliminates sorting [V10].

## Revision and deletion semantics

Prefer append-only decisions plus materialised current views for learning and proposal history. For bulk metadata, prefer snapshots and deltas with retention bounds rather than storing every filesystem event forever. These are different data classes with different retention needs.

A retracted human event should cease influencing subsequent refits. A deleted source may also require removing derived embeddings, cached teacher responses and exported projections. The prototype has feedback retraction and scoped snapshots, not a full derived-data erasure engine. Future provenance links need to support locating and invalidating those derivatives.

No database schema migration should rewrite the original evidence in place without an explicit compatibility or archive strategy. A v1 imported snapshot is data, not an authority-bearing v2 object record. Preserve the old format and perform a validated projection rather than pretending the two schemas mean the same thing.

## Source references

- [V10] SQLite query planning: https://sqlite.org/queryplanner.html
