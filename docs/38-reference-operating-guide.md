# Run and inspect the v0.2 reference

## Start without a model or provider account

Extract the complete source archive and enter the `loomward` directory. Python 3.11 or later is required; the authoring tests used Python 3.13.5 on Linux. Use the installed Python command appropriate to your machine. No installation, model download or administrator rights are needed for the synthetic web workbench.

```powershell
python scripts/run.py demo --open
```

Keep the printed session URL private. It contains the local session token. The application runs on loopback and demo feedback writes only to application state. Ctrl+C stops it. The no-server `preview.html` opens directly in a browser, uses synthetic data and has network connections disabled; it cannot retrain the Python student.

The new pages are Decision desk, Connections and Resource budgets. Decision choices are session review notes. The MCP builder exports an unsent request. Resource inputs are synthetic budgets, not measurements of your machine. In the live reference, tier simulation uses Python v2; the standalone HTML clearly identifies its simpler greedy preview.

## Select a small real metadata scope

```powershell
python scripts/run.py serve --root "C:\LoomwardTest" --open
```

Create and select a disposable folder first. Do not begin with a whole drive, Windows directory or only copy of important files. Running the server does not grant content hashing or model transmission automatically. Keep application state outside the selected scan root. Native Windows object identity and race-safe effects are not implemented by the portable path checks.

Use the CLI help for supported bounded scan/duplicate/training commands. The first pass's detailed README retains those workflows. Do not assume the interface's Connections page has connected an application just because a tile is present.

## Run the MCP reference

```powershell
python scripts/run_mcp.py --demo
```

This process waits for newline-delimited JSON-RPC on stdin. It writes protocol messages to stdout and its startup warning to stderr. A blank waiting terminal is not a hung web server. Use it under a configured MCP host or run the subprocess tests through `python scripts/verify.py`. Ctrl+C or closing stdin ends a manual session.

For an exported metadata snapshot, select the actual JSON file explicitly:

```powershell
python scripts/run_mcp.py --snapshot "C:\LoomwardTest\inventory.json" --scope-prefix "Documents"
```

The prefix is a relative component-bound path inside that snapshot, not a directory to scan. An empty matching scope returns an honest empty summary. The process reads the selected JSON file only and never opens paths named in its records. It accepts at most 20 MiB and at most 200,000 records; either bound can reject a larger export.

Names and relative paths require an additional owner launch choice:

```powershell
python scripts/run_mcp.py --snapshot "C:\LoomwardTest\inventory.json" --scope-prefix "Documents" --disclose-names
```

Without the flag only workspace summary and supplied placement simulation are advertised. With it, catalogue search and evidence explanation become available. This is not a content-read grant. A host may forward returned metadata to a cloud model, even though transport is local. Stop and relaunch to change or revoke the process-lifetime scope/disclosure choice. Durable per-client grants are future work.

## Host configuration template

A host that supports a conventional `mcpServers` stdio configuration may accept a shape like this, but its current documentation and version must be checked. This is a template, not an automatically installed or verified host integration:

```json
{
  "mcpServers": {
    "loomward-demo": {
      "command": "python",
      "args": ["C:\\ABSOLUTE\\PATH\\loomward\\scripts\\run_mcp.py", "--demo"]
    }
  }
}
```

Replace the path and interpreter with installed absolute paths. Start with demo, not personal names. Do not add network/effect arguments or credentials. Modern and legacy request examples are in `contracts/v2/`; they use separate versioning/lifecycle semantics. Current/legacy interoperability is tested by the repository subprocess harness, not by this configuration example.

## Verification commands

```powershell
python scripts/verify.py
python scripts/verify.py --ui --browser "C:\PATH\TO\chrome.exe"
python scripts/build_preview.py
python experiments/v2_benchmarks.py
```

The browser gate requires installed Playwright plus a usable Chromium/Chrome executable. The schema validation test uses the optional jsonschema package when installed; an absent validator is an explicit skipped test, not a schema-conformance pass. Do not install prerequisites silently on someone else's computer.

Cargo is a separate native gate. On a suitable host, run `cargo fmt --all -- --check`, `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`. The Tauri scaffold is separate from the root workspace and remains synthetic-only until native IPC integration is implemented and verified.

## Continuation and publication

Read `handoff/START-HERE.md` and paste `handoff/LOCAL-AGENT-PROMPT.md` into the local coding agent. The original native critical path remains first; the new interoperability and reference-UI lanes can progress separately. Publishing helpers default to dry-run and operate only after an explicit owner choice. The 100 LW identifiers are local backlog IDs, not already published GitHub issues.
