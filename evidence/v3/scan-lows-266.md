# Scan and Windows enumeration LOWs - #266, #285 item 1, #219

11 October 2026. Local Windows native tests, using disposable synthetic fixtures only.
Starting branch: `fix/scan-lows-266`; HEAD and base: `0377b0f5bb9de3a515e6d2e712dc9e5104332411`.
Changes are uncommitted for the driver. No push, elevation, personal scan or benchmark.
Read all three issue bodies and comments; none had comments.

## Changed

Paths below are relative to the repository root.

| Item | Change and location |
|---|---|
| #266(a) | Release the pending identity on both reservation rejection and the defensive arena-cap rejection: `crates/loomward-engine/src/scan/pipeline.rs:603`, `:733`. The direct regression at `:90` exercises adoption at the cap and two successive parent listings. |
| #266(b) | Increment failed counts for unavailable reopen identity and a refused/mismatched reopen, preserving earlier failures: `crates/loomward-engine/src/scan/pipeline.rs:1175`, `:1191`. Extend `crates/loomward-engine/src/scan/followups.rs:855` with failed-listing-plus-reopen-error and failed-listing-plus-identity-mismatch cases. |
| #266(c) | Targeted duplicate evidence cannot settle coverage: `crates/loomward-engine/src/scan/watch.rs:491`. Regression at `:839` starts with complete coverage, requests a two-directory targeted pass, then introduces duplicate identity without reparenting. It checks partial coverage and no sweep. |
| #266(d) | Reopen documents "same object, same volume" and the separate listing containment proof: `crates/loomward-windows/src/enumerate/mod.rs:166`. |
| #285 item 1 | Add an uppercase root alias to `crates/loomward-windows/src/enumerate/native.rs:841`. A full engine scan at `crates/loomward-engine/src/scan/followups.rs:7` verifies complete coverage, zero failures and exact synthetic totals using Extended, Both and Find. Both alias oracles passed before any production change; no enumeration repair was needed. |
| #219 guards | Direct forced-elevation tests at `crates/loomward-windows/src/win.rs:6`, `crates/loomward-windows/src/linkcount.rs:131`, `crates/loomward-windows/src/fixtures.rs:8`, `:19`. They distinguish the named guard from downstream refusal. A test-only thread-local RAII helper at `crates/loomward-windows/src/enumerate/native.rs:545` restores the previous forced-token state, also replacing the existing inline helper. |
| #219 documentation | `SourceError::IdentityUnverified` now covers listed-ID binding, identity-unverified Find and unsuitable reopen identities: `crates/loomward-windows/src/enumerate/mod.rs:134`. |

Native alias tests assert elevation refusal and return on an elevated host. Forced-elevation
tests assert refusal under the existing thread-local seam on either host posture. Production
elevation guards and token-query behavior are unchanged.

## Regression and mutation proof

The pending-marker and reopen-count oracles failed on the original implementation. The initial
targeted fixture was not discriminating: reparenting independently forced a full repair. Changing
the fixture to preserve the original parent isolated the targeted duplicate bug; it then failed
with the original `settled` expression. All three passed after the fixes.

All ten mutation executions failed at runtime, exit 101; no compilation failure is counted.
Every mutated file was restored byte-for-byte in a `finally` block.

| Mutation | Failing oracle |
|---|---|
| Remove arena-cap marker cleanup | `directory_limit_releases_pending_identity_before_parent_relist`: pending marker remains |
| Remove reservation marker cleanup | Same oracle: relist reports one false duplicate |
| Restore reopen `max(1)` counters | `reopen_failure_or_changed_identity_keeps_the_run_partial`: failed is 1 instead of 2 |
| Restore targeted `settled = !full \|\| next.complete` | `targeted_watch_duplicate_identity_never_completes`: incorrectly complete |
| Remove `win::open` guard | `metadata_open_refuses_forced_elevation` |
| Remove `linkcount::one` guard | `linkcount_one_refuses_forced_elevation_before_metadata` |
| Remove `fixtures::check_streams` guard | `check_streams_refuses_forced_elevation` |
| Remove `fixtures::scoped` guard | `scoped_refuses_forced_elevation_before_path_validation` |
| Simulate a noncanonical uppercase root containment operand | `native_scan_completes_with_different_case_root_alias`: partial scan |
| Same containment mutation | `native_containment_normalizes_verbatim_and_short_root_aliases`: child listing is `ContainmentUnproven` |

The alias mutations simulate inconsistent native normalization; they do not prove any particular
filesystem has that behavior. The documentation edits are checked by diff review, not source-text
unit tests. The private arena oracle's writer receive is bounded to two seconds.

Raw logs, JSON results and the mutation runner remain ignored under `target/scan-lows266/`.

## Gate results

| Command | Result |
|---|---|
| `cargo fmt --all --check` | PASS, exit 0 |
| `cargo test --workspace` | PASS: 649 passed, 0 failed, 1 pre-existing ignored (#246), exit 0 |
| `cargo test -p loomward-windows --all-features` | PASS on restored retry: 90 passed, 0 failed, 0 ignored, exit 0; initial failure detailed below |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, exit 0 |
| `git diff --check` | PASS, exit 0 |

The first all-features run had 89 passes and one failure in the unchanged
`native_stop_keeps_earlier_verified_group_and_later_input_unknown` test: no first group was retained.
That test allows only 100 ms for the entire inspection. Restoring all changed Windows files to
HEAD gave a passing baseline run (86 tests); restoring this lane gave 90 passing tests.
A temporary 110 ms pause before the first read reproduced the same empty-group assertion with
`stop: Some(TimeBudget)`. This supports deadline sensitivity; the original failure did not print
its stop reason, so its exact timing cause is not claimed proven. The diagnostic change was
restored byte-for-byte, and its unmodified test was rerun. No test was weakened or newly ignored.

## Declined / NOT verified / residual risk

- No FAT/exFAT identity-unverified mode: product decision, explicitly excluded.
- No #285 item 2 VHD qualification: out of scope and requires elevation.
- No token cache: no measurement establishes a need; no performance claim.
- Other #219 metadata-scope and junction hypotheses stay with that issue, outside this requested test/documentation slice.
- No hosted CI or actual elevated-host execution, physical ReFS/FAT/exFAT qualification,
  case-sensitive NTFS fixture, personal inventory or long benchmark.
- Alias proof covers the available synthetic fixture filesystem, not every Windows provider.
  The unchanged 100 ms duplicate-reader test remains sensitive to delays; no stable timing bound is claimed.

Recommended commits, without attribution trailers:

- `fix(scan): preserve partial coverage and count relist failures`
- `test(windows): cover root case aliases and elevation guards`
