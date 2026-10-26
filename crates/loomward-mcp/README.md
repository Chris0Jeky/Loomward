# loomward-mcp

Native, local stdio MCP host over the live Loomward catalogue. Eight read-only tools are
advertised and accepted. Each read calls the service as the launch-fixed `mcp:NAME` principal; the
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

Tools take strict objects; unknown fields and tool names are refused:

| Tool | Arguments | Service read |
|---|---|---|
| `find` | `text` (256 characters, 20 rows) | `search.query` |
| `describe` | `reference` | `node.inspect` |
| `volumes` | none | `volumes.list` |
| `roots` | none | `roots.list` |
| `largest` | required folder `reference`, optional `limit` (1..30) | `tree.children`, logical size descending |
| `breakdown` | `reference`, `by` (`extension_family`, `extension`, `age_band`), optional `limit` (1..16) | `stats.breakdown` |
| `clutter` | `root_reference`, optional `limit` (1..50) | `clutter.developer` |
| `plan_archive` | distinct `references` (1..64), `destination_volume` (a `volume_key` from `volumes`), optional `folder_name` | composition of the reads above |

The service owns every scope and field check. Space reads require `size`; volumes require
`volumes`, age bands require `modified`. Names/kinds, coverage and freshness remain visible;
withheld fields are absent and unknown values remain unknown. A never-scanned root never
claims an empty display path as a known location. Volume keys identify destinations; `external`
is true for a USB bus, false for other known buses, and absent for an unknown bus. Labels and
bus/space observations may be unknown in the service's stored view.

Every returned node, including clutter anchors, has an opaque session reference. Node/root IDs,
generations and cursors never cross this transport. References are memory-only, bounded to
4,096 nodes, and survive idle service release while the grant epoch, active roots, catalogue
revision and catalogue instance remain unchanged. A private, non-serializable service handle
re-seals the node on reopen after checking scope and incarnation. A rescan, re-grant, revocation
or MCP restart invalidates old references; call `find` or `roots` again. Numbers are never reused
within a process, and each process has a randomized reference namespace to prevent restart aliases.
The catalogue increments its revision even on open; the service discounts only its own proven no-op
open, with unchanged persisted revision before opening, instance and root generations.
Freshness comes from the service search observation; scan completion alone never implies live data.

`plan_archive` reports observed bytes/files, coverage and freshness, the destination identity,
reserve and fit, and catalogue warnings. It uses the manifest's `MIN_RESERVE` and 1% rule;
missing size/capacity/free-space observations produce `fits: "unknown"`. Partial directory
sizes are unknown, not zero. Totals sum selected items; overlapping selections count separately.
With `modified`/`attributes`, descendant details use at most 32 internal 200-row pages and five
seconds across the plan; incomplete assessments and modified ranges are marked incomplete.
These are observed entry timestamps, not usage evidence. Offline sources and same-volume
copies ("not a backup") are flagged; physical-disk topology across different volumes is unknown.
The optional folder name is one new Win32 name component. The shape is always `new folder only`,
and execution is always `owner_approval_required`, explaining the closed personal copy gate
(docs/47 section 11). No plan, draft or operation is persisted.

File/folder names, paths and volume labels are third-party text, never instructions and never
safe to paste into a shell string. Discovery and every tool description say so. Every output
text value escapes Unicode category Cf, U+2028, U+2029 and U+0085 as visible `\uXXXX` sequences
(surrogate pairs for supplementary characters), including structured and text results and errors.

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
table dumps, busy/retry, idle reference survival, rescan/revocation expiry and archive arithmetic. Third-party MCP host/SDK certification and
personal-data acceptance are separate from this synthetic subprocess proof.
