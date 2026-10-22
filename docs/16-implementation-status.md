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
(#160); cold-cache or HDD timings; the teacher on real data (q-8 answered broker-first on 10 October, design in `docs/superpowers/specs/2026-10-10-synthetic-inference-broker-design.md`; q-5 answered on 11 October: names only, per-request preview, after the broker's canaries). An intermittent
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
and the owner is carrying that lane. q-5 was answered on 11 October (names only, per-request preview, after the broker's canaries).

Not established: personal (real-disk) scans through the product (still gated: `personal_scan_gated_184`;
#206 and #240 are done, #184's remainder is not); P4 at 250k (#148); P12 at or under 1% (#160); the 10M tier
complete rather than partial; cold-cache timings; service, contract and UI wiring for duplicates, applications
and desktop contexts. No file or process effect exists.

## v0.3 status, wave 6 (10 October 2026, late; counts measured at `6052bad`)

The sections above are kept as history. This wave merged, each with a fresh-context review (Opus, Sonnet,
a security audit or a cross-vendor Muse lens) and green CI, except #261, which was merged before its
review finished (its post-merge review found a defect, fixed in #267); the desktop shell was re-proved locally:

| Area | State on `main` | Evidence |
|---|---|---|
| Personal-scan gates | #184 closed by an audit of all 15 items plus the last fix (checkpoint starvation) and the watch-lifetime disclosure (#274); #206 and #240 closed in wave 5. One gate remains in review: re-prove containment before every relist (#269, PR #278). `personal_scan_gated_184` still refuses personal scans | `evidence/v3/gate-184-closeout.md` |
| Scan pipeline | listed directory handles released and reopened by verified identity on NTFS and ReFS only; cancelled runs keep partial totals; duplicate identities counted; exact relist counters (#262) | `evidence/v3/scan-followups-180.md` |
| Engine API | a granted root binds to its exact catalogue row and instance; the teacher gate fails closed (#256) | engine tests |
| Service | bounded read permits with deadline interrupts; one CLI and picker deny-list; strong lab identity and canonical digests; validation before side effects; authenticated root IDs; legacy stores still migrate (#265) | service tests |
| Catalogue | link counts cover the visible tree only and are rebuilt on revocation (#270) | `crates/loomward-catalog/HANDOFF.md` |
| Duplicates | a failed representative skips only itself and its partition is re-verified; verified groups survive a late stop (#261, #267) | `evidence/v3/duplicate-reader-bughunt.md` |
| Desktop contexts | shortcut reads walk each component relative to its parent without following reparse points; removable media untouched; capped, feature-gated content reads (#257) | `evidence/v3/shell-lw044-lw045.md` |
| Desktop shell | disclosure capped by items, wrapped lines and characters; backslashes escaped; wider profile and credential fences; picker owned by the main window; busy calls retried once; retained replay preserved; WebView2 P9 16 ms median (#253) | `evidence/v3/native/webview2-probe.json` |
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

## Personal scans opened (11 October 2026)

The sections above are kept as history. All four personal-scan safety gates are closed: #184 (audit, #274),
#206 (#225), #240 (#252) and #269 (containment re-proved before every relist, #278). #284 hardened the
profile and store fences and keyed scan gates on the durable grant, and #286's profiles-directory fence now
resolves the directory from the OS rather than by its folder name.

| Area | State | Evidence |
|---|---|---|
| Personal scans | open: a personal session scans its own active owner grants (desktop picker or `--grant-root`), metadata only, explicit roots, never elevated; refused before any filesystem access: an unknown root is `not_found`, a revoked grant `permission_denied` (the grant is not active), and a lab root `permission_denied` with `personal_session_requires_owner_grant`. The `personal_scan_gated_184` refusal is gone | `evidence/v3/personal-scan-opening.md` |
| Unchanged | a synthetic session scans registered lab roots only; content reads for duplicates stay gated (`content_read_gated`); no file or process effect; no new endpoint | service tests |

Not established: personal scans on FAT, exFAT or RAM-disk volumes (#285 item 2; FAT/exFAT now list as identity-unverified, #372, untested on a real FAT volume); P4 at 250k (#148); P12 at or
under 1% (#160); service, contract and UI wiring for duplicates, applications and desktop contexts; the desktop
shell in hosted CI (#276). No file or process effect exists.

## v0.3 status, wave 7 (11 October 2026; counts measured at `0b27935`)

The sections above are kept as history. This wave merged 33 PRs (counting the wave-6 checkpoint #279 and the owner's teacher-sandbox #291), each with one fresh-context review (an Opus
deep review for the opening, Sonnet reviewers for engine, catalogue and wiring changes, Muse lenses for the
rest, scoped round-2 passes after logic fixes) and green hosted CI at merge (lanes' own receipts list hosted CI as not run because it ran on the PR). The wiring backlog
closed: every product surface the app shows is now served by the real service, not only the mock.

| Area | State on `main` | Evidence |
|---|---|---|
| Personal scans | open since #313 (see the section above); stored grants are re-checked against today's fences at scan start (#312, folded into #313) | `evidence/v3/personal-scan-opening.md` |
| Scan throughput | the catalogue writer no longer idles waiting to batch synchronous listings: 7.7x faster (median) on a 100k-file lab tree, whose runs varied about 2x, and 28x on one run pair over one agent-picked real folder (shared host, profiler on; not a stable bound, #322) | `evidence/v3/scan-throughput-310.md` |
| Duplicates | read-only inspection from catalogue candidates to a Duplicates view (#296); lossy or unreadable names keep the report partial; job lifecycle, idempotent revocation, redacted revoked reports and retention caps (#318); **personal content reads open (q-10)**: a personal session reads its own owner grants (same root gate and fences as its scans) through the same staged, pinned, revalidated, non-elevated, no-recall reader within the job's byte, time and candidate budgets; placeholders are skipped, never hydrated; nothing leaves the machine or is deleted; `content_read_gated` is gone | `evidence/v3/duplicates-wire-281.md`, `evidence/v3/jobs-287-duplicates-299.md`, `evidence/v3/personal-duplicates-q10.md` |
| Storage balancing | `tiers.model`, `placement.candidates`, `placement.simulate`, saved proposals from engine to the Tiers view, simulation only, nothing moves (#316); revoked-only volumes excluded as targets, bounded lab space reads (#331); volumes past 64 refused, never dropped | `evidence/v3/placement-wiring-108.md` |
| Learning | human feedback with durable replay, a revision-bound review queue and human-only refit jobs over the Rust student (#328); **open to personal sessions locally on the owner's labels (q-11)**: stores keep their dataset class, revocation sweeps features and models, the teacher refuses personal sessions with `teacher_personal_gated`; grants record their volume filesystem (#372), so real NTFS folders can be labelled; grouped temporal evaluation and held-out calibration, scores "relative, not a probability" (#294) | `evidence/v3/learning-wiring-l14.md`, `evidence/v3/eval-calibration-lw025.md` |
| Organisation | collections: durable, identity-checked members; idempotent writes; no effects (#325) | `evidence/v3/collections-wiring.md` |
| Applications and desktop | read-only `apps.list` and `desktop.contexts` with injected providers and dataset separation; personal sessions read the installed-applications registry inventory and known folders read-only when the Applications or Desktop view is opened; personal shortcut enumeration is unavailable by design (#324, #330) | `evidence/v3/apps-desktop-wiring.md` |
| Catalogue | each caller keeps its first error after a rollback (#295); a too-deep reparent rejects only its own command (#306, #315); first-scan batches charge validated bytes, P4 96k to 109k rows/s on the bench path (#332) | `crates/loomward-catalog/HANDOFF.md` |
| Telemetry | P12 0.53% of one core above idle in a shortened shared-host A/B (main 0.66%); interval renewals invalidate mapped batches (#307) | `evidence/v3/telemetry-engine.md` |
| Service and serve | OS profile and profiles-directory fences, scan gates keyed by the durable grant (#284, #313); `--static` and the picker share one stored-grant overlap rule (#304) | service tests |
| App | WCAG 2.2 AA audit findings pinned (#302); Explorer climbs to the nearest folder still listed (#311); hello resume watermark bounded (#314) | `evidence/v3/wcag-157.md` |
| Desktop shell | build, tests, clippy and `--self-test` in hosted CI; the WebView2 probe stays local (hosted WebView2 opens no DevTools endpoint) (#282) | `native/README.md` |
| Tests and privacy | HTTP security pins (#290); lab guard, shell and apps test gaps (#297, #298); an honest targeted watch repair (#301, #308); a deterministic duplicate-reader deadline test (#321); host CPU and GPU models withheld from older receipts (#305) | test suites |

Counts (Windows, at `0b27935`): Rust 763 (apps 23, catalog 130, core 30, engine 164, http 66, lab 27, learn 18, protocol 36, service 112, shell 27, telemetry 34, windows 93, doc 3); app 320 vitest plus Playwright e2e, `--a11y`
and `--live-serve`; Python 224 (1 symlink-privilege skip).

Not established: P4 at 250k (#148; the bench path is at about 109k rows/s); a 10-minute quiet-host P12 run
(#160); personal learning (q-5, #319, #329); personal shortcut
enumeration; FAT, exFAT and RAM-disk scans (#285; identity-unverified listing since #372, no real FAT volume tested); a real picker grant exercised by a person. No file or process
effect exists: duplicates, placement and collections observe, simulate or record preferences only.

## Owner decisions (11 October 2026)

Recorded in HUMAN_TODO q-5 and q-9 to q-19 (#360). The repository is **T2 (daily driver)**. Privacy and security
findings now weigh lower and new evidence no longer withholds host hardware; secrets and real file names, paths or
inventories in this public repository stay forbidden. Decided, not yet implemented (each lands in its own PR, which
updates this file when it opens a gate): backups stages 2 and 3, a guarded restic observer then a restore-drill executor (q-14), a native-save diagnostics bundle (q-15), and the first published preview pre-release (q-17; its workflow and prerequisites are merged). The teacher
stays synthetic until the broker passes its canaries; then names only, previewed per request (q-5). Implemented since: personal duplicate content reads (q-10), personal learning on the owner's labels (q-11),
the FAT/exFAT listing (q-12), the KPI retarget and first-scan bulk load (q-13), backups stage 1 (q-14) and Forget (q-9, #382); the hardware scrub
(q-18) covers the docs and mocks, with receipts and old fixtures still open; see the wave-8 table.

**FAT/exFAT listing (q-12, #372).** Fixed-disk FAT, exFAT and unknown filesystems list as identity-unverified:
metadata only, every row flagged `identity_unverified`, never watched, never a placement, duplicate or durable-reference
input; a personal root needs an explicit native confirmation (desktop) or `--allow-identity-unverified` (CLI). Every
grant now records its volume's real filesystem, so durable references (collections, learning) work on real NTFS folders.
State schema v5. No real FAT volume was available: that leg is unverified.

## v0.3 status, wave 8 (11 October 2026, night; counts measured at `b00c564`)

The sections above are kept as history. This wave merged 32 PRs, counting the wave-7 checkpoint #334 and Forget (#382), which landed after the counts below. Each PR carries its review record: a fresh-context reviewer, a Muse lens for the bug-hunt sweeps, or a clean Codex pass for very-low-risk changes. The night's merge resolutions that touched logic also got a scoped pass. Each merged with green hosted CI. Base refreshes were re-proved on the merged tree before merging. One such refresh caught a real merge interaction: #383's receipt picker inherited #379's folder-grant confirmation, fixed in #379.

An unexpected host shutdown mid-wave zeroed the primary checkout's `.git/config` and six manifests in one worktree. The config was rebuilt and the manifests restored from Git. Every checkout was scanned and no other file was damaged.

| Area | State on `main` | Evidence |
|---|---|---|
| Personal duplicates (q-10) | **open**: local, read-only content reads on owner grants through the session root gate (#376). Revocation redacts reports (#335). The budget is applied before I/O, and disclosure is paged (#358). An inspection that finds the root replaced records `identity_changed` (#388) | `evidence/v3/personal-duplicates-q10.md` |
| Personal learning (q-11) | **open** locally on the owner's labels (#368). Withdrawals are always possible and refits are serialised (#335, #351). The teacher refuses personal data | `evidence/v3/personal-learning-q11.md` |
| FAT/exFAT (q-12) | listed as identity-unverified, never acted on. Every grant records its real filesystem; state v5 (#372) | see "FAT/exFAT listing" below |
| Meaning and organisation | meaning threads, the Woven Atlas weft, from the student to the Atlas and Explorer (#338, #357). Collections pin every member guard and list in one query (#336, #357) | `evidence/v3/meaning-threads.md` |
| First-scan bulk load (q-13) | a root's first full scan builds off to the side in unindexed `bulk_*` tables and publishes atomically in `EndRun` (#387). The KPI is end-to-end files/s (`scripts/bench_scan_throughput.py`). **P14 regressed** from 155 to 230 bytes/entry at 1M (#386). The quiet-host A/B is **unverified** | `evidence/v3/first-scan-bulk-load-148.md` |
| Catalogue | the `EndRun` checkpoint never holds a committed receipt behind a reader. Async staging is parked as negative evidence (#343). References bound inside the pending-publication window are re-staged (#364). 19 query tests added (#354) | `crates/loomward-catalog/HANDOFF.md` |
| Backups (q-14, stage 1) | receipt normaliser (#347). Restic receipts are imported through a Rust-side open dialog into a Backups view (#383): `backups.list`, `backups.import` and `backups.forget`, state v6 `backup_evidence`. A green job is never shown as verified recovery | `evidence/v3/backups-import.md`, ADR-V3-23 |
| Diagnostic bundles (LW-055, q-15): **first file write, gate opened** | `diagnostics.preview` builds a redacted allowlist bundle (typed fields only: build, OS build, logical processors and RAM per q-16, health codes, store versions, counts, jobs, roots by opaque ID; unread values are explicit unknowns) with the size and SHA-256 of its exact text; `diagnostics.save` (desktop only, every session) writes exactly those bytes, named by digest, to a file the owner picks in a Rust save dialog: never the path over IPC, network, device, verbatim, device-name and stream targets refused, a parent folder resolving to a network path refused (local junctions are followed), a final component that is a link or reparse point refused, temporary file renamed over the target, an existing file replaced only after Loomward's own Cancel-default confirmation. Nothing is uploaded; no other file effect opens | `evidence/v3/diagnostics-bundle-q15.md`, ADR-V3-24 |
| Resource trends | lease-scoped growth and contention candidates in the Observatory, each citing facts, never a verdict (#374). Process trends are fetched at the 15 s cadence and machine candidates come first (#381) | `evidence/v3/resource-trends-lw051.md` |
| Desktop app | hosts the real engine service by default (#371). A safe picker with a named, Cancel-default confirmation; single-instance lock; visible launch errors; state outside the install folder (#379). "Add a folder" from Grants and health (#380). Request ids carry a per-client tag, so a reloaded page cannot replay an old outcome (#380) | `evidence/v3/desktop-preview-prereqs-373.md` |
| Release (q-17) | unsigned preview pre-release workflow (#362) and its prerequisites (#379). **No release is published yet** | `RELEASING.md` |
| Review sweeps and tests | review follow-ups (#340); placement and inventory resilience (#350); app honesty, where unknown is never 0 B (#355); live-serve rebuilds `app/dist` (#346); deterministic deadlines and a real `Service` drop race (#367); signature pass across the newer views (#345) | test suites |
| Forget a revoked root (q-9) | `roots.forget` (#382; 54 commands, state v7). It purges a revoked root's name and catalogue, its labels and references (an object another root on the volume still observes keeps them), and the start-up and migration copies of `state.db`, then rewrites `state.db` and cuts the logs. An anonymised audit entry stays. The UI says traces may remain in unused disk space. Not established: a long-lived `state.db` reader can delay the log cut (#390); `catalog.db` pages freed before the first forget (#391) | `crates/loomward-catalog/tests/forget.rs`, `crates/loomward-service/src/forget_tests.rs` |
| Hardware scrub (q-18) | docs and mocks now use generic labels (#365). **Not done:** raw `evidence/v3/` receipts and the old Python reference fixtures still carry processor counts and media models | HUMAN_TODO q-18 |

Counts (Windows, at `b00c564`): Rust 917 (apps 24, backup 19, catalog 164, core 30, engine 206, http 72, lab 27, learn 18, protocol 36, service 163, shell 27, telemetry 34, windows 97; doc tests counted in their crates); app 383 vitest plus Playwright e2e, `--a11y` and `--live-serve`; Python 227 (1 symlink-privilege skip).

**Not established:**
- P4 at 250k rows/s (#148);
- the bulk-load A/B on a quiet host (#148);
- P14 under the bulk path (#386);
- a 10-minute quiet-host P12 run (#160);
- a real FAT volume;
- a published preview release (q-17);
- a real picker grant exercised by a person.

**No file or process effect exists:** duplicates, placement, collections and backups observe, simulate or record only. Forget (q-9, #382) deletes Loomward's own records of a revoked root, never the owner's files.

## Owner decisions of 12 October 2026 (guided walkthrough; HUMAN_TODO q-20 to q-46)

Recorded here because q-33 changes product invariant 8. Nothing below is implemented unless a PR is named.

- **Licence:** AGPL-3.0-only since #396. Earlier versions stay MIT. Dual licensing is kept open by `CLA.md`.
- **Release gate:** invariant 8 now authorises unsigned preview pre-releases and a rolling nightly pre-release (q-17, q-31,
  q-33). The first publication to winget, Scoop or the Store is still a re-review trigger. No release is published at this commit.
- **Decided, not yet implemented:**
  - the nightly channel and an opt-in update check;
  - Find & Ask: a search index; Everything integration; an elevated NTFS helper behind its own gate, with invariant 2
    unchanged until then; a semantic layer; an Ask provider interface;
  - copy-and-verify as the first file effect, behind its own gate, with invariant 1 unchanged until then;
  - the daily-use readiness bar;
  - retiring the legacy browser UI and freezing the Python reference;
  - package channels.

## v0.3 status, wave 9 (11 to 13 October 2026; counts measured at `129373f`)

The sections above are kept as history.
- **Merged:** 22 PRs, every one with green hosted CI at merge. The 16 this session merged each had one fresh-context review (Sonnet reviewers), with Codex inline findings triaged on every PR. The concurrent agent's six carry their own review records (1 to 11 reviews each, plus Codex). Three hosted-runner timing failures in crates the PRs did not touch were investigated, tracked (#439, fixed by #443; #441; #446) and re-run green.
- **Owner walkthrough:** on 12 October it recorded decisions q-20 to q-46 (HUMAN_TODO).
- **Concurrent agent:** a second agent, working from a repository ZIP under the owner's account, merged six of the 22 in parallel: #428, #429, #430, #440, #443 and #445. Several close follow-ups this session filed. Its open drafts (#435, #444, #447) are not counted.

| Area | State on `main` | Evidence |
|---|---|---|
| Licence (q-20 to q-23) | **AGPL-3.0-only** since #396. Earlier versions stay MIT. Dual licensing is kept open by `CLA.md`; the CLA bot is not connected yet. `TRADEMARKS.md` requires modified forks to rename | `LICENSE`, `CLA.md`, `TRADEMARKS.md` |
| Releases (q-17, q-31, q-33) | **v0.3.0-preview.1 published** (unsigned GitHub pre-release; release notes checked against the code in #400). The **rolling nightly** publishes the latest qualifying main commit after green CI (runs that queue up while one publishes are coalesced); the first was `0.3.0-nightly.20261012.1` (#432). Opt-in update check (off by default; `requireSignedVersion`). The updater stays unconfigured until the owner's key exists | `RELEASING.md` |
| Package channels (q-28, q-29) | Scoop bucket (fail-closed placeholder), winget manifest generator, Store and signing runbooks (#425). **Nothing published to any package channel** | `packaging/` |
| Diagnostics (q-15, LW-055) | **The first file write.** A previewed, redacted bundle goes to an owner-picked file through a Rust-side save dialog. The path never crosses IPC. Loomward asks its own Cancel-default replace question before overwriting (#399, ADR-V3-24) | `evidence/v3/diagnostics-bundle-q15.md` |
| Search v1 (LW-113, q-34, q-36) | An FTS5 trigram index over names, kept by triggers, with exact, prefix, word-start and substring tiers. Each page has a 250 ms soft budget and resumes. Owner-local and active grants only; no MCP or Ask exposure yet. Forget leaves no index entry. Cost: about 100 B per entry, and first scans about 25% slower (#438, #442) | `evidence/v3/search-v1.md` |
| Forget (q-9) | Also rewrites `catalog.db` (`VACUUM main`) under a crash-safe scrub marker, because first scans stage chunks (#427). Follow-ups: RAM use and silent retry (#434) | `crates/loomward-catalog/tests/crash_sweep.rs` |
| Readiness bar (q-39) | `docs/45-daily-use-readiness.md`, with crash injection at every catalogue write. The full-mode run at a9299b4 recovered at all 272 lifecycle, 14 migration and 3,870 double-crash points; the default mode in CI samples 149 and 191. No full sweep has run since #438 added search triggers inside writer transactions (#427). **No line is PASS yet** | `docs/45-daily-use-readiness.md` |
| Designs (no capability opened) | Product thesis (docs/44, #403). Copy-and-verify gate (ADR-V3-25, docs/47, #423). Find & Ask: search tiers, a USN-journal helper behind its own gate, semantic layer, Ask provider (docs/46, #426, follow-ups #433) | docs |
| Concurrent agent's fixes | SQLite path identity is kept for verbatim, malformed and zero-numbered device-port paths (#428, closes the #398 items). Completed roots report partial coverage when an aggregate is dirty (#430, #361). `verify.py --app` fails closed when app gates are unverified (#440, #431). Only qualifying nightly jobs queue (#445, #436). The heartbeat test measures from the actual delivery (#443, #439). Search and updater contract docs (#429) | test suites |
| Housekeeping | Legacy browser UI retired; Python reference frozen as a test oracle (#424, q-40). Hardware scrub finished (#394, q-18; the PNG screenshots were not inspected). SQLite opens plain paths, fixing the long-reader WAL pin (#397, #390, follow-ups #398). Writer batching fix (#395). 18 LW items minted, LW-113 to LW-130 (#422). Decision record (#401) | test suites |

Counts (Windows, at `129373f`):
- **Rust:** 988 (apps 24, backup 19, catalog 213, core 30, engine 209, http 72, lab 27, learn 18, protocol 36, service 179, shell 27, telemetry 34, windows 100; doc tests counted in their crates).
- **App:** 406 vitest, plus Playwright e2e, `--a11y` and `--live-serve`.
- **Python:** 256 tests (2 skips).
- **Contract:** 56 commands.

**Not established:**
- updater installs (no owner key);
- every docs/45 line at PASS;
- a real power cut;
- P4 at 250k rows/s (#148);
- P14 under the bulk path and the search index (#386, #442);
- search and Forget timings at 10M;
- a real FAT volume;
- the real save dialog and replace box driven by a person (#437);
- any package-channel publication.

**Effects:** one file write exists: the owner-initiated diagnostic export. When the owner confirms Loomward's replace question, it overwrites that existing file; that is the only act on an existing owner file. No other effect touches existing owner files, and none touches processes. Copy-and-verify is designed, not built.

## v0.3 status, wave 10 (17 October 2026, overnight on the KRASPYON laptop; counts measured at `8a3dc6f`)

The sections above are kept as history.
- **Merged:** 42 PRs (#458 to #525), each with green hosted CI at merge.
- **Review:** each PR had at least one fresh-context review (Sonnet reviewer, test-reviewer or fact-checker) before merge. High-risk lanes had a second lens: C2 and C5 (Opus deep reviewer, gate item G8). Fix rounds were verified by a scoped pass.
- **Bug hunting:** four Muse lens waves (bug-hunt, security, a11y, perf, test-gaps, then retroactive review-range) fed verify-then-fix lanes. Confirmed findings were fixed test-first; unconfirmed claims are recorded as such in each PR.
- **Host:** first session on KRASPYON (Windows 11 Home 10.0.26100, 18 threads, 16 GB). Its five host failures were a real defect: `OpenFileById` returns error 50 when the hint handle sits under AppData. The fix retries against the volume root with every identity check intact (#479, closes #292). The workspace is now green on this host.

| Area | State on `main` | Evidence |
|---|---|---|
| Copy-and-verify (LW-124, q-32) | **Lanes C1 to C5 and C7 built; lab-only.** C1 read-only tree verifier (#471, #485, #511). C2 native primitives: create-new staging, non-cached read-back, no-replace publish, identity-bound staging cleanup; H1-H5 measured, plus an H6 cleanup measurement (#503). C3 journal, state machine and recovery reader (#474). C4 manifest, refusal classifier, digest-bound single-use approval and the `copy.*` contract (#508). C5 executor for **synthetic sessions on registered lab roots only** (#522). C7 UI with the gate shown honestly (#516, #524). Personal sessions get `copy_effect_gated`. G6 holds (HTTP refuses personal approvals; MCP has no copy command). Staging removal is off by default until the owner answers Q1 | `evidence/v3/copy-primitives.md`, `evidence/v3/copy-executor-lab.md` |
| Copy gate (C8) | **Closed.** Needs C6 (fault and crash matrix, in progress), the G5 power-cut proof, G9 proven restore, owner answers to Q1-Q3 (drafted for the owner inbox in agent-hq#461, which publishes them once it merges), and the pre-C8 items: revocation wiring, admission lock and the residual windows (#526) | docs/47 section 11 |
| Find & Ask | F1 federated-search contract and query grammar with Rust/TS parity (#476). F2 ranked search: token index, weighted RRF, relevance harness. MRR@10 0.993 vs 0.770 for v1 on a synthetic corpus written with the ranker. The token index is **off by default**: enabling it waits on a 10M baseline, a bulk-aware swap and a measured migration (#513, #520). `federated_search` stays false | `evidence/v3/search-relevance-f2.md` |
| Readiness bar (q-39) | Row 4 churn **PASS**: 100k files and 128 mutation batches during a scan; scan plus refresh equals a fresh scan and a filesystem walk (#484). Row 1: full crash sweep on KRASPYON, 334 of 334 boundaries and 6,652 double crashes, no violation (#477). Row 3 pathological trees (#484), with the depth limit now a shared 512 (#512). Row 5 whole-store SQLITE_FULL recovery (#486). Row 6 property fuzzing, proptest substituting for cargo-fuzz (#488). Row 7 soak harness and pass rule (#478); **a 72-hour run is in progress on KRASPYON** (started 2026-10-17 03:39 local). Rows 8 and 9 stay UNVERIFIED | `docs/45-daily-use-readiness.md` |
| Monitoring (LW-121) | Low-space and fast-growth alerts. Thresholds are settings; notifications are off, and the setting says nothing is sent yet. Unknown capacity stays unknown (#518). Free space is now observed for volumes holding active personal grants, with a 500 ms per-volume bound that degrades alone (#525) | `crates/loomward-service/src/alerts.rs` |
| Developer clutter (LW-119) | First slice, a library only: manifest-anchored build outputs and caches, advice only (#514). A scale check comes before any service exposure (#515) | `evidence/v3/lw119-developer-clutter.md` |
| Correctness fixes | #361 stale complete coverage (#459). A failed forget scrub keeps the rollback copy (#459). Forget no longer drops another root's reference on file-ID reuse (#499). Hard-link rebind collision (#510, closes #363). Duplicate-name guard over mixed listings (#499). Fail-closed DTO rules (#497). The tree verifier stays honest under the Find and Both strategies (#511). App stale-response races (#472, #524). Telemetry and volume honesty (#490). Release-notes sanitising in the installer prompt (#487) | PR tests |
| Test and CI health | Load-robust deadline tests (#500, refs #320). Bounded catalogue hangs (#495). CI job timeouts (#496). Jobs coverage (#480). Service gap pins (#501). Probe target dir (#507). The soak test skips under a profile checkout (#523) | CI |
| Performance | 6,000-node tree.slice 4.2-4.8x faster at 100k to 1M files (100-node slices about 3-3.7x). 200-candidate duplicate reads 1.2-1.4x, with overlapping ranges; the 2-candidate case did not improve. Forget keeps its whole-index optimize for erasure. Build and forget timings varied with host load and are not attributed to this change (#517) | `evidence/v3/catalog-perf-2026-10-17.md` |
| Real-system QA on KRASPYON | Desktop shell: self-test 14/14 and WebView2 probe 24/24 VERIFIED, first render 2.86 s (#505). Read-only real-folder cross-check: four strategies agreed exactly on 170k directories and 154k files (21.7 GB), 16k directories and 126k files (172 GB), and 426 directories and 8.7k files. Native handle strategy 9.5 s versus 694 s for single-threaded std, cold-cache first; profile-located roots were refused as designed. Only anonymised aggregates are recorded | `evidence/v3/kraspyon-desktop-qa.md`, `evidence/v3/kraspyon-real-folder-crosscheck.md` |

Counts (Windows, KRASPYON, at `8a3dc6f`):
- **Rust:** 1,288 tests: 1,283 passed, 1 ignored, and 4 lab-copy tests that fail only in a cloud-synced (OneDrive) checkout, where the copy rightly refuses a synced destination. #527 (open at the time of writing) makes those four skip with that reason. The workspace was green on the worktree checkouts outside OneDrive.
- **App:** 466 vitest, plus Playwright e2e, `--a11y` and the copy walk.
- **Python:** 279 tests (1 skip).
- **Contract:** 64 commands.

**Not established:**
- the 72-hour soak result;
- the copy crash matrix (C6, in progress), a real power cut (G5) and a proven restore (G9);
- any copy of personal files;
- the ranked index enabled in any store;
- ReFS;
- a real Linux run of the Windows-gated tests;
- the owner's answers to Q1-Q3.

**Effects:**
- The owner-initiated diagnostic export remains the only write that can touch an existing owner file.
- Copy-and-verify now writes, renames and (only if enabled) removes its own staging, but only for synthetic sessions on registered lab roots. No owner file and no process is touched.

## v0.3 status, wave 11 (17 October 2026, day on the KRASPYON laptop; counts measured at `4b05fd6`)

The sections above are kept as history.
- **Merged:** 33 PRs (#529 to #577), each with green hosted CI at merge.
- **Review:** each PR had at least one fresh-context review before merge; fix rounds got a scoped verification pass. A second lens (Opus deep reviewer) ran on #544 (revocation fence) and #556 (first outbound-network code). Muse lens waves 5 and 6, plus a re-run on forget after #562, fed verify-then-fix lanes; each claim was checked with a failing test first and false claims are recorded in the PRs.
- **Host:** WSL 2 Ubuntu 24.04 was installed on KRASPYON to reproduce the Linux CI hang (#495). The disk filled once from parallel build caches (now cleaned up as lanes finish).

| Area | State on `main` | Evidence |
|---|---|---|
| Copy-and-verify (LW-124) | **Still lab-only.** C6 fault and crash matrix landed (#532, 476 kills at its head); the full re-run at the later head: 482 kills (242 matrix + 240 random), 0 source changes, 0 false commits, plus journal-append faults (state.db full or locked) at every record boundary, 360 in the full sweep (#566). Journal head anchor detects truncation and replacement (#539). Revocation fence under a per-run admission lock; cancel interrupts publish retry; folder stream/EA checks; H7 measured (#544). Identity change fences running copies; bounded cancel wait (#564). Copy service: bounded maps and shutdown, lost-ack pinned (#573). Personal sessions still get `copy_effect_gated`; staging removal stays off | `evidence/v3/copy-faults.md`, `evidence/v3/copy-executor-lab.md` |
| Copy gate (C8) | **Closed.** Still needs the G5 power-cut proof, G9 proven restore, the owner's answers to Q1-Q3 (agent-hq#461), the remaining #526 items (window before Committed, refusal code naming, resume) and #567/#569 residuals | docs/47 section 11 |
| Find & Ask | **F3** persistent per-root watchers, unprivileged USN catch-up between sessions, dirty scopes on overflow or journal wrap, per-hit freshness (#531); unresolved journal records never mark a root fresh (#548). **F3a** per-client disclosure grants and the grant-scoped search adapter (LW-067, LW-069, #541), size-oracle guard (#555). **Ask provider** (LW-117, #556): local loopback broker only, cloud models refused, exact-bytes disclosure record (#572). Explicit scans and duplicates requests pre-empt automatic refreshes (#565, #577). Search v1 follow-ups: cursor progress or refusal, unknown sizes honest, 10M timings (#575). `federated_search` and the ranked index stay off | `evidence/v3/find-f3-freshness.md`, `evidence/v3/search-followups-442.md` |
| Forget | Exact-grant retention for migration copies, honest pending cleanup, file-backed VACUUM temp (1 GiB catalogue peak working set 1.54 GB to 609 MB), startup sweep non-fatal and visible, remapped legacy copies byte-checked (#562); state-copy WAL/SHM sidecars and catalogue scrub-pending health (#576) | `evidence/v3/forget-434-391.md` |
| Catalogue and performance | First-scan compaction reclaims bulk-build pages: 1M files 311.2 to 228.0 bytes/entry (P14 200 still open: the trigram index is 81 B/entry) (#563). Developer-clutter detector bounded: 9.6 billion to 783k VM steps on the wide lab (#558) | `evidence/v3/first-scan-bulk-load-148.md`, `evidence/v3/lw119-developer-clutter.md` |
| Readiness bar (q-39) | Row 7: the 72-hour soak on KRASPYON has run 14.2 h with 0 errors so far. It started before #534 (wall-clock continuity), so its receipt is evaluated as legacy: a sleep or hibernate gap cannot be ruled out from wall-clock time. Rows 8 and 9 stay UNVERIFIED | `docs/45-daily-use-readiness.md` |
| Test and CI health | F3-induced load flakes made deterministic (#536, #542, #557, #565, #577); Linux catalogue hang: not reproduced in about 640 WSL runs, the only overrun was a host disk stall (every thread in `fsync`); a watchdog now prints every thread's state and backtrace before aborting (#574, refs #495) | CI |
| Other | Diagnostic bundle follow-ups: honest dialog reasons, root cap, panel store (#559). Duplicates evidence stays honest after sanitising (#546, #551). Telemetry CPU denominator pinned (#540, #554). Volume space read bound and per-volume sharing (#542, #549, #570) | PR tests |

Counts (Windows, KRASPYON, at `4b05fd6`, `RUST_TEST_THREADS=3`):
- **Rust:** 1,442 tests: 1,440 passed, 2 ignored, 0 failed.
- **App:** 483 vitest, plus Playwright e2e.
- **Python:** 279 tests (1 skip).
- **Contract:** 64 commands.

**Not established:**
- the 72-hour soak result;
- a real power cut (G5) and a proven restore (G9);
- any copy of personal files;
- the Linux CI hang's root cause (#495);
- the ranked index or federated search enabled in any store;
- a real local model behind the Ask broker;
- ReFS;
- the owner's answers to copy Q1-Q3 and Ask Q3-Q4 (agent-hq#461).

**Effects:**
- The owner-initiated diagnostic export remains the only write that can touch an existing owner file.
- Copy-and-verify writes, renames and (only if enabled) removes its own staging, but only for synthetic sessions on registered lab roots.
- The Ask broker is the first outbound-network code: it contacts only a loopback endpoint the owner configures, never by default.

## Maintenance reconciliation — 20–21 October 2026

Reviewed the 173 initially open issues and four open PRs against current source, review comments and execution evidence. The [wave record](../evidence/v3/backlog-review-2026-10-20.md) lists 13 merged PRs, 1 remaining PR and 7 scoped closures before this documentation checkpoint; its JSON companion preserves exact heads, trees and hosted runs.

The changes advance forgotten-response handling, source packaging, scan settlement/history/cancellation, watch probing, durable feedback replay, terminal metadata and diagnostics as detailed in that record. Historical counts and receipts above are retained unchanged. This wave adds no claim of complete 10M service readiness, a continuous 72-hour pass, real native foreign-rename qualification, required power-cut/restore proof, teacher confinement or manual accessibility acceptance. C8 remains closed.
