# LW-137: storage-path and health observations

Windows, 2026-10-27. Base: `8ea068102be5357f78bbcf7952db3d9778922cad`.
Peer implementation for #876, left uncommitted for the driver. No new command,
existing DTO field, generated TypeScript, dependency, policy or safety gate changes.

## Design and native coverage

`loomward_windows::storage_path` exports `StoragePath`, `StorageObservation`,
`HealthObservation`, `ReliabilityCounters` and `StorageQueryProvider`.
`WindowsStorageQueryProvider` resolves one caller-selected root to its volume GUID
and opens only that volume with access 0, after the existing non-elevation guard.
It does not enumerate disks, read file contents or change device policy.

The identity tuple contains all fields from docs/51 section 4.10. Mount paths are
separate presentation data. An observed bridge, firmware or removal-policy change
changes the tuple. Unknown fields cannot prove an unchanged physical path, so
equal incomplete tuples must not be used to retain physical-path qualification.
This is observation data and never authorises an operation.

| Native field | Actual provider behavior |
| --- | --- |
| Volume GUID, presentation mount paths | `GetVolumePathNameW`, `GetVolumeNameForVolumeMountPointW`, `GetVolumePathNamesForVolumeNameW` |
| Bus type | Existing `IOCTL_STORAGE_QUERY_PROPERTY` helper, `StorageDeviceProperty`; unknown bus or query failure stays unknown |
| Logical and physical sector sizes | `StorageAccessAlignmentProperty`, validated by the same helper as the existing non-cached reader; malformed/failed reads stay unknown |
| Write-cache enabled | `StorageDeviceWriteCacheProperty`: enabled, disabled or unknown; failed reads stay unknown |
| Disk model, firmware | Unknown: existing descriptor reads only consume the fixed prefix, not variable strings |
| USB VID/PID, bridge, BOT/UAS | Unknown: USB/PnP discovery is not implemented |
| Removal policy | Unknown: no existing PnP removal-policy query; `RemovableMedia` is deliberately not substituted for policy |
| Windows build | Unknown: not queried by this minimal IOCTL provider |
| Health | `unsupported_or_hidden`: no WMI/COM reliability-counter provider exists yet |

The write-cache mapping follows Microsoft's
[STORAGE_WRITE_CACHE_PROPERTY contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddstor/ns-ntddstor-_storage_write_cache_property).
Serial bytes may transiently occur in the OS descriptor response buffer. They are
never extracted, retained, serialized or hashed by this implementation. The volume
GUID already supplies volume-level identity; collecting a device serial adds no
needed capability to this slice. No real device strings or inventories are fixtures.

Health counters preserve absent properties as `None`, including individual read
and write error totals/uncorrected counts. An absent query or an all-absent counter
record becomes `unsupported_or_hidden`. A measured zero remains measured. Counter
presence is not a health verdict: there is no `healthy` state. Hidden USB SMART
does not become a zero-error observation.

## Acceptance and red/green evidence

Tests use an in-memory synthetic provider and invented identifiers only:

- `reliability_counters_present_preserve_values_and_unknown_properties`: populated
  reliability fixture, zero preserved and missing properties serialized as null.
- `empty_reliability_result_is_unsupported_or_hidden_never_healthy`: absent query
  and all-absent record both normalize to the explicitly serialized unknown state.
- `bridge_hiding_smart_is_unknown`: USB bridge fixture preserves the hidden reason.
- `letter_change_keeps_identity_but_enclosure_firmware_and_policy_change_it`:
  presentation remount keeps the tuple; each named path change invalidates equality.
- `sector_geometry_and_observed_policy_cache_are_preserved_including_failed_reads`:
  512e/4Kn records, observed policy/cache values and explicit unknown read results.
- `native_alignment_read_accepts_512e_and_4kn_and_rejects_invalid_geometry`:
  exercises the shared validator, including zero, non-power-of-two, reversed and
  oversized sector geometry.
- `real_provider_refuses_elevation_before_querying`: forced elevation fails closed.
- `real_provider_reads_selected_temporary_volume_without_inventing_missing_data`:
  actual non-elevated Windows volume-query smoke test on a disposable temporary
  root; asserts GUID form, unavailable fields and any reported sector geometry.
  No native inventory or device strings are printed or retained.

Red command: `cargo test -p loomward-windows storage_path::tests --lib`.
The five initial acceptance tests compiled and failed at the unimplemented health
normalizer (0 passed, 5 failed); this was a behavior stub, not an import failure.
After implementation, the same command passed all seven tests then present. The
additional real-volume smoke test is included in the full crate run below.

Commands used `CARGO_TARGET_DIR=E:/loomward-build/target-876` and
`RUST_TEST_THREADS=3`, from the assigned worktree, on Windows:

```text
cargo fmt --all --check
exit 0

cargo test -p loomward-windows
test result: ok. 175 passed; 0 failed; 1 ignored
test result: ok. 8 passed; 0 failed; 0 ignored
test result: ok. 0 passed; 0 failed; 0 ignored (doc tests)

cargo clippy --workspace --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 43.34s
exit 0
```

The crate-only test build emitted the existing `watch.rs` dead-code warning for
`Watcher.release`; the workspace clippy command above passed without suppression.
The existing ignored test was not enabled or altered.

The all-features Windows test binary built by the workspace command also passed
the focused filters `storage_path::` (8 passed) and `sector` (6 passed, including
the existing copy-effects and non-cached reader tests). This is focused coverage,
not a claim that its complete 339-test harness ran.

## Workspace gate: red, investigation bounded

`cargo test --workspace --all-features` compiled successfully, then reported nine
failures in the service harness. It was stopped after the bounded investigation,
so remaining harnesses and doc tests are **unverified**, not passed. The failing
test names observed before stopping were:

```text
tests::copy_tests::lab_copy::a_lost_acknowledgement_at_committed_reports_committed
tests::copy_tests::lab_copy::a_malformed_digest_is_refused_and_spends_no_approval
tests::copy_tests::lab_copy::a_settled_run_is_dropped_from_the_running_map
tests::copy_tests::lab_copy::fencing_an_unrelated_grant_does_not_read_historical_plans
tests::copy_tests::lab_copy::racing_copy_starts_recheck_the_wire_id_before_journaling
tests::copy_tests::lab_copy::restart_status_reports_unreadable_published_copy_attention_without_journaling
tests::copy_tests::lab_copy::synthetic_lab_copy_previews_approves_over_http_and_commits
tests::copy_tests::lab_copy::identity_change_fences_a_running_copy_before_and_after_publish
tests::duplicates_tests::duplicates_select_candidates_before_opening_root_metadata
```

The fencing test reproduced alone using the exact workspace test binary (1 failed,
574 filtered out, 58.86 s): `needs_attention` instead of `committed` at
`crates/loomward-service/src/copy_tests.rs:617`. A diagnostic rerun via
`cargo test -p loomward-service --all-features --lib tests::copy_tests::lab_copy::fencing_an_unrelated_grant_does_not_read_historical_plans -- --exact --nocapture`
also failed (19.51 s). Its synthetic status reported 5 items total, 4 verified,
no refusal, and no unjournaled items. The temporary diagnostic print was removed;
there is no service-source diff. One earlier command used an unqualified filter
with `--exact`, ran zero tests, and is **not** evidence of a pass.

No pristine-baseline reproduction was performed: the causes of these broader
failures and their regression status remain unresolved. They are not labelled
flaky or proven pre-existing. The moved sector validator retains its original
checks and passed the focused native copy-reader/effects tests, but that does not
make the workspace gate green. No failing test was weakened or skipped.

The complete logs remain outside Git in the assigned build target directory:
`lw137-workspace-tests.log`, `lw137-copy-diagnostic.log`, `lw137-clippy.log`.
Driver resume: investigate the listed service failures before accepting a green
workspace gate; retain the current storage implementation for review under #876.

## Limits

Real WMI counters, USB bridges/BOT/UAS, disk strings, PnP policy, Windows build,
physical enclosure swaps and real drive-letter reassignment remain unverified.
The non-Windows fallback test was not executed on this Windows host.
The latter tuple behaviors are proven only through the synthetic provider. The
minimal native provider intentionally cannot qualify an unchanged physical path
while those observations are unknown. There is no health UI or service endpoint.
`HUMAN_TODO.md` is unchanged; no owner action or personal-copy authority is inferred.
