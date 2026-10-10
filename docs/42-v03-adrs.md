# v0.3 architecture decision records

All records: **Status: proposed** (2026-10-09), pending the coordinator's acceptance; ADR-V3-08 Q1, ADR-V3-10's
personal-data use and ADR-V3-17 also wait on owner answers (doc 41 section 18). They extend, and do not
supersede, [17](17-adrs.md) and [39](39-architecture-decisions-v2.md). Each says whether it accepts or overturns a
driver default from the v0.3 brief. "Two-way" means cheap to undo; "one-way" means costly.

**Revision 2 (after the cross-vendor review on PR #110).** ADR-V3-05, 06, 07, 10, 13, 15 and 16 carry an
"Amended" paragraph saying what changed and why; ADR-V3-18 to 22 are new. The per-finding disposition is in
doc 41 section 20.

## ADR-V3-01: Embed the engine in the shell process; keep the service boundary a trait

**Context.** Doc 04's target process table separates shell and catalogue engine; LW-017 plans an authenticated
named pipe. v0.3 has one user, one machine and no second engine consumer yet.
**Decision.** The engine is a library linked into `loomward-desktop` (Tauri) and `loomward-serve` (HTTP). Both
adapters call the same `ViewService` trait. A lock file stops two instances serving one dataset class.
**Consequences.** Good: no IPC authentication, no daemon lifecycle, one process to profile. Bad: an engine panic
closes the window; the WebView2 renderers stay isolated anyway. Neutral: LW-017 becomes an adapter behind the
same trait.
**Alternatives.** Separate engine daemon with named-pipe IPC now: rejected, it adds pipe ACLs, peer
authentication and supervision before anything needs them. Python sidecar engine: rejected, Python is the
reference, not the performance architecture (doc 25).
**Reversibility.** Two-way. Reopen when a second consumer (MCP v3, another app) needs the engine while the shell is closed.

## ADR-V3-02: Crate boundaries, names and a glob workspace (accepts the driver default, extends it)

**Context.** The driver proposed `loomward-windows`, `loomward-catalog`, an engine/service layer and an API
adapter. The backlog names `loomward-catalogue`, `loomward-protocol` and `loomward-service`. Eight lanes will
add crates in parallel.
**Decision.** Crates: `loomward-core` (exists), `loomward-windows`, `loomward-telemetry` (added by the
coordinator in #109 for read-only sampling), `loomward-lab`, `loomward-catalog` (the
driver's spelling; backlog paths `loomward-catalogue/...` map to it), `loomward-learn`, `loomward-engine`,
`loomward-protocol`, `loomward-service`, `loomward-http`; `native/` stays outside the workspace. Root
`Cargo.toml` uses `members = ["crates/*"]`; the coordinator pre-declares approved dependencies in
`[workspace.dependencies]`: `rusqlite` (bundled), `windows-sys` 0.61, `crossbeam-deque`, `crossbeam-channel`,
`axum` and `tokio` (HTTP crate only), `jsonschema` (dev only), `hmac`, `getrandom`, `time`, plus the existing
`serde`, `serde_json`, `sha2`, `tempfile`. No `rayon` (unbounded task queues, no cooperative cancellation) and
no async runtime outside the HTTP adapter and Tauri.
**Consequences.** Good: lanes never edit the member list; leaves stay pure and portable; only
`loomward-windows` holds `unsafe`. Bad: nine crates to version together. Neutral: `Cargo.lock` stays
coordinator-owned and regenerated at integration.
**Alternatives.** One `loomward-native` crate with modules: rejected, every lane would collide in one manifest
and one `lib.rs`. Separate engine and service merged: rejected, the service (DTO mapping, policy) changes at the
contract's pace, the engine at the algorithm's.
**Reversibility.** Two-way (crates can merge). Review if crate count slows builds by over 30%.

## ADR-V3-03: One typed view-model service, one dispatch command per adapter (accepts the driver default)

**Context.** The same UI must run in Tauri and in a browser for development and Playwright.
**Decision.** A closed list of 45 commands and 10 events defined in `contracts/v3/`. Tauri exposes exactly two
commands (`lw_call`, `lw_events` with a `Channel`); HTTP exposes `POST /api/v3/call` and
`GET /api/v3/events` (SSE). Long work returns a `Job` and reports through events.
**Consequences.** Good: adapters are thin and identical in meaning; the Tauri capability file lists two
commands; one TS client. Bad: per-command typing happens in the service, not in Tauri's macro layer. Neutral:
`commands.json` is the review surface for any new command.
**Alternatives.** One Tauri command per operation: rejected, it duplicates the HTTP routing and widens the
capability file. GraphQL or gRPC-web: rejected, heavy for a loopback app and harder to bound.
**Reversibility.** Two-way. Review if a command needs binary payloads (thumbnails) beyond JSON.

## ADR-V3-04: All byte quantities are decimal strings on the wire

**Context.** Doc 26 requires decimal strings for authority-bearing bytes; v0.3 has no authority-bearing values
but many byte values above 2^53 are possible in sums.
**Decision.** Every byte field uses `ByteCount` (decimal string) or `NullableBytes` (`null` = unknown).
Counts are integers below 2^53. Timestamps are RFC 3339. The TS side converts to `number` in one helper for
layout only.
**Consequences.** Good: one rule, no per-field judgement, no silent rounding. Bad: about 10% larger slices and a
parse per node. Neutral: rates (bytes per second) are display numbers, documented as such.
**Alternatives.** Numbers for display fields and strings for exact fields: rejected, every implementer would
have to decide which is which. Binary (MessagePack): rejected, opaque in Playwright and DevTools.
**Reversibility.** One-way once both sides ship; cheap now.

## ADR-V3-05: Directory enumeration by handle, default `FileIdExtdDirectoryInfo`, pending the spike; no MFT

**Context.** "A far faster WinDirStat" without elevation (invariant 2). Raw MFT and `FSCTL_ENUM_USN_DATA` need
an elevated volume handle; doc 04 forbids an MFT parser without a malformed-record corpus. The LW-101 spike is
measuring `read_dir`, `FindFirstFileExW` large fetch and `NtQueryDirectoryFileEx` right now.
**Decision.** Default strategy `file_id_extd_directory_info` (`GetFileInformationByHandleEx` on directory
handles), which returns the 128-bit file ID with sizes, allocation, attributes and timestamps in bulk. The spike
may substitute `NtQueryDirectoryFileEx` (same record) if measurably faster. Fallback
`FindFirstFileExW(FindExInfoBasic, LARGE_FETCH)` on volumes without 128-bit IDs (identity
`path_observation`). Parallel workers with work stealing; child directories opened relative to the observed
listing and verified by file ID.
**Consequences.** Good: identity for every entry without per-file handles; junction swaps detected; no
elevation. Bad: slower than MFT readers; claims are limited to measured comparisons. Neutral: the strategy is a
`DirSource` implementation, swappable per volume.
**Alternatives.** MFT read (WizTree style): rejected, needs elevation and a corpus. `std::fs::read_dir`:
kept for CI only, no file IDs. `FindFirstFileExW` everywhere: rejected as default, no file IDs.
**Reversibility.** Two-way per volume. Review on the LW-101 result (another strategy 20% faster with equal
information) or if an owner decision ever permits an elevated helper.
**Amended (review findings 2, 3, 20).** The fallback is now a capability chain: extended 128-bit IDs, then
`FileIdBothDirectoryInfo` (64-bit, width preserved, no reparse tag), then `FindFirstFileExW` (no ID). Revision 1
required matching a listed ID that the fallback cannot supply; now child directories are opened **relative to the
parent handle** (`NtCreateFile` with `RootDirectory`) and validated **after opening** (attributes, reparse tag,
`FileIdInfo`) before listing, with an explicit policy for absent IDs. Reparse, offline and recall directories are
never listed; the no-hydration guarantee is scoped to tested providers. Buffer and end-of-directory semantics are
specified. Reason: a listed attribute can change before open, and `RECALL_ON_DATA_ACCESS` directories hydrate on
enumeration.

## ADR-V3-06: SQLite catalogue, single writer thread, per-directory transactions, integer storage

**Context.** 10M rows, live UI reads during scans, readers must never see half a directory (doc 25).
**Decision.** `rusqlite` with bundled SQLite, WAL, `STRICT` tables, one writer thread owning the only read-write
connection, a pool of four read-only connections. One directory listing equals one transaction (small ones
coalesced up to 50k rows or 250 ms). `dir`/`file` rows store name plus parent, not full paths; sizes and FILETIME
as `INTEGER`. Refresh diffs each listing in its own transaction; row IDs stay stable.
**Consequences.** Good: proven, embedded, consistent reads, crash-safe commits. Bad: one writer caps throughput
(P4 is the check); deep path reconstruction needs a walk up the parent chain (bounded by depth). Neutral:
`catalog.db` is rebuildable, so corruption costs a rescan.
**Alternatives.** In-memory tree plus snapshot files: rejected, no queries, no incremental persistence. An
embedded KV store (sled, redb): rejected, we need secondary indexes and ad-hoc queries, and SQLite is the
documented plan (ADR-V2-03). DuckDB: rejected, analytical engine with weak single-row upsert behaviour for refresh.
**Reversibility.** One-way for data already written (rebuildable by rescanning, so moderate). Review if P4 or
P14 misses by over 25%.
**Amended (review findings 7, 13, 14).** "One listing, one transaction" is kept, but listings now stream in chunks
into `stage_entry` under a shared 64 MiB byte semaphore and are published atomically when `ListingDone` arrives;
revision 1's per-producer buffers were unbounded for huge directories. Only a **complete** listing may delete
or mark absent. Allocated-basis indexes (`dir_by_allocated`, `file_by_allocated`, both with `id` tie-break) are
added. Durability, grant placement and refresh reconciliation moved to ADR-V3-18 and ADR-V3-22.

## ADR-V3-07: Materialised subtree sums plus a live arena; bounded slices with fold-into-other

**Context.** The treemap and sunburst must work at 10M entries and grow live during a scan.
**Decision.** Every `dir` row stores subtree sums. During a scan a 40-byte-per-directory atomic arena holds live
sums and completion counts and writes exact sums when a subtree completes. `tree.slice` expands the largest
directories first up to a node budget and folds the rest into an `other` node computed as parent minus listed
children. Layout happens in the browser.
**Consequences.** Good: slice cost depends on `max_nodes`, not catalogue size; live growth for free. Bad: two
code paths for sums (arena when clean, SQL rollup after cancellation), so tests must compare them. Neutral:
allocated-basis ordering of tiny files is approximate and labelled.
**Alternatives.** Compute aggregates per query with recursive CTEs: rejected, seconds at 10M. Ship the whole tree
to the UI: rejected, gigabytes. Server-side layout (send rectangles): rejected, every zoom and resize would
round-trip.
**Reversibility.** Two-way (columns can be recomputed). Review if a slice needs more than 6,000 nodes.
**Amended (review findings 8, 14).** Sums are now published under revisions (ADR-V3-19), and a slice is either
`consistent` (every sum published at a revision covering its listings) or `provisional_live` (one arena read for
all directory sums, `other` clamped). Revision 1 could subtract newly committed children from an old parent. The
"approximate files" ordering is gone: allocated ordering has its own index.

## ADR-V3-08: Grants come from native surfaces, never from webview strings; volume roots refused in v0.3

**Context.** The Python server let only startup flags choose the root. A compromised or buggy UI must not widen
reach. Invariant 2: no whole-disk scans without explicit choice.
**Decision.** Desktop: `roots.request_grant` opens the native folder picker from Rust; the path never crosses
IPC. Browser: roots only from `loomward-serve --grant-root` flags. Personal disclosure grants need a native Rust
confirmation dialog, so the HTTP adapter refuses them. Volume roots, reparse/placeholder roots and roots
overlapping the state directory or another root are refused.
**Consequences.** Good: no command takes a path; XSS cannot add a root or disclose personal data. Bad: browser
mode cannot add roots interactively. Neutral: owner question Q1 can lift the volume-root refusal with a flag.
**Alternatives.** Path string in `scan.start` with validation: rejected, validation of attacker-chosen paths is
the weaker boundary. Token-holder equals owner: rejected, any local process that reads the token would gain
grant authority.
**Reversibility.** Two-way. Review on owner answer to Q1.

**Owner decision (2026-10-10, overrides "volume roots refused").** Whole volumes (`C:\`, `G:\`, `E:\`) may be personal scan roots when the owner picks them in the native grant dialog: read-only metadata, non-elevated, results in private state outside Git. Agents still never pick a volume root themselves (AGENTS.md invariant 2).

## ADR-V3-09: The production student is a Rust port; Python stays the oracle and evaluation lab

**Context.** The brief asks where the student lives: Rust port or Python sidecar.
**Decision.** Port the weighted multinomial Naive Bayes, tokeniser, abstention and retraction rules to
`loomward-learn`. Prove parity with fixtures exported from Python (including Unicode tokenisation cases).
Python keeps evaluation (LW-025), experiments and the reference server.
**Consequences.** Good: no Python runtime in the desktop app; inference beside the catalogue; one process.
Bad: two implementations to keep in step; the fixture export is the contract. Neutral: richer models (LW-024)
are evaluated in Python first and ported only if they win.
**Alternatives.** Python sidecar: rejected, it adds a supervised child, an IPC protocol and an interpreter
dependency for about 150 lines of arithmetic. ONNX runtime: rejected, unnecessary for Naive Bayes.
**Reversibility.** Two-way. Review if a Python-only model wins LW-024/LW-025 by a margin worth a sidecar.

## ADR-V3-10: Teacher = Codex CLI child spawned by the engine, single-use digest-bound grants, canary gate

**Context.** Owner decision 4: GPT-6.1 Sol at medium effort via the local Codex CLI. Codex is an agent with
tools, and it sends context to a cloud provider. Invariant 3 requires explicit, bounded, per-grant disclosure.
**Decision.** The engine spawns `codex.cmd exec -m gpt-6.1-sol -c model_reasoning_effort=medium
--output-schema <file> -o <file>` with fixed argv, an empty per-request working directory, a minimal
environment and the sandbox flags confirmed by the LW-109 spike, inside a Job Object
(`PROC_THREAD_ATTRIBUTE_JOB_LIST`, kill-on-close, memory cap, 180 s timeout). Payloads: at most 25 items,
request-local handles, four fields with a size bucket instead of exact size, previewed and digest-bound; a grant
authorises exactly one run. Personal data needs a native dialog and a passed canary test (the teacher cannot
read a file outside its empty directory when a filename asks it to). Results go to `teacher_label`, never to
human feedback. No retries, no fallback.
**Consequences.** Good: disclosure is inspectable before it happens and auditable after; tool misuse is tested,
not assumed away. Bad: per-item latency is high; batches are small. Neutral: until LW-052 lands, synthetic
teacher experiments run from the Python lab.
**Alternatives.** UI spawns Codex: rejected, the webview has no process rights by design. Python teacher module
as the production path: rejected with ADR-V3-09. Direct HTTPS to an OpenAI API: rejected, the owner chose the
Codex subscription and Loomward would then hold a credential.
**Reversibility.** Two-way. Review if the canary fails (synthetic-only, consider a local model) or if Codex
gains a documented tool-free mode.
**Amended (review findings 1, 12, 17, 26; this replaces the canary gate).** Revision 1's gate was unsound: an
empty working directory does not stop absolute-path reads, `-s read-only` still permits reads, a Job Object
limits lifetime and resources but not reads or destinations, the owner's configuration wires MCP servers, and a
prompt-injection canary can "pass" because the model declined. New rule: **personal teacher use remains disabled
until the runner enforces and verifies a filesystem read allowlist (C-FS), disabled tools/MCP/plugins (C-TOOLS)
and ambient context (C-CTX), and restricted egress (C-NET); canary tests exercise enforcement with a probe
launched under the identical profile; model refusal is not a passing result.** The runner launches the resolved
native Codex binary (hash and version pinned) directly with `STARTUPINFOEX` (job list with a non-inherited job
handle, explicit stdio handle list, no breakaway, fail closed), with `-s read-only --ignore-user-config
--ignore-rules --ephemeral --skip-git-repo-check`, `--disable` for every tool feature of the pinned version, an
empty working directory, a per-request `CODEX_HOME` and a minimal environment; the effective configuration is
hashed into `runner_profile_digest`, which previews and grants bind. Grants are consumed with the request row in
`state.db` (`synchronous=FULL`) before spawning. The native dialog renders every previewed item from a stored,
immutable, complete request. C-FS (credential copy into an AppContainer-readable home) and C-NET (destination
filtering appears to need elevation) are owner decisions (Q2); until they are resolved and tested, the teacher is
**synthetic-only in v0.3**.
**Measured (PR #116, Sol spike, `docs/research/teacher-confinement.md`): partially achievable.** With the hardened
invocation (code-mode host disabled), every injection, cooperative-read and network canary produced 0 executed
tool items, blocked by the CLI's router/policy rather than by model refusal; but a direct collaboration tool stayed
callable (1 executed item) despite the disabled multi-agent flags. So the CLI's own flags do **not** establish
complete tool removal, and they establish no filesystem or egress isolation at all. Structured output works:
10/10 schema-valid 25-item batches, median 21.8 s (range 19.0-88.8 s), about 24.6k input tokens per batch. Direct
launch of the native `codex.exe` with stdin and no shell works (`codex.cmd` is a batch wrapper).
**Decision after measurement.** The teacher runner ships **synthetic-only**. The gate for personal metadata is an
**OS-enforced boundary** (AppContainer or restricted token with a filesystem read allowlist, and egress restricted
to the model endpoint) demonstrated by **enforcement canaries** run under that boundary: probe processes that must
fail to read outside the allowlist and to reach a non-endpoint host, plus the #116 tool canaries repeated inside
it. CLI flags are kept as defence in depth, not as the gate. The 180 s timeout covers the observed 88.8 s maximum.

**Owner decision (2026-10-10).** L15b is authorised: build the OS-enforced boundary (per-request credential copy readable only inside the sandbox, filesystem allowlist, egress restricted to the model endpoint). Personal metadata stays off until its enforcement canaries pass. Any elevated step, such as a firewall rule, is performed by the owner from a prepared HUMAN_TODO item, never by an agent.

## ADR-V3-11: Svelte 5 shell in TypeScript; heavy visuals as JSDoc-typed ES modules (partly overturns the default)

**Context.** The driver proposed Svelte 5 + Vite + TypeScript with visuals as framework-agnostic Canvas/WebGL
**TS** modules. An Opus implementer is building a no-build prototype in `app/prototype/` with Canvas ES modules
right now.
**Decision.** Accept Svelte 5 + Vite + TS for the shell. Overturn "TS modules" for `app/viz/`: they are plain ES
modules with JSDoc types, checked by `tsc --checkJs --noEmit`, so the no-build prototype and the Vite app import
the same files. Canvas 2D first.
**Consequences.** Good: no port from prototype to app; one copy of every layout and draw routine; the same type
checking in CI. Bad: JSDoc is wordier than TS syntax. Neutral: if the prototype is retired, the modules can be
renamed to `.ts` mechanically.
**Alternatives.** TS viz modules: rejected for now, the prototype would need a build step or a port. React:
rejected, larger runtime and no advantage for a canvas-heavy app. Keep vanilla `ui/`: rejected, ten pages of
hand-wired DOM do not scale to the requested views.
**Reversibility.** Two-way. Review when `app/prototype/` is retired.

## ADR-V3-12: Loopback HTTP adapter on axum; SSE read through `fetch` with a header token

**Context.** Browser mode is for development, Playwright and agents. Its security must equal the Python server's.
**Decision.** `loomward-http` uses `axum` on `tokio` (confined to that crate), binds `127.0.0.1` only, checks
`Host`, `Origin` (plus CLI-allowlisted loopback origins for Vite dev), a 256-bit token header, content type,
body size and chunking, and sends the Python server's security headers. Events are SSE read with `fetch`
streaming so the token stays in a header, never a URL.
**Consequences.** Good: a maintained HTTP parser instead of a hand-rolled one; the same headers as the reference.
Bad: tokio in the dependency graph. Neutral: static `app/dist` served from the same origin in non-dev runs.
**Alternatives.** Hand-written HTTP on `std::net`: rejected, request parsing is a security surface. `tiny_http`:
rejected, weaker long-lived-stream support. `EventSource`: rejected, it cannot send the token header.
**Reversibility.** Two-way.

## ADR-V3-13: Balancing is simulation over observed volumes; tiers declared or hinted; unknown heat stays ineligible

**Context.** Owner pillar (c): speed tiers, v2 allocator in Rust, real volumes, nothing moved. Doc 07: speed
is unknown until declared or measured; no automatic benchmarks.
**Decision.** Tier = owner declaration, else device hint (bus type, seek penalty), else unknown. Candidate
groups are catalogue subtrees with relief = allocated, need and transfer = logical. Heat defaults to unknown
(ineligible, as v2 rules); `mtime_proxy_whatif` exists only as a labelled what-if. The scenario is built
server-side; the request carries the goal, budget, heat policy and overrides. `planner_v2.rs` is ported beside
the unchanged v1 with fixture parity. Plans are saved as `simulation_only`; there is no approve.
**Consequences.** Good: real capacities, honest ineligibility, no forged physics. Bad: day-one plans on personal
data will mostly say "nothing eligible" until heat or pins are supplied. Neutral: LW-028 usage observation is the
path to real heat.
**Alternatives.** Benchmark each disk: rejected, writes or long reads without permission. Treat mtime as heat by
default: rejected, invariant 4 (modification is not access).
**Reversibility.** Two-way.
**Amended (review findings 4, 5, 18).** Revision 1 fed entry allocation to the planner as relief and passed
unknown flags as `null`, which the reference planner rejects as invalid input. Now: candidates are an ancestry
antichain; relief is `verified_unique_allocation` only after a budgeted link-count pass shows every object's
names inside the group, else `unknown` and pre-rejected (`relief_unknown`) unless the owner asks for the labelled
`entry_allocation_whatif`; groups sharing objects are pre-rejected; `estimate_basis` is an enum; destination
need is cluster-rounded and labelled. Unknown flags and heat are **omitted** so the planner's own defaults reject
them with its own reasons; volumes with unknown tier or capacity are excluded and listed. `rejected` keeps exact
parity with the planner; service-level exclusions go to `pre_rejected` and `excluded_volumes`.

## ADR-V3-14: Read-only telemetry under leases; own budgets are in-process caps only

**Context.** Owner pillar (d). Invariant 1 lists priority and trim as effects; doc 31 says control own work first.
**Decision.** `loomward-telemetry` (#109; the coordinator's early split from `loomward-windows`) samples memory,
CPU, processes, PDH GPU and disk counters read-only, with the field definitions in doc 41 section 11.
Subscriptions are 60 s leases; no lease, no sampling. Processes are keyed by pid plus start time. Explanations
are rule-based text. `budgets.set` changes only Loomward's own thread-pool sizes; no OS priority, I/O priority,
affinity or working-set calls anywhere in v0.3.
**Consequences.** Good: zero idle cost; no effect on other processes. Bad: scans cannot self-lower their I/O
priority yet. Neutral: LW-052 later adds Job Object limits for owned children.
**Alternatives.** `THREAD_MODE_BACKGROUND_BEGIN` for scan threads: deferred, it is a priority change and needs
LW-052's gate even on own threads. psutil-style Python telemetry: rejected for the engine (reference only).
**Reversibility.** Two-way.

## ADR-V3-15: One database pair per dataset class; a session serves exactly one class

**Context.** Invariant 3: synthetic and personal evidence never mix; human and teacher labels stay distinct.
**Decision.** `synthetic` and `personal` each get their own `catalog.db` and `state.db`. The process starts with
`--dataset synthetic|personal`; every response carries `meta.dataset_class`; the UI shows it permanently. Lab
trees (generated on real disks) are `synthetic`.
**Consequences.** Good: mixing is impossible by construction, including in training and teacher payloads. Bad:
no side-by-side view of synthetic and personal data. Neutral: browser mode defaults to `synthetic`; `personal`
needs an explicit flag.
**Alternatives.** A `dataset_class` column: rejected, one missed `WHERE` clause breaks the invariant.
**Reversibility.** Two-way.
**Amended (review finding 16).** Separate files only help if classification is trustworthy; revision 1 let any
`--grant-root` folder into the default synthetic session, where a synthetic-policy disclosure grant needs no
dialog. Now: **synthetic sessions accept only trusted fixture sources and identity-verified lab-generated roots;
arbitrary owner-selected roots are personal and are rejected by synthetic sessions.** Lab roots are registered in
the synthetic `state.db` by `loomward-lab generate --register` (which creates the root and refuses non-empty
directories) with root identity and manifest digest; `meta.dataset_class` is checked on open; provenance is
rechecked at root-grant and disclosure-grant creation.

## ADR-V3-16: Hand-authored JSON Schema is the contract; Rust DTOs hand-written, TS generated

**Context.** Rust and TypeScript implementers must work in parallel without talking.
**Decision.** `contracts/v3/view-service.schema.json` plus `commands.json` are the source of truth, owned by
one lane. Rust DTOs in `loomward-protocol` are written to match and are tested against
`contracts/v3/examples/` with the `jsonschema` crate. TS types are generated by a dependency-free Python script
(`scripts/gen_contracts_ts.py`) into `app/src/lib/contracts.gen.ts`. Changes after wave 1 go through a
coordinator-reviewed contract PR.
**Consequences.** Good: neither language is the master; examples are executable on both sides. Bad: Rust DTOs
are manual. Neutral: the generator only needs the schema subset we use (objects, enums, consts, arrays,
`anyOf`/`oneOf` nullables, `$ref`).
**Alternatives.** Generate the schema from Rust (`schemars`): rejected, it makes Rust the master and lands late.
`ts-rs`/`specta`: rejected for the same reason. Hand-written TS: rejected, drift.
**Reversibility.** Two-way.
**Amended (review findings 22, 23, 24).** Isolated `$def` validation let a wrong payload pass inside a generic
envelope. Tests now validate **complete envelopes**, discriminated through `commands.json`, with `date-time`
format checks; byte strings are range-checked to `u64::MAX` at runtime; timestamps carry a UTC pattern. Protocol
behaviour that a schema cannot express (deadlines, revision scopes, idempotency, commit-before-event, epochs,
replay, timed-out mutations) is normative in `contracts/v3/semantics.md`. `EventStream` and `DisclosureSummary`
are defined so L1a is complete.

## ADR-V3-17: The synthetic lab is capped at 2M entries on G: until the owner answers Q3

**Context.** NTFS reuses but never shrinks MFT records; a 10M-entry tier on G: leaves about 10 GiB of MFT behind.
Creating a VHDX to contain it needs elevation.
**Decision.** Default cap 2M entries per lab tier on a real volume. The L tier (10M) runs only after the owner
allows it (Q3). P2 is measured at 2M and extrapolated, clearly labelled, until then.
**Consequences.** Good: no unrequested permanent footprint. Bad: the 10M target is unproven until Q3.
**Alternatives.** Run 10M anyway: rejected, a lasting side effect on the owner's disk without consent.
**Reversibility.** Two-way.

**Owner decision (2026-10-10).** The 10M-entry lab tier is allowed on the HDD lab root `E:\loomward-lab\scale` (850 GB free); `G:` stays capped at 2M entries.

## ADR-V3-18: Refresh reconciles identity first and finalises absence only after a completed run (new)

**Context.** Review findings 6 and 7. Revision 1 deleted absent children per listing: a directory moved from
`A/x` to `B/x` was cascade-deleted if A was listed first and hit the unique `(root, file_id)` index if B was
listed first (reproduced in memory). A cancelled or limited listing could delete valid subtrees.
**Decision.** Child directories are matched by `(root_id, file_id)` before name; a known identity under a new
parent is reparented. Directories absent from a **complete** listing become `absent_pending` (hidden, not
deleted); they are deleted only when the whole run completes with every listing complete; otherwise they remain
tombstones for a later run. Files absent from a complete listing are deleted at once (their labels survive on
`object_ref`); a name now carrying a different ID gets a new row. An **incomplete** listing only upserts.
Targeted (watcher) refreshes reconcile both parents of a rename or leave a tombstone.
**Consequences.** Good: order-independent moves, no data loss from partial listings, stable node IDs across
moves. Bad: tombstones persist after cancelled runs; depth fix-ups at finalise. Neutral: directories without
native IDs match by name only and cannot be reparented.
**Alternatives.** Global mark-and-sweep at the end only, files included: rejected, files cannot be matched by ID
(hard links share IDs) and a per-directory complete listing already proves their absence. Immediate deletion
with re-insert on discovery: rejected (revision 1), loses subtrees and violates the unique index.
**Reversibility.** Two-way.

## ADR-V3-19: Aggregates are published under revisions; live views are labelled provisional (new)

**Context.** Review finding 8: a listing could commit before its ancestors' totals changed, so "other" could go
negative, and a crash lost the arena while leaving inconsistent sums.
**Decision.** `catalog_rev` increments per writer transaction. A listing transaction sets `dirty_rev` and
`subtree_rev` on the directory and its ancestors and records `dirty_run`. `DirFinal` carries its run and is
applied only if that run is active and still the directory's `dirty_run`; it sets `agg_valid_rev`. A directory's
sums are consistent iff `agg_valid_rev >= dirty_rev`. Committed reads never use inconsistent sums: the writer
repairs the region first (bottom-up rollup). Recovery repairs before a root serves slices. During a run, slices
use one arena read for all directory sums and say `provisional_live`.
**Consequences.** Good: no cross-revision subtraction; crash-safe; cursors can bind to `subtree_rev`. Bad: an
ancestor walk per listing (bounded by depth) and extra columns. Neutral: watchers reuse the repair rollup.
**Alternatives.** Recompute all aggregates at the end of a run: rejected, no live view and long windows of
staleness. Snapshot isolation via one long read transaction during scans: rejected, blocks WAL checkpoints for
minutes.
**Reversibility.** Two-way.

## ADR-V3-20: Node IDs carry row incarnation; durable references are continuity-keyed (new)

**Context.** Review findings 9 and 10: SQLite reuses the largest deleted row ID (reproduced), so
`HMAC(session, kind, row_id)` could name another file; `(volume_key, file_id)` can be reused after deletion.
**Decision.** All row tables use `AUTOINCREMENT`; rows record `born_run`; node IDs encode kind, row ID,
`born_run` and catalogue instance under an HMAC tag and are verified on use. A row is never kept when the object
under its name changes. Durable references exist only for native IDs on NTFS/ReFS with an observed creation time,
keyed by `(volume_key, file_id, creation_ft)`; a mismatch makes the old reference `unresolved`, never
auto-reattached; rebuilds and reformats leave references unresolved until the exact key is seen again.
**Consequences.** Good: a delayed label can never land on a replacement file. Bad: a file whose creation time is
rewritten loses its labels until reconciled; FAT/exFAT items cannot be labelled durably. Neutral: unresolved
labels are counted for later owner reconciliation.
**Alternatives.** Path-based reattachment: rejected, a reused path is not the same object (doc 26).
Content hashing for continuity: rejected, it needs content reads.
**Reversibility.** Two-way.

## ADR-V3-21: Three byte quantities that are never conflated (new)

**Context.** Review findings 4 and 5: hard links counted once per name, links outside a group, and directory
records that observe only the default stream made "relief" an overclaim.
**Decision.** Every byte figure is one of: **bytes by directory entry** (what the tree shows: default-stream EOF
and allocation as listed), **unique observed objects within a named scope** (deduplicated by
`(volume_key, file_id)`), or **estimated reclaimable bytes** (only when every object's links are observed inside
the group; else unknown). Alternate streams are not observed (`stream_coverage: default_stream_only`). Link
counts come from a budgeted metadata-only pass (`FileStandardInfo`) for placement candidates only.
**Consequences.** Good: invariant 4 holds for storage numbers. Bad: on day one most groups show relief
`unknown`. Neutral: the what-if path remains for exploration, labelled.
**Alternatives.** Count unique bytes everywhere: rejected, needs a link-count open per file across 10M entries.
**Reversibility.** Two-way.

## ADR-V3-22: Consent and precious state are durable; derived state is rebuildable (new)

**Context.** Review findings 11 and 12: root grants sat in the rebuildable catalogue; WAL with
`synchronous=NORMAL` can lose acknowledged commits on power loss; `ATTACH` is not atomic across two WAL files.
**Decision.** Root grants, revocations and lab registrations live in `state.db`. The writer connection sets
`main.synchronous=NORMAL` (`catalog.db`) and `st.synchronous=FULL` (`state.db`). No transaction spans both files
and no correctness property depends on it; precious writes go first. `state.db` is backed up by `VACUUM INTO` at
every start (7 kept) and before migrations. Catalogue recovery recreates roots only from active grants.
Revocation commits first, then cancels the root's run and fences the writer against later messages for that
grant. Disclosure grants are consumed with their request row before any process starts.
**Consequences.** Good: acknowledged labels, revocations and grant consumption survive power loss; corruption of
derived data cannot create or revive consent. Bad: slower `state.db` commits (small, infrequent). Neutral: the
catalogue keeps `NORMAL` for scan throughput.
**Alternatives.** One file with `FULL` everywhere: rejected, it slows 10M-row publication for no benefit.
**Reversibility.** Two-way.
