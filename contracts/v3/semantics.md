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
| `roots.forget` | Yes | `request_id`, 10 min. A root already forgotten is `not_found` under a new `request_id`: read `grants.list`, where the anonymised record (`forgotten_at` set) shows it was forgotten | `roots.list`, `grants.list` | Re-send with the same `request_id` returns the original result. After cache expiry read first; re-send only with `expected_state_rev` from that read |
| `roots.request_grant` | Yes, if the owner already picked a folder | `request_id`, 10 min | `roots.list` | Never reopen the picker automatically |
| `backups.import` | Yes, if the owner already picked a receipt | `request_id`, 10 min | `backups.list` | Never reopen the dialog automatically; a repeat import of the same label replaces its record |
| `backups.forget` | Yes | `request_id`, 10 min | `backups.list` | Re-send is safe: an absent record is `not_found` and nothing else changes |
| `diagnostics.save` | Yes, if the owner already picked a file | `request_id`, 10 min | the chosen file, outside Loomward | Never reopen the dialog automatically; a re-send of the same `request_id` returns the original outcome and never writes twice |
| `grants.create_disclosure` | Yes | `preview_id`: a preview yields at most one grant, returned again | `grants.list` | Re-send with the same `preview_id` is safe |
| `teacher.run` | Yes, and disclosure may already have happened | `grant_id`: returns the existing job, never a second request | `teacher.results`, `jobs.list` | MUST NOT create a new preview or grant to retry |
| `scan.start` | Yes | One active scan per root: a second start is `busy` with `detail.job_id` | `jobs.list` for the root | Re-send is safe |
| `scan.cancel` | Yes | Idempotent | `jobs.get` | Re-send is safe |
| `learning.refit` | Yes | One refit at a time: `busy` with `detail.job_id` | `jobs.list` | Re-send is safe |
| `jobs.cancel` | Yes | Re-cancelling a terminal job returns its unchanged state | `jobs.get` | Re-send is safe |

| `placement.simulate` with `save: true` | Yes | `request_id`, 10 min | `proposals.list`; plans are deterministic for the same inputs digest | Re-send is safe (a duplicate digest returns the saved proposal) |
| `alerts.configure` | Yes | `request_id`, 10 min | `alerts.list` | Re-send is safe: it replaces the whole threshold record with the same values |
| `budgets.set` | Yes | `request_id`, 10 min | `budgets.get` | Loomward's own pools reset at restart. After cache expiry set a value only if the owner confirms it against the value read |
| `telemetry.subscribe`, `telemetry.unsubscribe` | Yes | `subscription_id` | Unknown or expired IDs are `not_found` | Re-subscribe with a fresh lease |

`scan.start` shares the `scan_enumerate` worker cap across roots and duplicate inspection.
`budget.threads` (1..32) is a ceiling, not a reservation guarantee: admission clamps it to the
workers currently free. Without it, scans (including automatic freshness refreshes) request at most
half the configured cap, rounded up (four with the default cap of eight; one with a cap of one).
`budgets.get` reports this ceiling as `default_workers`. A job retains its admitted workers until
it finishes; there is no queue for worker exhaustion. If no workers are free, `scan.start` returns
retryable `busy` with guidance to wait or cancel a running scan, then retry. Explicit owner scans
still pre-empt automatic refreshes on their own root, or on other roots when those refreshes fill
the pool; automatic refreshes never pre-empt owner work. Lowering the cap affects subsequent
admission and does not interrupt existing jobs.

Learning is local and human-only in both dataset classes. Personal sessions opened it under
HUMAN_TODO q-11: `feedback.record`, `learning.queue`, `learning.refit` and `threads.meaning` work on
nodes of the session's active owner grants, with features read from those roots on this machine;
`learning.status.gate` is null. Each `state.db` records its dataset class, and a session refuses a
store of the other class, so personal and synthetic (lab or fixture) labels never train together.
Training reads human events only; teacher labels and preferences are separate classes and never
train the student. A label is not a preference or an operation grant, and a suggestion is advice
with relative scores, never an approval.

Revoking a root (or its grant ending `identity_changed`) redacts the stored features of every
human event on its nodes to empty values, withdraws those events from training and deletes every
stored model, since a model carries name tokens; the sweep repeats at startup and before each refit.
A revoked root's nodes cannot be labelled (`not_found`); withdrawal of an earlier label stays possible.
Redaction is permanent for that feature snapshot, including after an `identity_changed` grant is
replaced. A fresh label under an active grant records a new snapshot; it does not restore the old one.
New feedback records the feature snapshot's grant in the same transaction as the event, so moves,
pending rebinds and a rebuilt catalogue do not change its origin. Historical events without that
provenance retain the older best-effort sweep through their current binding; a migration policy for
those snapshots remains open in #370. Cleanup is retried before model reads; an unsuccessful retry
returns an error rather than serving the retained candidate. An explicit revoke can report a cleanup
error after the grant has already ended; retrying the revoke repeats cleanup.

Every teacher entry point (`teacher.preview`, `grants.create_disclosure`, `teacher.run`,
`teacher.results`) refuses a personal session with `capability_unavailable` and
`detail.reason: teacher_personal_gated` once the envelope validates, before any handler work:
q-11 opens the local student only, and the teacher's personal conditions (q-5) are unmet.

`feedback.record` binds new labels only to a resolved durable native reference. Retractions use the prior human event and remain available after exclusion, loss of resolution or root revocation. `label` is null exactly when
`retract` is true; a retraction with no prior human label is `invalid_request`. It stores the
withdrawn label at the next human revision. The durable receipt preserves the original complete
result across restart, including its learning status; replay changes only `idempotent_replay`.
Replay is checked before resolving the session-local node ID. Human events are stored only in
`human_feedback`; teacher rows remain separate and never enter this lane's refit. Unresolved human
events are excluded and counted in `learning.status.unresolved_human_events`.

`learning.refit` produces a persisted **candidate**, without promotion (LW-027). Status and queue
use the latest candidate for advice only; `calibrated` and `autonomy_allowed` remain false. Scores
are **relative scores, not probabilities**. When no current fitted candidate exists, status,
queue and every queue suggestion return `model_id: null`; the cold student abstains with
`no_training_examples`. New fits record the highest human-feedback row ID observed at commit and use that
monotonic counter for freshness, independent of the wall clock. Older stored candidates retain
their timestamp guard until explicitly refitted; no feature history is removed by this change.
Queue order is descending review priority with a node
ID tie-break. The synthetic smoke queue is bounded to 10,000 eligible files; refits accept at most
the latest unretracted human labels for at most 10,000 resolved objects, newest event first. Human history and durable replay receipts remain intact; older objects fall outside this explicit training window.
Feedback changed during a refit is `stale_generation`; retry explicitly. Cancellation is available
through `jobs.cancel` for any retained job kind; `scan.cancel` remains scan-only. Cancellation
does not join the worker; completion remains a separate job state. Refit cancellation and candidate
commit are serialized: cancellation may wait for an in-flight durable commit. Cancellation before
commit rolls back without `learning.updated`; a successful commit wins and the job completes.
The Learning view warns when human history exceeds fitted examples; this is a conservative freshness
warning, not an exact stale flag.

### Placement reads and stored scenarios

`tiers.model`, `placement.candidates`, `placement.simulate` with `save: false`, `proposals.list`
and `proposals.get` are reads: no mutation ticket, idempotency-cache claim or state revision change.
Only `placement.simulate` with `save: true` writes a scenario to the existing `state.db` proposal
table; it never writes a file-operation manifest. A duplicate canonical inputs digest returns the
saved scenario. Saving checks the read's catalogue revision, declarations, active grants and root
generations before committing, including catalogue changes within a generation.

The engine save digest retains both the candidate projection (including its empty
`excluded_volumes` member) and the simulation's excluded-volume reasons. These are part of the
historical explanation, even when the planner scenario is unchanged. Overrides are hashed in
group-ID order; their request order does not change the scenario's identity. Existing sorted
digests are unchanged, and stored details remain readable without digest recomputation.
Pre-#350 digests omitted that candidate member; older unsorted overrides also used array order.
Re-saving either legacy form can create one new row. This is a deduplication boundary, not
verification of a legacy digest against current inputs.
Historical simulations never authorise an effect.

The catalogue adapter reads active granted roots only, in one bounded SQLite snapshot. Missing
fresh identity-matched link observations leave relief Unknown; default `verified_only` simulation
pre-rejects those groups. Entry-allocation what-ifs remain explicitly labelled assumptions. Missing
reserve preferences are disclosed as an assumed zero reserve. Unknown target clusters and hidden
descendants retain the engine's assumptions; `pre_rejected` and `excluded_volumes` are preserved.

Both `placement.candidates` and `placement.simulate` expose `excluded_volumes`. Unusable observed
physics (zero capacity, free or reserve above capacity, zero cluster size, or capacity/free/reserve/
cluster above the planner's safe integer range) excludes only that volume with `physics_unusable`.
Excluded targets do not contribute cluster estimates. Selecting an unusable source refuses with
`invalid_request`, `detail.reason = "source_physics_unusable"`; healthy sources remain usable.

Personal placement reads catalogue metadata only. When source volume physics are not recorded,
it refuses with `capability_unavailable`, `detail.reason = "personal_placement_physics_unobserved"`,
explaining the missing observation. It performs no per-file native opens or content reads. A
synthetic registered lab root may supply scoped volume-space observations; no other volumes are
enumerated. Unprovable ancestry across overlapping grants is `partial_coverage` with the named
reason `overlapping_grant_ancestry_unknown`.

Volumes whose recorded roots have no active grant remain visible in `volumes.list` and
`tiers.model`, with their observed `online` state and a "No active grant" tier note. They are
excluded from the engine snapshot. The service adds them to `excluded_volumes` on both
`placement.candidates` and `placement.simulate` with `reason = "no_active_grant"`; choosing one
as the source refuses with `invalid_request`, `detail.reason = "no_active_grant"`. Candidate
groups themselves come only from active granted roots.

Each `tiers.model` volume has a required `source_eligible` boolean. It is false for offline,
unknown-tier, missing or unusable capacity/free-space observations, out-of-range planner
quantities, or recorded roots with no active grant. Clients use it to default, preserve and
enable source choices; historical scenarios remain readable when their source is ineligible.
It describes the current model snapshot, never a grant or a promise that a later call succeeds;
the service rechecks scope and observations on every placement call. Read-only/unknown writability
is not itself a source-selection refusal: the planner reports it through group rejection.

Proposals are historical simulations. Changed catalogue revisions, declarations or root
generations mark them `stale`; inactive grants hide them from list/get. Wire IDs are bound to the
current session and catalogue instance: after restart, clients re-list rather than reuse old IDs.
Free-space and capacity drift alone does not mark a scenario stale; physics is a point-in-time
simulation input, not a permission or a current feasibility promise. The UI compares free space
"now" with free space "in this simulation". A catalogue revision recording new physics still marks
it stale. Fresh lab-space reads stamp `observed_at` when the read completes, rather than reusing
the catalogue volume timestamp. Lab identity and space calls run under the call deadline; a timeout
releases the service read permit while at most eight unfinished observation helpers retain their
own permits until the OS calls return.
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
the observation. Registry EstimatedSize zero is null with a reason (often unset), never a known
zero-byte size. A malformed size is unknown for that entry without downgrading provider coverage.
Projection checks the 4 MiB encoded budget incrementally before retaining rows; it does not build
an unbounded JSON value or encoded buffer first. Application and shortcut snapshot entry bounds
are 6,144 and 400 respectively. A cursor is issued only for a successfully received response,
so a timed-out worker cannot evict a cursor a client already received.
Paging itself is not `budget_hit`; incomplete provider coverage stays in its
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
| `state_rev` | One `state.db` instance | Every committed `state.db` transaction that changes owner data: feedback, collections and members, tier declarations, grants, backup evidence (monotonic, never reused) | `ResponseMeta.state_rev`, `EventEnvelope.state_rev`, request precondition `expected_state_rev` |

Rules:

- `expected_generation` is accepted only for commands anchored in exactly one root (`tree.*`, `node.inspect`,
  `stats.breakdown` on a root or node anchor). Elsewhere it is `invalid_request`. If the root's current
  generation differs, the response is `stale_generation`.
- `expected_state_rev` is accepted only by `volumes.declare_tier`, `collections.update_members` and `roots.forget`; elsewhere it
  is `invalid_request`. A mismatch is `stale_generation` with `detail.reason = "state_rev_changed"` and nothing
  is written (section 3, step 3).
- A cursor binds the command, the complete filter set, the sort, the basis, the tie-breaker (row ID), the
  catalogue instance, the session, and revisions: `subtree_rev` of the anchor for `tree.children`; none for
  `search.query`, whose cursor is a keyset position in the live name index (below); **`catalog_rev` and `state_rev` for `collections.members` and `learning.queue`**, because
  their pages depend on `state.db` data (membership, feedback) that changes without a catalogue commit. If a
  bound revision changed, the next page is `stale_generation`; clients restart from the first page.
- While a root is being scanned its `subtree_rev` changes continuously, so paging inside it will go stale. The UI
  SHOULD page from slices (which are snapshots) during scans and use cursors once the scan finishes.
- A page that stops on its work budget returns the items found so far, `budget_hit: true`, and a cursor that
  resumes the scan after the last examined row (not the last returned row).
- `search.query` (LW-113; client principals require an owner-issued LW-067 grant through the LW-069 shared adapter) reads a name index
  that every catalogue commit keeps exact in the same transaction. Hits come in tiers, each tier folders first and
  then files in index-key order: `exact` (the whole name, or the name up to a dot), `prefix`, `token` (after a
  separator), `substring`, then for several words `token` (every word at a word start) and `substring` (every word
  anywhere). Matching is case-insensitive under SQLite's Unicode case fold. An empty `text` lists by filters
  only, `filter` hits, one extension largest first. Its cursor binds the filters and the catalogue instance but
  no revision, so watcher commits do not stale a paging user; a row changed between pages may be missed or
  shown twice. Only active grants are searched, a revoked root never appears, and a forgotten root leaves no
  index entry. `index_state` is `building` while a searched root is scanned (a first scan publishes at its end),
  `stale` when one is stale or failed; `partial` is true when any searched root lacks complete coverage, so an
  empty page is not proof of absence; `substring_searched` is false when the text, or a word of it, was too
  short for a trigram (under three characters). In v1, one character searches whole names (including the name
  up to a dot) only; two characters also search name and word starts. Matching words separately requires
  at least two characters per word for starts and three for inner matches. Ranked search additionally uses
  short token prefixes. An unsettled folder aggregate is returned with null sizes and counts and `unknown`
  coverage. A hit with cyclic or over-depth ancestry is skipped and the page is partial. A budget cut that
  cannot advance its input cursor is refused with `resource_budget`; narrow the query or increase the budget
  rather than retrying an unchanged cursor.
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
- Forget privacy also covers an event already copied into an adapter's pending queue. The adapters
  suspend admission while `roots.forget` can commit, and retained `roots.changed` events remove a
  root whose durable forgotten marker exists even if its later catalogue purge fails. Fresh root
  snapshots also exclude that marker. A pending event invalidated by redaction ends its adapter
  stream; the existing reconnect rules below recover from the last applied sequence.
  Admission is the final synchronous privacy check immediately before yielding an HTTP frame or
  passing an envelope to the native sink, with no intervening await. Bytes already admitted to the
  transport, its buffers or a client cannot be recalled. Admission stamps are internal Rust state;
  they add no envelope field, sequence number, stream reason or forgotten-root history.
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

`commands.json`'s `result` describes the complete owner reply. For a client principal under an
LW-067 grant, `search.query` and `node.inspect` use the command's `client_result` instead:
`ClientSearchPage` and `ClientNodeDetail`. The service validates the owner-shaped result before
projection and the final client result after projection. Other commands have no client result.
`ClientEntryRow` always carries node reference, kind, name, extension, extension family and
coverage. Path, size, modified time and attributes are present only when the grant discloses
them; withheld fields are absent, while disclosed unknown values remain null. Client node
detail contains only its row, root and optionally a root-relative display path; owner identity,
timestamps, subtree totals, threads, exclusions, capabilities, memberships and suggestions stay
with the owner. Unverified federated hits are not part of the client contract.

## 11. Disclosure

The Rust-owned native disclosure dialog pages synthetic summaries within its display limits;
every page must be accepted. A summary that exceeds the character or height limit returns
`resource_budget` to its caller, rather than being represented as an owner decline. Personal
disclosure remains gated.

- A teacher preview is immutable and expires after 10 minutes. It yields at most one disclosure grant.
- A disclosure grant authorises exactly one `teacher.run` with exactly the previewed request (by
  `payload_digest`) under exactly the pinned runner profile (by `runner_profile_digest`). Any mismatch is
  `permission_denied` before anything is spawned.
- The grant is consumed, and the teacher request row created, in one `state.db` transaction with
  `synchronous=FULL` **before** the process starts. A crash after consumption leaves a consumed grant and a
  request in state `interrupted`; it is never retried.
- Personal sessions are refused at every teacher entry point with `teacher_personal_gated` (section 3);
  behind that gate, personal-dataset grants are refused with `confinement_not_enforced` until the
  confinement profile in `docs/41-v03-architecture.md` section 9.2 is demonstrated by enforcement tests.

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

A requested, sampled system section remains present even when CPU capacity is unknown.
`system.cpu.logical_cpus` is required but nullable: a failed or zero native count maps to null,
while known memory and CPU readings remain visible. Numeric counts keep their existing encoding;
readers built against the earlier non-nullable v3 draft must accept null before consuming these samples.

`TelemetrySample.trends` (LW-051) is read-only and projects the same requested channels.
`trends` is a required field of every v3 `TelemetrySample`: a v3 reader may rely on it, and a
sample without it is invalid. This was added in v3 before the contract shipped, so it is a
compatibility note for any consumer built against an earlier draft, not a version bump.
The shared lease union retains at most eight points per series, no older than ten minutes:
the twelve byte-valued memory ledger fields, machine CPU fraction, disk busy fraction and
queue length (at most sixteen disk rows), and the seven quantities in each sampled process
row (at most 1,024 rows). GPU process memory remains unknown. The resulting native series
bound is 7,213; the wire bound is 8,192. Losing a channel's last lease clears its history;
the final release, expiry, shutdown or sampler unwind discards all history. Snapshot reads
do not sample or restart observation. Cached process rows do not become new points.

Point offsets and candidate windows use the sampler's monotonic clock. `observed_at` and
candidate start/end timestamps retain native observation time for display, and can move
with the wall clock. A gap larger than twice the requested interval breaks a window;
changing a series cadence discards its prior window explicitly, so a slower lease cannot
rehabilitate an earlier gap. A fresh contiguous window can then form at the new cadence.
Process series use at least the fifteen-second enumeration cadence. Null points stay
null and are drawn as breaks. New exact PID/creation-time instances start new series;
missing identity, denied handles, absent providers and enumeration gaps cannot yield growth.
Repeated disk labels cannot establish continuity and their trend values remain null;
disk labels/ordering do not grant durable device identity.

Units are fixed by `TrendMetric`: byte quantities are exact decimal strings; fractions
are numbers in [0,1]; queue lengths and bytes/s are nonnegative finite numbers. Schema
and Rust reject mismatched metric, unit or value representation. Growth calculations
subtract byte integers before approximate slope conversion; charts likewise subtract
byte integers before scaling. `slope_per_s` is a signed decimal string in the series
unit per second, the end-minus-start secant slope, not a score or probability.

Candidates use the newest contiguous run of known points, at least four samples: a null, a repeated or backward timestamp or a gap wider than twice the interval ends the run behind it, so one earlier bad point never hides a later window and no window crosses one. Process private
commit or private resident growth requires at least thirty seconds, no decreases,
at least three increases and at least 1 MiB net growth. Machine contention windows span
at least three seconds: CPU is at least 0.90 of logical capacity throughout; disk queue
is at least 2 throughout; available memory is at most 10% of contemporaneous total
throughout, with standby declining or modified bytes rising over the aligned window.
These conservative thresholds name candidates, never leaks, unnecessary processes or
an instruction to act. Facts, exact start/end values, sample counts, windows and uncertainty
travel with each candidate. `series_index` refers to this response's channel-projected
series list. At most 64 candidates are returned, machine (system) candidates first, then disks, then processes, so the cap cuts process candidates first; truncation remains explicit. A candidate's unit must agree with its `start_value` and `end_value`, like a series' points; schema and Rust reject a mismatch. No candidate
is not proof of no problem. No process or file effects are added.

Telemetry event retention keeps the existing 1,024-event count ceiling and adds a
conservative 16 MiB heap allowance (older events drop first); each retained event contains recent bounded history for system/disks and only
the process references in its at-most-twenty-row summary. This keeps the emitted
SSE line below the parser limit; full process history is served by `telemetry.snapshot`.
Omitted event process coverage is explicit and candidate indexes are remapped to
that event's series. Snapshot history covers all leased process rows. Sequence gaps remain visible. Phase diagnostics
separately report trend ingestion/event projection and snapshot trend projection wall
costs, which are not CPU attribution or whole-application overhead. The event scope projection
is also included in event mapping wall time; parent/child timers must not be summed.

## Duplicate inspection (LW-012 / LW-013)

`duplicates.inspect` starts a retained `duplicates` job for one active root of the session's own dataset
class: a registered lab root in a synthetic session, an owner-granted folder in a personal session
(owner decision q-10).
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
Retention prefers the newest job submission sequence; an oversized report is evicted even if newer than retained small reports. Eviction removes job.duplicates, increments rev and publishes
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

Personal content reads are open, local and read-only. Before resolving catalogue candidates or
opening anything, the service applies the scan's root gate: an inactive grant is permission_denied;
a personal session reads only its own owner grants (a lab root is permission_denied with
detail.reason=personal_session_requires_owner_grant) that still pass today's profile, profiles-directory
and store fences (detail.reason names the refusal, e.g. protected_profile_or_store); a synthetic
session reads only registered lab roots (synthetic_session_requires_lab_root). The engine repeats the
admission before metadata and content I/O: the grant, the engine and the durable state store must
name the same dataset class (dataset_class_mismatch otherwise), so neither session reads the other's
roots. The candidate cap is enforced before any metadata or content is opened. Every read goes
through pinned, revalidated, non-elevated handles opened without recall: cloud placeholders and
offline files are skipped (placeholder, offline) and never hydrated or downloaded, reparse points are
never followed, and a row without a 128-bit native file identity is never read (unsupported).
Nothing leaves the machine, no file is written, moved or deleted, and the job's byte and time budgets
bound all of it. `Capabilities.observation.duplicate_content_read` reports whether local,
read-only duplicate inspection is available on explicit request (Windows only). It does not grant
access to any root. `Capabilities.effects.content_read` is the legacy, always-false file-content
return flag: no command returns file content; inspection returns equality evidence, never bytes.
The UI shows the observation separately from file and process effects; mocks report it unavailable.
Scan file identity, size, attributes and creation/write/change ticks must match
fresh root-relative metadata before the existing handle reader compares content. Root revocation
uses the existing root cancellation fence and refuses the report: after revocation `jobs.get` and
`jobs.list` carry no report, including evidence from an already-terminal job: no report survives revocation. Retained `job.state` events are redacted, and `duplicates.cancel` and `jobs.cancel` receipts never contain a report. Removing terminal evidence increments rev and
publishes job.state once, without changing its outcome. Repeated revocation is idempotent. A pending
job's permission error is published only with its terminal state; pending evidence stays private.
`duplicates.cancel` uses the existing non-joining job cancellation seam and is idempotent; jobs.get
shows cancel_requested until the worker drains. Cancelled jobs may retain verified partial evidence
with stop=cancelled when no earlier reader stop exists; a permission refusal instead carries error
and no duplicates report. seq and rev
have the same ordering semantics as scan jobs.

## Identity-unverified volumes (#219)

Only NTFS and ReFS verify file identity. A root on a FAT, FAT32, exFAT or unknown filesystem is
granted identity-unverified: a personal one only after the owner confirms it (the desktop picker's
one-line native confirmation, or `--allow-identity-unverified` beside `--grant-root`; declining the
confirmation is `cancelled_by_user`). A synthetic or lab root needs no confirmation. Without such a
grant the root opens refused, as before. Such a root lists metadata only (names, sizes, dates); every
row carries `identity.quality` `unverified` and the `identity_unverified` entry flag, no
`file_id_hex`, and `durable_reference` `unavailable_filesystem`. Each relist rechecks containment
by normalised path under the granted root; a failed recheck leaves that listing partial and never
refreshes its rows. Unverified rows are never placement candidates (`placement.simulate` refuses
`identity_unverified`), never duplicate content-read inputs (they count as unknowns, so candidate
coverage is partial, and the engine refuses content reads with `identity_unverified`), and never a
durable reference. NTFS and ReFS never fall back to this mode.

## Collections (local preferences)

The four `collections.*` commands work in synthetic and personal sessions. They read catalogue
metadata and write only Loomward's FULL-synchronous state store. Membership is meaning, never a
root grant, disclosure approval or file effect; no content is read, moved, copied or linked.
Names are exact, case-sensitive keys (1-64 characters, no controls); a duplicate returns
`invalid_request` with `detail.existing_id`. At most 256 collections are stored; further creates
return `resource_budget`, never a truncated list. There is no rename command.

`collections.update_members` requires `expected_state_rev`; omission is `invalid_request`, a
mismatch is `stale_generation` / `state_rev_changed` before any state write. Adds/removes are atomic
sets of current node IDs, require durable native identity and an active root, and may not overlap.
Optional `remove_members` is a unique set of at most 500 stable `member_id` values from a members
page (omission means empty). It removes only membership in the named collection, even when the
object vanished, was replaced or its root is unavailable; it never resolves or authorises an
object. IDs use canonical `om_<positive decimal>` spelling; malformed IDs are `invalid_request`.
An ID absent from this collection is a no-op. Adding and removing the same reference is refused.
An unchanged set is a no-op. Removed or replaced objects are never rebound by path. Members bind
`object_ref`, including pending reference publication during rescans; unresolved references remain
members until explicitly removed by node ID or `remove_members`. `member_id` is a stable local
reference identifier, not an operation grant. `status` is `resolved`, `root_revoked`,
`root_unavailable` (another non-active grant state) or `unresolved`;
`entry` is present exactly when resolved, so no names, paths or zero sizes are fabricated. Collection
`member_count` includes unresolved members; `logical_bytes` is null if any member's size is unknown
or directory coverage is incomplete. The sum is virtual entry size, not reclaimable disk space.
Pages order by durable member ID and cursors bind the collection, session, catalogue instance,
`catalog_rev` and `state_rev`. Changed limits may reuse a cursor; changed bindings may not.
Member hydration shares the page deadline and its interrupt handle; expiry stops further rows
and interrupts an ongoing catalogue inspection rather than retaining a read slot for the page.

## Backups (stage 1: evidence import)

`backups.import` exists on the desktop adapter only and runs only in a personal session: a synthetic
session answers `refused` / `synthetic_session` before any dialog, HTTP answers `capability_unavailable`.
Rust opens the native open-file dialog for a restic `snapshots --json` file and, with `integrity_check`,
a second time for a `check --json` output. The path never crosses IPC. Each pick is refused by name
before it is opened when it is a network, device or verbatim namespace, a Windows device name, a link,
a reparse point or a cloud placeholder (`not_a_regular_file`), so no network, device or download I/O
starts. A regular local file is opened read-only, at most 262,144 bytes are read (`too_large` past
that), and the handle closes before parsing. Cancelling either dialog answers `cancelled_by_user` and
stores nothing. The folder and file pickers share one dialog at a time and one 2 s cooldown (`busy`).
Volume IDs are checked before any dialog opens (`not_found`). Send `deadline_ms` up to 60,000: the
call waits on the owner.

The snapshot list needs a unique 64-digit ID on every entry; only the snapshot named by
`snapshot_id` is validated strictly, so exporting just that one (`restic snapshots <id> --json`) is
enough. A check output may carry status or error lines before the summary; the service passes its
last non-empty line to the strict parser. An empty, plain-text or non-summary check document is not
an error: the evidence keeps the snapshot and refuses `integrity_not_checked`. Malformed or duplicate
JSON is `invalid_json`, a wrong shape `invalid_receipt`, and a bad manifest name or label
`invalid_source`.

Only the normalised evidence is stored, one record per `repository_label`, in `state.db`
(`backup_evidence`, state schema v6): never receipt bytes, a path, a file name, a host or user name,
or a credential. At most 64 labels are kept (`repository_limit`); importing an existing label
replaces its record and keeps its `evidence_id`. `backups.forget` removes Loomward's record only.

Observation times are the picked files' modification times, not the import time. Ages,
`stale_observation` (older than `max_age_seconds`, 604,800 s) and `invalid_observation_time`
(a future time, age null) are recomputed on every read, and `reported_recovery_evidence` with them.
Stage 1 cannot see check arguments, so `integrity_read_all_data` is always false and a clean check
still refuses `integrity_not_checked`; it has no snapshot manifest, credential observation or restore
observation, so `version_not_covered`, `credentials_unavailable`, `restore_not_succeeded`,
`partial_restore` and `unsupported_metadata` always hold and `reported_recovery_evidence` is never
true. Topology comes from `volumes.list` identity, never from text: the same declared volume for
source and target is `same_volume`; different volumes are `unknown` with basis
`different_volumes_device_unknown` until a physical device identity exists; an undeclared side is
`unknown` / `undeclared`. `proposed_drill` is data with `executable: false`; no command executes it.
No command runs restic, opens a backup repository or reads its configuration.

## Diagnostic bundles (LW-055, q-15)

`diagnostics.preview` exists on both adapters and in every dataset class. It writes nothing. It returns
`bundle`, `bytes` and `digest`: `bytes` and `digest` are the length and SHA-256 of the canonical text, the
bundle's wire value with its keys sorted, pretty-printed with a two-space indent (`serde_json` pretty, equal to
`JSON.stringify(sortedKeys(bundle), null, 2)`; the bundle holds no floating-point number). The service keeps the
last preview's bytes, one slot per session; a new preview replaces them. Past 262,144 bytes the preview is
`resource_budget`, never cut.

The bundle is an allowlist (`DiagnosticsBundle`, every object closed): build and session facts, the OS family and
build number, logical processor count and installed RAM, `health.get` warning codes and sizes without messages,
store versions, counts, jobs counted by kind and state over the newest 50, and each root by its opaque ID with
origin, grant and scan state, coverage, filesystem family and identity mode. It never carries a path, file or
folder name, volume label, serial or key, process name or argument, label, prompt, collection name, receipt or
error message. A value the service could not read is `{"unknown": "<reason>"}` with a fixed snake_case reason,
never omitted, `null` or zero; `recent_errors` is always `{"unknown": "not_retained"}`. More than 64 roots
(revoked ones count) cannot be listed whole, so `roots` is `{"unknown": "root_cap_exceeded"}`, never a cut list.

`diagnostics.save {digest}` exists on the desktop adapter only (HTTP answers `capability_unavailable`), in every
dataset class. A digest other than the held preview's is `refused` / `preview_changed` before any dialog opens.
Rust opens the native save dialog (one dialog at a time with the pickers, 2 s cooldown: `busy`). When the
chosen file exists, Loomward asks its own question naming it (Cancel the default) before replacing it. A confirmed
user cancellation is `cancelled_by_user` and writes nothing. A save dialog that never opened is
`refused` and says why: `dialog_busy` (another dialog is open) or `dialog_unavailable` (no owner window yet, no start
folder, or no dialog on the adapter). If the backend returns no path but cannot distinguish cancellation from
an error (rfd on Windows), the outcome is `refused` / `dialog_no_selection`: no location was returned, and nothing
was written. This never claims that the owner cancelled or that the dialog opened. On Windows, a chosen
location in an elevated session is `refused` / `elevated_session` before inspecting the target or asking
for replacement confirmation. Nothing is written; restart Loomward without administrator rights, then
preview and save again. The native non-elevated handle guard remains in force. A relative,
network, device or verbatim path, a device name, a stream name, a parent folder that is missing or resolves (through a junction or link) to a share, or an existing target
that is not a regular file or is a link or reparse point is `unsupported_target`; an existing online-only cloud
placeholder is `cloud_placeholder`; a write
that fails (or finds a file that appeared after the replace check) is `write_failed`. Neither writes anything: the bytes go to a temporary file beside the target that is
renamed over it, or removed. `saved` reports `bytes_written`. The path is never returned. Send `deadline_ms`
up to 60,000: the call waits on the owner. It is the only open command whose `effects` is not `none`
(`owner_picked_export`, docs/42 ADR-V3-24).

## Copy and verify (docs/47 lane C4, contract only)

| Command | Request | Result | Effects |
|---|---|---|---|
| `copy.preview` | `sources` (node IDs), `destination_node_id` (a folder under an active grant) | `CopyPreview` | none |
| `copy.approve` | `operation_id`, `digest` | `CopyApproval` | none |
| `copy.start` | `operation_id`, `digest` | `CopyStatus` | `copy_and_verify` |
| `copy.cancel`, `copy.abandon` | `operation_id` | `CopyStatus` | `copy_and_verify` (own staging only) |
| `copy.status` | `operation_id` | `CopyStatus` | none |

Synthetic sessions are served on registered lab roots only (docs/47 lane C5): every source and the destination
must sit under a lab-root grant. A personal session answers every copy command with `capability_unavailable`,
`detail.reason` `copy_effect_gated`, on both adapters, until docs/47 lane C8 opens the gate. `capabilities.effects`
has no flag that can say "synthetic only", so it stays all false and serving is decided by dataset class. A
personal session's `copy.approve` over HTTP is refused by its own rule, `copy_personal_approval_http` (docs/47
G6): personal approvals come only from the desktop's native confirmation. MCP has no copy command. `copy.cancel`
and `copy.abandon` stop the run; by default (docs/47 Q1 open, setting `loomward-copy-staging-removal` off) they
leave the staging folder in the destination parent and return its name in `staging_folder`; with the setting on,
Loomward removes only its own journaled staging, by identity. In C5 a `blocked`, `needs_attention` or interrupted
operation's only exit is `copy.abandon` (no resume yet); `copy.start` on a started operation is refused. The rules
for the lab path are these. A node ID only selects what the engine observes afresh; the manifest records the native
identities, sizes and metadata observed, and its `digest` (SHA-256 over the destination grant, parent identity and
new folder name, every exact item name in sorted order, bytes, identities and preserved metadata) is what the
owner approves. An approval is single use and expires after 30 minutes. `copy.start` rebuilds the manifest from
fresh observations and is refused unless it has no refusal and exactly the approved digest: another membership
or destination is `digest_mismatch`, other bytes or identities under the same membership are `source_changed`,
a revocation since approval is `grant_revoked`, and a destination parent that no longer opens as its pinned
identity under its grant's root (renamed, replaced, removed or moved) is `destination_changed`, before any effect.
A refusal's `node_id` names the refused entry's catalogue node when the catalogue holds that source path with the
same native file ID; it is null for an operation-wide refusal and for an entry the catalogue does not hold under
that identity, never a guessed node. v1 always creates one new folder inside the destination parent
(" (2)", " (3)" when the name is taken) and never writes into an existing folder (docs/47 Q3 open).

Recovery records detected journal/disk damage as `needs_attention` wherever the journal permits that
transition; otherwise status reports the observed attention state without inventing a journal edge.
`unjournaled_items` reports the first 64 staging-relative manifest names observed without journaled
identities, with the full total in `unjournaled_item_count`. These entries are never adopted or removed.

## Lazy meaning threads

| Command | Revisions | Retry | Effects |
|---|---|---|---|
| `threads.meaning` | `catalog_rev` and `state_rev` | Read-only; retry stale/deadline with a smaller visible set | none |

Accepts 1-6,000 unique current session node IDs. It resolves at most the first 1,000
IDs in request order, including eligibility checks and bounded feature extraction;
`budget_hit` is true exactly when this work cap omits a suffix. Missing items remain
unknown, never neutral predictions. The read deadline (default 2 seconds) interrupts
catalogue work and is checked between feature reads and predictions; expiration returns
`deadline_exceeded`, not a completed projection. A concurrent catalogue/state revision
change returns `stale_generation`. Unknown or stale IDs in the processed prefix refuse
the request. A suffix beyond the work cap is not resolved or authorised.

Personal sessions read meaning like synthetic ones (q-11): features come only from nodes of the
session's active grants, and the candidate is the session's own, fitted from the owner's labels.

Only non-excluded files with an available durable native identity are eligible. Folders,
virtual/folded nodes and files lacking identity are `not_eligible`, with null label/share/source.
Eligible files without a stored candidate return `no_model` and null `model_id`. A stored
candidate is read once per request; this does not refit, promote, create references, record
feedback or contact a teacher. Student `suggestion` uses the existing ranked relative scores,
heuristic abstention and reasons, with `calibrated=false`, `autonomy_allowed=false`, `teacher=null`.
`abstained` keeps null label/share and a neutral cloth; ranked alternatives remain inspectable.
`suggested` summarises the top ranked label and relative score in label/share. This student
projection does not overwrite human labels, preferences or collection membership.
The optional suggestion field leaves existing tree summary projections compatible.

Atlas requests at most 1,000 nodes with visible rendered cells per projection while Meaning is enabled,
keeping current sealed IDs below the HTTP ingress limit. A larger visible set gets an explicit
unknown-suffix budget notice; its inspector can still request a selected node. Navigation,
toggle-off and destruction abort the projection and ignore late replies. Explorer requests
one inspected node. Meaning crosses as horizontal weft using the existing palette; intensity
is relative score (quantised to the eight-thread tile), never a probability. The inspector
and visible-node text list carry labels, scores, abstention and reasons independently of colour.
`no_model`, `abstained` and `not_eligible` all stay undyed but weave differently (a slit across every
fourth crossing, every second thread missing, no meaning thread), and the legend names each. A human
`labelled` or `mixed` meaning (`source: "human"`) already in a slice is never replaced by the student
projection; a system aggregate or a teacher label is.

## Federated search extension and query grammar (docs/46 section 4, lane F1)

F1 lands the **contract and the parser only**. No engine serves the extension yet:
`session.hello` `features.federated_search` is `false`, and a `search.query` carrying any extension field
(even `sources: ["own"]`) answers `capability_unavailable` instead of ignoring the field. The lanes that
serve it (F2 ranking and filters, F3 freshness, F4 Everything, F9 semantic) flip the flag per engine.
Search v1 requests and pages are unchanged and stay valid.

- **Request fields (all optional; absent means v1 behaviour).** `sources` (unique, from `own`,
  `everything`, `semantic`; absent means `["own"]`), `terms` (at most 8 of `{text, mode: token|substring,
  negated}`), `extensions` (a lower-case set of at most 16, no dot), `collection_id`, `under_node_id`.
  Every field **narrows**: `terms` together with `text`, `extensions` together with `extension`, and so
  on; none replaces a v1 field. `under_node_id` and `collection_id` are opaque identifiers, never a path
  or a name.
- **Hit fields (all optional).** `source` (absent means `own`), `freshness` and `score`. `freshness` is
  `{state, at}` with `state` one of `live`, `caught_up`, `stale`, `everything_live`, `verified`,
  `semantic_stale` (docs/46 section 7); `at` names a time exactly for `caught_up`, `stale` and `verified`
  and is `null` otherwise (schema and Rust both reject the other combinations). `score` is a relative
  rank in 0..1: never a probability, not comparable across sources. `match_kind` gains `semantic`.
- **`unverified_hits` (optional page field).** Hits from an adapter that are not yet catalogue rows:
  `name`, `path` (a root-relative display path under a granted root), `root_id`, `kind`, `source`
  (`everything`), `freshness`, `match_kind`, `score`, and **no `node_id`**. No command accepts such a hit;
  native verification mints a node ID and the hit then arrives in `hits`. They are never counted in
  `hits` and a cursor never covers them (F4 binds its own).
- **No effect fields.** Every new object is closed (`additionalProperties: false`); the contract tests
  carry invalid examples for an extra field on a request and for a node ID on an unverified hit.

### Query grammar (people type it; the model fills structured fields)

`parseQuery` (TypeScript, `app/src/lib/search/grammar.ts`) and `parse_query` (Rust,
`loomward_protocol::query_grammar`) turn the find-bar text into the structured filters above. They are
pure and total, mirror each other, and are held to `fixtures/v3/query-grammar-parity.json` (every case
must give the same JSON in both; `modified_bounds` is checked against a fixed `now`). Where docs/46
section 4.1 left a reading open, this is the choice:

- **Separators** are exactly space, tab, line feed and carriage return (no Unicode whitespace), so the
  two languages cannot disagree. Case folding of keys, units and extensions is ASCII only.
- **Terms.** A bare word is a `token` term, `"quoted text"` a `substring` term, a leading `-` (directly
  before a word or a quote) negates it; a lone `-` and a `-` inside a word are literal. There are no
  escapes: a quote ends at the next quote, and an unterminated quote runs to the end of the input and
  raises `unterminated_quote`. At most 8 terms of 1 to 64 characters (code points); the rest are refused
  with `too_many_terms` or `term_too_long`. An unknown `key:value` is just a bare term.
- **Filters** are `ext:`, `size:`, `modified:`, `kind:`, `in:` and `under:`; a value may be quoted
  (`in:"Tax Papers"`). A negated filter is refused (`negated_filter`). A bad value is refused whole with
  `bad_value`, never half applied, and the text stays visible in `issues`; nothing is dropped silently.
- `ext:pdf;docx` is a set; a leading dot is stripped; repeated tokens accumulate without duplicates, up
  to 16 (`too_many_extensions`). `kind:` is `file` or `dir`.
- `size:` takes an optional comparison (`>`, `>=`, `<`, `<=`, `=`), an integer and a unit (`b`, `kb`,
  `mb`, `gb`, `tb`, powers of 1024; none means bytes). Wire bounds are inclusive, so `>` and `<` move by
  one byte; a bound that cannot be named (`<0`, above `u64`) is a bad value. No decimals.
- `modified:` takes a UTC date (`2025`, `2025-03`, `2025-03-14`, years 1601 to 9999) that means the whole
  period, or a relative age (`30d`, units `h`, `d`, `w`, `mo` = 30 days, `y` = 365 days) that must carry
  a comparison. Relative values compare the **age**: `<30d` is newer than 30 days, `>30d` older. For a
  date, `>`/`<` leave the period out and `>=`/`<=` include it. Relative bounds stay symbolic
  (`{"ago": {n, unit}}`) until `modified_bounds(now)` resolves them; nothing in the parser reads a clock.
- A later `size:`, `modified:`, `kind:`, `in:` or `under:` token replaces the same side of the earlier one
  (a range is `size:>1mb size:<2mb`); contradictions are left to the service.
- `in:` and `under:` stay names: the find bar resolves them to `collection_id` and `under_node_id`
  (docs/46 section 4, "under:<pinned node>"), and an unresolved name is shown as an unmatched chip.

## Growth and low-space alerts (LW-121)

`alerts.list` evaluates on demand (no scheduler, no background service, no network) and `alerts.configure`
replaces the owner's thresholds in `state.db` (`meta` key `alert_settings`; a preference, never a file effect).
Alerts are advice, never an action, and never an approval.

- **Low free space** is raised per volume that holds an active root grant, from the same observation
  `volumes.list` reports. It is an alert when `free < max(capacity x low_space_free_percent / 100,
  low_space_free_bytes)`. A volume whose capacity or free space is `null` (offline, or a personal volume
  whose physics are not observed yet) is `unknown` with `coverage: unknown`: suppressed, never read as zero
  free. Personal volumes stay `unknown` until a volume observation lane records their space.
- **Fast growth** is raised per active root from the catalogue's completed scan history. Each completed run
  keeps the root's logical size and a complete-coverage bit in `scan_run.counters_json`; a run without them
  (older rows, cancelled or failed runs) is not a sample. Over the runs finished inside `growth_window_days`
  the growth is the newest complete sample minus the oldest complete sample; it is an alert when that reaches
  `growth_bytes`. Fewer than two complete samples is `insufficient_history`; a newest scan that is partial (or
  complete scans bracketing only partial ones) is `incomplete_coverage`, and no growth is claimed. A shrinking
  root reports `measured_bytes: "0"` with `ok`: net growth, measured, not missing.
- **Defaults** (`AlertReport.defaults`): `notifications_enabled: false`, `low_space_free_percent: 10`,
  `low_space_free_bytes: 10 GiB`, `growth_window_days: 7`, `growth_bytes: 5 GiB`. Before the owner configures
  anything, `settings` equals `defaults`.
- **Notifications** are off until the owner enables them. This slice records the setting and lists alerts in the
  app; it wires no operating-system notification channel, and `notifications_note` says so.

## Ask (LW-118, docs/46 section 8)

- **Who may call.** Every `ask.*` command is the owner's. A client principal (`ask:local`, `mcp:<name>`) reaches
  `search.query` and `node.inspect` only and is refused `permission_denied` on these, whatever its grant says.
- **Profile.** `ask.configure` exists on the Tauri adapter only (docs/46 section 8.1: profiles are created on the
  desktop); HTTP answers `capability_unavailable`. A profile is `local` only for an `http` loopback endpoint with an
  explicit port and a model name that is not remote (`:cloud`, `-cloud`); anything else is refused,
  `capability_unavailable` with `detail.reason` `ask_cloud_gated`, because q-5's cloud conditions are not met. There
  is no default profile and nothing is discovered: no endpoint is contacted until the owner configured one and asked
  a question. `ask.status` never contacts it.
- **Grant.** `ask.grant` replaces the standing `ask:local` client grant (LW-067) over active granted roots and the
  optional fields `path`, `size`, `modified`, `attributes`; `ask.revoke` ends it. The grant is read on every tool
  call, so a revocation holds from the next call and the next turn. It does not cancel a question already
  running: that turn finishes with what it already looked up, and its later rounds re-send that conversation
  (rows read before the revocation included) to the same loopback model; it gets nothing new. For the local class there is **no per-round
  preview**: the grant is the control (docs/46 section 11 Q3 recommendation; pending the owner's decision).
- **Turns.** `ask.turn` returns at once with `state: running` (a turn may take up to 180 s, longer than any call
  deadline) and `ask.result` reads it. One turn runs at a time (`busy`). It is refused before anything is sent when
  no profile is stored (`capability_unavailable`, `ask_not_configured`) or no grant is live (`permission_denied`,
  `ask_no_grant`). Turns are held in memory for the session only (no `ask.db` yet).
- **What a turn reports.** `rounds` holds one disclosure record per model round, logged before the round is sent:
  the SHA-256 and length of the exact request body, the profile digest and the principal; `dispatched` is false only
  when nothing was sent. `tool_calls` lists every call the model made, never its arguments; a name outside the read
  catalogue (`ask.status.tools`, every entry `read_only: true`) is refused as `unknown tool` and shown.
  `citations` are resolved from the turn's handle ledger, never from the answer text; `resolved: false` means the
  entry no longer reads for the owner. `unverified` lists handles the answer names that the ledger does not hold.
- **No authority.** The answer is text. A suggestion in it is a proposal for the owner to act on elsewhere; no Ask
  command executes, approves or drafts anything.

## Developer clutter (LW-119)

`clutter.developer` answers advice for one active granted root from the catalogue alone (recorded names and
recorded aggregates; no live path is read, no manifest is opened). It is advice, never an action and never a claim
that anything is safe to remove.

- **Who may call.** The owner. A client principal is refused `permission_denied` (it reaches `search.query` and
  `node.inspect` only). An unknown, revoked or forgotten root is `not_found` or `permission_denied` as for the
  other root reads.
- **What qualifies.** An exact-case conventional output directory directly beside a recorded manifest: Cargo
  `target`; npm `node_modules`; Python `.venv`, `venv`, `__pycache__`; .NET `bin`, `obj`; Gradle `build`, `.gradle`.
  Nested outputs inside a candidate are not listed again. Source folders, datasets, model files and a manifest with
  no adjacent output name are never candidates. `kind` reads the directory name (`build_output`,
  `dependency_install`, `environment`, `bytecode_cache`, `build_cache`), not its contents.
- **Sizes.** `logical_bytes` and `allocated_bytes` are what the catalogue observed under the directory, or `null`
  (unknown) when the directory is partial, stale, unscanned or has an unknown allocation: never zero. Observed bytes
  are not reclaimable bytes (a shared store, hard links or unique content change what a removal would free), so the
  report carries no total. Candidates are ordered by known logical size, largest first, then unknown sizes.
- **Honest limits.** `partial` is true when the root is mid-scan, stale or not fully covered, or any listed size is
  not fully known: projects may be missing. `truncated` is true when more than `limit` candidates exist.
  `budget_hit` is true when the detector reached its work budget; `candidates` is then empty and means nothing
  about the root (`budget_note` says so). A deadline expiry is still `deadline_exceeded`.
- **Not assessed.** `not_assessed` names what no provider supplies yet: regeneration cost (LW-087), model-host
  lifecycle (LW-086) and stale worktrees (LW-076). Each candidate's `regeneration_cost` is `unknown`.
