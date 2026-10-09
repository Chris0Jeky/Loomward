# Loomward

**A local-first Windows workspace companion that learns how you organise, explains storage pressure, and keeps changes under your control.**

Status: **research prototype**, not a production file manager or system optimiser. The Python reference and browser workbench run today. The Rust engine foundation and Tauri shell are source drafts that have not been compiled in the authoring environment. No user-file move/delete or process-control capability is enabled.

Loomward is a provisional name. Original project code is MIT licensed. No GitHub repository or remote issues were created during this authoring pass.

## v0.2 expansion

The existing project has been extended, not replaced. The Python reference now includes a scoped, bounded SQLite snapshot catalogue; an improved budgeted allocation search; multidimensional resource admission and ephemeral lease accounting; and a four-tool, two-resource MCP stdio reference. The UI has ten pages, including Decision desk, Connections and Resource budgets. All effects remain disabled.

Read the [expanded thesis](docs/24-thesis-and-research-program.md), [control/evidence architecture](docs/25-control-evidence-architecture.md), [MCP support matrix](docs/28-mcp-specification.md), [26 use cases](docs/33-use-case-atlas.md), [operating guide](docs/38-reference-operating-guide.md), and [parallel work packs](docs/40-parallel-work-packs.md). New primary-source references are in [EXPANSION-SOURCES.md](docs/EXPANSION-SOURCES.md).

The authored verification is 184 Python tests, 14 bridged Chromium checks, nine JavaScript boundary assertions and 80 Python/JavaScript parity cases. Rust, Windows, actual MCP hosts and real models remain unverified. The 51-case allocator comparison resolves the original nine missed feasible targets in its static synthetic model, not in a live filesystem. Full receipts and limitations are in [the current capability matrix](docs/37-expansion-verification.md).

Start the scoped synthetic MCP process with `python scripts/run_mcp.py --demo`. This is a stdio process for a client, not a web page; it does not scan or disclose names by default. Protocol examples and labelled draft integration schemas are in [contracts/v2](contracts/v2/README.md). No external provider is connected by the Connections page.

## Start here

From an extracted source checkout with Python 3.11 or later:

```powershell
py -3 scripts/run.py demo --open
```

On Linux/macOS, substitute `python3` for `py -3`. This starts a loopback-only Python server and opens a synthetic workbench. It does not scan your machine or call a model. It creates an application-state database for demo feedback. Stop the server with Ctrl+C. Port 8765 is the default; add `--port 0` to allocate an available port. Keep the complete printed session URL, including its token fragment, private.

For a no-server visual preview, open `preview.html`. Its data and tier recommendations are synthetic. Feedback stays in page memory until exported. It cannot inspect your disk or processes and does not retrain the backend model.

Read [the local-agent handoff](handoff/START-HERE.md), [current implementation status](docs/37-expansion-verification.md), and [the ordered backlog](backlog/INDEX.md) before extending the project.

## Original reference foundation, retained

This table describes the original modules. The v0.2 additions and precise limitations are listed above and in the current capability matrix.

| Area | Current implementation | Not yet implemented |
|---|---|---|
| Inventory | Bounded, explicit-root Python metadata scan; coverage/errors/unknown allocation; exclusions and link/placeholder refusal | Stable native Windows identities, NTFS journal index, production-scale persisted catalogue |
| Storage UI | Search, file-family treemap, logical sizes, coverage and snapshot import | Full hierarchical million-object explorer and native physical-size accounting |
| Duplicate inspection | Opt-in bounded SHA-256 followed by byte comparison, stale-record checks | Safe cleanup executor, exact reclaimability, native Windows handle-based reader |
| Personal learning | Real weighted Naive Bayes student; persisted human labels, teacher provenance, refit, retraction semantics and abstention | Embeddings, calibrated deployment thresholds, real personal dataset evaluation, learned access heat |
| LLM teacher | Optional loopback structured-output adapter and offline request builder; adversarial/mock HTTP tests | A real model quality evaluation and interactive teacher-review queue |
| Disk tiers | User-supplied scenario simulator with activity/pins/cooldown, reserves and distinct byte estimates | Live volume discovery, bidirectional relocation, recall, path continuity, production allocator |
| Processes | Optional read-only psutil snapshot and explicit metric limitations | Windows-native controls, Job Object policy management, actual memory optimisation |
| Desktop, apps, backup | Detailed providers, contracts, safety gates and acceptance issues | OS integration, backup/restore jobs, app uninstall or shortcut editing |
| Native foundation | Rust scanner, typed policy, transition simulator, greedy planner and 20 written tests | Successful compilation/lint/testing, native integration and Windows validation |
| Desktop packaging | Tauri 2 synthetic-preview scaffold with no custom privileged commands | A packaged Windows application connected to the real engine |

## Why the design is split

Meaning and location are different decisions. A collection can show all files for a project without moving any. A disk policy can keep the active project on an SSD without changing its semantic membership. Neither decision grants permission to execute a move.

The target architecture separates observation/catalogue, personal learning, constrained proposals, scoped consent, an auditable action broker, backup evidence and native providers. The broker is deliberately absent from the prototype. A model score, filename, timestamp, imported snapshot or teacher sentence must never become a filesystem command.

The design includes virtual workspaces, project-aware atomic groups, source-retirement and recovery protocols, LLM leases, hot/cold placement, model promotion/rollback, active questions, resource budgets and future app-specific providers. See the [product specification](docs/02-product-spec.md), [architecture](docs/04-architecture.md), [learning design](docs/06-learning.md), [tiering design](docs/07-tiering.md) and [extension experiments](docs/18-opportunities-experiments.md).

## Observe a small real scope

Start with a disposable folder, not your Windows directory, whole profile or only copy of important files:

```powershell
py -3 scripts/run.py serve --root "C:\LoomwardTest" --open
```

This reads metadata under the explicit root. The local UI can persist labels and exports into application state. Content inspection is a separate opt-in. Portable checks are not a guarantee against malicious concurrent reparse/ancestor replacement; run non-elevated in a trusted test scope. Native handle-based tests are first-class backlog gates.

Normal state locations are `%LOCALAPPDATA%\Loomward-reference` on Windows or the XDG state directory on other platforms. Demo and observed profiles are separate. `--state-dir` can select another state location, which should be outside the scanned scope. Do not publish this state, observed filenames, training exports or diagnostics. Do not host the reference server on the network.

## Optional process observation

No optional package is required for metadata scanning, learning, tier simulation or the UI. For read-only process metrics, create a local virtual environment and explicitly install the optional dependency:

```powershell
py -3 scripts/bootstrap.py --create-venv --telemetry
.venv\Scripts\python.exe scripts/run.py serve --root "C:\LoomwardTest" --processes --open
```

The bootstrap refuses to replace an existing `.venv`. Dependency installation uses the network only when `--telemetry` is requested. Use that virtual environment's Python for subsequent commands. Running `py -3` does not automatically select it. The process page has no kill, suspend, priority, quota or trim action.

## Command-line examples

The CLI is a test/debug interface, not the intended replacement for the visual application:

```powershell
py -3 scripts/run.py doctor
py -3 scripts/run.py scan "C:\LoomwardTest" --output snapshot.json
py -3 scripts/run.py duplicates "C:\LoomwardTest" --snapshot snapshot.json --consent-content
py -3 scripts/run.py plan fixtures/tier-scenario.json
py -3 scripts/run.py train fixtures/training-demo.json --allow-synthetic --output demo-model.json
py -3 scripts/run.py predict demo-model.json fixtures/features.json
py -3 scripts/run.py teacher-request fixtures/features.json --model YOUR-LOCAL-MODEL-ID --item-id example
```

`--output` exclusively creates a new file; it refuses overwrite. Use a fresh output filename on a second run. A duplicate report means byte equality at observation time, not permission to delete or a physical-space saving. The teacher-request command only prints a request; it makes no network call.

A live teacher request is a separate explicit operation:

```powershell
py -3 scripts/run.py teacher fixtures/features.json --model YOUR-LOCAL-MODEL-ID --item-id example --endpoint http://127.0.0.1:1234/v1/chat/completions --consent-metadata
```

The endpoint must be literal loopback with an explicit port and supported path. The server must already be running with the selected model available. This prototype does not configure, download or authenticate a model automatically. Only bounded filename/type/context/size metadata is sent; no document body, command or tool is included. Even local model software has its own settings and retention behavior. No real model was queried during authoring.

## Verification

```powershell
py -3 scripts/verify.py
py -3 experiments/reference_smoke.py
```

The final authored reference passed **89 Python tests** and **11 bridged Chromium UI checks** on Linux, with JavaScript syntax validation. See [evidence/verification.txt](evidence/verification.txt). Native source contains **20 unexecuted tests**. A missing Rust compiler is explicitly UNVERIFIED, not PASS. The Windows PowerShell bootstrap, Windows reference behavior, Tauri/WebView2, actual LLM output and hosted CI remain unverified.

For UI checks, install Playwright and a compatible local Chromium under your own tooling policy, then run `python scripts/test_ui.py --browser PATH_TO_CHROMIUM`. The recorded test uses Chromium rendering with a Python bridge to the real reference API because direct browser navigation was blocked in the authoring environment. It is not a Windows-native or direct browser-network integration claim.

The synthetic experiment scans 5,000 immediately created tiny files and checks 51 small tier scenarios against an exhaustive oracle. It is not a comparison with WinDirStat, Everything or Czkawka. The greedy allocator misses some feasible targets; [the findings](docs/22-experiment-findings.md) and counterexample are preserved rather than hidden.

## Native continuation

Install the appropriate Rust MSVC toolchain and Microsoft C++ Build Tools on the local Windows host using official instructions. Review prerequisites in [the Windows validation pack](handoff/WINDOWS-VALIDATION.md). Then:

```powershell
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
py -3 scripts/verify.py --require-native
```

The first `cargo fmt` is intentional: no formatter/compiler ran in the authoring environment. Resolve and review dependency lockfiles before claiming reproducibility. Address genuine compiler/test failures; do not weaken tests to make the handoff look green. `native/` is a separate, excluded Tauri project and needs its own build gates. A root cargo pass does not prove that desktop packaging works.

## Repository map

`python/loomward/` is the runnable reference. `crates/` contains the native foundation. `ui/` is the browser workbench. `native/` is the synthetic Tauri scaffold. `schemas/` contains four interchange schemas. `fixtures/` contains synthetic examples and a shared planner golden output. `tests/` and `experiments/` preserve checks. `docs/` contains the product and engineering blueprint, 32-source research register, 12 ADRs and safety/evaluation decisions. `backlog/` contains 64 dependency-linked local issues. `handoff/` contains the execution contract and local-agent launch prompt.

A separate proposed catalogue SQL design lives in `docs/catalogue-v2.sql`. It is not an automatic migration of the current feedback database.

## Publishing

A GitHub connector was inspected, but it did not expose repository creation. No remote repository, PR, issue or Actions run was created. `scripts/publish_github.py` and `scripts/publish_issues.py` are dry-run by default. They require an authenticated local `gh` CLI for execution. Repository publishing defaults private; a public publication needs an explicit matching confirmation. Read their dry run and [release policy](docs/14-open-source-release.md) first.

Never import all 64 issues by accident. The issue importer defaults to the first eight critical-path items; `--all` is explicit. The manifest's `LW-001` style IDs are stable local IDs, not GitHub issue numbers. No script merges PRs or changes an unrelated repository.

## Safety and contribution

Read [SECURITY.md](SECURITY.md), [AGENTS.md](AGENTS.md) and [CONTRIBUTING.md](CONTRIBUTING.md). Do not enable file moves merely because the interface looks complete. Recovery, metadata fidelity, destination capacity, current native identities and scoped approval all need their separate Windows gates. Backup, synchronisation and relocation are not interchangeable. Quarantining originals on the source disk does not clear that disk's space.
