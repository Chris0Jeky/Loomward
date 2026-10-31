# Recorded folder-growth history (LW-121 / Mission62)

Base: `6141bf1efd27259f09b6a06804cda266e133a8f9`. The inspector is a bounded owner-only read over
existing scan records. It does not complete broader issue #412 or establish physical-device,
assistive-technology, daily-use or owner UX acceptance. HUMAN_TODO q-39 remains authoritative.

## Direct local evidence

Before implementation, the new Python contract test failed at the absent `alerts.history` command
(13 existing tests passed); the real-view control test failed at the absent View scan history button.
The completed contract suite passes 14 tests; the focused UI suite passes 109 tests. Svelte typechecking
reports zero errors and warnings. Generated TypeScript matches the canonical schema. Raw red and green
logs are retained in the driver mission outputs; the coordinator's exact-head delivery receipt records
hosted results and archive identity after qualification.

The first hosted head exposed three test-harness issues: an Alerts selector accidentally applied to
Backups, Rust fixture helper qualification/shared ownership, and an optional Python schema dependency
absent from the standard-library CI lane. The correction restores the Backups selector, qualifies the
clock helper and shares the concurrent service through `Arc`. Python checks the declarations without
an optional import; required full JSON Schema/DTO validation stays in the native protocol tests.
No skipped optional Python schema check counts as that validation. The independent review found the
Backups regression, reproduced by the hosted browser log; no other blocking finding was raised.

## Executable proof

- `crates/loomward-protocol/tests/contract.rs`: closed request/result DTOs, required nullable bytes,
  lossless values above 2^53 and a strict 64-record response bound; registry schema/DTO parity.
- `crates/loomward-service/src/alerts_tests.rs`: zero versus unknown, malformed/partial counters,
  unfinished-run exclusion, stable timestamp ties, 65-row truncation, unchanged window math,
  active/foreign/revoked/forgotten/client refusal, pending revoke/forget and deadline fences,
  concurrent snapshot invalidation, restart with rediscovered IDs, and unchanged settings,
  revisions, grants, fixture bytes and mtime.
- `app/tests/unit/view-races.test.ts` and `mock-views.test.ts`: actual control handler, duplicate
  activation, two-root switching, abort signals, explicit error/retry, draft preservation and
  refresh/save/session/client/connection/disconnect/unmount response fencing.
- `scripts/test_app_views.py`: actual mock inspector controls, exact/zero/unknown/partial values,
  empty second folder, saved-versus-draft threshold text, keyboard focus and responsive captures.
- `scripts/test_app.py --live-serve`: actual HTTP reads on disposable synthetic records, two-root
  switching, held-response refresh/save/unmount, explicit failure/retry, no eager history read,
  390/1360 captures, unchanged grants/settings/witness bytes and mtime. The existing browser harness
  retains console and loopback-request checks. Route holds/failures are transport fixtures, not claims
  about a physical device or provider failure.

Full hosted Python, app/type/build/browser, native and desktop gates remain required at the published
head; no local Rust build or substantial browser allocation was inherited. Workflow and gate scripts
are unchanged. Native tests above are authored proof until the hosted receipt confirms execution.

## Scope

No schema migration, engine evaluator change, scheduler, scan, provider, dependency, MCP/client/Ask
surface or operating-system notification. Two adjacent fixture/test updates are necessary: the DTO
registry must enumerate the new definitions, and the canonical alert report adds a second synthetic
folder for selection/empty-history proof. All previous proof seals and parked PR #1118 remain untouched.
