# Engine telemetry: L11b baseline and L11c follow-up (LW-050, LW-107)

The L11b section below is historical evidence. Its retained-sample behavior and P12 result are
superseded by the L11c follow-up at the end of this receipt.

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
- `evidence/v3/telemetry-engine.md`
- `evidence/v3/bench/telemetry.json`

Recommended commits: `Implement lease-driven read-only engine telemetry`; then
`Measure telemetry overhead and idle engine usage` for the harness and receipts.
No attribution trailers.

## NOT verified

No service/HTTP/Tauri/UI event delivery or health route was implemented or tested here; those lanes
must wire the two helper methods. Hosted CI and a Linux run were not performed. P13 covers the C2
engine with telemetry, not a future engine with its other components populated. There is one
10-minute phase per condition, uncontrolled concurrent host workload, no p95 or repeatability claim.

## Residual risk

Telemetry overhead exceeded P12 by 2.143 percentage points of one core in this run. Original L11b samples retained their time after sampling stopped and could be stale; L11c clears them.
Native access failures and first-sample rates remain null; native per-process GPU memory and adapter
capacity/name remain limited. Bounded display/adapter/disk projections are not whole-machine coverage.
The service must observe the sample queue's bounded-drop behavior when wiring event delivery.

`HUMAN_TODO.md` was read and left unchanged. q-5 (personal teacher disclosure) remains open and q-6
(firewall action) remains not yet needed; neither grants a cloud or process-effect capability here.


## L11c follow-up: issues #163 and #160

Windows 11, rustc 1.97.1, branch `fix/l11-telemetry-followups`, base
`35f020850e282f223aa37bc29475f9b9b1158f2a`. Changes remain uncommitted for the driver.
Only read-only personal observations were used; persisted results are aggregates.

### Changed

- Health reads `own_usage()` directly, even without a lease. CPU seconds, private commit and
  working set are current observations. Health thread count is null: that API does not supply
  it, and a retained thread count would be stale. Native leased EngineSample thread counts still
  come from the current enumeration.
- A sampler exit guard clears running/busy, latest and pending events, including on unwind.
  A next lease joins the old failed thread and respawns. If the caller notices final-lease expiry
  before the sleeping sampler, it joins that session before installing the next lease.
- Unknown-creation-time references use image name, PID and parent PID, independent of sequence.
  The explanation names the limitation: reuse of all three cannot be distinguished. These
  references support explanation continuity, never permission or process effects. Hash keys are
  for this runtime, not a persisted cross-version identity contract.
- Event mapping occurs outside the state mutex. Dropped mapping failures are counted in aggregate
  diagnostics. Released/expired leases are checked again before mapped events enter the queue.
- First summaries use working set when no CPU rates exist. GPU sorting is explicitly PID-only
  because all process GPU quantities are unknown. Sort regressions use inputs requiring reorder.
- Native sampling uses the current lease channel union. PDH registers only requested counters;
  process enumeration is skipped without process/engine leases, and an engine-only lease reads
  only its own enumerated row. A changed union resets rate history and the PDH query.
- Wildcard audits run every ten collections per counter, or next collection after a returned
  invalid/churning value. Between audits the formatted array is decoded and each CStatus is
  validated anew; no old counter values or instance sets are reused. Fresh audits retain the
  existing churn reconciliation and malformed-instance bounds. The Windows
  [formatted-array API example](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhgetformattedcounterarrayw)
  likewise uses a wildcard counter and repeated collection/array reads without per-tick expansion.
- Aggregate diagnostics measure native process enumeration/reads, PDH collection, wildcard
  expansion, PDH decoding, other native work, rate derivation/projection and event DTO mapping.
  These costs are elapsed wall time, not CPU attribution. Whole-process and harness-thread CPU
  are measured separately. The benchmark observes idle sampling/event counts instead of asserting
  no sampling as a string. Its default remains 600 seconds per phase; shorter runs are marked.

### Verified

All four requested checks passed on Windows: `cargo fmt --all --check`, `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings`, and `git diff --check`.
No failing test was skipped or weakened.

Red/green proof (each distinguishes the missing fix):

| Regression | Observed red | Green behavior |
|---|---|---|
| `health_reads_own_usage_without_a_lease_or_retained_values` | Original health returned PartialCoverage without a sample | Reads live own usage and ignores seeded stale commit |
| `sampler_exit_discards_latest_before_restart` | Original exit retained latest | Exit drops the previous session sample |
| `expired_session_cannot_supply_the_next_lease` | Next lease saw seeded previous-session sequence 999 | Expired session is joined before next lease |
| `unknown_creation_ref_survives_list_to_explain_tick` | Explain returned not_found after sequence advanced | Ref survives tick; changed image or parent changes key |
| `sampler_and_mapping_panics_reset_flags_and_allow_next_lease` | Sampler panic left flags true; mapping panic poisoned mutex | Both injected cfg(test) panics clear state and allow respawn |
| `first_summary_ranks_working_set_when_cpu_is_unknown` | PID 7 preceded PID 99 with larger working set | PID 99 ranks first |
| `sorting_reorders_known_values_and_pid_ties` and strengthened missing-value test | Always-Equal comparator mutation failed both | Known values reorder; unknowns last; PID ties deterministic |
| `event_mapping_errors_are_counted_and_state_remains_readable` | Disabled-counter mutation yielded 0 instead of 1 | Oversized synthetic adapter ID causes one counted error, no event, readable state |
| `gpu_only_sampling_does_not_enumerate_processes_or_collect_other_channels` | Unfiltered native sampler returned processes | No process enumeration, memory/disk collection or own row |
| `wildcard_audit_waits_ten_samples_and_status_failure_requests_retry` | Always-refresh predicate failed waiting assertion | Predicate respects cadence and forced retry |

Existing native first-rate, instance-churn, malformed-buffer, denied-access and schema tests pass.
The boundary lifecycle test now expects PartialCoverage after release and a fresh first interval
on the next lease, rather than accepting a frozen stopped sample.

### P12/P13 measurement

The initial 30-second idle + 30-second leased profile measured 4.843471% of one core above idle.
It is diagnostic evidence only, not the P12 gate. Its 30 samples averaged 21.804 ms of process
work, 21.402 ms PDH collection, 4.281 ms wildcard expansion, 0.796 ms PDH decoding, 0.392 ms
engine mapping, 0.360 ms other native work and 0.245 ms rate/projection work. There were 205
wildcard audits. The short-profile binary hash was not retained; its recipe and aggregates are
preserved separately from the original and final full-duration measurements in the JSON receipt.

The original L11b full-duration result remains in `before_l11b`; the pre-optimization profile is
in `profile_before_optimization`; the current full-duration results are the top-level idle/leased
sections. Source and release binary hashes identify the final measured implementation.


| Metric | Original L11b (600 + 600 s) | L11c (600 + 600 s) |
|---|---|---|
| P12 one-core overhead above idle | 3.143222% (MISS) | 1.997391% (MISS; target <= 1%) |
| Leased CPU / elapsed | 18.859375 s / 600.001402 s | 11.984375 s / 600.001406 s |
| Idle one-core CPU | 0% at counter precision | 0% at counter precision |
| P13 idle peak private commit | 987,136 bytes | 933,888 bytes (0.890625 MiB); PASS |
| Last published sample sequence / drained events | 595 / 595 | 595 / 595 |
| Idle sampling | Previously a hard-coded string | Measured 0 samples / 0 events |

| Phase (elapsed wall ms per completed sample) | 30-s pre-optimization profile | Full L11c run |
|---|---:|---:|
| Process enumeration and per-process reads | 21.804 | 18.123 |
| PDH collection | 21.402 | 1.217 |
| Wildcard expansion | 4.281 | 0.402 |
| PDH decode | 0.796 | 0.824 |
| Other native work | 0.360 | 0.263 |
| Rates/projection | 0.245 | 0.229 |
| Event DTO mapping | 0.392 | 0.315 |

The final run performed 420 wildcard audits across 595 samples (0.706/sample), compared with
205 across 30 samples (6.833/sample) in the short pre-optimization profile. No event-path mapping
errors were observed. Process enumeration/reads account for approximately 85% of accumulated
phase wall time and dominate the measured cost. The measured P12 floor for this implementation
and workload is 1.997391%, not a theoretical minimum. Reaching 1% needs a cheaper process
collection strategy or separately timestamped slower process observations; cached old rows cannot
be published as fresh under this DTO. Handle reuse needs exact-instance/exit tests before adoption.

The numeric decrease from the historical full baseline is approximately 36.5%. These are separate
uncontrolled runs, so it does not isolate the causal contribution of either optimization. The
30-second profile is not a 10-minute P12 baseline. Only the final full soak is the current
post-optimization P12/P13 measurement.

After the soak, a small engine-only correction kept the existing 8,192-entry visit cap even when
other entries are skipped, and two observation/identity comments were clarified. The all-channel
native reads/counts used by P12 are unchanged; no second soak was run for that narrower seam.
`post_measurement_changes` records these final-source hashes separately from the measured binary
and its source hashes. The required final checks were rerun after this correction.

[Full aggregate receipt](bench/telemetry.json).

### Files changed

- `crates/loomward-engine/src/telemetry/{mod,mapping,tests}.rs`
- `crates/loomward-engine/tests/telemetry.rs`
- `crates/loomward-engine/examples/telemetry_bench.rs`
- `crates/loomward-telemetry/src/lib.rs`
- `crates/loomward-telemetry/src/windows.rs`
- `crates/loomward-telemetry/src/windows/pdh.rs`
- `crates/loomward-telemetry/tests/native.rs`
- `evidence/v3/bench/telemetry.json`
- `evidence/v3/telemetry-engine.md`

Recommended commits: `Fix telemetry freshness identity and sampler recovery`; then
`Profile telemetry and reduce unused channel and wildcard work`.

### NOT verified

No hosted CI, Linux, HTTP/Tauri/UI integration, controlled repeated performance study or independent
review was run by this lane. Wall-clock phases do not prove per-phase CPU ownership. Process-handle
reuse and slower process cadence were not implemented; requested observations remain fresh at 1 Hz.
The closed wire schema still cannot distinguish unrequested and unavailable system/process channels,
or expose adapter/disk truncation caveats; those other #163 LOWs remain open.

### Residual risk

P12 remains above target by 0.997391 percentage points of one core.
Unknown-time explanation keys cannot distinguish reuse of the same image/PID/parent tuple, and
hash collisions remain possible; no effects are offered or authorised. Native access failures,
unsupported GPU fields and first rates remain unknown. Periodic audits can detect expansion-only
churn up to ten collections later; returned invalid CStatus still makes that value unknown immediately.
`HUMAN_TODO.md` was read and left unchanged: q-5 remains open and q-6 is not yet needed.
