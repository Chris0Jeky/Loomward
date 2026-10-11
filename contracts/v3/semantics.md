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

### Placement reads and stored scenarios

`tiers.model`, `placement.candidates`, `placement.simulate` with `save: false`, `proposals.list`
and `proposals.get` are reads: no mutation ticket, idempotency-cache claim or state revision change.
Only `placement.simulate` with `save: true` writes a scenario to the existing `state.db` proposal
table; it never writes a file-operation manifest. A duplicate canonical inputs digest returns the
saved scenario. Saving checks the read's catalogue revision, declarations, active grants and root
generations before committing, including catalogue changes within a generation.

The catalogue adapter reads active granted roots only, in one bounded SQLite snapshot. Missing
fresh identity-matched link observations leave relief Unknown; default `verified_only` simulation
pre-rejects those groups. Entry-allocation what-ifs remain explicitly labelled assumptions. Missing
reserve preferences are disclosed as an assumed zero reserve. Unknown target clusters and hidden
descendants retain the engine's assumptions; `pre_rejected` and `excluded_volumes` are preserved.

Personal placement reads catalogue metadata only. When source volume physics are not recorded,
it refuses with `capability_unavailable`, `detail.reason = "personal_placement_physics_unobserved"`,
explaining the missing observation. It performs no per-file native opens or content reads. A
synthetic registered lab root may supply scoped volume-space observations; no other volumes are
enumerated. Unprovable ancestry across overlapping grants is `partial_coverage` with the named
reason `overlapping_grant_ancestry_unknown`.

Proposals are historical simulations. Changed catalogue revisions, declarations or root
generations mark them `stale`; inactive grants hide them from list/get. Wire IDs are bound to the
current session and catalogue instance: after restart, clients re-list rather than reuse old IDs.
Stored canonical row references and input digests survive restart and never confer effect authority.

## 4. Idempotency

| Read-only command | Idempotency | Deadline | Pagination and unknowns |
|---|---|---|---|
| `apps.list` | Safe to retry; no state or registry writes | 2,000 ms default; shares the eight-read admission pool | `limit` 1–200, required nullable `cursor`. First page observes at most 6,144 registry entries across three bounded views; provider coverage is explicit (packaged/portable remain unavailable). `estimated_size_bytes` is the registry's EstimatedSize in KiB converted to bytes, never a disk measurement; absent/malformed fields stay null with `size_reason`. Registry text stays literal, including expandable values. |
| `desktop.contexts` | Safe to retry; no file, icon, shortcut or process effects | 2,000 ms default; shares the eight-read admission pool | Five configured known folders, personal/public ownership and nullable redirection; configured paths establish neither existence nor online state. At most 200 shortcut entries per page. Synthetic sessions use injected fixtures only. Personal shortcut enumeration currently reports `desktop_enumeration_unavailable`: the existing safe reader requires an explicit path and exposes no bounded pinned-directory listing; pathname enumeration would weaken no-follow containment. It performs no desktop content or target reads. |

Both commands have `effects: none`, no events, and null catalogue/state revisions. Cursors retain
one bounded observation (maximum 4 MiB encoded snapshot), bind the command and session, and never
trigger a new provider read. Unknown/evicted/foreign cursors answer `stale_generation` with
`cursor_expired`; a cursor for the other command answers `invalid_request`. `cursor: null` refreshes
the observation. Paging itself is not `budget_hit`; incomplete provider coverage stays in its
provider state and reason. Provider calls may block in the OS after a caller's deadline; their read
permit remains held until they finish, preventing unbounded worker accumulation. An elevated
personal session refuses both reads with `personal_metadata_elevated`; unsupported platforms
report `personal_metadata_unsupported`. There is no synthetic fallback for personal sessions.

- **Session cache.** For every mutating command the service keeps, for 10 minutes, the response keyed by
  `request_id` alone, scoped to the session. Replaying the same `request_id` with a byte-identical canonical payload returns
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

- Every `Job` response and `job.state` snapshot requires unsigned `seq` and `rev` counters
  (JSON integers in `0..=2^53-1`). The engine registry assigns `seq` at submission, starting at 1,
  uniquely and increasingly across all job kinds for that engine's lifetime; `jobs.list` is newest first.
  Each job starts at `rev = 0`; every state or progress change increments it before publication.
  An unchanged read or repeated cancellation does not increment it. Clients order different jobs by
  `Job.seq` and accept same-job snapshots only at a higher `Job.rev`, regardless of delivery order.
  Both counters are engine-local, not durable: they MUST NOT be compared across engine restarts.
  On an epoch change clients discard retained job ordering and resync. `Job.seq` is distinct from
  the event envelope's stream `seq`. Missing `rev` in legacy snapshots may use a null-preserving merge.
  Bare `scan.progress` events have no job revision; revision-aware clients use the progress carried by
  `job.state` snapshots so a late progress event cannot overwrite a newer revision.
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
- A live subscriber whose unread cursor falls outside the retained replay buffer (1,024 events or
  60 seconds, whichever is smaller) receives `stream.lagged { reason: "subscriber_overflow",
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

System and own-process counters follow the fastest live lease. Process enumeration refreshes at
most every fifteen seconds, on a lease tick; coarser ticks can extend that interval.
`ProcessRow.observed_at` retains the
process enumeration time, including in newer `TelemetrySample`s; process list and explanation
timestamps refer to that enumeration too. Rates use the interval between process observations.
An engine thread count not observed on the current tick is null. Requests for channels outside
the sampled lease union return `partial_coverage` (not sampled), distinct from hardware unavailable.

## Duplicate inspection (LW-012 / LW-013)

`duplicates.inspect` starts a retained `duplicates` job for one active registered synthetic lab root.
It is a mutation of the job registry: request_id is idempotent per session for ten minutes, like
scan.start; a timed-out submission has an unknown outcome and is recovered with the same request_id
or jobs.list (kinds=["duplicates"]). One active inspection per root returns busy with detail.job_id.
Submission uses the mutation permit/FIFO and call deadline. Catalogue selection has a bounded SQL
work budget and deadline; a query refusal produces no content read or job. Content work runs on one
reserved scan worker, never the request thread.

The required budget names max_candidates (2..200), max_bytes (default-stream bytes across all
passes, both comparison inputs), max_ms (1..60000, including metadata hydration), and optional
min_bytes (default 1). Empty files are never selected. Visible, non-shielded, non-excluded files are
grouped by exact logical size; only sizes with at least two names are candidates. Selection above
the cap sets truncated=true, partial=true and stop=entry_budget unless a later stop takes precedence.
A cap can omit an entire trailing size group; truncated never means those names were absent.
Catalogue selection also caps aggregate JSON-encoded relative names at 64 KiB, with the same
truncated flag and stop. An unrepresentable candidate path is omitted, leaving partial=true and
stop=path_depth or path_length unless a budget or cancellation stop takes precedence; other pairs
remain inspectable. Reader groups and link sets exceeding the wire bounds are truncated into a
partial report with stop=report_budget unless an earlier stop takes precedence.

Terminal job status remains recoverable for ten minutes. Duplicate evidence is retained for at
most ten minutes, 50 reports and 4 MiB of JSON-encoded reports per engine, whichever expires first.
Newest reports are retained first; eviction removes job.duplicates, increments rev and publishes
job.state without changing the terminal outcome. jobs.list returns at most 50 jobs; retained report
bytes plus bounded job metadata fit below the HTTP client's 8 MiB response limit. Tauri carries the
same envelope and imposes no separate response-byte limit.

jobs.get/jobs.list and terminal job.state carry optional job.duplicates. Its entries are root-relative
names and logical byte sizes; groups/links/skipped.entry refer to indices in entries. Groups are
verified equal default streams at observation time. Link sets are one native object (including a
single visible name when native nlink>1), with no space to reclaim, and are not compared as duplicates.
links_not_compared is the number of listed names in those link sets. skipped gives refusal reasons;
bytes_read, partial, truncated and nullable stop preserve negative and incomplete evidence.
No result authorises deleting, moving, renaming or creating links. No reclaim estimate is supplied.

Personal sessions are refused before resolving or opening a root, with capability_unavailable and
detail.reason=content_read_gated. The engine repeats the synthetic-grant admission before metadata
and content I/O. Scan file identity, size, attributes and creation/write/change ticks must match
fresh root-relative metadata before the existing handle reader compares content. Root revocation
uses the existing root cancellation fence and refuses the report: after revocation `jobs.get` and
`jobs.list` carry no report, including evidence from an already-terminal job. Events published before
the revocation (the replay ring) and responses already cached for idempotent replay are not rewritten
and may still hold the earlier report (tracked for redaction). Removing terminal evidence increments rev and
publishes job.state once, without changing its outcome. Repeated revocation is idempotent. A pending
job's permission error is published only with its terminal state; pending evidence stays private.
`duplicates.cancel` uses the existing non-joining job cancellation seam and is idempotent; jobs.get
shows cancel_requested until the worker drains. Cancelled jobs may retain verified partial evidence
with stop=cancelled when no earlier reader stop exists; a permission refusal instead carries error
and no duplicates report. seq and rev
have the same ordering semantics as scan jobs. Personal scan gating is unchanged.
