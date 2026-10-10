# v0.3 parallel implementation plan: waves, lanes and integration order

Status: **proposed, revision 2** (2026-10-10, after the cross-vendor review on PR #110 and the #116 teacher
spike). Implements [41](41-v03-architecture.md) under the decisions in [42](42-v03-adrs.md) against
[`contracts/v3/`](../contracts/v3/), including the normative
[`contracts/v3/semantics.md`](../contracts/v3/semantics.md). Supersedes the pack ownership of
[40](40-parallel-work-packs.md) for v0.3 work only; doc 40's integration receipt and stop conditions still apply.

## Landed or in flight before this plan merged (coordinator notes)

- **L4 is done**: the v2 allocator port with 222 parity cases merged as #107 (`crates/loomward-core/src/planner_v2.rs`,
  `scripts/export_planner_v2_fixtures.py`, `fixtures/v2/planner-v2-parity.json`); L12 builds on it.
- **W0-A** is #108 (`crates/loomward-windows`); the fixture lab sits behind an opt-in `fixtures` feature.
- **Telemetry started early** as its own read-only crate, `crates/loomward-telemetry` (#109: memory, processes,
  two-sample rates, PDH disk). It is folded into the tables below: **L11 owns that crate** (adds PDH GPU) and
  `loomward-engine/src/telemetry/`; nobody creates telemetry modules in `loomward-windows`.
- The glob workspace (`members = ["crates/*"]`) arrived with #108/#109; C1 only adds the dependency table.
- **L3** (app shell) is PR #114; it treats 45 s of event-stream silence as Unavailable, so every event source
  must heartbeat (L6, L8, L9).
- **L15's spike** is PR #116 (`docs/research/teacher-confinement.md`): CLI hardening is partial; the runner ships
  synthetic-only (ADR-V3-10).
- **Contract revision 2** (this PR) changes `$defs` that L1 is mirroring in parallel; the changed list is in the
  PR description and must be picked up by L1b before its merge.

## Rules every lane follows

- **Binding errata.** Issue #117 lists the verification findings on revision 2 (N1-N6 and the partials). The
  lane each item names applies it, with its test, before merge, and links #117 in its PR.
- **Owner decisions (2026-10-10).** Whole-volume personal roots are allowed through the native grant (ADR-V3-08);
  L15b is authorised (ADR-V3-10); the 10M lab tier runs on `E:` only (ADR-V3-17).

- **One worktree, one branch, one lane.** A lane edits only the paths in its **Owns** list. A needed change
  elsewhere is a one-line request to the coordinator, not an edit.
- **Coordinator-owned files**: root `Cargo.toml`, `Cargo.lock`, `.github/workflows/`, `AGENTS.md`,
  `HUMAN_TODO.md`, `.agent-harness/`, `docs/16-implementation-status.md`, `handoff/CHECKPOINT.json`, `backlog/`,
  `docs/INDEX.md`, and `contracts/v3/` once L1 has merged. New dependencies are requested from the coordinator;
  only those pre-declared in ADR-V3-02 may be used without asking.
- **Skeleton commits.** Before waves 1 and 2 the coordinator lands a skeleton (C1, C2), so lanes add files
  instead of editing shared ones.
- **End-to-end tests.** `scripts/test_app.py` is a runner owned by L3 that discovers
  `app/tests/e2e/test_*.py`; each view lane owns its own `test_<view>.py` and never edits the runner.
- **Proving checks.** Every lane runs its listed checks plus `cargo fmt --all --check` and `git diff --check`,
  and reports exact commands, platform and results. Missing tooling is UNVERIFIED, never PASS. New behaviour
  lands with the test that pins it. A Codex (Sol) worker cannot commit in a worktree: it leaves the change ready
  with its proving output and the driver commits.
- **Invariants.** No lane adds a file or process effect, a path-accepting command, a network call outside the
  teacher runner, or a test that touches a path outside a disposable root. Synthetic data only in Git.
- **Receipts.** Doc 40's integration receipt.

## Proposed new backlog IDs (the coordinator mints them)

| ID | Title | Lane |
|---|---|---|
| LW-101 | Measure enumeration strategies and promote the winner into `loomward-windows` | W0-B, L7 |
| LW-102 | Deterministic synthetic scale lab with manifest oracles and lab-root registration | W0-B |
| LW-103 | v3 view-service contract, protocol crate and generated TS types | L1 |
| LW-104 | Svelte app shell, transports and mock mode | L3 |
| LW-105 | Woven-atlas treemap, Observatory sunburst and gauge modules | W0-C, L10 |
| LW-106 | Loopback HTTP and SSE adapter (`loomward-serve`) | L6 |
| LW-107 | Read-only system, GPU and disk telemetry samplers | L11 |
| LW-108 | Tier model and placement data path over observed volumes | L12 |
| LW-109 | Codex teacher runner, synthetic-only, with single-use grants | L15 |
| LW-110 | Rust student port with Python parity fixtures | L5 |
| LW-111 | OS-enforced teacher confinement and enforcement canaries (authorised by the owner 2026-10-10) | L15b |
| LW-112 | Wave-3 service integration and grant policy | L20 |

## Wave 0: in flight

| Lane | Model | Owns | Issues | Hand-off expected by this plan |
|---|---|---|---|---|
| W0-A | Sol high | `crates/loomward-windows/` (identity, volumes, fixture helpers), `fixtures/windows/`, `scripts/windows-fixtures.ps1`, `evidence/windows/` | LW-064, LW-003, LW-004 (#108) | `ObservedIdentity` with width-preserving IDs; `VolumeObservation` with 64-bit serial, **bytes per cluster** and `DeviceHint`; no mutation functions |
| W0-B | Sol high | `crates/loomward-lab/`, `evidence/v3/bench/` (spike results) | LW-102, LW-101 (measurement) | Generator with S/M tiers, the doc 41 §14 distributions, manifests and `generate --register <state-dir>` (creates the root, refuses non-empty directories); spike numbers per strategy at 1-16 threads on G: and E:, warm and cold |
| W0-C | Opus | `app/prototype/` | LW-105 (prototype) | Canvas ES modules; L10 extracts them to `app/viz/` |
| W0-D | Opus | `docs/41-43`, `contracts/v3/` | (this plan) | These documents, schemas and `semantics.md` |

## Wave 1: starts immediately against `contracts/v3/`

### C1: coordinator skeleton

**Owns** root `Cargo.toml`. **Does**: adds the ADR-V3-02 dependency table (including `windows-sys` features
`Wdk_Storage_FileSystem` for `NtCreateFile`). **Proves**: `cargo test --workspace` green.

### L1: protocol, contract examples and TS generation

- **Model.** Sonnet high; Haiku may author examples under L1's review.
- **Owns.** `crates/loomward-protocol/`, `contracts/v3/examples/`, `scripts/gen_contracts_ts.py`,
  `app/src/lib/contracts.gen.ts` (generated only). `contracts/v3/*.json` and `semantics.md` changes go through
  the coordinator from now on (revision 2 is the baseline).
- **Issues.** LW-103.
- **L1a.** Envelopes (`RequestEnvelope`, `ResponseEnvelope` with `ResponseMeta.catalog_rev`, `ErrorBody`,
  `ErrorCode`, `EventEnvelope` with `epoch` and `catalog_rev`, `EventName`), the 45-variant `Command` enum,
  `ViewService`, `EventStream`, `RecvOutcome`, `CloseReason`, `CallContext`, `Adapter`, `NativeDialogs` exactly as
  doc 41 §5.3, and the `DisclosureSummary` DTO.
- **L1b.** Every `$def` as a DTO with `deny_unknown_fields`; `Bytes(u64)` serialised as a decimal string and
  range-checked; UTC timestamp type; one request and one result example per command, one per event, plus
  `examples/invalid/` negatives (unknown field, number for bytes, 20-digit byte string above `u64::MAX`,
  non-`Z` timestamp, effect capability `true`, oversized arrays, an `EventEnvelope` whose `job.state` data is
  not a `JobResult`).
- **Acceptance.** **Complete envelopes** validate with payloads discriminated by `commands.json`; `date-time`
  format validation enabled; every valid example round-trips through its DTO; every invalid one fails; forbidden
  command-name pattern absent; every `Capabilities.effects` member `const false`; deterministic TS output with
  `CommandMap` and `EventMap`.
- **Proving.** `cargo test -p loomward-protocol`; clippy; `py -3 -m json.tool` on both JSON files;
  `py -3 scripts/gen_contracts_ts.py --check`.

### L2: catalogue

- **Model.** Sol high; an Opus review lens on the reconciliation and revision code (risk R10).
- **Owns.** `crates/loomward-catalog/`.
- **Issues.** LW-005, LW-009, LW-011.
- **Acceptance.**
  - **Schema and durability.** Doc 41 §6.2 for both files; `AUTOINCREMENT` row tables; `born_run`; open refuses
    foreign `application_id`, newer `user_version`, wrong `meta.dataset_class`, UNC paths; `quick_check` at open;
    writer connection sets `main.synchronous=NORMAL` and `st.synchronous=FULL`; **no transaction spans both
    files**; `VACUUM INTO` backup of `state.db` at open (7 kept) and before migrations; **root grants and lab roots
    live in `state.db`**; catalogue rebuild recreates roots only from active grants and never writes a grant.
  - **Writer API.** `BeginRun`, `StageChunk`, `ListingDone { outcome: Complete | Incomplete(reason) }`,
    `DirFinal { run, dir, sums }`, `EndRun`, `RevokeGrant` (fences later messages for that grant), `Repair`.
    The writer side of the byte semaphore; a writer failure is signalled to producers.
  - **Refresh rules (ADR-V3-18).** Publish one listing per transaction from staging. Complete listing: delete
    absent files; new row when a name's file ID changes; absent child directories become `absent_pending`.
    Incomplete listing: upsert only, never delete or mark absent. Directories match by `(root_id, file_id)` before
    name and are **reparented** on a new parent. `absent_pending` rows are deleted only at the end of a run whose
    every listing was complete; otherwise they stay as hidden tombstones. Depths fixed at finalise.
  - **Aggregate rules (ADR-V3-19).** `catalog_rev` per transaction; listing transactions set `dirty_rev`,
    `subtree_rev` and `dirty_run` along the ancestor chain; `DirFinal` applied only for the active run and
    matching `dirty_run`, setting `agg_valid_rev`; repair rollup for inconsistent regions; committed reads never
    use inconsistent sums.
  - **Identity rules (ADR-V3-20).** IDs stored with width (8 or 16 bytes) and quality; continuity-keyed
    `object_ref` with `resolved`/`unresolved`; never reattach on mismatch.
  - **Queries.** Slice in one read transaction with fold-into-other and `aggregate_state`; children by size for
    both bases (index-backed, `id` tie-break), name/modified only up to 10,000 children; cursors bound per
    `semantics.md` §5; search with a progress-handler budget and resumable cursor; breakdowns; `multilink` rebuild
    and unique-object figures.
  - **Required tests.** `A/x → B/x` in both listing orders; cancellation between the two parents; destination
    replacement; incomplete listing never deletes; row-ID reuse impossible and stale node IDs `not_found`; crash
    between listing and `DirFinal` leaves ancestors dirty and repairable; SQL repair equals brute-force sums on
    random trees; power-loss is out of scope for tests but `st.synchronous` is asserted.
- **Proving.** `cargo test -p loomward-catalog`; clippy; `cargo run -p loomward-catalog --release --example bench
  -- --rows 10000000` on an in-process generated catalogue (doc 41 §14 distributions, temp directory deleted
  afterwards) writing `evidence/v3/bench/catalog-10m.json` with P4, P6, P7, P8, P14.

### L3: app shell and transports (PR #114)

- **Model.** Sonnet high.
- **Owns.** `app/` except `app/prototype/`, `app/viz/`, `app/src/lib/contracts.gen.ts`, `app/src/views/atlas/`,
  `app/src/views/observatory/`, `app/src/views/review/` and other lanes' `app/tests/e2e/test_<view>.py`; plus the
  runner `scripts/test_app.py` and `app/tests/e2e/conftest`-style helpers.
- **Issues.** LW-104; UI half of LW-018; LW-068 intent.
- **Acceptance.** As in revision 1, plus: epoch-aware resume (`Last-Event-ID: <epoch>.<seq>`, Tauri `lastEpoch`
  and `lastSeq`); idempotent event application; `stream.lagged` refetch of the named resources; repeated
  `stream.hello` treated as liveness; 45 s of silence shows Unavailable; mutation retries reuse `request_id` and,
  for feedback, `client_event_id`, per `semantics.md` §3-4.
- **Proving.** `npm.cmd --prefix app ci|run check|run test|run build`; `py -3 scripts/test_app.py`.

### L4: planner v2 port — done (#107)

### L5: student port

- **Model.** Haiku high with a fixture oracle; escalates to Sol if parity still fails after the fixture set is
  complete.
- **Owns.** `crates/loomward-learn/` (`features.rs`, `student.rs`), `fixtures/learning-v3/`,
  `experiments/export_learning_fixtures.py`.
- **Issues.** LW-110.
- **Acceptance.** Revision 1's tokeniser and scoring parity, plus the **storage-to-training mapping** of doc 41
  §9.1 as fixtures: (a) a retraction event carrying the **withdrawn label**, which suppresses older teacher labels;
  (b) human and teacher events with per-object revisions assigned `max + 1`, including a teacher revision ordering
  case; (c) same-revision conflicting events (error) and an identical replayed event (accepted once). Python's
  `Student.fit` is the oracle for all three.
- **Proving.** `py -3 experiments/export_learning_fixtures.py --check`; `cargo test -p loomward-learn`; clippy.

### L6: loopback HTTP and SSE adapter (after L1a)

- **Model.** Opus.
- **Owns.** `crates/loomward-http/`.
- **Issues.** LW-106; loopback half of LW-017.
- **Acceptance.** Revision 1's security tests, plus: `--dataset synthetic` accepts `--grant-root` only for roots
  registered in the synthetic `state.db` with matching identity and manifest digest; `--dataset personal`
  (with `--allow-personal`) accepts owner folders under doc 41 §12; SSE `id: <epoch>.<seq>` and `Last-Event-ID`
  resume; **a `stream.hello` heartbeat after every 15 s without other events** (test: an idle stream delivers
  one within 16 s); at most four streams, the fifth gets 429; `close` called on disconnect. Until L8 lands a
  `FixtureService` answers from `contracts/v3/examples/` and also heartbeats.
- **Proving.** `cargo test -p loomward-http`; clippy.

### Supporting swarms in wave 1

- **Muse** (within `.agent-harness/delegation.json`): review lens on every wave-1 PR; bug hunts on L6 auth and
  on L2's reconciliation and revision rules (the section above's test list is the hunting ground); edge-case
  tests offered to the owning lane as patches.
- **Haiku**: contract examples under L1; fixture generation under L5.

## Wave 2: engine, service and views (after W0-A, W0-B, L1b and L2 merge)

### C2: engine and windows skeleton

**Owns** the `crates/loomward-engine/` skeleton and the `mod` lines of `crates/loomward-windows/src/lib.rs`.
Creates `loomward-engine` with its public API as signatures returning `Err(EngineError::Unavailable)` and module
stubs `scan/`, `jobs/`, `telemetry/`, `placement/`, `learning/`, `teacher/` (each `mod.rs` then owned by its
lane); adds `pub mod` stubs for `enumerate`, `jobs`, `watch`, `linkcount` in `loomward-windows`. No telemetry
modules in `loomward-windows` (they live in `loomward-telemetry`). Ownership then passes in sequence: L3's
placeholder Explorer and Grants & health views to L13; L6's `loomward-serve` `main.rs` to L8; L8's crate to L20
in wave 3; one `mod watch;` line in L7's `scan/mod.rs` to L16.

| Lane | Model | Owns | Issues | Starts | Acceptance (summary) | Proving |
|---|---|---|---|---|---|---|
| L7 scan pipeline | Sol high | `loomward-engine/src/{lib.rs, jobs/, scan/, events.rs, budgets.rs}`; `loomward-windows/src/enumerate/` | LW-006, LW-101 | After C2 | Strategy chain of doc 41 §4.1 (extended IDs, then `FileIdBothDirectoryInfo` with width kept, then `FindFirstFileExW`); **handle-relative child opens** via `NtCreateFile`; **post-open validation** before listing (reparse, offline, recall refused); ID policy for absent IDs; buffer and end-of-directory validation; `ListOutcome::Incomplete` for every non-EOF stop; chunks to staging under the shared byte semaphore with cancellable sends; arena with run-tagged `DirFinal`; cancel ack 250 ms, `CancelSynchronousIo` for blocked workers, handles closed after the call returns; restart recovery to `repairing`; event stream with epochs, replay buffer and the 15 s `stream.hello` heartbeat; S and M totals equal manifests; fixture lab: junctions and placeholders never followed or hydrated | `cargo test -p loomward-engine -p loomward-windows`; clippy; `cargo run -p loomward-lab --release -- bench scan --tier M --runs 20` (enumeration and persistence reported separately) |
| L8 service | Sonnet high | `crates/loomward-service/`; `loomward-serve` `main.rs` once L6 has merged | LW-103, LW-069 intent | After C2 and L7's first PR | Every wave-2 command; wave-3 commands `capability_unavailable` until L20; ingress order, deadlines and the 10-minute idempotency cache per `semantics.md`; node IDs with `born_run` and catalogue instance under HMAC; cursors per §5 of `semantics.md`; provenance policy (synthetic sessions: fixture and registered lab roots only); revocation commits first, cancels the run, fences the writer; every response validates as a complete envelope | `cargo test -p loomward-service`; clippy; `py -3 scripts/test_app.py --live` on an S-tier catalogue |
| L9 Tauri shell | Opus | `native/` | LW-018 | After L6, L8 | Tauri 2 build; `lw_call`, `lw_events` with `lastEpoch`/`lastSeq`; **`stream.hello` heartbeat on the Channel after 15 s of silence**; `NativeDialogs` from Rust only; the disclosure dialog renders every `DisclosureSummary` item; capability file lists the two commands; CSP. **Native interaction tests**: picker refused in a synthetic session, disclosure decline and accept paths (personal path asserts `confinement_not_enforced`), revocation event delivery, channel teardown on window close; **WebView2 P9 timing evidence** | `cargo build`/`clippy --manifest-path native/Cargo.toml`; `loomward-desktop --self-test`; native test log and screenshots in `evidence/v3/native/` |
| L10 Atlas and Observatory | Opus | `app/viz/`, `app/src/views/atlas/`, `app/src/views/observatory/`, `app/tests/e2e/test_atlas.py`, `test_observatory.py` | LW-105, part of LW-019 | When W0-C is done (mock) | Revision 1's criteria, plus visible rendering of `aggregate_state: provisional_live` and hatched unknown allocation | `npm.cmd --prefix app run check|test`; `py -3 scripts/test_app.py` |
| L11 telemetry | Sol high | `crates/loomward-telemetry/` (from #109; adds PDH GPU) and `loomward-engine/src/telemetry/` | LW-050, LW-107 | After C2 | Field definitions of doc 41 §11 and the schema renames (`private_commit_bytes`, `private_working_set_bytes`); machine-normalised CPU; PDH English paths with wildcard re-expansion and per-value status; GPU utilisation = max over engine types of summed per-type utilisation, adapters joined by LUID; global GPU memory from adapter counters, per-process never summed; disk busy separate from queue; leases; P12 against the idle baseline, P13 | `cargo test -p loomward-telemetry -p loomward-engine`; clippy; soak in `evidence/v3/bench/telemetry.json` |
| L12 placement | Sol high | `loomward-engine/src/placement/`, `loomward-windows/src/linkcount.rs` | LW-108, LW-030 (wiring) | After C2 | Tier model; **ancestry-antichain** candidates; **budgeted link-count pass** (metadata-only opens, placeholders skipped, 200k files per group); three byte quantities; `relief_basis`, `estimate_basis` enum, cluster-rounded destination estimate; `pre_rejected` (`relief_unknown`, `shares_objects_with_other_group`, `group_coverage_incomplete`); unknown flags and heat **omitted** from the scenario, never `null`; `excluded_volumes`; `rejected` byte-equal to the #107 planner's output | `cargo test -p loomward-engine placement`; clippy; a parity test feeding the scenario builder's output to both the Rust planner and the Python reference |
| L13 product views | Sonnet high plus Haiku | `app/src/views/{explorer,tiers,companion,health}/`, matching `app/tests/e2e/test_*.py` | LW-068, LW-104 | After L3 (mock), live after L8 | Revision 1's criteria, plus Tiers shows relief basis, `pre_rejected` and `excluded_volumes`; Explorer restarts paging on `stale_generation` | `npm.cmd --prefix app run check|test|build`; `py -3 scripts/test_app.py` |

## Wave 3: learning, teacher, service integration, change tracking, scale and polish

| Lane | Model | Owns | Issues | Starts | Acceptance (summary) | Proving |
|---|---|---|---|---|---|---|
| L14 learning integration | Sol high | `crates/loomward-learn/` (adds `teacher_payload.rs`, `teacher_validate.rs`), `loomward-engine/src/learning/`, `app/src/views/review/`, `app/tests/e2e/test_review.py` | LW-021, LW-026, LW-110 | After L5, L8 | Feedback stored per doc 41 §9.1: withdrawn label on retraction, unique `(object_ref, revision)` for both sources, `client_event_id` replay and conflict, labels only on durable references; refit job; `learning.queue`; `threads.meaning`; teacher payload builder with **ancestor-context screening** (`sensitive_context`) and complete-request preview serialization; strict validator | `cargo test -p loomward-learn -p loomward-engine`; app checks; `py -3 scripts/verify.py` |
| L15 teacher runner (synthetic-only) | Sol high, plus an Opus deep-review lens | `loomward-windows/src/jobs.rs`, `loomward-engine/src/teacher/`, `evidence/v3/teacher/` | LW-052 (owned teacher child only), LW-023, LW-109 | After L14 (spike #116 done) | Resolve and pin the native `codex.exe` (path, SHA-256, `--version`); launch directly with `CreateProcessW` + `STARTUPINFOEX` (job list with non-inherited job handle, stdio-only handle list, no breakaway, fail closed), stdin prompt, no shell; the #116 hardened argv; `runner_profile_digest`; grant consumed with request row before spawn, `interrupted` never retried; 180 s timeout; strict output validation; **personal grants refused with `confinement_not_enforced`**; synthetic runs only from fixture or registered lab roots; no hostile canary names sent to the real service | `cargo test -p loomward-engine teacher`; synthetic run log (schema-valid rate, latency, tokens) |
| L15b teacher confinement | Sol high, plus Opus review | `experiments/teacher-confinement/`, a later `loomward-windows/src/confine.rs` | LW-111 | Authorised (owner, 2026-10-10); after L15 | The gate for personal metadata: an **OS-enforced boundary** (AppContainer or restricted token, filesystem read allowlist, egress restricted to the model endpoint) demonstrated by **enforcement canaries** run under it: probes that must fail to read outside the allowlist and to reach a non-endpoint host, plus the #116 tool canaries repeated inside the boundary. Model refusal is never a pass | Canary logs in `evidence/v3/teacher-confinement/` |
| L20 wave-3 service integration | Sonnet high (L8's worker) | `crates/loomward-service/` in wave 3 | LW-112 | After L14; L15 for teacher commands | Dispatch for taxonomy, feedback, learning, collections, threads, teacher and disclosure commands; grant and provenance policy (synthetic-policy grants only for fixture or lab items; personal refused until LW-111); idempotency and timed-out-mutation behaviour of `semantics.md` §3 for these commands, including `teacher.run` replay returning the same job | `cargo test -p loomward-service`; `py -3 scripts/test_app.py --live` |
| L16 change tracking | Sol high | `loomward-windows/src/watch.rs`, `loomward-engine/src/scan/watch.rs`, `experiments/usn-unprivileged/` | LW-008, LW-007 (spike) | After L7 | Watch established **before** the scan; both overflow forms (`ERROR_NOTIFY_ENUM_DIR` and successful zero-byte completion) dirty the root; dirty epochs; dirtied directories relisted before a run completes; renames reconcile both parents or leave a tombstone; ancestor sums via the repair rollup; USN hypothesis reported either way | `cargo test -p loomward-engine scan`; fixture-lab overflow case |
| L17 scale and comparison | Sol high | `experiments/windows-benchmarks/`, `evidence/v3/bench/` | LW-056 | After L7, L8 | All P targets with doc 41 §15 methodology: phase-separated timings, generated 10M-row catalogue, stated distributions, `PeakPagefileUsage`, at least 20 runs for p95, one tool per cold boot, P12 against idle baseline; real-folder runs as aggregates only | Bench commands and JSON evidence |
| L18 accessibility, parity, `ui/` retirement | Sonnet high plus Opus | app-wide fixes by request to owning lanes; then the `ui/` retirement PR | LW-019, LW-096 | After L10, L13 | As in revision 1 | `py -3 scripts/test_app.py --a11y`; `py -3 scripts/verify.py --ui` |
| L19 CI | Coordinator | `.github/workflows/` | LW-002 follow-up | With L3 and L9 | App job and native build job; actions pinned by SHA | Hosted results, or named local equivalents when billing blocks CI |

## Integration order

1. **C1** (dependency table).
2. **L1a** (envelopes, traits, `DisclosureSummary`). Unblocks L6 and C2.
3. **W0-B, W0-C** as they finish.
4. **L5, L2, L6, L1b, L3 (#114)** in any order; L1b must include contract revision 2.
5. **C2** (engine API skeleton, windows module stubs).
6. **L7** first PR (portable source end to end), then **L8**, **L11**, **L12**; **L10** and **L13** on mock at any
   time; **L9** after L6 and L8.
7. Wave 3: **L14**, then **L15** and **L20**; **L16**, **L17**, **L18** in parallel; **L19** alongside L3 and L9;
   **L15b** after L15 (authorised by the owner on 2026-10-10; elevated steps are the owner's).

At every merge the coordinator runs `cargo fmt --all --check`, `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings`, `py -3 scripts/verify.py --ui`, and once `app/` exists
`npm.cmd --prefix app run check`, `test` and `build` plus `py -3 scripts/test_app.py`, and keeps
`docs/16-implementation-status.md` and `handoff/CHECKPOINT.json` current, with Python-reference and native results
kept separate (invariant 9).

## Which lanes can start right now

L1 (picking up revision 2), L2, L3 (#114, continuing), L5 can proceed immediately. L6 starts when L1a merges.
L10 starts when W0-C hands off; L13 starts on mock data once L3's shell exists. Wave 2 waits for C2, which waits
for L1b and L2. L15b is authorised and follows L15.
