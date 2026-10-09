# Expansion research sources

Accessed 9 October 2026. These sources support protocol/platform facts, not claims of completed Loomward functionality. Design choices, experiments and limitations are recorded separately. No source text or private repository code is copied into the implementation.

## V01: MCP versioning

https://modelcontextprotocol.io/specification/versioning

Current protocol revision and per-request version declarations.

## V02: MCP versioning and compatibility

https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning

Modern versus legacy lifecycle and dual-era compatibility.

## V03: MCP base protocol

https://modelcontextprotocol.io/specification/2026-07-28/basic

Request/result metadata and bounded protocol implementation decisions.

## V04: MCP server discovery

https://modelcontextprotocol.io/specification/2026-07-28/server/discover

Discovery method, supported versions and server metadata.

## V05: MCP tools

https://modelcontextprotocol.io/specification/2026-07-28/server/tools

Tool input/output schemas and untrusted tool annotations.

## V06: MCP resources

https://modelcontextprotocol.io/specification/2026-07-28/server/resources

Resource list/read representation; arbitrary filesystem URIs are not necessary.

## V07: MCP stdio transport

https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio

Newline-delimited UTF-8 JSON and stdout/stderr separation.

## V08: MCP authorization

https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization

Future HTTP authorization boundary; not implemented by this stdio build.

## V09: Legacy MCP lifecycle

https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle

Initialization and initialized notification for the supported legacy revision.

## V10: SQLite query planning

https://sqlite.org/queryplanner.html

Compound indexes and query plans; does not establish Loomward speed.

## V11: SQLite WAL

https://sqlite.org/wal.html

Local concurrency and storage constraints; production database choices.

## V12: Windows quality of service

https://learn.microsoft.com/en-us/windows/win32/procthread/quality-of-service

Owned-worker scheduling options; native integration not tested.

## V13: ReadDirectoryChangesW

https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-readdirectorychangesw

Directory observation limitations and reconciliation needs.

## V14: Windows change journal records

https://learn.microsoft.com/en-us/windows/win32/fileio/change-journal-records

Journal coverage, coalescing and non-undo semantics.

## V15: CloudEvents project

https://cloudevents.io/

Interoperable event envelope, not delivery or authorization semantics.

## V16: OpenTelemetry error recording

https://opentelemetry.io/docs/specs/semconv/general/recording-errors/

Diagnostic error representation; proposed redacted instrumentation only.

## V17: On Calibration of Modern Neural Networks

https://proceedings.mlr.press/v70/guo17a.html

Calibration is distinct from raw prediction score; no Loomward calibration result is claimed.

## V18: Windows Job Objects

https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects

Grouped process management and constraints for future owned workers.

## V19: restic working with repositories

https://restic.readthedocs.io/en/stable/045_working_with_repos.html

Read-oriented repository/snapshot inspection; no restic process was invoked.

## V20: Everything SDK

https://www.voidtools.com/support/everything/sdk/

Optional existing index adapter feasibility, not bundled functionality.

## V21: JSON-RPC 2.0 specification

https://www.jsonrpc.org/specification

Base message and notification semantics; MCP revision-specific rules remain authoritative.

## Connected repository context

Only the three named README files were reviewed through the GitHub connector. Their recorded blob hashes are in `evidence/v2/estate-review.json`. This is documentation-level context, not a live deployment inventory or proof of API compatibility. Taskdeck is described by its current README as proprietary; the proposed bridge must not copy its implementation into this MIT project.
