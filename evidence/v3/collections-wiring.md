# Collections: engine service to app

Measured 2026-10-11 on Windows 11. Synthetic fixtures only, including invented catalogue
identities exercised in both synthetic and personal session classes. No personal inventory,
content, host resource observations, cloud calls or native file/process effects were used.

Working tree: `feat/collections-wiring`, based on `e9ed2b5511eee6737ff99643580be5ff61b5d235`.
The changes are uncommitted for the driver. All counts below come from local runs.

## Changed

- `crates/loomward-service/src/collections.rs` implements `collections.create`, `collections.list`,
  `collections.members` and `collections.update_members`. `handlers.rs` adds only four dispatch
  lines. The service's ingress gate admits these four wave-3 commands; other wave-3 commands retain
  their existing gate. Existing decoding, mutation FIFO, retained request IDs, owner-state mutex,
  FULL state transactions and bounded read/deadline machinery are reused.
- Collection writes require current, sealed catalogue node IDs with native identity, creation
  time and an active root. All nodes are validated inside the transaction before membership is
  changed. A missing update precondition is `invalid_request`; a stale one changes no state.
  Overlapping adds/removes are refused; repeated set membership is a no-op. Replaying an update's
  retained request ID does not undo a later removal.
- Members bind the existing `object_ref` table, honoring pending reference publication. An
  observed rename/rescan keeps membership; object replacement, disappearance and revoked roots
  leave explicit unresolved/revoked members with null entries. No path-based reattachment occurs.
  Paging is by durable reference ID, bound to collection/session and both revisions.
- The existing state-schema collection tables are reused. A v1-to-current migration test preserves
  the collection and member, retains the backup and reports the legacy reference unresolved;
  no new schema version or duplicate service-owned tables were necessary.
- The rail discovers `app/src/views/collections/View.svelte`: list, create, select, refresh and
  paged members with status/unknown sizes. Explorer gains a focused node inspector and
  `AddToCollection.svelte`. The action reads members' `state_rev` and sends it as the envelope
  precondition. Existing tokens, escaped names, row headers, persistent result regions and
  focus patterns are used. Mock transport implements all four commands, including retained
  outcomes, duplicate-name details, preconditions and state-bound cursors.
- `CollectionMemberPage` replaces `EntryPage` for this command: stable local member ID, explicit
  status, nullable entry. Collection size is nullable when a member/size is unavailable. Commands,
  Rust DTOs, schema, examples, normative semantics and generated TypeScript agree. Create examples
  now show an empty new collection; list/update examples preserve unknown aggregate size.

## Verified

| Command | Observed result |
|---|---|
| `cargo fmt --all --check` | PASS |
| `cargo test --workspace` | PASS: 719 tests, including 84 service unit tests (7 new collection regressions) |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, no warnings |
| `py -3 scripts/verify.py` | PASS: 223 Python tests, 1 existing symlink-privilege skip; 2 JS syntax checks, 9 JS boundary assertions, 80 parity fixtures; Rust gates also PASS |
| `cargo test -p loomward-protocol --test contract` | PASS: 11 conformance tests, including schema/DTO corruption checks on the updated examples |
| `py -3 scripts/gen_contracts_ts.py --check` | Up to date |
| `npm.cmd --prefix app run check` | 0 errors, 0 warnings |
| `npm.cmd --prefix app run test` | 310 passed, including 3 new collection unit tests |
| `npm.cmd --prefix app run build` | PASS |
| `py -3 scripts/test_app.py` | ALL PASS; collection creation, duplicate refusal, inspector add, focus, persistent result nodes, row headers and retained revoked member |
| `py -3 scripts/test_app.py --a11y` | ALL PASS; Collections included in reflow, names/row headers, focus-obscuring and target-size passes |
| `py -3 scripts/test_app.py --live-serve` | ALL PASS; real service/HTTP create, identical replay, duplicate-name `existing_id`, sealed synthetic node, preconditioned add, members and production UI rendering |
| `git diff --check` | PASS |

The no-effects regression compares the synthetic fixture file's bytes and modification time,
plus its root directory's modification time, before/after create, membership update and read.
Root grants remain unchanged. No new test uses a native identity open or real-root scan; native
identities in these new tests are explicitly synthetic catalogue fixtures. The existing live
personal-scan leg uses disposable synthetic files and retains its elevated-host refusal branch.

A bounded visual pass inspected both existing themes at desktop and mobile widths. The
Impeccable mechanical detector returned no findings. Screenshots and raw gate logs remain
local under the gitignored `.loomward/collections/` directory.

### Mutation checks (red, restore, green)

| Temporary reverted behavior | Observed failure |
|---|---|
| Refuse collection create/members at dispatch | All 7 new Rust tests fail; after restoration all 7 pass |
| Revert mock create to `capability_unavailable` | All 3 new Vitest tests fail; after restoration all 3 pass |
| Same reverted mock wiring through the browser | Collection-result assertion fails; restored browser flow passes |
| Refuse real service create at dispatch | `--live-serve` fails specifically at `live collections: create`; restored live leg passes |
| Inject a write to the disposable fixture from create | Bytes/mtime equality assertion fails; restored no-effects regression passes |

Every temporary source mutation was restored. The full applicable gates passed on the restored
working tree. Earlier failures found during implementation (DTO registry omission, encoding/test
selector mistakes and fixture setup mistakes) were fixed; none is an outstanding gate failure.

## NOT verified

Hosted CI and the elevated hosted-runner branch were not run. No real personal inventory was
used, and no Tauri/WebView2 or human screen-reader acceptance was performed. No large-scale
benchmark, release, package, deployment, commit or push was performed.

## Residual risk and declined scope

The member-page result and nullable aggregate are contract changes: consumers must use the
updated DTOs (the app and generated TypeScript do). Reference resolution describes catalogue
observations, never fresh permission or proof that a file is accessible now. Per the existing
update payload, removal names a resolvable current node ID; unresolved members remain visible
until a later reconciliation capability exists. The UI does not add an uncontracted removal or
reconciliation workflow.

Rename was declined because no collection rename command exists. No filesystem operation,
permission grant, content-read surface, learning surface or scan-pipeline change was added.

Recommended commit: `Wire durable collections through the service and app`.
