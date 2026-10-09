# Resume Loomward v0.2

This is an existing source checkout expanded from the v0.1 bundle at `1e51c3b`, not a greenfield scaffold. The original implementation, docs, issue IDs and negative experiment are retained. The current branch in the bundle is `expansion/interop-v2`; inspect Git rather than assuming a remote exists.

Read in order: `AGENTS.md`, this file, `CHECKPOINT.json`, `docs/23-expansion-overview.md`, `docs/37-expansion-verification.md`, `docs/38-reference-operating-guide.md`, and the exact assigned issue. `docs/40-parallel-work-packs.md` defines non-overlapping local lanes.

Run `python scripts/verify.py` before editing. The authored gate is 184 Python tests, both JavaScript syntax checks, nine JS boundary assertions and 80 cross-language fixtures. Optional schema validation ran in the authoring environment; an absent jsonschema validator is a skip. Browser integration is separate: 14 Chromium checks using a Python HTTP bridge. No native compilation, Windows, real MCP host or actual model validation occurred.

The three new UI pages are functional references. Review notes are session-only, integration tiles are not connected, admission has no OS effect, and standalone tier planning is a labelled greedy preview. The live Python tier route uses v2. The native Rust planner is still v1 and uncompiled.

The first native pack remains LW-001/LW-002/LW-064. An independent protocol lane may start LW-065 on installed hosts, and a reference UI lane may start LW-068. Do not interpret the 100 planned backlog tasks as already delivered features. Do not enable effects through MCP or add arbitrary file/process endpoints.

The paste-ready next-agent prompt is `LOCAL-AGENT-PROMPT.md`. The initial handoff remains under `handoff/v1/`; use it as history, not the current status source. No repository or issue was published remotely. Publication helpers remain opt-in and dry-run-first.
