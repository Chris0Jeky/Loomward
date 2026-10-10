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
