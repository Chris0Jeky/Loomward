# Engine telemetry: lane L11b (LW-050, LW-107)

10 October 2026, Windows 11, rustc 1.97.1. Worktree `feat/l11b-telemetry-engine`, base
`3462de4e450c43ae6bfadff2de8c070e8885be8f` (C2 and native telemetry revision 2). Driver commits;
no commit, push, PR, merge or process/file effect was performed by this lane.

## Changed

The five C2 telemetry/process stubs now use one dedicated native sampler with no async runtime.
Leases last 60 s, renew by subscription ID, share the fastest live interval (1/2/5/10 s), and expire
on monotonic time. The last release and engine drop join in-flight sampling; expiry stops the thread
without waiting for a client call. There are at most 32 live leases. Snapshots/list/explanations read
the latest retained sample without triggering observation; before the first sample they report
`partial_coverage`. Released/expired IDs are `not_found`. Restart after stop resets rate history.

Only personal sessions can lease host telemetry. Synthetic mapping tests use invented counters;
native tests and P12/P13 use personal sessions and persist no process metadata. The sampler does
not open paths or request mutation rights. `available_actions` is always empty.

Per-lease, channel-projected samples are queued for `telemetry_events()` at their own interval;
the queue is capped at 1,024 (oldest drop) and purged on release/expiry. The service lane still owns
putting these into its event envelopes and implementing event-stream overflow/resume semantics.
`telemetry_health()` supplies Health.engine for its owning service lane.

## DTO mapping

| Native schema v2 | Protocol v3 | Units and unknown handling |
|---|---|---|
| `Memory` physical/commit fields | `SystemSample.memory` | Exact decimal-string bytes, null when missing; physical load fraction is unchanged |
| logical CPU count / system busy fraction | `SystemSample.cpu` | Count / machine fraction; whole system channel is null when CPU capacity is unknown, never a fabricated zero count |
| process PID + exact creation FILETIME | `ProcessRow.process_ref`, `started_at` | Instance key uses exact ticks; display RFC3339 UTC retains 100 ns; missing creation time produces a sample-scoped key so PID reuse cannot reconnect it |
| process private commit / private working set / total working set | corresponding `ProcessRow` fields | Separate decimal-string bytes; EX2 absence remains null; shared working sets are never unique physical RAM |
| two-sample process CPU / I/O | `ProcessRow.cpu_fraction`, read/write rates | Machine-normalised fraction and numeric bytes/s; first/missing/regressed counters stay null |
| process unknowns / enumeration totals | row access, `ProcessSummary`, `ProcessList`, explanation facts | Failed opens with access denial are denied; partial reads limited; reasons retained verbatim in bounded explanation facts; list count is enumerated count (including denied), not invented complete coverage |
| GPU adapters joined by LUID | `GpuSample` | Global dedicated/shared usage only, decimal strings; busy already max over summed per-type engines; capacity stays null and name is explicitly unknown when not observed |
| unsupported `process_gpu` | `ProcessRow.gpu_dedicated_bytes` | Always null, with a named explanation; no per-process GPU aggregation or fabricated adapter total |
| PDH physical disk | `DiskSample` | Numeric read/write bytes/s; idle-complement busy fraction distinct from numeric queue length; unavailable stays unavailable; no invented catalogue volume IDs |
| own process before display truncation | `EngineSample`, `Health.engine` | Decimal-string commit/residency, machine-normalised interval CPU vs cumulative CPU seconds, observed thread count; only the implemented telemetry pool is reported |
| native snapshot timing | `TelemetrySample` sequence/time/elapsed | Monotonic interval rounded to integer ms, null first interval; original wall-clock observation timestamp retained |

The closed v3 sample schema has no numeric-field reason properties. Reasons remain in the retained
native snapshot, process explanation facts and list caveats; unavailable channel state/null is the
exact wire representation. No schema or DTO was extended. Displays are sorted over the native capped
1,024-row observation, ties by PID, unknown values last; requests cap output at 200 and summaries at
20. `truncated` records display/enumeration/request limits, and the note does not claim a complete
ranking beyond native coverage. GPU adapters cap at 8 and disk rows at 16 as the contract requires.

## Verified

- Baseline: `cargo test -p loomward-engine -p loomward-telemetry` passed before implementation.
- Red: the added lease/latest test failed against C2's `Unavailable` stub.
- Final Windows proving: `cargo fmt --all --check`, `cargo test --workspace` (166 tests),
  `cargo clippy --workspace --all-targets -- -D warnings` passed.
- Six engine unit tests pin exact bytes/units, null handling, FILETIME/calendar conversion, PID reuse,
  sorting/access reasons/no actions, expiry/restart, one shared thread and 60 s/bounded leases.
- Four engine boundary tests pin request bounds, lifecycle, schema validation/DTO round trips and
  token-agnostic Windows structural truths. Validation uses the workspace's existing jsonschema
  0.58.6 with date-time checks against the committed view-service schema, including Health.engine.
- Existing native counter tests still pass; own usage and pre-display self-row retention are checked.

The former smoke assertions for telemetry stubs were updated to the new behavior. No failing test
was disabled. Native calls stay in `loomward-telemetry`; the engine and benchmark forbid unsafe code.
Test logs and build/source identity are retained locally under gitignored `.loomward/checks/`.

## P12/P13

The full 600 s idle + 600 s leased run completed; aggregates and checked source/binary identities
are in [bench/telemetry.json](bench/telemetry.json).

| Target | Measured | Outcome |
|---|---|---|
| P12: at most 1% of one core above idle, all channels at 1 Hz | 3.143222% above idle; leased CPU 18.859375 s over 600.001402 s | **MISS** |
| P13: at most 0.5% idle CPU; 80 MiB private commit | 0.000000% of one core; 987,136 bytes (0.941406 MiB) true peak private commit | **PASS for the C2 engine** |

The idle phase observed zero CPU ticks at Windows counter precision. The leased phase drained
595 events and reached sample sequence 595; this is an actual native sampler run.
P12 missed its engineering hypothesis; this lane does not claim the telemetry overhead target met.
No sampler is created during the no-lease phase; release/expiry/drop stop is separately tested.

Measurement uses
GetProcessTimes (all engine-process threads), GetThreadTimes (harness main), PrivateUsage and true
PeakPagefileUsage, same engine and process, release profile, all channels at 1 Hz. Main/other thread
CPU are reported separately without claiming Windows library helper threads belong to the sampler.

## Files changed

- `Cargo.lock`
- `crates/loomward-engine/Cargo.toml`
- `crates/loomward-engine/src/lib.rs`
- `crates/loomward-engine/src/telemetry/mod.rs`
- `crates/loomward-engine/src/telemetry/mapping.rs`
- `crates/loomward-engine/src/telemetry/tests.rs`
- `crates/loomward-engine/tests/smoke.rs`
- `crates/loomward-engine/tests/telemetry.rs`
- `crates/loomward-engine/examples/telemetry_bench.rs`
- `crates/loomward-telemetry/src/lib.rs`
- `crates/loomward-telemetry/src/windows.rs`
- `crates/loomward-telemetry/tests/native.rs`
- `docs/16-implementation-status.md`
- `handoff/CHECKPOINT.json`
- `evidence/v3/telemetry-engine.md`
- `evidence/v3/bench/telemetry.json`

Recommended commits: `Implement lease-driven read-only engine telemetry`; then
`Measure telemetry overhead and idle engine usage` for the harness, receipts and status/checkpoint.
No attribution trailers.

## NOT verified

No service/HTTP/Tauri/UI event delivery or health route was implemented or tested here; those lanes
must wire the two helper methods. Hosted CI and a Linux run were not performed. P13 covers the C2
engine with telemetry, not a future engine with its other components populated. There is one
10-minute phase per condition, uncontrolled concurrent host workload, no p95 or repeatability claim.

## Residual risk

Telemetry overhead exceeded P12 by 2.143 percentage points of one core in this run. Latest samples deliberately retain their original time after sampling stops and may be stale.
Native access failures and first-sample rates remain null; native per-process GPU memory and adapter
capacity/name remain limited. Bounded display/adapter/disk projections are not whole-machine coverage.
The service must observe the sample queue's bounded-drop behavior when wiring event delivery.

`HUMAN_TODO.md` was read and left unchanged. q-5 (personal teacher disclosure) remains open and q-6
(firewall action) remains not yet needed; neither grants a cloud or process-effect capability here.
