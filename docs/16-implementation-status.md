> Historical v0.1 status, with a v0.3 section at the end. The v0.2 matrix is in [37-expansion-verification.md](37-expansion-verification.md); v0.3 is planned in [41](41-v03-architecture.md)-[43](43-v03-lanes.md).

# Implementation status: initial authoring pass

Date: 9 October 2026. This is a **reference prototype plus native source foundation**, not a completed Windows application.

## Verified in this environment

The Python source-checkout application runs on Linux/Python 3.13.5. Its metadata scanner, bounded content-equality inspector, weighted supervised student, structured teacher validator/client, constrained tier simulator, SQLite feedback/audit store, loopback HTTP boundary, optional telemetry adapter and CLI are implemented. The final full Python suite contains 89 passing tests. Read evidence/verification.txt for the executed command/result rather than inferring a Windows pass.

The browser workbench has seven navigable pages and real reference-API integration for inventory/review/feedback/retraining, duplicate inspection, audit, scenario simulation and optional process observation. Eleven recorded Chromium checks passed, including refresh/session preservation, skip-link token preservation, persistence/retraining, hostile snapshot display/import, search, navigation and mobile width. The harness renders assets with set_content and bridges fetch through Python to the real HTTP server because direct browser navigation is blocked. This is not direct localhost-network, Windows or WebView2 evidence. Screenshots are in evidence/screenshots/.

Four JSON schemas and the proposed SQL catalogue design are included. The schemas document interchange validation, not permissions. The SQL is a separate future design, not an automatic database migration. The current durable database stores feedback and audit, not a million-file native catalogue.

## Written, but not executed or integrated

Rust core/CLI sources implement bounded metadata enumeration, typed deny-by-default capability checks, a pure simulated transaction state machine and the greedy planner port. Twenty Rust tests are written, including a Python-generated shared golden scenario. No rustc/cargo was available; a toolchain download attempt failed. Compilation, formatting, lint, tests and lockfile resolution remain open. No native binary is supplied or implied.

The Tauri 2 project is a separate packaging scaffold showing synthetic UI data. It has no custom action commands or live native bridge, and it was not compiled. The Python Windows launch scripts and the proposed GitHub Actions workflows have not run on Windows/GitHub. There is no executed CI result, signing proof, installer or update mechanism.

The live teacher adapter was exercised against a loopback HTTP mock, including schema/identity/redirect/response-size failures. No actual LM Studio model was called. No model quality, speed, memory or personal-preference result is claimed. The actual student learns from labelled metadata examples but remains a small uncalibrated baseline. No embedding model, graph learner, usage predictor, heat model, bandit or automatic learning deployment is installed.

## Explicitly designed future work

Native volume discovery and stable file identity, USN journal tracking, persistent inventory indexing, physical reclaimability, metadata-preserving movement, destructive cleanup, backup/restore execution, shortcut edits, desktop shell integration, application uninstall and process control are not implemented. Bidirectional live disk balancing and automatic recall are not implemented. Current tier inputs are supplied estimates; the current engine emits non-executable demotion scenarios only.

The future operation protocol, consent/grant model, backup evidence, crash/undo tests, learning promotion gates, provider interfaces and release acceptance criteria are specifications with backlog issues. Their presence does not confer safety guarantees on an unbuilt executor.

## Experiment results and limitations

The reference scanned 5,000 newly created tiny files in three local ephemeral-storage passes; median elapsed time was approximately 0.868 seconds. Cache conditions were not controlled. This is a smoke observation, not a Windows throughput target or competitor comparison.

An exhaustive tiny-case oracle compared 51 synthetic tier scenarios. The greedy planner respected the checked transfer/destination-reserve constraints but missed feasible targets in 9 cases and had a worse shortfall in 24. Preserve the experiment before changing allocation strategy. It does not assess crash safety, changing files, sparse allocation, real heat or performance at scale.

## Known limitations that must remain visible

- Portable path/stat checks do not provide a race-free native authority boundary. Use non-elevated, trusted disposable scopes. Imported snapshots must never authorise effects.
- A reference scan can include app state when the owner chooses an overlapping broad root. Keep state outside the scope now; exact native self-exclusion is LW-006.
- The local HTTP server is development tooling, not a hardened multiuser/network service. Production IPC, worker admission, resource budgets and request scheduling are future work.
- Byte counters are bounded for JavaScript display, while some Python nanosecond fields can lose precision through UI JSON round-trips. Native protocol v2 uses exact decimal strings. UI snapshots are not valid execution manifests.
- The audit chain is unkeyed. It can reveal some edits relative to known state but cannot prove authenticity against an attacker who controls the database or detect all truncation without an external anchor.
- Duplicate equality is not physical reclaimability. Hard links, protected contexts and unsupported states are excluded; content inspection can affect caches/access metadata.
- Feedback is bounded to 10,000 events per prototype training run. Pre-commit validation prevents an over-budget/untrainable event from being saved, but full retention/export/retraction UX remains future work.
- The UI displays a bounded subset of predictions/review items; this is not a million-file personalised organiser yet.
- No independent code/security review was available. The authoring pass performed and recorded a self-review, not an independent audit.

## External-action receipt

GitHub account access and naming were inspected. The connector did not expose a repository-creation action; plugin discovery did not provide another suitable creation tool. **No remote repository, branch, PR, issue, label, merge or workflow was created or modified.** The bundle includes local, dry-run-first publishing helpers. Any local Git snapshot delivered with the package is not a GitHub publication.

## Next work

Use handoff/LOCAL-AGENT-PROMPT.md and the first critical-path pack. Establish real Windows behavior and native compilation, then native identities/capabilities/protocol. Do not skip directly to unattended cleanup, source deletion or broad process control.

## v0.3 status (10 October 2026, Windows 11, rustc 1.97.1, Python 3.14.3)

The v0.2 bundle is on the public `main` with its history (#2). Everything below ran on Windows; hosted CI
runs Python 3.11/3.13 and the Rust workspace on ubuntu and windows.

| Area | State on `main` | Evidence |
|---|---|---|
| Python reference | 216 tests (1 symlink-privilege skip); two Windows bugs and eleven review findings fixed test-first (#2, #106) | `evidence/v3/README.md` |
| `loomward-core` | v1 planner plus the v2 allocator port; 222 parity cases equal Python field by field (#107) | `evidence/v3/planner-v2-rust.md` |
| `loomward-windows` | handle-derived identity, volume capabilities, opt-in disposable fixture lab; read-only (#108) | `evidence/v3/windows-native.md` |
| `loomward-telemetry` | schema v2: memory ledger (standby, modified, free), machine-normalised CPU, disk busy separate from queue, PDH GPU aggregated by LUID with churn kept unknown (#109, #139) | `evidence/v3/telemetry.md` |
| `loomward-lab` | deterministic synthetic scale trees and the enumeration spike: 3M files in 0.558 s warm (#111); hardened real-root guards (#118) | `docs/research/enumeration-spike.md`, `docs/research/real-folder-stress.md` |
| `loomward-learn` | the student model in Rust with exact Python parity (#113) | `crates/loomward-learn/tests/parity.rs` |
| `loomward-protocol` | v3 envelopes with complete per-command validation and duplicate-key refusal; `ViewService` trait (#133) | `crates/loomward-protocol/tests/` |
| `loomward-engine` | C2 skeleton (#140); native scan pipeline: strategy chain, handle-relative no-recall opens, junction-refusing root walk, staged sink with revision fences, jobs, events (L7, #162); placement: antichain candidates, verified relief, planner parity (L12, #158); lease-driven read-only telemetry (L11b, #161) | `evidence/v3/scan-l7.md`, `evidence/v3/placement-engine-l12.md`, `evidence/v3/telemetry-engine.md` |
| `loomward-http` | `loomward-serve`, the loopback HTTP + SSE adapter: token, Host and Origin checks, bounded bodies, static files that cannot escape; synthetic fixtures until L8 (#143) | `crates/loomward-http/tests/serve.rs` |
| Change tracking (spike) | non-elevated: watcher overflow arrives as zero-byte completions; unprivileged USN catch-up works through directory handles but strips names (#138) | `docs/research/change-tracking.md` |
| `app/` | Svelte 5 app: shell (#114), Explorer/Tiers/Companion/Health (#119), Atlas and Observatory over bounded slices (#136, #155), contract conformance (#149), WCAG 2.2 AA fixes with an `--a11y` leg (L18, #168) | `evidence/v3/app-views/` |
| `native/` | first Tauri 2 desktop shell: `lw_call` and `lw_events` only, CSP, Rust-only dialogs, `--self-test`; WebView2 probe 13/13, Atlas P9 17.5 ms (L9, #164) | `evidence/v3/native/` |
| `app/prototype` | woven atlas + Observatory signature prototype on synthetic data (#112) | `evidence/v3/ui-prototype/` |

Rust: 240 tests on Windows (core 30, engine 61, http 30, lab 22, learn 8, protocol 35, telemetry 25, windows 29), plus the desktop shell's 7; app: 154 vitest plus Playwright e2e and `--a11y`; Python: 216.

Measured: M-tier scan 1M files at about 4.9M files/s warm with 20/20 exact manifest matches (#162); telemetry P13 idle 0% CPU and 0.94 MiB commit, P12 lease overhead 3.1% of a core against 1% (#160).

In review or in flight on 10 October 2026 (afternoon): the SQLite catalogue (#147, parked after two review rounds; a superseding durability fix for #171 is in progress; P4 is about 16k rows/s against 250k, #148), the service (L8, after the catalogue), engine telemetry follow-ups (#160, #163), and the L15b teacher sandbox spike.

Not established: the view service, a persistent native catalogue on main, cold-cache or HDD timings, a WinDirStat comparison, a connected teacher. No file or process effect exists.

## v0.3 status, wave 4 (10 October 2026, evening; `main` at f910c89)

The earlier section above is kept as history. Since then these merged, each with a fresh-context review and green
CI:

| Area | State on `main` | Evidence |
|---|---|---|
| Catalogue (`loomward-catalog`) | durable SQLite catalogue on revision 2 plus errata: crash-safe pending-reference retirement, FULL sync for intent-bearing publications (#173); P4 3.2x to about 53k rows/s at 1M rows (#187); query errors never hidden as budget cuts (#203) | `crates/loomward-catalog/HANDOFF.md`, `evidence/v3/bench/catalog-1m.json` |
| Scan pipeline | identity dedupe, dirty-epoch relists, moved or vanished folders report partial, never a wrong total (#178); child tickets follow the parent's publication (#205) | `evidence/v3/scan-l7.md` |
| Change tracking (L16) | guarded `ReadDirectoryChangesW` watchers feeding dirty epochs (#183); scoped to each scan, so they never block the user's renames between scans; attribute and ACL hints; identity-based absence (#202) | `evidence/v3/l16-change-tracking.md` |
| View service (L8) | `loomward-service`, the `ViewService` over the engine and catalogue: instance-bound IDs, revocation never queued, bounded admission (#179); session-bound node IDs, roots past 64, device-path refusal (#197); **scans through the product for synthetic lab roots**: catalogue-backed scan writer, `GrantedRoot` only from an active durable grant (#205) | `crates/loomward-service/` |
| `loomward-serve` | runs the engine service (`--fixtures` for the contract examples); gates before any body drain, write deadlines, SSE stall release, exclusive port, static-scope refusal (#195) | `crates/loomward-http/tests/serve.rs` |
| Jobs and placement | `Queued` is published, spawn failures publish their terminal state, cancel is idempotent (#199); placement zero relief, unknown clusters, hidden descendants, unusable volumes read Unknown (#200) | engine tests |
| Telemetry | P12 lease overhead 2.45% to 1.005% of one core; process rows carry their own `observed_at` (contract addition) (#207) | `evidence/v3/telemetry-engine.md`, `evidence/v3/bench/telemetry.json` |
| App | WCAG second half (#188); Explorer returns to its starting points when a saved breadcrumb goes stale (#204); README status brought current (#192) | `scripts/test_app_a11y.py` |
| Tooling | `verify.py --app` (#186); text checked out as LF everywhere (#193) | `scripts/verify.py` |

Counts (Windows): Rust 410 (catalog 69, core 30, engine 106, http 40, lab 22, learn 8, protocol 35, service 35,
telemetry 28, windows 37), plus the desktop shell's 7; app 174 vitest plus Playwright e2e and `--a11y`; Python 221.

Measured on a shared host (other lanes running): P4 about 44.5k rows/s median in an interleaved A/B (53k when
measured alone earlier); a first-scan bulk-load candidate (-31%) and a spill-bounded cache (-37.5%) were rejected and
recorded (#208, branch `perf/catalog-p4b-parked`); P12 1.005% against 1%; L16 hint latency p50 0.32 ms.

In review or in flight: the catalogue auto-rollback guard (#208), loomward-http security tests (#209), the 10M lab
tier on E: (L17a).

Not established: personal (real-disk) scans through the product, gated on #206 (excluded entries must stay
visible, held names charged to the byte budget) and the rest of #184; P4 at 250k (#148); P12 at or under 1%
(#160); cold-cache or HDD timings; the teacher on real data (q-8 answered broker-first on 10 October, design in `docs/superpowers/specs/2026-10-10-synthetic-inference-broker-design.md`; q-5 still open). An intermittent
native watch test failure under heavy host load is tracked (#201). No file or process effect exists.

## v0.3 status, wave 5 (10 October 2026, night; counts measured at `ee39877`)

The sections above are kept as history. This wave merged, each with a fresh-context review (Opus, Sonnet, a
security audit or a cross-vendor Muse lens) and green CI; the desktop shell, which CI does not build, was
re-proved locally:

| Area | State on `main` | Evidence |
|---|---|---|
| Scan robustness | per-root start gate and fenced revocation (#212); cancellation collects jobs before stopping the watch, a real race, 18% to 0 under load (#233); burst oracle accepts overflow FullRoot (#201 fixed, 43% to 0); revocation test made deterministic (#227) | `evidence/v3/scan-robustness-201-184.md`, `evidence/v3/scan-cancellation-228.md` |
| Scan writer (gates personal scans) | excluded entries stay visible, flagged and uncounted; held names charged to the byte budget; `identity_changed` recorded; stale prepares relist; failed EndRun releases the root (#225, closes #206); a completed scan never retires an excluded directory's subtree (#252, closes #240) | catalogue tests `tests/writer.rs` |
| Catalogue | P4d: dirty ancestors once per transaction, one publication per listing, reference refresh 1.63x (#216); guards pinned by 29 new tests (#251) | `crates/loomward-catalog/HANDOFF.md`, `evidence/v3/bench/catalog-1m.json` |
| 10M lab (L17a) | E:-only 10M tier; 20 fresh native runs, p50 546k observed files/s, partial by design at the arena cap; limit outcomes are not errors (#215); G: capped at 2M per ADR-V3-17 (#242) | `evidence/v3/bench/scan-10m.json` |
| Windows identity | Find without identity assurance refused; every native open refuses elevation; list-time refusals stay `Refused` (#217) | `docs/42-v03-adrs.md` |
| Duplicates (LW-012, new) | staged size, partial hash, full hash and byte comparison through pinned, revalidated handles; synthetic and lab roots only (`content_read_gated` for personal); reader feature-gated to the engine, inspection on `Engine` with an active-grant recheck and revocation (#234, #244) | `evidence/v3/duplicates-lw012.md`, `evidence/v3/duplicate-reader-hardening-238.md` |
| Applications (LW-047, new) | read-only Uninstall registry inventory with scopes, provenance and honest provider states; churn reads partial (#229, #249) | `evidence/v3/app-inventory-lw047.md` |
| Desktop contexts (LW-044/045, new) | known folders with redirection as data; a pure, bounded `.lnk` parser with target health; no COM, no execution (#245) | `evidence/v3/shell-lw044-lw045.md` |
| Telemetry | renewals keep samples; an unattributable GPU engine nulls busy values; `Denied` means no process handle; no reads after the last lease (#237) | `evidence/v3/telemetry-bughunt.md` |
| loomward-serve | stored and picker grants in the static overlap check (offline grants compared as text), graceful lifetime drain, bounded static reads (#221); security behaviour pinned (#209) | `crates/loomward-http/tests/serve.rs` |
| Desktop shell | dialog, IPC, stream, navigation and picker hardening; P9 back in the WebView2 probe (#226) | `evidence/v3/native/webview2-probe.json` |
| App | Observatory process strip with observed ages (#214); scan, refresh and cancel for lab roots in Grants & health (#222); follow-ups (#250); an order-dependent e2e check fixed (#243) | `evidence/v3/app-views/` |
| Contracts and docs | idempotency keyed by `request_id` per session (#220); README drift (#232); wave-4 checkpoint (#210) | `contracts/v3/semantics.md` |

Counts (Windows, at `ee39877`): Rust 516 (apps 10, catalog 78, core 30, engine 125, http 55, lab 26, learn 8,
protocol 35, service 40, shell 15, telemetry 33, windows 61); app 248 vitest (at `befd890`) plus Playwright e2e,
`--a11y` and `--live-serve`; Python 221. Later merges in this wave add catalogue (#251, #252), app (#250) and
apps (#249) tests.

Public evidence: on 10 October the owner replaced workstation diagnostics with synthetic public evidence; new
receipts withhold host hardware (processor, memory, device model, media) and private paths.

Teacher: q-8 answered broker-first (inference broker, synthetic only, no credential reuse, no paid fallback,
no Windows security change); the design spec is `docs/superpowers/specs/2026-10-10-synthetic-inference-broker-design.md`,
and the owner is carrying that lane. q-5 (personal-data disclosure) stays open.

Not established: personal (real-disk) scans through the product (still gated: `personal_scan_gated_184`;
#206 and #240 are done, #184's remainder is not); P4 at 250k (#148); P12 at or under 1% (#160); the 10M tier
complete rather than partial; cold-cache timings; service, contract and UI wiring for duplicates, applications
and desktop contexts. No file or process effect exists.

## v0.3 status, wave 6 (10 October 2026, late; counts measured at `6052bad`)

The sections above are kept as history. This wave merged, each with a fresh-context review (Opus, Sonnet,
a security audit or a cross-vendor Muse lens) and green CI; the desktop shell was re-proved locally:

| Area | State on `main` | Evidence |
|---|---|---|
| Personal-scan gates | #184 closed by an audit of all 15 items plus the last fix (checkpoint starvation) and the watch-lifetime disclosure (#274); #206 and #240 closed in wave 5. One gate remains in review: re-prove containment before every relist (#269, PR #278). `personal_scan_gated_184` still refuses personal scans | `evidence/v3/gate-184-closeout.md` |
| Scan pipeline | listed directory handles released and reopened by verified identity on NTFS and ReFS only; cancelled runs keep partial totals; duplicate identities counted; exact relist counters (#262) | `evidence/v3/scan-followups-180.md` |
| Engine API | a granted root binds to its exact catalogue row and instance; the teacher gate fails closed (#256) | engine tests |
| Service | bounded read permits with deadline interrupts; one CLI and picker deny-list; strong lab identity and canonical digests; validation before side effects; authenticated root IDs; legacy stores still migrate (#265) | service tests |
| Catalogue | link counts cover the visible tree only and are rebuilt on revocation (#270) | `crates/loomward-catalog/HANDOFF.md` |
| Duplicates | a failed representative skips only itself and its partition is re-verified; verified groups survive a late stop (#261, #267) | `evidence/v3/duplicate-reader-bughunt.md` |
| Desktop contexts | shortcut reads walk each component relative to its parent without following reparse points; removable media untouched; capped, feature-gated content reads (#257) | `evidence/v3/shell-lw044-lw045.md` |
| Contracts | jobs carry an engine-monotonic `seq` and a per-job `rev`; the app orders and merges by them (#272) | `contracts/v3/semantics.md` |
| Tests and tooling | e2e P9 gates the median of five warm runs at 50 ms; race-free async checks (#271); `#![forbid(unsafe_code)]` in four more crates and an accurate unsafe inventory (#259, #263) | `scripts/test_app_timing.py` |

Counts (Windows, at `6052bad`): Rust 631 (apps 18, catalog 115, core 30, engine 143, http 56, lab 26, learn 8,
protocol 36, service 58, shell 23, telemetry 33, windows 82, doc 3); app 255 vitest plus Playwright e2e, `--a11y`
and `--live-serve`; Python 222.

Process note: from this wave, the driver updates this file and `handoff/CHECKPOINT.json` once per wave
(AGENTS.md says so), so feature PRs no longer carry status edits.

Not established: personal scans through the product (one gate in review, then an explicit opening step);
P4 at 250k (#148); P12 at or under 1% (#160); service, contract and UI wiring for duplicates, applications and
desktop contexts; the desktop shell in hosted CI (#276). No file or process effect exists.
