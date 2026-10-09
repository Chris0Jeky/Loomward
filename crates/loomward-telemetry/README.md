# Native read-only resource telemetry

LW-050 observation and LW-051 rate groundwork. Uses only std, the workspace's
existing serde/serde_json for JSON, and the already locked windows-sys 0.61.2.
No new third-party package is introduced. No process effects, elevation, device
opens, global configuration, remote requests or admission authority exist here.

```powershell
cargo run -p loomward-telemetry -- snapshot --json
cargo run -p loomward-telemetry -- watch --interval-ms 1000 --count 3 --json
```

The bin writes one compact JSON object per line and flushes each line. Watch has
a required count (1..10000) and optional interval (default 1000, 100..60000 ms).
The interval is a target start-to-start cadence; if collection or output takes
longer, the next sample starts afterward. Actual elapsed time is reported.
Invalid commands/options return exit code 2. Unknown observations still return
valid JSON, not an error disguised as a zero.

Rust callers use `snapshot(max_processes)` or `Sampler::default().sample(n)`.
Rows are ordered by decreasing known working set, then PID, with unknown working
sets last. Display is capped at 1024 rows (the CLI requests 200); zero requests
totals only. Enumeration and queries are capped at 8192 process entries. Totals
and previous-sample counters cover the entire enumerated set, before projection.
The Toolhelp snapshot itself is OS-owned; the row budget bounds user-space
queries and retained data, not the kernel's internal snapshot allocation.
PDH output is capped at 1 MiB, 256 instances and 1024 UTF-16 units per name.

## JSON schema version 1

All listed fields are present. Optional scalar fields are `null` when unknown;
zero is an actual observation. Reasons are in the containing `unknowns` list.
Group observations have exactly one of these shapes:

```json
{"status":"observed","value":{}}
{"status":"unknown","reason":"requires_two_disk_counter_samples"}
{"status":"unsupported","reason":"native telemetry is implemented only on Windows"}
```

`value` has the documented type below, not necessarily an object. Consumers must
check both group status and null fields. An observed memory group can be partial.
`unknowns[]` entries contain `field` (scalar or named group), `reason`, and
`windows_error` (numeric Win32 error, or null for a sampling/identity reason).

| Location | Fields and units |
|---|---|
| Root identity/coverage | `schema_version` = 1; `status` = observed/partial/unsupported; `platform` = OS name; `display_truncated` = boolean |
| Root time | `captured_at_unix_ms` = sample start, milliseconds since Unix epoch; `snapshot_cost_ms` = collection, rates, retained copy and projection cost, excluding JSON/output; `sample_interval_seconds` = monotonic sample-start interval, null first sample |
| Root CPU capacity | `logical_processor_count` = all active logical processors across processor groups, null if unavailable |
| `memory` | observed Memory object, or unsupported with reason |
| `memory.value` | `physical_total_bytes`, `physical_available_bytes`, `commit_charge_bytes`, `commit_limit_bytes`, `commit_peak_bytes`, `system_cache_bytes`, `kernel_paged_bytes`, `kernel_nonpaged_bytes`, `page_size_bytes`; each bytes or null; `unknowns[]` |
| `processes[]` identity | `pid`, `parent_pid` = numeric IDs; `image_name` = basename or null, no executable paths; `session_id` = ID or null; `start_time_windows_100ns` = **decimal string** of Windows FILETIME ticks since 1601, or null |
| `processes[]` CPU | `cpu_user_100ns`, `cpu_kernel_100ns` = cumulative 100 ns units since process start, or null |
| `processes[]` memory | `working_set_bytes`, `private_commit_bytes`, `private_working_set_bytes` = bytes or null; private working set requires EX2 |
| `processes[]` I/O | `io_read_bytes`, `io_write_bytes`, `io_other_bytes` = cumulative bytes since start, or null; includes non-disk I/O |
| `processes[]` counts/quality | `handle_count` = count or null; `thread_count` = enumerated count; `protected_or_unknown` = any query failure/exit; `unknowns[]`; `rates` |
| `processes[].rates` | `cpu_percent_of_machine` = percent of total logical CPU capacity; `io_read_bytes_per_second`, `io_write_bytes_per_second`, `io_other_bytes_per_second` = bytes/s; `working_set_delta_bytes`, `private_commit_delta_bytes` = signed bytes; each null when unknown; `unknowns[]` |
| `process_totals` counts | `enumerated`, `opened`, `access_denied`, `partial_or_unknown`, `working_set_observed`, `private_commit_observed` = counts over enumerated entries |
| `process_totals` sums | `working_set_sum_bytes`, `private_commit_sum_bytes` = bytes summed over successful observations only; interpret together with observed counts |
| `process_totals` coverage | `enumeration_truncated` = budget reached; `enumeration_unknowns[]` = enumeration failures |
| `gpu`, `process_gpu` | unsupported observations with reasons; no adapter/process GPU values supplied in v1 |
| `disk_io` | observed array of physical-disk instances, or unknown/unsupported with reason |
| `disk_io.value[]` | `instance` = PDH physical-disk label; `bytes_per_second` = combined read/write bytes/s or null; `sample_interval_seconds` = PDH collection timestamp difference in seconds; `unknown_reason` = null or invalid counter reason |

Byte/counter integers are unsigned JSON numbers; consumers needing arbitrary
precision must preserve integers when parsing. The FILETIME identity is a string
because it exceeds JavaScript's exact integer range. Identity remains observation,
never a permission to operate on that process.

Named unknown groups: `physical_memory` covers physical total/available;
`performance_memory` covers the remaining memory fields;
`process_handle_fields` covers every process field other than PID, parent PID and
enumerated thread count; `start_and_cpu_times` covers creation/user/kernel time;
`process_memory` covers all three process memory fields; `io_counters` covers
read/write/other counters; `all_rates` covers the six rate/delta fields. Other
reason entries name their scalar field directly.

## Accounting and limits

Available physical memory includes reclaimable pages; it is not unused RAM.
Commit is a promise of backing, not residency. Working sets can share pages, so
their sum is not unique machine physical use. Cache and kernel accounting are not
a disjoint pie chart. Memory growth is evidence of change, not a leak diagnosis.
Measured available memory is not an admission guarantee; reservations and demand
estimates remain the separate accounting described in `docs/31-resource-economics.md`.

Only `PROCESS_QUERY_LIMITED_INFORMATION` is requested. EX2 is tried first and EX
is the read-only fallback. A denial does not borrow Toolhelp's executable name to
pretend the full-image query worked. A failed or exited process retains its PID,
parent and thread enumeration metadata and explicit unknowns. The marker does
not assert that the process is protected: it may simply have exited.

CPU/I/O rates require matching PID **and exact creation time** and two samples.
Missing identity, a new/reused PID, a regressed cumulative counter or an invalid
interval leaves the affected rate unknown. CPU is `(user_delta + kernel_delta) /
10^7 / seconds / logical_processors * 100`. Rates are estimates from sequential
queries, not an atomic machine snapshot; process tree/thread/session metadata can
race with lifecycle changes. Memory deltas may be negative. An undisplayed
process still has previous counters retained for the next sample.

Disk collection uses the local language-neutral PDH
`\PhysicalDisk(*)\Disk Bytes/sec` counter. First sample is unknown, not zero.
PDH's own collection timestamps describe its rate window, which may differ from
the process window. `_Total` is excluded to avoid double counting. Instances can
contain multiple volume letters: **isolated per-volume throughput is unknown**.
No disk handles are opened and missing counters never trigger elevation.

GPU budgets are explicitly unsupported: windows-sys 0.61 exposes no DXGI adapter
COM interfaces; hand-writing a COM ABI or adding a COM dependency is deferred.
Optional per-process GPU counters are also deferred: GPU adapter budgets are
unavailable and PID-only PDH instances do not establish stable process identity.
Non-Windows builds return unsupported groups and an empty observation set, never
synthetic host data.

Microsoft API contracts: [process memory query](https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-getprocessmemoryinfo),
[PDH wildcard arrays](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhgetformattedcounterarrayw),
[language-neutral counters](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhaddenglishcounterw).
Measurements and verification are in `evidence/v3/telemetry.md`.

