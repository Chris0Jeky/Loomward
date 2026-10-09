# v0.3 parallel implementation plan: waves, lanes and integration order

Status: **proposed** (2026-10-09). Implements [41](41-v03-architecture.md) under the decisions in
[42](42-v03-adrs.md) against the contract in [`contracts/v3/`](../contracts/v3/). Supersedes the pack
ownership of [40](40-parallel-work-packs.md) for v0.3 work only; doc 40's integration receipt and stop
conditions still apply to every lane.

## Landed before this plan merged (coordinator note, 2026-10-09)

- **L4 is done**: the v2 allocator port with 222 parity cases merged as #107 (`crates/loomward-core/src/planner_v2.rs`,
  `scripts/export_planner_v2_fixtures.py`, `fixtures/v2/planner-v2-parity.json`); L12 builds on it.
- **W0-A** is #108 (`crates/loomward-windows`); the fixture lab sits behind an opt-in `fixtures` feature.
- **Telemetry started early** as its own read-only crate, `crates/loomward-telemetry` (#109, memory, processes,
  two-sample rates, PDH disk). L11 wraps that crate from `loomward-engine/src/telemetry/` instead of re-creating
  `loomward-windows/src/{processes,pdh,system}.rs`.
- The glob workspace (`members = ["crates/*"]`) arrives with #108/#109; C1 then only adds the dependency table.

## Rules every lane follows

- **One worktree, one branch, one lane.** A lane edits only the paths in its **Owns** list. A needed change
  elsewhere is a one-line request to the coordinator, not an edit.
- **Coordinator-owned files** (the session driver): root `Cargo.toml`, `Cargo.lock`, `.github/workflows/`,
  `AGENTS.md`, `HUMAN_TODO.md`, `.agent-harness/`, `docs/16-implementation-status.md`, `handoff/CHECKPOINT.json`,
  `backlog/`, `docs/INDEX.md`, and `contracts/v3/` once lane L1 has merged. New dependencies are requested from
  the coordinator; only those pre-declared in ADR-V3-02 may be used without asking.
- **Skeleton commits.** Before waves 1 and 2 the coordinator lands a skeleton (C1, C2) that creates shared
  manifests and module stubs, so lanes add files instead of editing shared ones.
- **Proving checks.** Every lane runs its listed checks plus `cargo fmt --all --check` and `git diff --check`,
  and reports exact commands, platform and results. Missing tooling is UNVERIFIED, never PASS. New behaviour
  lands with the test that pins it. A Codex (Sol) worker cannot commit in a worktree: it leaves the change ready
  with its proving output and the driver commits (AGENTS.md).
- **Invariants.** No lane adds a file or process effect, a path-accepting command, a network call outside the
  teacher runner, or a test that touches a path outside a disposable root. Synthetic data only in Git.
- **Receipts.** Doc 40's integration receipt: branch, base, head, owned files changed, criteria met or blocked,
  red/green evidence, platform, side effects, unverified claims, next task.

## Proposed new backlog IDs (the coordinator mints them in `backlog/issues.json`)

| ID | Title | Lane |
|---|---|---|
| LW-101 | Measure enumeration strategies and promote the winner into `loomward-windows` | W0-B, L7 |
| LW-102 | Deterministic synthetic scale lab with manifest oracles | W0-B |
| LW-103 | v3 view-service contract, protocol crate and generated TS types | L1 |
| LW-104 | Svelte app shell, transports and mock mode | L3 |
| LW-105 | Woven-atlas treemap, Observatory sunburst and gauge modules | W0-C, L10 |
| LW-106 | Loopback HTTP and SSE adapter (`loomward-serve`) | L6 |
| LW-107 | Read-only system, GPU and disk telemetry samplers | L11 |
| LW-108 | Tier model and placement data path over observed volumes | L12 |
| LW-109 | Codex teacher runner with single-use disclosure grants and canary gate | L15 |
| LW-110 | Rust student port with Python parity fixtures | L5 |

## Wave 0: in flight now

| Lane | Model | Owns | Issues | Hand-off expected by this plan |
|---|---|---|---|---|
| W0-A | GPT-6.1 Sol high | `crates/loomward-windows/` (identity, volumes, fixture helpers), `fixtures/windows/`, `scripts/windows-fixtures.ps1`, `evidence/windows/` | LW-064, LW-003, LW-004 | The facts listed in doc 41 section 4.1: handle-derived `ObservedIdentity`, `VolumeObservation` with 64-bit serial and `DeviceHint`; no mutation functions |
| W0-B | GPT-6.1 Sol high | `crates/loomward-lab/`, `evidence/v3/bench/` (spike results) | LW-102, LW-101 (measurement) | Generator with S/M tiers and manifests (L tier gated by Q3, ADR-V3-17); spike numbers for each strategy at 1, 2, 4, 8, 16 threads on G: and E:, warm and cold |
| W0-C | Opus 5.5 | `app/prototype/` | LW-105 (prototype) | Canvas ES modules for woven treemap, sunburst, gauges; lane L10 extracts them to `app/viz/` |
| W0-D | Opus 5.5 | `docs/41-43`, `contracts/v3/` | (this plan) | These documents and schemas |

## Wave 1: starts immediately against `contracts/v3/`

### C1: coordinator skeleton (first, small)

**Owns** root `Cargo.toml`. **Does**: `members = ["crates/*"]`; add the ADR-V3-02 dependency table to
`[workspace.dependencies]`; keep `exclude = ["native"]`. **Proves**: `cargo test --workspace` still green with
whatever W0 crates have merged.

### L1: protocol, contract examples and TS generation (can start now)

- **Model.** Sonnet 5.5 high; Haiku 5.5 workers may author examples inside L1's worktree under L1's review.
- **Owns.** `crates/loomward-protocol/`, `contracts/v3/` (including new `contracts/v3/examples/`),
  `scripts/gen_contracts_ts.py`, `app/src/lib/contracts.gen.ts` (generated only).
- **Issues.** LW-103 (supersedes LW-016's "v2 wire" naming for the view service).
- **L1a, first PR, same day.** Envelopes (`RequestEnvelope`, `ResponseEnvelope`, `ErrorBody`, `ErrorCode`,
  `EventEnvelope`, `EventName`), the 45-variant `Command` enum, `ViewService`, `CallContext`, `Adapter`,
  `NativeDialogs`, `EventSubscription` exactly as doc 41 section 5.3. Lanes L6 and C2 build on this.
- **L1b.** Every `$def` as a Rust DTO with `#[serde(deny_unknown_fields)]` and bounds validation; byte fields as
  a `Bytes(u64)` newtype serialised as a decimal string; one request and one result example per command and one
  example per event, plus `examples/invalid/` negatives (unknown field, number where bytes string expected,
  effect capability `true`, oversized arrays); the TS generator emitting every `$def` plus `CommandMap` and
  `EventMap` from `commands.json`.
- **Acceptance.** Every example validates against its `$def` (`jsonschema` dev-dependency) and round-trips
  through its DTO to an equal `serde_json::Value`; every invalid example fails; no command name matches the
  forbidden pattern in `commands.json`; every `Capabilities.effects` member is `const false`; generator output
  is deterministic and committed.
- **Proving.** `cargo test -p loomward-protocol`; `cargo clippy -p loomward-protocol --all-targets -- -D warnings`;
  `py -3 -m json.tool contracts/v3/view-service.schema.json`; `py -3 -m json.tool contracts/v3/commands.json`;
  `py -3 scripts/gen_contracts_ts.py --check`.

### L2: catalogue (can start now)

- **Model.** GPT-6.1 Sol high.
- **Owns.** `crates/loomward-catalog/`.
- **Issues.** LW-005, LW-009 (accounting columns and hard-link flags), LW-011 (query layer).
- **Acceptance.** Doc 41 section 6 schema as migration 1 for `catalog.db` and `state.db`; open refuses a
  foreign `application_id`, a newer `user_version` and UNC paths; `quick_check` at open. Writer thread API
  (`BeginRun`, `DirListing`, `DirFinal`, `EndRun`) with one listing per transaction, coalescing and a bounded
  queue. Refresh diff keeps row IDs, cascades removed subtrees, matches renames by file ID. Read API for slice
  (doc 41 section 8, including fold-into-other and `approximate_*` flags), children (keyset cursor bound to
  `listing_rev`), path, inspect, search (progress-handler budget, `budget_hit`), breakdown. A property test shows
  the SQL rollup equals brute-force sums on random trees. Simulated crash mid-transaction keeps the last
  committed listing.
- **Proving.** `cargo test -p loomward-catalog`; clippy; `cargo run -p loomward-catalog --release --example bench
  -- --rows 1000000` writing `evidence/v3/bench/catalog-1m.json` with P4, P6, P7, P8 and P14 (rows synthesised
  in-process into a temp directory deleted afterwards).

### L3: app shell and transports (can start now)

- **Model.** Sonnet 5.5 high.
- **Owns.** `app/` except `app/prototype/`, `app/viz/`, `app/src/lib/contracts.gen.ts`,
  `app/src/views/atlas/`, `app/src/views/observatory/`, `app/src/views/review/`; plus `scripts/test_app.py`.
- **Issues.** LW-104; the UI half of LW-018; LW-068 intent (paged views, never full state).
- **Acceptance.** Svelte 5 + Vite + TS scaffold with `npm.cmd --prefix app run check|test|build`; `check` runs
  `svelte-check` and `tsc --checkJs --noEmit` over `app/viz/`. Transports: `mock` (examples plus a deterministic
  synthetic slice generator up to 6,000 nodes), `http` (token from the URL fragment, `fetch`-stream SSE parser
  with reconnect and `Last-Event-ID`), `tauri` (`invoke` plus `Channel`). Views discovered by
  `import.meta.glob('./views/*/View.svelte')`; hash routes; both theme token sets; the dataset class and
  "simulation" labels always visible; disconnection shows "unavailable", never demo data. No `{@html}` (a test
  greps for it). Placeholder Explorer and Grants & health views on mock data.
- **Proving.** `npm.cmd --prefix app ci`; `npm.cmd --prefix app run check`; `npm.cmd --prefix app run test`;
  `npm.cmd --prefix app run build`; `py -3 scripts/test_app.py` (Playwright: loads in mock mode, both themes,
  hostile names render as text, no console errors).

### L4: planner v2 port (can start now)

- **Model.** GPT-6.1 Sol high.
- **Owns.** `crates/loomward-core/src/planner_v2.rs`, one `pub mod planner_v2;` line in
  `crates/loomward-core/src/lib.rs`, `fixtures/planner-v2/`, `experiments/export_planner_v2_fixtures.py`.
- **Issues.** LW-080, the simulator part of LW-030.
- **Acceptance.** Python exports inputs and `plan_tiers_v2` outputs for the 51 original cases (same seed as
  `experiments/v2_benchmarks.py`), the counterexample, node-budget cutoffs, `problem_size_limit` (13 groups),
  offline and read-only sources, unknown heat and reserve edges. Rust output equals every fixture exactly; heat
  tie-breaks use exact rationals. `planner.rs` and `tier-plan-golden.json` are untouched (invariant 6).
- **Proving.** `py -3 experiments/export_planner_v2_fixtures.py --check`; `cargo test -p loomward-core`; clippy.

### L5: student port (can start now)

- **Model.** Haiku 5.5 high with a fixture oracle; escalates to Sol (L4's worker) if parity is still failing
  after the fixture set is complete.
- **Owns.** `crates/loomward-learn/` (`features.rs`, `student.rs` only in this wave), `fixtures/learning-v3/`,
  `experiments/export_learning_fixtures.py`.
- **Issues.** LW-110.
- **Acceptance.** Fixtures from Python: token sets for at least 200 names covering case folding (`ß`, dotted
  and dotless I, Greek final sigma), combining marks, CJK, digits, underscores and size buckets; fit and predict
  cases with retractions, same-revision conflicts (errors), teacher weight 0.2 and every abstention reason. Rust
  matches tokens and reasons exactly, scores within 1e-12 relative, and `model_id` exactly. If exact Python
  `casefold` needs a Unicode crate, request it from the coordinator; do not hand-roll tables.
- **Proving.** `py -3 experiments/export_learning_fixtures.py --check`; `cargo test -p loomward-learn`; clippy.

### L6: loopback HTTP and SSE adapter (starts after L1a)

- **Model.** Opus 5.5 (security-sensitive boundary).
- **Owns.** `crates/loomward-http/` (library `serve(service, options)` and binary `loomward-serve`).
- **Issues.** LW-106; the loopback half of LW-017.
- **Acceptance.** Flags `--dataset synthetic|personal` (`personal` also needs `--allow-personal`),
  `--state-dir`, repeatable `--grant-root <dir>` (doc 41 refusal rules), `--port` (default 0), `--allow-origin`
  (loopback origins only), `--static <dir>`; prints the fragment-token URL once. Until L8 lands, a
  `FixtureService` answers from `contracts/v3/examples/` so L3 can test a real transport. Integration tests with
  raw `TcpStream` requests: wrong `Host` 403, foreign `Origin` 403, missing or wrong token 403, body over 64 KiB
  413, wrong content type 415, chunked 400, early error reply after draining the body (the LW-001 Windows
  reset), SSE without token 403, SSE heartbeat every 15 s, `stream.lagged` on overflow, non-loopback bind
  refused, security headers present on every response.
- **Proving.** `cargo test -p loomward-http`; clippy.

### Supporting swarms in wave 1

- **Muse** (within `.agent-harness/delegation.json` ceilings): fresh-context review lens on every wave-1 PR;
  bounded bug hunts on L6's auth paths and L2's refresh diff; extra edge-case tests proposed to the owning lane
  as patches, never pushed to its branch.
- **Haiku**: contract examples under L1; mechanical fixture generation under L4 and L5.

## Wave 2: engine, service and views (after W0-A, W0-B, L1b and L2 merge)

### C2: engine and windows skeleton (coordinator or Opus, first in wave 2)

**Owns** `crates/loomward-engine/` skeleton and the `mod` lines of `crates/loomward-windows/src/lib.rs`.
**Does**: creates `loomward-engine` with its public API as signatures returning
`Err(EngineError::Unavailable)` (jobs, scan, events, telemetry, placement, learning, teacher, budgets) and stub
module directories `scan/`, `jobs/`, `telemetry/`, `placement/`, `learning/`, `teacher/` (each with a `mod.rs`
that its lane then owns); adds `pub mod` stubs for `enumerate`, `processes`, `pdh`, `system`, `jobs`, `watch` in
`loomward-windows`. Afterwards each module belongs to exactly one lane. Ownership passes in sequence where a
worker continues its own area: L3's placeholder Explorer and Grants & health views pass to L13; L6's
`loomward-serve` `main.rs` passes to L8; L7's `scan/mod.rs` gains one `mod watch;` line from L16.

| Lane | Model | Owns | Issues | Starts | Acceptance (summary) | Proving |
|---|---|---|---|---|---|---|
| L7 scan pipeline | Sol high (W0-A's worker) | `loomward-engine/src/{lib.rs, jobs/, scan/, events.rs, budgets.rs}`; `loomward-windows/src/enumerate/` | LW-006, LW-101 (promotion) | After C2 | Spike winner promoted behind `DirSource`; portable `read_dir` source for CI; arena, writer integration, progress at most 4/s, cancel ack 250 ms and stop 2 s, refresh, finalise, restart recovery; S and M lab totals equal manifests exactly; fixture lab: junctions not followed, placeholders never opened, denied dirs counted; P1, P3, P5, P10, P11 recorded | `cargo test -p loomward-engine -p loomward-windows`; clippy; `cargo run -p loomward-lab --release -- bench scan --tier M --runs 5` |
| L8 service | Sonnet high (L1's worker) | `crates/loomward-service/`; the `loomward-serve` `main.rs` wiring once L6 has merged | LW-103 (implementation), LW-069 intent | After C2 and L7's first PR | Every wave-2 command implemented; wave-3 commands answer `capability_unavailable`; session-tagged opaque IDs (HMAC over row ID and kind); signed cursors bound to command, anchor, `listing_rev` and session; dataset-class and adapter policy; every response in tests validates against the schema | `cargo test -p loomward-service`; clippy; `py -3 scripts/test_app.py --live` against `loomward-serve` on an S-tier catalogue |
| L9 Tauri shell | Opus (L6's worker) | `native/` | LW-018 | After L6, L8 | Tauri 2 build against `app/dist`; `lw_call` and `lw_events`; `NativeDialogs` via the dialog plugin from Rust only; capability file lists only the two commands; CSP; WebView2 loads; real/synthetic/imported states distinct; disconnect shows unavailable; `--self-test` runs `session.hello` in-process and exits 0 | `cargo build --manifest-path native/Cargo.toml`; `cargo clippy --manifest-path native/Cargo.toml -- -D warnings`; `loomward-desktop --self-test`; screenshots in `evidence/v3/native/` |
| L10 Atlas and Observatory | Opus (W0-C's worker) | `app/viz/`, `app/src/views/atlas/`, `app/src/views/observatory/` | LW-105, part of LW-019 | When W0-C is done (mock data) | Prototype modules moved to `app/viz/` with the doc 41 section 13 contract, consuming `TreeSlice`; pure layout and colour modules unit-tested; keyboard focus over nodes with `aria-live`; reduced motion; three threads distinguishable without colour; P9 measured on a 2,500-node mock slice | `npm.cmd --prefix app run check`; `npm.cmd --prefix app run test`; `py -3 scripts/test_app.py` (Atlas and Observatory checks, P9 marks) |
| L11 telemetry | Sol high (W0-B's worker) | `loomward-windows/src/{processes.rs, pdh.rs, system.rs}`; `loomward-engine/src/telemetry/` | LW-050, LW-107 | After C2 | `process_ref` = pid + start time; first-sample rates null; resident, private and commit separate; access denied counted; PDH via English counter paths; GPU and disk `unavailable` when absent; 60 s leases, sampler idle without one; rule-based explanations; P12 and P13 measured | `cargo test -p loomward-windows -p loomward-engine`; clippy; soak run recorded in `evidence/v3/bench/telemetry.json` |
| L12 placement | Sol high (L4's worker) | `loomward-engine/src/placement/` | LW-108, LW-030 (wiring) | After C2 and L4 | Tier model (declared, hint, unknown) with pressure watermarks; candidates from catalogue subtrees; server-built scenario; three heat policies with labelled assumptions; plans saved `simulation_only`; schema-validated `executable: false`, `filesystem_changed: false`; fixture volumes for tests (no live disk needed) | `cargo test -p loomward-engine placement`; clippy |
| L13 product views | Sonnet high (L3's worker) plus Haiku | `app/src/views/{explorer,tiers,companion,health}/` | LW-068, LW-104 | After L3 (mock), live after L8 | Explorer paging and search with stale-generation handling; Tiers shows simulation everywhere; Companion with process list, explanations and own budgets; Grants & health with revocation and self-health card | `npm.cmd --prefix app run check|test|build`; `py -3 scripts/test_app.py` |

## Wave 3: learning, teacher, change tracking, scale and polish

| Lane | Model | Owns | Issues | Starts | Acceptance (summary) | Proving |
|---|---|---|---|---|---|---|
| L14 learning integration | Sol high | `crates/loomward-learn/` (now including `teacher_payload.rs`, `teacher_validate.rs`), `loomward-engine/src/learning/`, `app/src/views/review/` | LW-021, LW-026 (selection), LW-110 | After L5, L8 | Feedback with revisions and retraction into `human_feedback` via `object_ref`; refit job; `learning.queue` by review priority; `threads.meaning`; teacher payload builder and strict validator ported from `teacher.py` (size bucket, handles); Review desk view | `cargo test -p loomward-learn -p loomward-engine`; app checks; `py -3 scripts/verify.py` |
| L15 owned child and teacher runner | Sol high, plus an Opus deep-review lens | `loomward-windows/src/jobs.rs`, `loomward-engine/src/teacher/`, `evidence/v3/teacher-canary/` | LW-052 (owned teacher child only), LW-023, LW-109 | After L14 | Job Object via `PROC_THREAD_ATTRIBUTE_JOB_LIST`, kill-on-close, memory cap, timeout; Codex flags confirmed against `codex exec --help`; empty working directory; minimal environment; single-use grants; canary test recorded (teacher cannot read an outside canary file when a filename asks it to); until it passes, personal grants refuse with `canary_gate_not_passed`; malformed, oversized, wrong-handle and tool-shaped outputs rejected | `cargo test -p loomward-engine teacher`; the canary run log; synthetic teacher run log; no personal data |
| L16 change tracking | Sol high | `loomward-windows/src/watch.rs`, `loomward-engine/src/scan/watch.rs` (new file), `experiments/usn-unprivileged/` | LW-008, LW-007 (spike) | After L7 | `ReadDirectoryChangesW` dirty marking and targeted relisting; overflow marks the root stale and schedules a refresh; the unprivileged-USN hypothesis measured and reported either way | `cargo test -p loomward-engine scan`; fixture-lab overflow case |
| L17 scale and comparison | Sol high | `experiments/windows-benchmarks/`, `evidence/v3/bench/` | LW-056 | After L7, L8 | All P targets measured; WinDirStat comparison on identical trees and cache state; owner-authorised real-folder metadata runs reported as aggregates only (no names, no databases committed) | Bench commands and JSON evidence |
| L18 accessibility, parity, `ui/` retirement | Sonnet high plus Opus (visual) | app-wide fixes by request to owning lanes; then `ui/` retirement PR | LW-019, LW-096 | After L10, L13 | 100/150/200% scale, keyboard-only flows, text equivalents for every canvas; the old `ui/` checks re-expressed against `app/`; `ui/` retired in its own PR only after parity is shown | `py -3 scripts/test_app.py --a11y`; `py -3 scripts/verify.py --ui` |
| L19 CI | Coordinator | `.github/workflows/` | LW-002 follow-up | With L3 and L9 | App job (`npm ci`, check, test, build) on ubuntu and windows; native build job on windows; actions pinned by SHA | Hosted run results, or local equivalents named when billing blocks CI |

## Integration order

1. **C1** (glob workspace, dependency table).
2. **L1a** (envelopes and trait). Unblocks L6 and C2.
3. **W0-A, W0-B, W0-C** as each finishes, in any order (coordinator resolves their `Cargo.toml` member edits
   into the glob).
4. **L4, L5, L2, L6, L1b, L3** in any order; each is self-contained. After L1b, `contracts/v3/` passes to the
   coordinator.
5. **C2** (engine API skeleton, windows module stubs).
6. **L7** first PR (scan end to end on the portable source), then **L8**, **L11**, **L12** in any order;
   **L10** and **L13** at any time on mock data; **L9** after L6 and L8.
7. Wave 3: **L14** then **L15**; **L16**, **L17**, **L18** in parallel; **L19** alongside L3 and L9.

At every merge the coordinator runs `cargo fmt --all --check`, `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings`, `py -3 scripts/verify.py --ui`, and, once `app/` exists,
`npm.cmd --prefix app run check`, `test` and `build` plus `py -3 scripts/test_app.py`. The coordinator updates
`docs/16-implementation-status.md` and `handoff/CHECKPOINT.json` after each wave, keeping the Python reference
and native engine results separate (invariant 9).

## Which lanes can start right now

L1 (all of it), L2, L3, L4 and L5 can start immediately against `contracts/v3/` and the existing Python
reference. L6 starts as soon as L1a merges (hours). L10 starts when W0-C hands off; L13 starts on mock data as
soon as L3's shell exists. Everything in wave 2 that touches the engine waits for C2, which waits for W0-A,
L1b and L2.
