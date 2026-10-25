# loomward-mcp

Native, local stdio MCP host over the live Loomward catalogue. Only `find` and `describe` are
advertised and accepted. Both call the service as the launch-fixed `mcp:NAME` principal; the
owner must already have issued that client a root/field disclosure grant. A client cannot grant
itself access. Metadata returned to an MCP host may be forwarded to that host's model.

```text
loomward-mcp --client NAME [--state-dir DIR] [--dataset synthetic|personal]
             [--allow-personal] [--idle-release-secs N]
```

`NAME` matches `[a-z0-9-]{1,32}`. The default dataset is personal and its default state directory
is `%LOCALAPPDATA%\Loomward Data\v3\personal`, matching the desktop. Choosing this default is the
personal opt-in. Any explicit personal `--state-dir`, even the same path, requires
`--allow-personal`, as an override does in `loomward-serve`. Synthetic mode defaults to the
separate `v3\synthetic` sibling; tests always pass an explicit synthetic scratch directory.
There is no root argument, scan, provider, network listener or MCP host configuration change.
Usage errors are stderr-only with exit code 2; stdout is protocol only.

Tools take strict objects: `find {"text":"report"}` (at most 256 characters, 20 rows) and
`describe {"reference":"r1"}`. Unknown fields and every other tool name are refused. Results
retain the service's granted fields, coverage, freshness and unknown values; node and root IDs
and service cursors never cross this transport. References are in-memory only, bounded to the
latest 4,096 distinct nodes and bound to the client grant epoch and catalogue revision. After a
grant/revision change, idle release, or restart, call `find` again. Reference numbers are not
reused within the process, including after idle release.

The transport ports the frozen Python reference's newline-delimited JSON-RPC handling:
modern `2026-07-28` per-request metadata, or legacy `2025-11-25` initialize/initialized.
It supports `server/discover`, `ping`, `tools/list` and `tools/call`, with no resources.
Input/output limits are 65,536/262,144 bytes, nesting is at most 32, tool calls at most 120
per rolling minute, and oversized frames are drained only up to the reference's 1 MiB recovery
budget. Duplicate keys, nonfinite numbers and malformed IDs are rejected.

The first valid tool needing the service takes an exclusive OS lock on `state-dir/lock` and
opens it using `Service::open_client`. This runs the common store initialization/migrations
without the ordinary host's freshness watches, automatic scans, telemetry, backup scrubber or
synthetic copy recovery.
Without those writers the catalogue is a stored view: search freshness remains visibly stale.
Every read still passes through the service's grant check and projection. MCP creates no Ask
conversation or `ask.db`, and it never creates a store: with no `state.db` it answers "open the Loomward app once
first". Calls write nothing to `state.db`. Known limits: opening an existing store can run its migrations,
and a catalogue read can repair derived aggregates in `catalog.db` (shared reader behaviour, tracked separately).

After 10 seconds without tool calls (and at once after a refused call without a live grant) the service drops before its lock; `--idle-release-secs 0`
holds it until exit. Listing, discovery and partial input do not keep the state open. When another
host holds the lock, tools return `isError` with a plain busy message and can retry later.

The shared lock helper is `loomward_service::paths::lock_state_dir`, ported with the same file
name and OS semantics from `native/src/launch.rs`. **Driver follow-up:** switch the desktop to
the shared helper; this job deliberately leaves `native/` unchanged. The service now requires
Rust 1.89, matching the desktop, because this is when the standard library's file lock API landed.

Prove with `cargo test -p loomward-mcp --no-fail-fast` and
`cargo test -p loomward-service --no-fail-fast`, plus workspace fmt/clippy. The real subprocess
tests include both eras, native Windows registered lab roots and owner-issued grants, database
table dumps, busy/retry and idle reference expiry. Third-party MCP host/SDK certification and
personal-data acceptance are separate from this synthetic subprocess proof.
