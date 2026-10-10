# loomward/3 protocol semantics (normative)

Status: **proposed** (2026-10-09, revision after review of PR #110). This document is normative for every
implementation of the view-model service (`loomward-service`), its adapters (`loomward-http`, `native/`) and its
clients (`app/`). The shapes are in `view-service.schema.json`; the command and event bindings are in
`commands.json`. MUST, MUST NOT, SHOULD and MAY are used in the RFC 2119 sense. Where this document and a
`$comment` in the schema disagree, this document wins and the schema is fixed.

## 1. Ingress order

1. The adapter enforces transport limits (HTTP: Host, Origin, token, content type, 64 KiB body, no chunking).
   Failures are transport errors (HTTP 400/403/413/415), not envelopes.
2. The body MUST parse as one JSON object without duplicate keys. Otherwise: `ResponseError` with
   `request_id: null`, code `invalid_request`.
3. `protocol` other than `loomward/3`: `unsupported_protocol`, `request_id` echoed if it was valid, else `null`.
   No command logic runs.
4. Unknown `command`: `unknown_command`. A command not offered by this adapter (`commands.json` `adapters`):
   `capability_unavailable`.
5. `payload` is decoded into the command's request type with unknown fields rejected (`invalid_request`), then
   bounds-checked (string lengths, ranges, byte-count range, array sizes).
6. Only then does the command run.

## 2. Deadlines

- `deadline_ms` is optional. Defaults: reads 2,000 ms, except `tree.slice` 5,000 ms and `search.query` 3,000 ms;
  mutations 5,000 ms. Maximum 60,000 ms; larger values are `invalid_request`.
- A deadline bounds **how long the caller waits**, not whether a mutation commits. For reads, the server stops
  work at the deadline and returns `deadline_exceeded` (or a partial page with `budget_hit: true` where the
  result type allows it). Reads have no side effects and MAY be retried freely.
- A mutation that has passed step 5 of section 1 runs to commit or to a clean failure even if its deadline
  passes. A `deadline_exceeded` response to a mutation means **outcome unknown**. Section 4 says how to recover.
- Work that cannot finish within a deadline by design (scans, refits, teacher runs) MUST be a job: the call
  returns the `Job` and progress arrives as events.
- `placement.simulate` is **never a job** in v0.3 (its result is always a `PlacementPlan`). It is synchronous,
  bounded by `node_budget`, and has a default deadline of 10,000 ms. A request the service estimates it cannot
  finish, or that overruns, is `resource_budget` or `deadline_exceeded` (retryable with a smaller `max_groups` or
  `node_budget`); a plan with `save: true` that overran is either committed and listed by `proposals.list` or
  not saved at all, never half-saved.

## 3. Recovering from an unknown outcome

A `deadline_exceeded` response to a mutation, a transport failure after the request was sent, and a restart
between request and response all leave the outcome **unknown**: the mutation may have committed. The owner may
have acted again in the meantime, so **recovery never resends a mutation blindly**. There is no "last
declaration wins" and no "resend the absolute value". Exactly three recoveries exist, tried in this order:

1. **Retry through the action key** while its outcome is retained. The action key is the `request_id` of the
   original call (retained 10 minutes in the session cache, section 4) or the durable key in the table. The
   retry MUST be byte-identical (same `request_id`, same canonical payload); the service returns the original
   outcome and never executes twice.
2. **Read current state** with the command named in the table, once the action key is no longer retained
   (cache expiry, restart, new epoch). The read shows whether the mutation landed and what the owner has chosen
   since. Do not mint a fresh `request_id` to "retry".
3. **Replay under a revision precondition**, only for commands that carry one. The client re-sends the old
   intent with `expected_state_rev` set to the `meta.state_rev` of the read in step 2. If `state.db` has changed
   since that read the call fails with `stale_generation` and `detail.reason = "state_rev_changed"`, nothing is
   written, and the client shows the owner the current state instead of replaying. A client MUST NOT replay an
   old owner choice after a read that shows a different value without asking the owner again.

Commands whose repetition cannot undo a later choice (revocation, cancellation and durable keys) are safe to
re-send as stated.

| Command | May have committed? | Retained action key | Read (step 2) | Replay (step 3) |
|---|---|---|---|---|
| `feedback.record` | Yes | `client_event_id`, forever: an identical replay returns the original result with `idempotent_replay: true` | `learning.status`, `learning.queue` | Not needed: the durable key makes a resend with the same `client_event_id` safe. Reuse with different content is `invalid_request` |
| `volumes.declare_tier` | Yes | `request_id`, 10 min | `volumes.list` | Re-declare only with `expected_state_rev` from that read |
| `collections.create` | Yes | `request_id`, 10 min | `collections.list`; a duplicate name is `invalid_request` with `detail.existing_id` | Not needed: the name is the key |
| `collections.update_members` | Yes | `request_id`, 10 min | `collections.members` | Re-send adds and removes only with `expected_state_rev` from that read; set operations are not idempotent against intervening changes |
| `roots.revoke`, `grants.revoke` | Yes | Revocation is monotone: revoking a revoked grant returns the original `revoked_at` | `roots.list`, `grants.list` | Re-send is safe |
| `roots.request_grant` | Yes, if the owner already picked a folder | `request_id`, 10 min | `roots.list` | Never reopen the picker automatically |
| `grants.create_disclosure` | Yes | `preview_id`: a preview yields at most one grant, returned again | `grants.list` | Re-send with the same `preview_id` is safe |
| `teacher.run` | Yes, and disclosure may already have happened | `grant_id`: returns the existing job, never a second request | `teacher.results`, `jobs.list` | MUST NOT create a new preview or grant to retry |
| `scan.start` | Yes | One active scan per root: a second start is `busy` with `detail.job_id` | `jobs.list` for the root | Re-send is safe |
| `scan.cancel` | Yes | Idempotent | `jobs.get` | Re-send is safe |
| `learning.refit` | Yes | One refit at a time: `busy` with `detail.job_id` | `jobs.list` | Re-send is safe |
| `placement.simulate` with `save: true` | Yes | `request_id`, 10 min | `proposals.list`; plans are deterministic for the same inputs digest | Re-send is safe (a duplicate digest returns the saved proposal) |
| `budgets.set` | Yes | `request_id`, 10 min | `budgets.get` | Loomward's own pools reset at restart. After cache expiry set a value only if the owner confirms it against the value read |
| `telemetry.subscribe`, `telemetry.unsubscribe` | Yes | `subscription_id` | Unknown or expired IDs are `not_found` | Re-subscribe with a fresh lease |

## 4. Idempotency

- **Session cache.** For every mutating command the service keeps, for 10 minutes, the response keyed by
  `(stream epoch, request_id)`. Replaying the same `request_id` with a byte-identical canonical payload returns
  the cached response. The same `request_id` with a different payload is `invalid_request` with
  `detail.reason = "idempotency_conflict"`. The cache does not survive a restart; durable keys below do.
- **Durable keys.** `client_event_id` (feedback, forever), `preview_id` (one grant), `grant_id` (one teacher
  request), one active scan per root, one active refit. Durable keys win over the session cache.
- Clients MUST generate `request_id` values that are unique per user action and reuse one only to retry that
  same action while it is retained (section 3, step 1). Recovery after expiry reads state; it never resends.

## 5. Revision and generation scopes

| Name | Scope | Changes when | Exposed as |
|---|---|---|---|
| `catalog_rev` | One `catalog.db` instance | Every committed writer transaction (monotonic, never reused) | `ResponseMeta.catalog_rev`, `EventEnvelope.catalog_rev` |
| catalogue instance | One `catalog.db` file | Rebuild after corruption or a new database | Embedded in every opaque ID and cursor; old IDs become `not_found` with `detail.reason = "catalog_instance_changed"` |
| root `generation` | One granted root | A scan run of that root reaches `completed` (the run's ID) | `RootScanState.generation`, `RootGeneration` |
| `listing_rev` | One directory's direct membership and names | That directory's completed listing changed its members | Inside cursors |
| `subtree_rev` | One directory's subtree | Any committed change at or below it, including sizes (set to `catalog_rev` along the ancestor chain in the same transaction) | Inside cursors |
| `state_rev` | One `state.db` instance | Every committed `state.db` transaction that changes owner data: feedback, collections and members, tier declarations, grants (monotonic, never reused) | `ResponseMeta.state_rev`, `EventEnvelope.state_rev`, request precondition `expected_state_rev` |

Rules:

- `expected_generation` is accepted only for commands anchored in exactly one root (`tree.*`, `node.inspect`,
  `stats.breakdown` on a root or node anchor). Elsewhere it is `invalid_request`. If the root's current
  generation differs, the response is `stale_generation`.
- `expected_state_rev` is accepted only by `volumes.declare_tier` and `collections.update_members`; elsewhere it
  is `invalid_request`. A mismatch is `stale_generation` with `detail.reason = "state_rev_changed"` and nothing
  is written (section 3, step 3).
- A cursor binds the command, the complete filter set, the sort, the basis, the tie-breaker (row ID), the
  catalogue instance, the session, and revisions: `subtree_rev` of the anchor for `tree.children`; `catalog_rev`
  for `search.query`; **`catalog_rev` and `state_rev` for `collections.members` and `learning.queue`**, because
  their pages depend on `state.db` data (membership, feedback) that changes without a catalogue commit. If a
  bound revision changed, the next page is `stale_generation`; clients restart from the first page.
- While a root is being scanned its `subtree_rev` changes continuously, so paging inside it will go stale. The UI
  SHOULD page from slices (which are snapshots) during scans and use cursors once the scan finishes.
- A page that stops on its work budget returns the items found so far, `budget_hit: true`, and a cursor that
  resumes the scan after the last examined row (not the last returned row).
- Node IDs are bound to the session and catalogue instance and carry the row's incarnation. A deleted row's ID
  is `not_found`; IDs are never reissued for another row.

## 6. Commit before event, and response ordering

- A response to a mutation is sent only after its transaction is committed at the durability level of the file
  it wrote (`state.db` `synchronous=FULL`; `catalog.db` `NORMAL`).
- An event describing a state change is emitted only after the change is committed; `EventEnvelope.catalog_rev`
  is at least the revision of that commit (`null` for events about `state.db`-only changes or telemetry).
- A client MAY receive an event before the response to the request that caused it. Clients reconcile by
  `catalog_rev`: a read whose `meta.catalog_rev` is at least an event's `catalog_rev` already reflects that event.
- `job.state` events are emitted for every transition; `completed`, `failed` and `cancelled` are terminal and
  final. Non-terminal jobs found after a restart become `failed` with `detail.reason = "engine_restarted"`.

## 7. Event streams, epochs and replay

- An **epoch** is a random identifier created at each service start. `seq` starts at 1 in every epoch and
  increases by exactly 1 per sequenced event. `seq` values from different epochs are not comparable.
- Every stream begins with `stream.hello { epoch, last_seq, oldest_replayable_seq }`. `stream.hello` is **not
  sequenced**: its envelope `seq` equals the current `last_seq`, it is never stored in the replay buffer and it
  is never replayed.
- **Heartbeat.** On both transports (SSE and the Tauri `Channel`) the service MUST re-send `stream.hello` whenever
  15 seconds pass without any other event on that stream. Clients treat 45 seconds of silence as "unavailable"
  (the app shell, PR #114) and MUST treat a repeated `stream.hello` as liveness plus an epoch check: same epoch,
  nothing to do; different epoch, resync as for `epoch_changed`.
- The service keeps a replay buffer of the last 1,024 events or 60 seconds, whichever is smaller.
- Reconnecting: HTTP sends `Last-Event-ID: <epoch>.<seq>`; Tauri passes `lastEpoch` and `lastSeq`.
  - Same epoch and `seq + 1 >= oldest_replayable_seq`: the service replays from `seq + 1` in order, then
    continues live.
  - Same epoch but older than the buffer: `stream.lagged { reason: "replay_gap", dropped: null, resync: all }`.
  - Different or unknown epoch: `stream.lagged { reason: "epoch_changed", dropped: null, resync: all }`.
- A subscriber whose queue (1,024 events) overflows receives `stream.lagged { reason: "subscriber_overflow",
  dropped, resync }` and the stream continues; the `seq` gap is visible.
- After `stream.lagged`, clients MUST refetch every resource named in `resync` before trusting incremental
  events again. Clients MUST apply events idempotently and ignore any `seq` at or below the last applied in the
  same epoch.
- HTTP streams MAY additionally send SSE comment lines, but the `stream.hello` heartbeat above is what clients
  rely on. At most four concurrent HTTP streams per service; a fifth connection receives HTTP 429.

## 8. Errors and retry

| Code | Retry? |
|---|---|
| `invalid_request`, `unsupported_protocol`, `unknown_command`, `capability_unavailable`, `permission_denied` | No; fix the request or the state |
| `not_found` | No; refetch the parent resource (IDs may have expired) |
| `stale_generation` | Yes, after refetching from the first page or re-reading the root; with `detail.reason = "state_rev_changed"` only after re-reading the state and, for an old owner choice, asking the owner again (section 3) |
| `busy` | Yes, after the job in `detail.job_id` finishes |
| `resource_budget`, `deadline_exceeded` (read) | Yes, with a narrower request |
| `deadline_exceeded` (mutation) | Section 3 only: action key, read, or revision precondition; never a blind resend |
| `device_offline`, `partial_coverage` | When the condition changes (events) |
| `cancelled` | No |
| `internal_error` | At most once, after `health.get`; never for `teacher.run` |

`retryable` in `ErrorBody` follows this table. Nothing in the service retries automatically.

## 9. Numbers, timestamps and IDs

- Byte quantities are decimal strings in `0..=18446744073709551615`. Values outside that range are
  `invalid_request`. Aggregate arithmetic is checked; overflow is an `internal_error` with
  `detail.reason = "byte_overflow"`, never a wrapped or saturated value.
- Timestamps are UTC with a literal `Z`; servers emit milliseconds. Durations and rates use monotonic clocks.
- Opaque IDs are validated by the service (tag, session, catalogue instance, incarnation) before use. A
  structurally valid but untagged or foreign ID is `not_found`, never `permission_denied` (no oracle).

## 10. Validation of complete envelopes

The generic `RequestEnvelope.payload`, `ResponseOk.result` and `EventEnvelope.data` are `object` in the schema.
Conformance tests MUST validate **complete** envelopes with the payload discriminated by `command` or `event`
through `commands.json`, so that, for example, an `EventEnvelope` with `event: "job.state"` and a data object that
is not a `JobResult` fails. Implementations MUST enable `format` validation for `date-time` in tests.

## 11. Disclosure

- A teacher preview is immutable and expires after 10 minutes. It yields at most one disclosure grant.
- A disclosure grant authorises exactly one `teacher.run` with exactly the previewed request (by
  `payload_digest`) under exactly the pinned runner profile (by `runner_profile_digest`). Any mismatch is
  `permission_denied` before anything is spawned.
- The grant is consumed, and the teacher request row created, in one `state.db` transaction with
  `synchronous=FULL` **before** the process starts. A crash after consumption leaves a consumed grant and a
  request in state `interrupted`; it is never retried.
- Personal-dataset grants are refused with `confinement_not_enforced` until the confinement profile in
  `docs/41-v03-architecture.md` section 9.2 is demonstrated by enforcement tests.

## 12. Telemetry leases

A subscription lives 60 seconds from its last `telemetry.subscribe` call. Renewal with the same
`subscription_id` extends it and may change channels and interval. When no lease is live the sampler stops. An
expired subscription produces no further events and its ID is `not_found`.
