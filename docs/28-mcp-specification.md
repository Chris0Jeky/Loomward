# MCP design and implemented reference contract

Research checked 9 October 2026. The official current MCP revision is **2026-07-28**, with per-request version/capability metadata. The older **2025-11-25** revision uses initialization. These are separate behaviours, not interchangeable request shapes [V01-V04].

## Implemented now

`python/loomward/interop.py` implements a protocol-neutral snapshot tool service. `python/loomward/mcp_stdio.py` is a small, hand-written stdio protocol adapter. `scripts/run_mcp.py` launches it against an explicit synthetic fixture or an exported snapshot. There is no network listener and no filesystem scan from record paths.

It implements modern `server/discover`, `ping`, tool listing/calls and two fixed JSON resources. It also implements a separate legacy initialize/initialized path for 2025-11-25. Tests exercise actual subprocess stdio in both eras. No third-party MCP host or official SDK was installed or exercised. Treat this as a reference integration, not a certified or comprehensive MCP server.

## Tool surface

| Tool | Input | Output | Authority |
|---|---|---|---|
| `workspace_summary` | Empty object | Scoped count, logical bytes, generation and freshness limits | No names by default; no new scan |
| `catalog_search` | Literal query, extension, page limit/cursor | Bounded names/relative paths and opaque references | Requires launch-time metadata disclosure |
| `evidence_explain` | One opaque reference | Item evidence and explicit unknowns | Same granted view only |
| `placement_simulate` | Strict, bounded scenario | Non-executable proposals and search status | Pure supplied-estimate computation |

The default tool list contains summary and placement simulation. Search and explanation are listed only with metadata disclosure. Calling a hidden name directly does not bypass the grant check. The two resources are `loomward://reference/summary` and `loomward://reference/policy`; arbitrary URI/path resolution does not exist.

Tool annotations are descriptive hints, not access controls. MCP's tools specification also warns that annotations should not be treated as trusted security assertions merely because a server supplies them [V05]. Loomward enforces its own small operation set independently.

## Scope and privacy

The owner chooses a snapshot at launch. `--scope-prefix` restricts component-bound relative paths within it. `--disclose-names` separately enables names and relative paths. Every caller of that process receives the same launch-selected view. This is not authenticated per-client or multi-tenant isolation.

The process reads only the selected JSON file, up to 20 MiB. Record paths are data. A catalogue reference cannot be used to open an underlying file. Sensitive/excluded flags and scope are applied before aggregates. Unknown live freshness is always visible.

Local stdio does not imply local inference. An agent host may send returned metadata to a hosted model. The owner must understand the host's policy before supplying personal snapshots. Launch the reference with `--demo` first; terminating and relaunching the process revokes that reference view. Persistent, per-request grants are a future service capability.

## Modern request example

```json
{
  "jsonrpc": "2.0",
  "id": "summary-1",
  "method": "tools/call",
  "params": {
    "_meta": {
      "io.modelcontextprotocol/protocolVersion": "2026-07-28",
      "io.modelcontextprotocol/clientCapabilities": {}
    },
    "name": "workspace_summary",
    "arguments": {}
  }
}
```

Each modern request supplies its metadata; successful results include `resultType: "complete"`. A previous request cannot lend its version or capabilities to a later one. Self-reported client information is not authentication. Missing mandatory fields receive `-32602`; unsupported modern versions receive `-32022` with a supported-version list. The legacy handshake remains isolated from these rules [V02-V04].

## Framing and limits

The stdio adapter uses newline-delimited UTF-8 JSON. Standard output contains protocol messages only; startup guidance goes to standard error, consistent with the transport specification [V07]. Input frames are capped at 65,536 bytes, output at 262,144 bytes, nesting at 32, and tool calls at 120 per minute per process. Oversized lines are drained only within a bounded recovery limit. The service is serial and bounded; it is not a concurrent long-running task runner.

The catalogue has at most 200,000 supplied records; direct CLI snapshot loading is also byte-capped. Search returns at most 100 rows and has a SQLite VM work limit. Tool placement accepts at most 64 groups across four volumes and a 5,000-node search budget. Exact byte strings are converted only after range validation.

Duplicate JSON keys, non-finite numbers, malformed IDs, unknown tool fields and injected operation fields are rejected. A tool business/input failure uses `isError`; unsupported/malformed protocol operations return JSON-RPC errors. Output schemas describe the structured result envelope. The implementation validates its fixed input vocabulary in code and does not load arbitrary third-party schemas or dereference network `$ref` values. Schema/host conformance still needs a broader independent test matrix.

## Not implemented

No HTTP transport, OAuth flow, remote gateway, per-request principal authentication, subscriptions, Tasks extension, MCP Apps, prompts, roots, sampling, elicitation, binary content, provider installation, dynamic tool loading, live refresh, approval or execution. Cancellation does not interrupt a long-running worker because no such worker is exposed; a future asynchronous implementation needs its own cancellation and resumability tests.

Unsupported features are not advertised. A client's feature capability cannot expand the server's operation set. No fallback turns an unsupported method into shell execution.

## Useful next MCP capabilities

**Explain a storage decision.** Return the exact scope, candidate group, current evidence, excluded alternatives, predicted relief, byte costs and unknowns. The host can explain the proposal without receiving every filename.

**Prepare a review item.** A future `proposal_prepare` can persist a bounded draft. It must not accept approval credentials and must not be paired with `proposal_approve` in the same agent-facing server. The UI or separately trusted owner channel retains approval.

**Ask for cooperative work admission.** A future work adapter can request a budget for an identified workload and return a lease. The token must not confer arbitrary process authority. The current lease broker is not exposed over MCP.

**Inspect recovery evidence.** Query a provider's snapshot/restore evidence under a distinct read grant. Do not let a model trigger backup pruning through a generic backup tool.

**Embed a review view.** An optional MCP App could render the same evidence card within a compatible host. Treat the embedded UI as another client of a backend policy boundary. A browser message or untrusted tool-result HTML must not mint an execution grant. Start with read-only inspection, bounded local assets and no third-party network content.

**Long-running analysis.** A future task extension can expose progress and cancellation for an authorised scan or extractor. Persist a work identifier and input scope; revalidate consent after restart. A protocol task-complete result is still not a filesystem-effect receipt.

## Production gateway requirements

Choose a maintained SDK only after verifying its supported protocol revisions and constraints; “latest SDK” is not a compatibility guarantee. Pin the dependency and record actual host tests. Preserve the protocol-neutral ToolService boundary so the adapter can be replaced without changing policy.

If HTTP is later introduced, implement the current MCP authorization requirements rather than reusing the prototype UI token. Credentials need intended-audience validation, expiry, least privilege and explicit client consent. Never forward a client token blindly to an upstream service. Host/Origin checks, redirect policy, network allowlists, response budgets and anti-confused-deputy checks belong in that gateway [V08].

A production compatibility matrix must identify host name/version, transport, protocol era, schema validation, errors, Unicode, pagination, process shutdown, permission refusal and metadata forwarding configuration. This pass records protocol fixtures and subprocess tests, not that matrix's successful completion.

## Source references

- [V05] MCP tools: https://modelcontextprotocol.io/specification/2026-07-28/server/tools
- [V07] MCP stdio transport: https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio
- [V08] MCP authorization: https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization
