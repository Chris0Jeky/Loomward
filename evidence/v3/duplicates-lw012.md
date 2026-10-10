# LW-012: native duplicate inspection

2026-10-10. Windows, `x86_64-pc-windows-msvc`. Issue [#18](https://github.com/Chris0Jeky/Loomward/issues/18).
Base `cf27a4ec6734026d1bb3a9d0b393c7e294e4eb60`, branch `feat/duplicate-reader-lw012`;
the tested changes are uncommitted. All file contents and names in these fixtures are synthetic.
There was no personal-content inspection, whole-disk scan or benchmark. Fixture tests contacted no
external service and performed no file effects outside their disposable scope.

## Implemented boundary and placement

`GrantedRoot::inspect_duplicates` derives eligibility from the grant's dataset class. Personal grants
return `content_read_gated` before a native open. Synthetic grants (including lab grants, which use that
class) call the low-level Windows reader with the grant's expected root identity and a trusted listing.
The low-level library entry point is for trusted native callers; it does not resolve paths or grants
from protocol input. No service, protocol, contract or app wiring is part of this slice.

The platform-neutral grouping core is in `loomward-windows::duplicate_reader`. This crate already has
portable helpers and Linux unit-test seams, so this needs no new crate or dependency on the catalogue
or protocol. It accepts a bounded listing of relative names with native metadata observations;
catalogue adaptation is deferred. SHA-256 reuses the existing workspace `sha2` dependency.

The stages are nonempty size groups, SHA-256 of up to 64 KiB at each end (without rereading an overlap),
full SHA-256, then exact default-stream comparison. Even a forced hash collision is partitioned by
byte equality. A global byte budget counts every read and both comparison sides. A shared deadline
and a 200,000-entry cap cover all stages. A budget stop reports `partial` plus its stable stop reason;
unexamined input is unknown. Skips are visible and also make coverage partial.

Native opens reuse the component-wise, handle-relative enumeration helpers and their no-recall,
open-reparse and non-elevated safeguards. Ancestor handles deny delete sharing while held, so an
ancestor cannot be renamed during a stage. Metadata is checked before requesting a content handle.
Handle-derived identity, ancestors, size, write/change timestamps, link count and attributes are
checked before and after each pass; the final name is reopened through pinned ancestors. Returned
groups get a final metadata revalidation. Known hard-linked objects are reported as links and excluded
from content inspection, matching the conservative Python reference. Legacy 64-bit identities are
reported unsupported rather than used as uniqueness evidence.

The output consists of evidence groups, links, skips, partial coverage and bytes read. It supplies no
effect operation, deletion approval or guaranteed reclaimable-byte estimate. Equality covers only
the default stream, not ACLs, alternate streams or application roles.

## Fixture checks

New tests: 3 portable grouping tests, 12 Windows reader tests (including the real named-pipe handle
refusal), and 2 engine grant tests. The existing injected-elevation test additionally exercises the
new reader. Ordinary fixture runs exercised the native cases; elevated refusal was separately injected
through the existing native guard seam, without elevating the process.

| Case | Test and result |
| --- | --- |
| Equal files; equal size but different bytes | `native_equal_and_same_size_different_bytes`: one two-file group; 28 bytes across the stages |
| Equal head/tail, different middle | `native_equal_ends_different_middle_need_full_hash`: no group, complete coverage, 640 KiB read |
| Same-size modification during content read | `native_modified_during_read_is_rejected`: `changed_during_read`, no group |
| Already compared anchor subsequently changes | `native_final_validation_drops_an_earlier_equal_pair`: earlier group removed |
| Hard links | `native_hardlinks_are_links_not_duplicates`: one link object, no content read |
| Junction swap between partial and full stages | `native_junction_swap_between_stages_is_rejected`: ordinary parent replaced with a real junction to equal-content fixtures; `changed_during_read`, no group |
| File replacement; path traversal | `native_replacement_and_traversal_are_rejected`: stable changed/invalid-path reasons |
| Special files | `native_refuses_special_and_placeholder_classes`: real directory and injected device attribute; `named_pipe_handle_is_a_visible_special_file_refusal`: real pipe handle refused by the same file-type check used on reader handles |
| Placeholder/offline/reparse | `native_refuses_special_and_placeholder_classes`: listed attribute seam; visible `placeholder`, `offline`, `reparse_point`; zero content bytes |
| Sharing violation | `native_sharing_violation_visible`: a real exclusive handle; visible `sharing_violation` |
| Budgets | `native_budget_exhaustion_is_partial`: limits at partial hash, full hash and comparison, plus an expired deadline; `native_deadline_during_read_is_partial`: deadline crosses after a real read, with bytes counted |
| Hash collision | `core_hash_collision_requires_exact_partitioning`: forced equal digests still split unequal bytes |
| Volume identity and empty files | `core_identity_is_volume_scoped_and_empty_files_claim_nothing`: volume-qualified IDs; zero-byte files excluded |
| Listing cap, time and byte accounting | `core_entry_and_time_budgets_are_partial`: explicit stop reason, no false absence |
| Personal refusal and synthetic positive control | `duplicate_content_is_gated_for_personal_grants`, `duplicate_synthetic_grant_returns_evidence` |
| Elevation | `native_opens_refuse_elevation_consistently`: existing thread-local elevation seam, new reader refuses |

## Red/green and mutation proof

The initial eight native fixtures all failed assertions against the reader stub (not compilation or
setup errors), then passed with the implementation. Every new test was subsequently demonstrated
to fail by reverting the relevant implementation and restoring it byte-for-byte. Mutants were never
committed. Commands were run sequentially in the lane's worktree.

| Temporary implementation revert | Command filter | Observed assertion failures |
| --- | --- | --- |
| Return the empty reader report instead of grouping | `cargo test -p loomward-windows duplicate_reader`; `cargo test -p loomward-engine duplicate_synthetic_grant_returns_evidence` | All 14 grouping/staged-reader tests and the synthetic-grant control fail; the separately tested pipe guard remains independent |
| Remove personal dataset gate | `cargo test -p loomward-engine duplicate_content_is_gated_for_personal_grants` | Personal refusal test fails |
| Remove reader's elevation guard | `cargo test -p loomward-windows native_opens_refuse_elevation_consistently` | Injected-elevation assertion fails |
| Ignore write/change timestamps | `cargo test -p loomward-windows native_modified_during_read_is_rejected` | Same-size modification test fails |
| Accept matching digests without exact comparison | `cargo test -p loomward-windows core_hash_collision_requires_exact_partitioning` | Collision partitioning test fails |
| Remove final group revalidation | `cargo test -p loomward-windows native_final_validation_drops_an_earlier_equal_pair` | Changed-anchor test fails |
| Remove native file-type check | `cargo test -p loomward-windows named_pipe_handle_is_a_visible_special_file_refusal` | Real pipe's stable special-file refusal fails |

All seven mutants were restored. Final gate results below apply to the restored source.

## Existing workspace-test race investigated

The first workspace run passed 443 tests before the pipe-handle test was added. A subsequent
default workspace run failed in the unchanged
`scan::watch::native_tests::concurrent_scan_start_reserves_root_before_touching_watch`:
`cancelled.len()` was 0 instead of 1 (`scan/watch.rs:1120`). Duplicate tests were green separately.

The existing `scan_cancel_root` stops its watcher **before** enumerating active jobs. Watch shutdown
can let the runner become terminal; `root_jobs` then omits it. To distinguish this lane from that
existing race, all changed tracked build/source files were temporarily replaced with the exact base
versions. `cargo test -p loomward-engine concurrent_scan_start_reserves_root_before_touching_watch -- --nocapture`
passed on the unmodified base. Adding only a temporary 100 ms scheduling pause after `watch.stop()`
on that base reproduced the same 0-versus-1 assertion. This establishes the existing scheduling race,
without claiming the uninstrumented base run failed. All lane files were restored byte-for-byte.
The watch/cancellation repair is deferred to its owner; no assertion or production logic there was
weakened or changed in LW-012. The failed run is retained alongside the final gate receipt.

## Final proving commands

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | PASS, exit 0 |
| `cargo test --workspace` | PASS, exit 0: 444 passed, 0 failed, 0 ignored; final default run, with the earlier failure retained above |
| `cargo test -p loomward-windows --all-features` | PASS, exit 0: 55 passed, 0 failed, 0 ignored |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, exit 0 |
| `git diff --check` | PASS, exit 0 |
| `cargo clippy -p loomward-windows --all-targets --all-features --target x86_64-linux-android -- -D warnings` | PASS: type-check/lint of the non-Windows build and portable test code using an already installed target; not test execution |

## Changed surface and suggested commit

| Item | Source location |
| --- | --- |
| Listing input, shared budget and visible evidence report | `crates/loomward-windows/src/duplicate_reader.rs:11`, `:16`, `:69` |
| Portable staged grouping, volume-scoped links, exact collision partitioning | `crates/loomward-windows/src/duplicate_reader.rs:216` |
| Pinned relative opens and handle revalidation | `crates/loomward-windows/src/duplicate_reader/native.rs:57`, `:90`, `:180` |
| Bounded async reads, partial/full hashes and byte comparison | `crates/loomward-windows/src/duplicate_reader/native.rs:193`, `:259`, `:296` |
| Grant dataset gate | `crates/loomward-engine/src/scan/mod.rs:116` |
| Reused handle identity query and crate-internal native open helpers | `crates/loomward-windows/src/identity.rs:125`; `crates/loomward-windows/src/enumerate/native.rs:157`, `:225`; re-exports in `enumerate/mod.rs` |
| Portable/native fixtures and grant positive/negative controls | `crates/loomward-windows/src/duplicate_reader/tests.rs:45`; `duplicate_reader/native.rs:338`; `crates/loomward-engine/src/scan/tests.rs:6`, `:23`; injected-elevation assertion in `enumerate/native.rs:830` |
| Module export and existing SHA-256 dependency reuse | `crates/loomward-windows/src/lib.rs`, `crates/loomward-windows/Cargo.toml`, `Cargo.lock` (one existing dependency edge) |

Suggested commit: `Add bounded native duplicate inspection for synthetic roots`.

Declined: personal reads require a later owner decision; service/app/catalogue wiring and unused-file
heuristics are later slices; file effects are outside this capability. The existing cancellation race
needs its own scan/watch follow-up. No new dependency version, effect endpoint or optional feature was
introduced.

## NOT verified and limits

- Linux test execution, actual cloud hydration behavior, a real cloud placeholder, and pending-I/O
  cancellation on a stalled filesystem driver were not exercised. Placeholder/offline classes used
  the listed-attribute refusal seam; real disk, sharing, junction and pipe cases used native handles.
- Deadline checks bound work admission and content waits. Metadata opens/queries and cancellation
  draining depend on the filesystem driver returning; this is not a hard real-time guarantee.
- Equality is point-in-time evidence under concurrent writers, not an atomic multi-file snapshot or
  security against same-user malware that can deliberately conceal changes.
- Python reference code is unchanged; its gate was not rerun. UI, service integration, unused-file
  heuristics, personal reads and any file effects are deferred from this lane's scope.
- No commit, push, PR or remote publication was performed. No independent review is claimed.
