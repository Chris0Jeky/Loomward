# Find & Ask design follow-ups (#433)

2026-10-18, Windows, lane `find-433`. Left uncommitted for the driver.
Starting HEAD: `4babf6710c6425086520a8697d495dd15dff2d12`.
Compared with fetched `origin/main`, including `94515f7` (#614); no main-side change to docs/46.
Read #433's body and comments with `gh issue view 433 --comments --repo Chris0Jeky/Loomward`
(`--json number,title,body,comments,state` supplied to capture the output; no comments).

This slice changes the proposed design only. No service, Ask loop, app, schema, authority or gate is
changed. No new MCP tools or write capability. Behavioural red/green tests are not applicable to this
prose correction; the existing roadmap/dependency checks below still pass.

## Item reconciliation

| Item | Evidence on current main and disposition |
|---|---|
| M2: MCP handles | Native MCP remains a future F12 lane. Section 8.2 now specifies session-local in-memory references, bound to client/epoch/generation, no conversation ledger, and F12 tests for no ask.db creation or state/Ask writes. Not a native MCP implementation claim. |
| M3: MCP projection | Runtime already narrow in `524a94d` (#541): `clients.rs::client_handle` admits only SearchQuery and NodeInspect; other commands return PermissionDenied. Corrected the design's wider MCP catalogue to find/describe only. Rootless telemetry/apps and global aggregates require a separate grant design. Python doc 28's frozen snapshot surface is unchanged. |
| L1: drop counts | Corrected sections 5.3 and 12: internal-only counts, no totals, diagnostics, error, cursor or model disclosure. Also made the helper's ACL count internal-only. Everything/helper adapters remain design-only. |
| L2: calls and rounds | Runtime already accounts invalid/unknown attempts before dispatch in `1daa6d8` (#556). `turn_tests::budgets_end_the_turn_with_a_round_that_offers_no_tools` already covers a 10-call batch (only eight reads) and the ninth no-tools round. Clarified those rules and that a repair retry consumes existing budgets. Dedicated one-repair semantics are not yet built. |
| L3: context | Removed the false 6,000-token fit claim. The existing runtime cap is 24,000 successful-result bytes, not tokens; errors/framing still need context accounting. Designed separate full-request context accounting with answer reserve and explicit refusal/partial handling. Runtime tokenizer/context qualification is still open under #433; the current 256 KiB request limit cannot establish model-context fit. |
| L4: Everything/cloud | Section 8.3 now agrees with 5.3: even verified Everything-sourced hits never reach cloud. Local/MCP verified hits still need the relevant adapter and disclosure gates. |
| L5: semantic budget | Corrected the hypothetical mapping from 12 B to 36 B (32 B durable reference + 4 B vector index): 344 B/entry at d=256, 488 B at d=384; maximum shares before overhead 13.95% and 9.83%. Directories have priority within the cap, not an exemption. Final format/overhead remain unmeasured. |
| L6: speed wording | Removed the seconds-level full-index claim. Candidate names may accelerate; sizes and file times wait for the ordinary scan and stay unknown meanwhile. Helper speed remains unmeasured. |
| L7: first-slice dependencies | Already fixed by `42b0dbd`: F5 explicitly depends on LW-067 and LW-069; the first-slice paragraph requires F3a to qualify LW-069. Verified in current main; no duplicate edit. |
| L8: Windows API facts | Re-fetched Microsoft F12/F27: page dates are 2021-10-21 and 2023-03-13. The pages do not guarantee meaningful enum TimeStamp/Reason or inactive-journal enumeration. Design now treats enum values as unavailable and requires a healthy journal preflight or ordinary-scan fallback, never journal creation. F28/F29 establish V2/V3 buffers and 128-bit V3 IDs. F30-F32 support the proposed OpenFileById(READ_CONTROL), GetSecurityInfo, AccessCheck path; READ_CONTROL alone is not list permission. Actual enum, impersonation/ACL and malformed-record canaries remain F10b gate work. |

The API sources and direct Microsoft links are F12, F27-F32 in [docs/46](../../docs/46-find-and-ask.md).
Source review is not an elevated helper test; no elevation, volume scan, personal metadata or model call
was performed.

## Verification

Run from `C:/lw/wt/find-433`, with `CARGO_BUILD_JOBS=4` and
`CARGO_TARGET_DIR=C:/lw/target/lane-find-433`:

- `py -3 -m unittest discover -s C:/lw/wt/find-433/tests -p test_find_ask_roadmap.py -v`: 2 passed.
- `py -3 -m unittest discover -s C:/lw/wt/find-433/tests -p test_search_dependency_plan.py -v`: 4 passed.
- `cargo fmt --all --manifest-path C:/lw/wt/find-433/Cargo.toml --check`: passed.
- `git -C C:/lw/wt/find-433 diff --check`: passed.

No Rust crate or app source changed: crate tests, clippy and npm checks were not run. No whole-workspace
test run, as required by the lane brief. The driver owns that integration gate and commits/publication.

## Remaining scope and human actions

Keep #433 open for the explicit runtime context/repair gap and L8's unmeasured API cases, or track those
before closing the design follow-up. This document does not implement future F4/F9/F10/F12 capabilities.
[HUMAN_TODO.md](../../HUMAN_TODO.md) q-47 still holds the local-preview and conversation-retention choices;
section 11 Q1/Q2 still gate the helper launch model and any owner-only outside-grant Everything view.
Those owner choices are unchanged and have not been inferred. No protected status/checkpoint files edited.
