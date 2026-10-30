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
| Health | NVMe: `counters_present` from the NVMe SMART / Health log page (#876, section below). Every other bus: `unsupported_or_hidden` with a bus-specific reason |

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

## Reliability counters without elevation (#876)

Windows 11, 2026-10-30, non-elevated token, aggregate only (no serials, models or paths).
Probe: a scratch program opened each fixed or removable volume with access 0 (the provider's own
open path) and its physical drive with access 0, then sent read-only
`IOCTL_STORAGE_QUERY_PROPERTY` queries. Six volumes on three buses were probed:

| Bus (count) | NVMe health log (property 50, log page 2) | `StorageDeviceTemperatureProperty` (52) | `StorageDeviceEnduranceProperty` (62) |
| --- | --- | --- | --- |
| NVMe (3) | succeeded, 560 bytes, under 15 ms | succeeded | error 1117 (I/O device error) |
| SATA (2) | error 1 (invalid function) | error 1 | error 1 |
| USB (1) | error 87 (invalid parameter) | error 87 | error 87 |

The volume handle and the physical-drive handle gave identical results, so the provider keeps
using the volume handle it already opens (no disk-number lookup). `StorageDeviceUnsafeShutdownCount`
returned error 1 everywhere. Nothing needed elevation. `MSFT_StorageReliabilityCounter` (WMI,
`root\Microsoft\Windows\Storage`) and ATA SMART pass-through were not attempted: both are outside
the no-elevation, no-new-dependency slice, and neither is claimed to work or to be refused.

Mapping from the 512-byte NVMe log page (NVMe Base Specification, SMART / Health Information):

| `ReliabilityCounters` field | Log page source |
| --- | --- |
| `temperature_celsius` | Composite Temperature (Kelvin, bytes 1-2) minus 273; 0 K or below 273 K is unknown |
| `wear_percent` | Percentage Used (byte 5), reported as is (it may exceed 100) |
| `power_on_hours` | Power On Hours (128-bit); a value above `u64` is unknown, not saturated |
| `start_stop_cycles` | Power Cycles (128-bit), the nearest NVMe counterpart |
| read/write error totals and uncorrected counts | Not mapped: the log has one aggregate Media Errors count, which is neither a read nor a write total |

An all-zero page is treated as a driver stub, not a measurement. The query runs on a worker thread
and waits at most the smaller of 2 s and what the request deadline leaves minus a 250 ms reserve
(`volumes.list` passes that remainder, so a stalled disk cannot fail the listing); with nothing
left it is skipped ("not queried: request deadline"). On timeout the worker is detached and the
row reports `unsupported_or_hidden` ("did not answer within its time budget"). A worker panic
has its own reason, and so does the cap: at most four stuck workers at once, after which queries
are refused without spawning ("not queried: earlier health queries are still waiting"). Reasons never include identifiers. Through the real provider on this
host (one query per volume): NVMe volumes reported `counters_present` with temperature, wear,
power-on hours and power cycles all present (2 to 12 ms each); SATA volumes reported the "no
unelevated health query" reason (under 2 ms); the USB volume reported the bridge reason (14 ms).
Specific values are deliberately not recorded here.

Sources: Microsoft Learn,
[Working with NVMe Drives](https://learn.microsoft.com/en-us/windows/win32/fileio/working-with-nvme-devices)
(protocol-specific queries, health log page, temperature queries) and
[IOCTL_STORAGE_QUERY_PROPERTY](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddstor/ni-ntddstor-ioctl_storage_query_property).

## Limits

WMI counters, SATA/SAS health, other NVMe drivers or controllers, USB bridges/BOT/UAS, disk strings, PnP policy, Windows build,
physical enclosure swaps and real drive-letter reassignment remain unverified.
The non-Windows fallback test was not executed on this Windows host.
The latter tuple behaviors are proven only through the synthetic provider. The
minimal native provider intentionally cannot qualify an unchanged physical path
while those observations are unknown. (Written before #1049; `volumes.list` now surfaces the
storage path and health read-only.)
`HUMAN_TODO.md` is unchanged; no owner action or personal-copy authority is inferred.
