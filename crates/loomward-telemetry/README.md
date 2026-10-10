# loomward-telemetry

Read-only Windows memory, process, physical-disk and GPU observations. No process
control, quota, job assignment, elevation, network, filesystem scan or COM bindings.
No new dependency: Windows APIs and PDH use the existing `windows-sys` crate.

```powershell
cargo run -p loomward-telemetry --release -- snapshot --json
cargo run -p loomward-telemetry --release -- watch --interval-ms 1000 --count 3 --json
```

`watch` requires `--count` (1..10000), accepts an interval of 100..60000 ms and
emits NDJSON. `Sampler::sample(max_processes)` collects only when called. It
retains observations for undisplayed processes, then projects at most 1,024 rows;
process enumeration stops at 8,192 entries. Engine subscriptions, leases, DTO
projection and pool accounting belong to the engine lane, not this raw sampler.

## Schema version 2

Revision-2 quantity names and meanings follow [architecture section 11](../../docs/41-v03-architecture.md)
and the normative [view-service schema comments](../../contracts/v3/view-service.schema.json).
The CLI is a native diagnostic format, not a view-service response: byte counters
remain unsigned JSON integers, identity is exact FILETIME text, and process rates
are nested in `rates`. The engine adapter supplies contract envelopes, decimal
byte strings, opaque process/volume references and ownership metadata.

Version 2 replaces `physical_total_bytes`, `physical_available_bytes` and
`commit_charge_bytes` with `total_bytes`, `available_bytes` and `commit_bytes`.
It replaces `cpu_percent_of_machine` with `cpu_fraction` (divide old values by
100), and `*_bytes_per_second` with `*_bytes_per_s`. Disk rows replace `instance`,
combined `bytes_per_second` and `unknown_reason` with `disk_label`, separate read
and write rates, busy fraction, queue length and `unknowns[]`. No `private_bytes`
alias exists: private commit and private working set already had distinct names.

A null scalar means unknown; zero is emitted only after a valid observation.
Reasons appear in the containing `unknowns[]` (or root `pdh_unknowns[]`). Entries
contain `field`, `reason` and `windows_error` (Win32 code or null). PDH failure
codes are included in the reason text. Group observations have one of these forms:

```json
{"status":"observed","value":{}}
{"status":"unknown","reason":"no_gpu_adapter_instances_observed; see pdh_unknowns"}
{"status":"unsupported","reason":"native telemetry is implemented only on Windows"}
```

An observed group can contain unknown scalars. Root `status` is partial when
process, memory, PDH, disk or GPU observations are incomplete. `gpu.value` and
`disk_io.value` are arrays; `memory.value` is an object. Missing fields are never
replaced with demo values.

### Fields and sources

All PDH paths below are English paths added with `PdhAddEnglishCounterW`.
`PhysicalDisk`, `GPU Engine` and `GPU Adapter Memory` use `(*)` instances.

| Location / name | Unit | Source | Unknown semantics |
|---|---|---|---|
| Root `schema_version`, `status`, `platform`, `display_truncated` | version 2 / coverage / OS / boolean | sampler | unsupported on non-Windows; partial coverage stays visible |
| `captured_at_unix_ms` | ms since Unix epoch | sample start wall clock | collection is sequential, not an atomic snapshot |
| `processes_observed_at_unix_ms` | ms since Unix epoch | process enumeration start | null without an enumeration; leased rows retain this time between enumerations (minimum fifteen seconds, extended by coarser lease ticks); standalone sampling remains fresh every call |
| `sampled_channels` | booleans | requested native channels | false means not sampled under this lease, distinct from failed or unavailable hardware |
| `snapshot_cost_ms` | ms | monotonic collection + rates + retained copy + projection | excludes JSON, output, startup and build |
| `sample_interval_seconds` | seconds | monotonic sample-start difference | null first sample |
| `logical_processor_count` | logical CPUs | `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)` | null if unavailable |
| `system_cpu_busy_fraction` | fraction [0,1] | `\Processor Information(_Total)\% Processor Time` / 100 | null first sample or invalid PDH value; root PDH reason |
| `memory.total_bytes`, `available_bytes`, `load_fraction` | bytes / bytes / fraction | `GlobalMemoryStatusEx` (`ullTotalPhys`, `ullAvailPhys`, `dwMemoryLoad` / 100) | null with `physical_memory` reason |
| `memory.commit_bytes`, `commit_limit_bytes`, `commit_peak_bytes` | bytes | `GetPerformanceInfo` commit pages x page size | null with `performance_memory` reason |
| `memory.system_cache_bytes`, `kernel_paged_bytes`, `kernel_nonpaged_bytes`, `page_size_bytes` | bytes | `GetPerformanceInfo` (page counts converted to bytes) | null with `performance_memory` reason |
| `memory.standby_bytes` | bytes | sum of `\Memory\Standby Cache Core Bytes`, `Normal Priority Bytes`, `Reserve Bytes` | null if any component unavailable/invalid or sum overflows; no partial sum |
| `memory.modified_bytes` | bytes | `\Memory\Modified Page List Bytes` | null with field or `memory_lists` reason |
| `memory.free_bytes` | bytes | `\Memory\Free & Zero Page List Bytes` | null with field or `memory_lists` reason; excludes standby |
| `processes[].pid`, `parent_pid`, `thread_count` | counts / IDs | Toolhelp enumeration | parent/thread metadata can race with process lifetime; thread count is null without a current enumeration |
| `image_name`, `session_id` | basename / ID | `QueryFullProcessImageNameW`, `ProcessIdToSessionId` | null on denial, query failure or exit; no full path |
| `start_time_windows_100ns` | exact decimal string, 100 ns since 1601 | `GetProcessTimes` creation FILETIME | null on denial/failure/exit; used with PID to match instances |
| `cpu_user_100ns`, `cpu_kernel_100ns` | cumulative 100 ns | `GetProcessTimes` | null with `start_and_cpu_times` or process-handle reason |
| `working_set_bytes` | bytes | `GetProcessMemoryInfo` EX2/EX `WorkingSetSize` | null with process-memory reason; includes shared pages |
| `private_commit_bytes` | bytes | EX2/EX `PrivateUsage` | null with process-memory reason; commit, not residency |
| `private_working_set_bytes` | bytes | EX2 `PrivateWorkingSetSize` | null with explicit reason on EX fallback; private resident pages |
| `io_read_bytes`, `io_write_bytes`, `io_other_bytes` | cumulative bytes | `GetProcessIoCounters` | null with `io_counters` reason; includes non-disk devices |
| `handle_count`, `protected_or_unknown`, `unknowns[]` | count / boolean / reasons | handle query and observation quality | handle count null on failure; marker is not a protection diagnosis |
| `rates.cpu_fraction` | fraction [0,1] | delta user + kernel CPU seconds / (interval x all active logical CPUs) | null first sample, unmatched instance, invalid interval/capacity/counter or out-of-range fraction |
| `rates.io_read_bytes_per_s`, `io_write_bytes_per_s`, `io_other_bytes_per_s` | bytes/s | matched process counter deltas / monotonic interval | null first sample, missing identity/counter, regression or invalid interval |
| `rates.working_set_delta_bytes`, `private_commit_delta_bytes` | signed bytes | matched process memory differences | null on missing counter/identity or out-of-range delta |
| `process_totals` | counts / bytes / coverage | enumeration before display projection | counts explain partial sums; working-set sums can repeat shared pages; enumeration failures in `enumeration_unknowns[]` |
| `disk_io[].disk_label` | PDH instance label | `PhysicalDisk(*)` | malformed names skipped and counted; `_Total` excluded |
| `read_bytes_per_s`, `write_bytes_per_s` | bytes/s | `Disk Read Bytes/sec`, `Disk Write Bytes/sec` | null first sample or missing/invalid instance; per-field reason |
| `busy_fraction` | fraction [0,1] | 1 - `% Idle Time` / 100, clamped | null first sample or missing/invalid instance; never derived from queue length |
| `queue_length` | average outstanding requests | `Avg. Disk Queue Length` | null first sample or missing/invalid instance; may exceed 1 |
| Disk `sample_interval_seconds` | seconds | `PdhCollectQueryDataWithTime` difference | null until two advancing successful collection timestamps |
| `gpu[].adapter_id` | LUID string | adapter LUID parsed from current PDH instances | malformed identity skipped and counted; not a device name or grant |
| `dedicated_total_bytes` | bytes | unavailable from these usage counters | always null with `capacity_not_provided_by_gpu_usage_counters` |
| `dedicated_used_bytes`, `shared_used_bytes` | bytes | `\GPU Adapter Memory(*)\Dedicated Usage` / `Shared Usage` | independently null on missing/invalid instance/query; no process sums or dedicated+shared "total" |
| `engine_busy_fraction` | fraction [0,1] | max over engine types of summed `\GPU Engine(*)\Utilization Percentage` / 100, clamped | null first sample, missing engine/type/value or unidentified malformed instance; never a maximum of individual process values |
| `engines[].engine_type`, `busy_fraction`, `unknowns[]` | PDH type / fraction / reasons | instances grouped by adapter LUID and engine type | unnamed provider types are null with `engine_type_unavailable`; incomplete type sums stay unknown |
| `process_gpu` | group status | optional GPU Process Memory collection deferred | unsupported with reason; no per-process GPU bytes |
| `pdh_malformed_instances`, `pdh_unknowns[]` | count / reasons | shared PDH parser, expansion and queries | count includes malformed occurrences across counter arrays/expansions; valid disk rows survive |

## Accounting and limits

Available RAM includes standby and free pages; standby is reclaimable, not empty.
Modified pages need writeback. Cache, kernel accounting and process working sets
are overlapping views, not slices of a disjoint physical-memory pie. Samples from
different APIs need not sum exactly while the machine changes. Memory lists use
PDH directly: no privilege acquisition or `SystemMemoryListInformation` call.

CPU's denominator is `sample_interval_seconds * logical_processor_count * 10^7`
when the numerator is cumulative Windows CPU ticks. It measures the machine's
whole logical CPU capacity, across groups. Missing/reused PID+creation-time pairs,
regressed counters, invalid intervals and impossible fractions yield unknowns.

PDH wildcard English paths are localized with `PdhGetCounterInfoW`; each sample
re-expands them with `PdhExpandWildCardPathW(PDH_REFRESHCOUNTERS)`. Wildcard arrays
are read afresh, checked against that expansion, and their `CStatus` checked
individually. Names must be bounded, aligned, terminated UTF-16 within the returned
buffer; one bad disk name never discards other valid instances. Expansion/array
sizing gets at most three attempts during churn. PDH buffers are capped at 1 MiB,
arrays/expanded lists at 16,384 instances, and names at 1,024 UTF-16 units. No stale
adapter list survives into the next sample. Newly appearing rate instances remain
null when PDH has no valid two-sample history. `_Total` never doubles the sums.

GPU bytes use exact 64-bit PDH integers with no default scaling. Adapter memory
comes only from global adapter counters; shared usage is reported separately.
An empty engine-type string is a real provider limitation, not a guessed type:
that adapter's utilisation stays unknown, while other adapters remain usable.
Dedicated capacity, friendly adapter names and optional process GPU memory are not
invented. No DXGI caller-budget query is used. Physical disk labels can cover several
volumes; isolated per-volume I/O and native volume-ID mapping belong to later integration.

Non-Windows builds return unsupported groups, never synthetic host data.
Microsoft references: [process memory query](https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-getprocessmemoryinfo),
[memory counters](https://learn.microsoft.com/en-us/windows/win32/memory/memory-performance-information),
[PDH wildcard arrays](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhgetformattedcounterarrayw),
[English counters](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhaddenglishcounterw),
[wildcard refresh](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhexpandwildcardpathw).
Measurements and proof: [telemetry evidence](../../evidence/v3/telemetry.md).
