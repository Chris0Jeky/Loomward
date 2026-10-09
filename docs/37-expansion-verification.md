# v0.2 implementation and verification status

9 October 2026. This is the current capability matrix for the expansion. `16-implementation-status.md` and the original evidence describe the first pass; they remain useful historical records, not the current test-count source.

## What changed in code

| Component | Actual reference behavior | Not established |
|---|---|---|
| `planner_v2.py` | Heuristic portfolio plus budgeted exact search for small supplied scenarios; explicit feasibility and optimality scope | Native port, live storage measurements, bidirectional movement or a production optimiser |
| `catalog.py` | Immutable scope-filtered in-memory SQLite metadata view; bounded keyset pages, exact byte strings, per-view keyed references/generation | Persistent native index, native identity, authenticated multi-user authority or a replacement for the original full-state UI path |
| `scheduler.py` | Multidimensional admission simulation and thread-safe ephemeral lease broker; idempotency, expiry, capacity accounting | OS enforcement, authenticated cross-process broker, restart recovery or true demand measurement |
| `interop.py` | Four fixed snapshot/read/simulate tool contracts and two fixed resources; no names by default | General file access, new scan scopes, approval, effects or actual provider integration |
| `mcp_stdio.py` | Bounded stdio reference with current per-request semantics and a separate legacy handshake | Official SDK/host certification, HTTP, Apps, Tasks, sampling, elicitation or dynamic registration |
| Existing HTTP API | Added `/api/plan-v2` and `/api/schedule` pure simulations | Lease mutation, connection, approve/apply, filesystem or process effect endpoints |
| Browser UI | Ten routes, including Decision desk, Connections and Resource budgets | Durable review history, connected providers, native desktop IPC or a production-scale virtualised explorer |
| Contracts | Runtime-owned tool input/envelope schemas and parity checks; labelled draft provider/event/proposal examples | A running provider registry/event bus or a schema match conferring a grant |

The original scanner, duplicate reader, weighted student, optional teacher, state store and process observation remain. The Rust/Tauri sources remain the original uncompiled native foundation; the new allocator was not silently copied into an untested native implementation.

## Executed gates

The final Python suite contains **184 tests**, compared with 89 in the baseline. All passed in the authoring Linux/Python 3.13.5 environment. The optional JSON Schema validator was present and its schema/example validation test ran, not skipped. Node parsed both UI modules and ran nine boundary assertions plus 80 Python/JavaScript admission-parity fixtures.

The Chromium integration harness ran **14 checks** using `set_content` and a Python bridge to the real loopback HTTP application. It exercises persistent feedback, simulations, input sanitisation, all routes, new review/request-builder/admission interactions and narrow layout. This is not direct browser-network or Windows/WebView2 evidence. Screenshots are real rendered reference UI with synthetic data, not design mockups.

The MCP tests include actual stdin/stdout subprocess exchanges using both protocol eras. They are not tests with Claude Desktop, Codex, LM Studio or another official MCP host. No external host configuration was modified. The live teacher still has mock HTTP tests only; no actual model or personal dataset was evaluated.

Use `evidence/v2/final-suite.txt`, `ui-final.txt`, `standalone-checks.json`, `benchmarks.json` and `verification-summary.json` for final receipts. Earlier red/green logs remain so the test-first history and corrected defects are visible. A previous suite surfaced an unclosed SQLite connection in an original test fixture; the fixture now explicitly closes it. The application Store already closed its own connections.

## Experiments

The allocation experiment repeats the original seed and 51 tiny scenarios. The old baseline misses nine feasible targets and has positive shortfall regret in 24 cases. The new search has zero of each, all searches complete, and no checked transfer/reserve violations. The claim is scoped to the supplied disjoint-group model, not real filesystem safety or workload forecasting.

The catalogue experiment constructs 50,000 synthetic records in memory, measures bounded 50-row queries, and compares a known-position SQL keyset with a deep OFFSET query. The receipt includes build time, repeated timings, exact payload sizes, query plan and limitations. It does not measure Windows scanning, native memory use, cold-disk I/O or whole-application speed. The bounded page omits information present in the full snapshot; this is projection, not lossless compression.

## Review findings corrected

Oversized Unicode flags could prevent a first page from making progress; projection bounds now make a row pageable. Externally mutable lease capacity could change admission accounting; capacity is now copied at the boundary. Unicode release tokens could raise the wrong exception; comparison is byte-safe. A public generation hash could confirm guessed metadata; it is now keyed to the view, with a regression proving excluded records do not affect the same-key projection. New issue bodies originally lacked durable importer markers; all 100 now carry one exact marker and the dependency graph is checked for cycles.

## Explicit limitations

No Windows execution, Rust compilation, Cargo lockfile resolution, Tauri build, installer, native CI, code signing, external provider connection, real model quality evaluation, or personal telemetry measurement occurred. There is no move/delete/rename/uninstall/backup-execution/process-control capability. A local lease is an accounting record, not proof that the operating system enforces it.

No GitHub repository, issues, PRs or remote commits were created. Existing estate READMEs were inspected read-only for integration design. Work and review were inline: no independent subagent or independent security review was available. Final archive/bundle extraction receipts are generated outside the source tree so their hashes can be checked without recursive self-hashing.
