# Collections follow-ups — #327

2026-10-11, Windows. Tested base `3fa9be7816e7c69c452dc2f201a9b676931b02ff`
plus this uncommitted diff on `fix/collections-followups-327`. Synthetic fixtures only.
No commit, push, file/process capability, provider request or real-inventory benchmark.

Changed: `collections.rs:213` removes membership by optional `remove_members`, without
resolving a live object, under the existing revision precondition and transaction. Other
collections and object references survive removal. Contracts, examples, Rust DTOs, generated
TS and the mock agree; empty/omitted sets are accepted, null/duplicates/malformed IDs and
add/remove overlap are refused. `collections/View.svelte:74` offers Remove, refreshes after
stale revisions and restores keyboard focus after removal, including unavailable rows.

LOW fixes: `collections.rs:383` distinguishes revoked from other unavailable grants;
`explorer/View.svelte:338` uses the intended separator; the collection member schema and
Rust decoder require an entry exactly when resolved; `collections.rs:371` registers the
catalogue interrupt handle and checks the deadline between rows; `types.rs:582`, the schema
and mock reject all Unicode Cc controls, including DEL/C1 and a trailing newline.

## Protection mutations

Each row ran `cargo test -p loomward-service <test> -- --test-threads=1` with one guard
mutated, observed an assertion failure, then restored the guard. All controls and the final
full suite pass. Test definitions are in `crates/loomward-service/src/collections_tests.rs`.

| Guard mutation in collections.rs | Test | Mutant |
|---|---|---|
| Remove `g.state='active'` | `collections_protection_refuses_revoked_root` | FAIL |
| Remove `g.state='active'` | `collections_protection_refuses_identity_changed` | FAIL |
| Replace `absent_pending` in admission with a nonmatching state | `collections_protection_refuses_absent_subtree` | FAIL |
| Bypass the filesystem refusal | `collections_protection_refuses_unsupported_volume` | FAIL |
| Bypass the `label_view` refusal | `collections_protection_refuses_label_view` | FAIL |
| Change `count >= 256` to `count > 256` | `collections_protection_caps_creates_at_256` | FAIL |
| Bypass final `self.revisions() != revs` | `collections_protection_rejects_publication_race` | FAIL |

The race test changes each of catalog/state revisions independently after hydration and
before final validation. Refusal tests assert no references, memberships or revision writes,
then restore admission conditions and successfully add the same nodes.

## Follow-up regression mutations

| Mutation | Discriminating check | Mutant |
|---|---|---|
| Skip member-ID deletion | `collections_remove_unresolved_by_member_id` (vanished, replaced, revoked; another collection retained) | FAIL |
| Accept noncanonical member IDs | `collections_remove_members_validation_and_noop` | FAIL |
| Report every inactive grant as revoked | `collections_non_revoked_inactive_grant_is_unavailable` | FAIL |
| Bypass the between-row deadline | `collections_hydration_stops_at_page_deadline_and_releases_slot` | FAIL |
| Leave interruption on the idle state connection | `collections_hydration_interrupt_reaches_running_inspection` | FAIL |
| Revert Rust names to C0-only validation | `collections_names_reject_del_and_c1_controls` | FAIL |
| Revert mock names to C0-only validation | Vitest `rejects Unicode control` (DEL, U+0085, U+009F) | FAIL |
| Skip mock member-ID deletion | Vitest `retains revoked members` | FAIL |
| Revert schema names to C0-only validation | Python `test_names_reject_all_unicode_controls` | FAIL |
| Remove the schema status/entry condition | Python `test_resolved_requires_entry_and_unavailable_requires_null` | FAIL |
| Remove the schema remove_members field | Python `test_remove_members_is_optional_unique_bounded_and_nonnullable` | FAIL |
| Disable the Remove handler | Chromium `run_collections` | FAIL |
| Restore the literal question-mark separator | Chromium `run_collections` | FAIL |

20 mutation checks total; all failures were behavioral assertions, not compilation failures.
The active-inspection test uses 20,000 invented database rows and a successful uncancelled
control. This is cancellation proof, not a throughput benchmark. The restored Chromium flow
passes, including keyboard removal/focus.
The live HTTP flow additionally removes a vanished member through the production app.

## Final gate outputs

| Command | Output |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo test --workspace` (executed by verify.py) | 759 passed, 0 failed, 0 ignored, including 20 collections service tests |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | exit 0 |
| `py -3 scripts/verify.py` | exit 0; 227 Python tests, 1 privilege skip; 2 JS syntax gates, 9 boundary assertions, 80 parity fixtures; Rust gates PASS |
| `npm.cmd --prefix app run check` | exit 0, 0 errors/warnings |
| `npm.cmd --prefix app run test` | 31 files, 323 tests passed |
| `npm.cmd --prefix app run build` | exit 0 |
| `py -3 scripts/test_app.py` | ALL PASS, no console/page errors |
| `py -3 scripts/test_app.py --a11y` | ALL PASS (a11y), no console/page errors |
| `py -3 scripts/test_app.py --live-serve` | ALL PASS (live serve), loopback only, no console/page errors |
| `py -3 scripts/gen_contracts_ts.py --check` | generated TS up to date |
| `git diff --check` | exit 0 |

Initial regressions reproduced the unavailable-member removal, inactive-state and Unicode
name defects in Rust and the mock. Full verification also found schema/DTO parity and
closed-object integration failures; both were fixed before the final successful gates.

NOT verified: hosted CI, Tauri/native-window interaction, real-inventory scale or timing;
the Python symlink-privilege test remains skipped. No large benchmark was run.

Declined: the issue's pending-publication-window investigation and collection-list scalability
measurement require shared writer/performance work beyond this lane; both remain tracked in #327.

Suggested commits: `Allow unresolved collection members to be removed`; then
`Pin collection guards and bound member hydration` (include the validation/status follow-ups).
