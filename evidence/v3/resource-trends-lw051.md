# LW-051 resource trends

2026-10-11. Windows 11, shared host. Python 3.14.3, cargo/rustc 1.97.1,
Node 24.13.1, npm 11.8.0. Source baseline: `c030528d9db1eacd5511966be455b590d1ca5b1b`.
Committed by the driver from a Sol lane. No external action was taken.

## Changed

1. History lives in the existing engine telemetry lease module; no new crate.
   System byte ledger and CPU, disk busy/queue, and the existing process quantities
   have at most eight points, at most ten minutes old. Cached process rows do not
   become new observations. Process creation time separates instances; denied or
   unknown identity/coverage stays null. Disappearance removes the old series.
   Ended disk enumerations are removed; duplicate labels cannot establish continuity.
   Only live leased channels admit or retain observations. Expiry, release, channel
   changes and sampler exit purge history. Existing last-lease read refusals remain.
2. Candidates require at least four complete contiguous observations. Growth needs
   at least 30 seconds, 1 MiB, no decrease and at least three positive steps.
   CPU needs at least 0.90 throughout a three-second window; average queue needs
   at least 2 throughout it. Memory needs aligned total/available samples with
   available at most 10%, plus declining standby or increasing modified bytes.
   Each candidate carries exact endpoints, observation timestamps, duration, units,
   count, signed secant slope, cited facts and unknown attribution/coverage.
   Byte subtraction precedes float conversion. No candidate is a verdict or score.
3. The existing `TelemetrySample` has a required `trends` field. Schema, examples,
   Rust DTOs, semantics and generated TypeScript agree. Metric/value units are
   checked on deserialization. Existing snapshot/event handlers carry it; there is
   no new command. Reports have at most 8,192 series and 64 candidates; omission is
   explicit. Native admission is bounded at 7,213 series. Immutable histories share
   storage across snapshots/events; updates preserve earlier observations. Pending
   events additionally have a conservative estimated 16 MiB heap allowance. Events
   include histories for their at-most-twenty process summary rows; full leased
   process histories are served by snapshots. Omitted event coverage stays visible.
4. Observatory shows selectable sparklines with units, actual elapsed spacing,
   window/count/timestamps and inspectable values. Null and excessive cadence gaps
   split paths. Exact integer byte deltas preserve small changes near u64 limits.
   Explanations cite endpoints, slope, facts and uncertainty. Missing/refused data
   clears the prior report; session changes clear retained UI data. Existing palette,
   light/dark and forced-colour tokens remain in use.
5. Sampler diagnostics separately accumulate ingestion/event projection and snapshot
   projection time. The synthetic micro-measurement includes full 1,024-process
   history admission and candidate projection. Engine-only short lease comparison
   is below; this does not qualify the complete app under P12.
6. This receipt records checks and limitations without publishing native inventory.
   Public fixtures are synthetic. Raw local benchmark observations stay ignored. Parent/child wall timers overlap and must not be summed as CPU costs; snapshot trend timing is a subset of snapshot mapping timing.

## Synthetic fixtures and tests-first proof

| Fixture | Expected result |
|---|---|
| Steady state | No growth candidate; known values remain present |
| Real monotone growth | One private-commit candidate, four samples, 45 s, exact bytes and slope |
| Sawtooth / GC-like | No growth candidate despite three rises; a decrease breaks sustained growth |
| Gap / missing value | Visible null or split path; no candidate crossing the window |
| PID reuse / missing creation time | Old instance removed, new history starts; no joined growth |
| Denied handle / failed enumeration | Unknown values, no exact-instance growth |
| Unit mismatch | Native value becomes unknown; schema and DTO reject mismatched wire values; UI draws no misleading path |
| CPU / queue / memory contention | Separate candidates with thresholds, aligned facts and missing-counter uncertainty |
| Cached rows / cadence change | No duplicate process sample; changed cadence starts a new window |
| Churn / expiry / queue bounds | Removed instances do not accumulate; channel history and events are bounded |
| Near-u64-max bytes | Small exact deltas survive detection and plotting |

The first seven detector tests were compiled and run against an empty-history stub:
assertions failed, then passed after implementation. The three new Vitest tests
failed against stub helpers before implementation. Additional lifecycle, bound,
identity, cadence, uncertainty and immutable-sharing tests were demonstrated red
before their fixes or by the single-guard mutations below. The Chromium test was
run after removing the trends component: its panel assertion failed; restoring and
rebuilding made it pass. Compilation failures are never counted as mutation kills.
Two surviving mutations initially showed weak fixtures (nonmonotonic window and
ended process retention); their assertions were strengthened and those mutants
were rerun. No production guard was weakened to satisfy a test.

## Commands and final gate results

All final commands below exited 0 on Windows. The final Rust control set
`$env:RUST_TEST_THREADS = '2'`; it did not overlap a lane-owned browser gate.
Earlier deadline failures and their isolated controls are retained below.

| Command | Final result | Wall seconds |
|---|---|---:|
| `cargo fmt --all --check` | PASS | 2.70 |
| `cargo test --workspace` | PASS: 802 tests, including 17 trend tests and 11 contract conformance tests | 128.20 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS | 6.51 |
| `py -3 scripts/verify.py` | PASS: 227 Python tests run, 226 passed, one symlink-privilege skip; two JS syntax checks, nine JS boundary assertions, 80 parity fixtures; Rust gates passed | 201.56 |
| `npm.cmd --prefix app run check` | PASS: zero errors and warnings | 127.43 |
| `npm.cmd --prefix app run test` | PASS: 33 files, 334 tests | 57.16 |
| `npm.cmd --prefix app run build` | PASS | 39.31 |
| `py -3 scripts/test_app.py` | ALL PASS, including LW-051 synthetic view scenarios and route-polling regression | 121.38 |
| `py -3 scripts/test_app.py --live-serve` | ALL PASS against rebuilt native service | 34.26 |
| `py -3 scripts/gen_contracts_ts.py --check` | PASS: generated TS matches schema | — |
| `git diff --check` | PASS; untracked additions separately checked for whitespace | — |

The app was built before both browser checks. Frontend gates remained applicable
after the backend SSE projection fix; the live-server gate was refreshed after
that fix. Detailed local output and command receipts remain under ignored
`.loomward/lw051/`; no operational output is staged or committed.

Additional focused commands:

```text
cargo test -p loomward-engine --lib event_histories_fit -- --nocapture
py -3 scripts/test_app_timing.py
```

The SSE fixture passed one test: **150,910 serialized event bytes**, with full
snapshot coverage retained. The timing helper suite passed four tests.

## Guard mutation proof

One guard at a time was removed or relaxed, the targeted assertion was required to
fail, then original bytes were restored in `finally`. Rust guards used:

```text
cargo test -p loomward-engine --lib telemetry::trends -- --nocapture
cargo test -p loomward-engine --lib <lifecycle-test-name> -- --nocapture
cargo test -p loomward-protocol --test contract invalid_examples_fail_the_schema_and_the_dto -- --nocapture
npm.cmd --prefix app run test -- resource-trends.test.ts
npm.cmd --prefix app run test -- observatory-feed.test.ts
py -3 .loomward/lw051/wait_polling_regression.py
```

The local orchestration commands included `py -3 .loomward/lw051/projection_mutations.py` and `py -3 .loomward/lw051/pump_mutation.py`. The earlier orchestration commands were `py -3 .loomward/lw051/mutate.py` and
`py -3 .loomward/lw051/mutate_ui.py --ui-only`; local helpers and raw outputs are
ignored operational artifacts. The exact mutations and observed exits follow.
A nonzero exit is counted only with a test assertion failure. Restored source is
then checked by the complete gates above.

**74 of 74 mutations killed** (Rust exit 101; Vitest and the polling regression exit 1). Final UI mutations were rerun serially after overlapping helpers were stopped; event attachment was re-proved after bounding SSE history.

| Guard | Exact removal or relaxation | Exit |
|---|---|---:|
| point-cap | `if points.len() == POINTS {` -> `if false {` | 101 |
| history-age | `at_ms.saturating_sub(p.offset_ms.get()) <= HISTORY_MS` -> `true` | 101 |
| minimum-samples | `MIN_SAMPLES: usize = 4` -> `MIN_SAMPLES: usize = 3` | 101 |
| nonmonotonic-window | `dt == 0 \|\| dt > s.max_gap_ms.get()` -> `dt > s.max_gap_ms.get()` | 101 |
| gap-window | `dt == 0 \|\| dt > s.max_gap_ms.get()` -> `dt == 0` | 101 |
| missing-values | `s.points.iter().map(\|p\| p.value.as_ref()).collect()` -> `Some(s.points.iter().filter_map(\|p\| p.value.as_ref()).collect())` | 101 |
| fresh-process-samples | `raw.processes_observed_at_unix_ms != self.process_at` -> `true` | 101 |
| known-identity | `identity && !denied && raw.process_totals.enumeration_unknowns.is_empty()` -> `!denied && raw.process_totals.enumeration_unknowns.is_empty()` | 101 |
| canonical-identity | `.filter(\|v\| v.len() <= 20 && v.bytes().all(\|c\| c.is_ascii_digit()))` -> removed | 101 |
| denied-handle | `identity && !denied && raw.process_totals.enumeration_unknowns.is_empty()` -> `identity && raw.process_totals.enumeration_unknowns.is_empty()` | 101 |
| enumeration-coverage | `identity && !denied && raw.process_totals.enumeration_unknowns.is_empty()` -> `identity && !denied` | 101 |
| process-instance-retention | `metric.channel() != TelemetryChannel::Processes \|\| keys.contains(key)` -> `true` | 101 |
| missing-provider | `self.record(&key, metric, None, at_ms, &at, interval_ms.max(15_000));` -> removed | 101 |
| disk-churn | `metric.channel() != TelemetryChannel::Disks \|\| keys.contains(key)` -> `true` | 101 |
| duplicate-disk-labels | `number(v.filter(\|_\| unique), metric)` -> `number(v, metric)` | 101 |
| system-channel-admission | `self.enabled(TelemetryChannel::System)` -> `true` | 101 |
| process-channel-admission | `self.enabled(TelemetryChannel::Processes)` -> `true` | 101 |
| disk-channel-admission | `self.enabled(TelemetryChannel::Disks)` -> `true` | 101 |
| fraction-units | `metric.unit() != TrendUnit::Fraction \|\| *v <= 1.0` -> `true` | 101 |
| growth-duration | `if ms < 30_000` -> `if false` | 101 |
| growth-minimum-size | `\|\| delta < (1 << 20) as f64` -> removed | 101 |
| growth-no-decreases | `\|\| v.windows(2).any(\|p\| difference(p[0], p[1]) < 0.0)` -> removed | 101 |
| growth-three-increases | `.count()                     < 3` -> `.count()                     < 0` | 101 |
| cpu-window | `if ms < 3000 \|\| !v.iter().all(\|v\| value(v) >= 0.9)` -> `if !v.iter().all(\|v\| value(v) >= 0.9)` | 101 |
| cpu-threshold | `value(v) >= 0.9` -> `value(v) >= 0.0` | 101 |
| queue-window | `if ms < 3000 \|\| !v.iter().all(\|v\| value(v) >= 2.0)` -> `if !v.iter().all(\|v\| value(v) >= 2.0)` | 101 |
| queue-threshold | `value(v) >= 2.0` -> `value(v) >= 0.0` | 101 |
| memory-window | `if ms < 3000                 \|\| !v` -> `if false                 \|\| !v` | 101 |
| memory-low-available | `value(t) > 0.0 && value(a) / value(t) <= 0.1` -> `true` | 101 |
| memory-movement | `if !movement(TrendMetric::StandbyBytes, false)                 && !movement(TrendMetric::ModifiedBytes, true)` -> `if false` | 101 |
| memory-alignment | `.all(\|(a, b)\| a.offset_ms == b.offset_ms)` -> `.all(\|_\| true)` | 101 |
| missing-movement-unknown | `unknowns.extend(missing);` -> removed | 101 |
| integer-byte-deltas | `(i128::from(b.get()) - i128::from(a.get())) as f64` -> `b.get() as f64 - a.get() as f64` | 101 |
| candidate-cap | `candidates.truncate(64);` -> removed | 101 |
| candidate-truncation-marker | `let candidates_truncated = candidates.len() > 64;` -> `let candidates_truncated = false;` | 101 |
| channel-projection | `.filter(\|s\| channels.contains(&s.metric.channel()))` -> removed | 101 |
| cadence-reset | `if cadence_changed {             points.clear();         }` -> removed | 101 |
| immutable-sharing | `.filter(\|s\| channels.contains(&s.metric.channel()))             .cloned()` -> `.filter(\|s\| channels.contains(&s.metric.channel()))             .map(\|s\| Arc::new((**s).clone()))` | 101 |
| expiry-purge | `self.trends.retain_channels(&channels);` -> removed | 101 |
| sampler-exit-purge | `state.trends = trends::History::default();` -> removed | 101 |
| last-release-purge | `state.expire(Instant::now());         let last` -> `let last` | 101 |
| renewal-channel-purge | `state.expire(Instant::now());         if !state.running` -> `if !state.running` | 101 |
| event-budget | `retained + weight > EVENT_BYTE_BUDGET` -> `false` | 101 |
| oversized-event | `if weight > EVENT_BYTE_BUDGET` -> `if false` | 101 |
| snapshot-history | `sample.trends = state.trends.report(&request.channels);` -> removed | 101 |
| history-ingestion | `if !state.leases.is_empty() {             state.trends.push` -> `if false {             state.trends.push` | 101 |
| last-lease-read-floor | `if !self.leases.values().any(\|lease\| lease.expires > now)` -> `if false` | 101 |
| wire-unit-match | `if v.unit != v.metric.unit()` -> `if false` | 101 |
| wire-byte-value | `Some(TrendValue::Bytes(_)) => v.unit != TrendUnit::Bytes` -> `Some(TrendValue::Bytes(_)) => false` | 101 |
| wire-number-value | `v.unit == TrendUnit::Bytes \|\| (v.unit == TrendUnit::Fraction && n.get() > 1.0)` -> `v.unit == TrendUnit::Fraction && n.get() > 1.0` | 101 |
| wire-fraction-range | `v.unit == TrendUnit::Bytes \|\| (v.unit == TrendUnit::Fraction && n.get() > 1.0)` -> `v.unit == TrendUnit::Bytes` | 101 |
| wire-point-cap | `BoundedVec<TrendPoint, 0, 8>` -> `BoundedVec<TrendPoint, 0, 9>` | 101 |
| process-row-cap | `.take(loomward_telemetry::MAX_PROCESS_ROWS)` -> removed | 101 |
| disk-row-cap | `.take(16)` -> removed | 101 |
| identity-overflow | `.and_then(\|v\| v.parse::<u64>().ok())` -> `.map(\|_\| 0u64)` | 101 |
| ui-unit-match | `if (unit !== series.unit) return [];` -> removed | 1 |
| ui-fraction-range | `(unit !== 'fraction' \|\| v <= 1)` -> `true` | 1 |
| ui-finite-values | `Number.isFinite(v)` -> `true` | 1 |
| ui-nonnegative-values | `v >= 0` -> `true` | 1 |
| ui-null-breaks | `if (v === null \|\| v === undefined)` -> `if (false)` | 1 |
| ui-time-gap-breaks | `dt > series.max_gap_ms` -> `false` | 1 |
| ui-nonmonotonic-breaks | `dt <= 0 \|\| dt > series.max_gap_ms` -> `dt > series.max_gap_ms` | 1 |
| ui-negative-window | `if (duration < 0) return [];` -> removed | 1 |
| ui-byte-precision | `Number(v - base)` -> `Number(v) - Number(base)` | 1 |
| ui-elapsed-spacing | `(p.offset_ms - start) / duration` -> `i / (values.length - 1)` | 1 |
| feed-process-projection | `this.processesAvailable ? ['system', 'gpu', 'disks', 'processes'] : ['system', 'gpu', 'disks']` -> `['system', 'gpu', 'disks']` | 1 |
| browser-wait-route-pump | `page.wait_for_timeout(50)` -> `time.sleep(0.05)` | 1 |
| sse-process-scope | `s.metric.channel() != TelemetryChannel::Processes \|\| keys.contains(s.key.as_str())` -> `true` | 101 |
| sse-candidate-scope | `let new = indices.get(&(c.series_index.get() as usize))?;` -> `let new = indices.get(&(c.series_index.get() as usize)).unwrap_or(&0);` | 101 |
| sse-candidate-index | `c.series_index = Count::saturating(*new as u64);` -> removed | 101 |
| sse-coverage-note | `if series.len() < before {` -> `if false {` | 101 |
| sse-note-cap | `notes.truncate(8);` -> removed | 101 |
| sse-runtime-projection | `trends::event_projection(trends, sample.processes.as_ref())` -> `trends` | 101 |
| event-history | `sample.trends =                                 trends::event_projection(trends, sample.processes.as_ref());` -> removed | 101 |

## Gate failure investigation

A final concurrent workspace/browser pass exposed two existing SSE timing failures:
`followup_finished_sse_restores_the_lifetime_cap` failed at `end.send(()).unwrap()`
with `SendError`, and the browser resume-id assertion timed out before reaching
product-view checks. The HTTP source and its test were unchanged. The isolated
command below passed one test in 0.53 s; the first subsequent full Rust control used
`RUST_TEST_THREADS=1` and no concurrent lane-owned browser gate. The precise cause
of the original HTTP stream closure remains unproven; it is not dismissed as flaky.

```text
cargo test -p loomward-http --test serve followup_finished_sse_restores_the_lifetime_cap -- --exact --nocapture --test-threads=1
```

The browser failure was reproduced in a small routed-fetch probe. `check_resume`
polls a Python predicate; its `time.sleep` loop never dispatches Playwright route
callbacks. With a delayed routed request, nine seconds of polling saw no resume
ID; a Playwright wait immediately dispatched the request and the expected synthetic
ID arrived. The shared helper now pumps Playwright for the same 50 ms polling
interval. No timeout or assertion was relaxed. A new browser regression with a
1.5-second bound failed before this fix, passed afterward, failed again when the
fix was reverted, and passed on restoration. The existing four timing unit checks
were adjusted to mock the dispatching wait rather than Python sleep and passed.
The route regression runs before the LW-051 view scenarios. A subsequent run passed both the resume check and every LW-051 scenario, then caught an encoding regression from a local Windows text rewrite in existing test punctuation. Original Unicode literals in the test and contract semantics were restored from Git UTF-8 bytes; assertions remain unchanged.

The first failed logs are retained locally as `rust1-initial.log` and
`app3-initial.log` under the ignored run folder. This is a bounded correction in
the browser test seam, not a transport or HTTP product change.

The complete browser gates subsequently passed. A later `verify.py` workspace
pass failed two unchanged service deadline assertions: a provider did not start
before its 20 ms call deadline (expected call count 1, observed 0), and a meaning
projection hit its default read deadline. Both isolated controls passed with
unchanged assertions. The exact command filters were:

```text
cargo test -p loomward-service --lib wrong_dataset_and_timed_out_providers_never_publish_fallback -- --nocapture
cargo test -p loomward-service --lib meaning_work_budget_is_an_honest_prefix -- --nocapture
```

The final full control uses `RUST_TEST_THREADS=2`. Scheduling sensitivity is an
inference from these controls; the original deadline failures are retained, not
called flaky. No inventory or learning product/test behavior was changed.

A separate integration defect was found in LW-051: full histories exceeded the
unchanged SSE parser's 1 MiB line limit despite fitting the 8 MiB HTTP response
cap. A new 1,024-row/eight-point fixture failed before the fix. Events now project
process series to the current summary references (at most twenty), reindex
remaining candidates, omit candidates for omitted series and add a coverage note.
Snapshots still contain all 7,183 series in this fixture. The test pins the event
size below the line limit with 4 KiB reserved for envelope framing, retains machine
candidates and checks their new indexes, missing-summary behavior and note bounds.
It passed after the fix, and six projection/runtime bypass mutations failed.
The final release comparison below was remeasured after this transport fix.

## Diagnostic cost and short P12-style comparison

```text
cargo test --release -p loomward-engine --lib diagnostic_micro_measurement -- --nocapture
cargo build --release -p loomward-engine --example telemetry_bench
telemetry_bench.exe <ignored-output.json> 20
py -3 .loomward/lw051/p12_short.py
```

The micro-measurement passed: 16 synthetic iterations, 1,024 process rows,
7,183 series, eight-point histories, **5.427 ms per ingestion plus projection**
in the final release build (the earlier build measured 5.670 ms).
The report serialized to **6,565,659 bytes**, below the existing 8 MiB response
limit. Serialization is measured for size, excluded from the per-sample timer.
The process byte counters in this micro-measurement are steady; it exercises complete mature histories and one disk-queue candidate, not worst-case growth-fact construction. This is a diagnostic measurement on a shared host, not a latency guarantee.

For the lease comparison, a release binary was built from the baseline production
files (saved owned bytes, substituted `git show HEAD:<path>`, built, then restored
in `finally`). Each binary ran 20 seconds idle then 20 seconds with all existing
channels leased at 1 Hz and snapshot/event reads. No other benchmark ran in
parallel. Only the engine's own CPU and internal phase timing aggregates are
published here; no process inventory or machine capacity is included.

| Binary | Idle CPU seconds | Leased CPU seconds | Leased wall seconds | CPU above idle, one core |
|---|---:|---:|---:|---:|
| Baseline | 0 | 0.8125 | 20.0077378 | 4.060928867% |
| Final candidate | 0 | 0.53125 | 20.0032549 | 2.655817779% |

Idle sampling and events were zero for both. No CPU regression was measured at
counter precision in this run; these measurements do not establish an improvement.
Candidate trend ingestion/event projection was **17.0400 ms / 20
ticks = 0.852000 ms per tick**, and snapshot trend projection was **6.7727 ms / 20
reads = 0.338635 ms per read**. These include the native sampler's already-cached
process cadence, unlike the all-rows-fresh synthetic micro-measurement. A 20-second leased phase is shorter than the 45 seconds needed for four fresh process observations at that cadence, so native mature growth-window overhead remains unverified by this comparison.

The first comparison with copied histories measured 0.765625 baseline versus
0.8125 candidate CPU seconds (3.82799% versus 4.05861% above idle). That observation
prompted immutable storage sharing. Its one short re-measure observed equal CPU
seconds (0.4375 each; about 2.187% of one core). The final comparison was repeated
only because the subsequently found SSE size defect changed production projection
and timing. The substantial changes in the baseline illustrate shared-host
uncertainty. No comparison met the P12 <=1% target. The final short engine-only
result is **not** a P12 pass or proof of steady-state production overhead.

Executable SHA-256:

- Baseline: `2b7af3a31865da541ea9d95cf7cd476f922173611aa4db4d8436bebfdea9ae76`
- Final candidate: `4784ca45be0cac107f93f9b00d68e42b963ce47f25ea9d472bd92693770caa37`

## NOT verified and residual risk

- Full 600-second P12, P13 soak, steady-state overhead, complete HTTP/event
  serialization plus browser cost, and statistical significance were not measured.
- Real leak diagnosis, workload intent, unique shared resident memory, denied
  counters and observations outside the bounded window remain unknown by design.
- Native provider accuracy during real PID/device churn, additional hardware,
  Linux, hosted CI and the Tauri/WebView2 shell were not exercised in this lane.
- Candidate thresholds are conservative heuristics. Short bursts, decreases or
  missing samples can suppress a candidate; no candidate does not prove health.
- Disk labels/order are observational keys, not durable identity. The explanations
  retain that limitation. History resets when cadence changes rather than inferring
  continuity from incomparable sampling windows.
- No release/package, effect capability or long host benchmark was attempted.
  A new crate was declined because lease/history logic belongs with existing
  telemetry ownership. Commit and push are reserved to the driver.
