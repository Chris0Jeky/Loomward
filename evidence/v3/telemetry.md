# Native telemetry lane: Windows measurement and proof

2026-10-09, Windows 11, rustc/cargo 1.97.1, non-elevated token (administrator
membership check returned false). Lane `feat/telemetry`, base/unchanged HEAD
`42e34e749059a93637d8f43af10dad134bc61d5f`; implementation is uncommitted for
driver integration. Commands ran at the lane worktree root. Other crates were
not edited. Schema and limitations: [crate README](../../crates/loomward-telemetry/README.md).

## One real snapshot, aggregates only

`cargo build -p loomward-telemetry --release`, then
`target/release/loomward-telemetry.exe snapshot --json`. Raw JSON was consumed
in memory and not saved; this curated receipt contains no process names, PIDs,
executable paths or disk-instance labels. Values are one observation, not fixtures.

| Measurement | Observed value |
|---|---:|
| Physical total | 34,064,613,376 bytes |
| Physical available (including reclaimable pages) | 11,142,094,848 bytes |
| System commit charge / limit / peak | 33,219,547,136 / 102,784,090,112 / 42,685,349,888 bytes |
| System cache | 12,236,759,040 bytes |
| Kernel paged / nonpaged | 3,311,017,984 / 2,066,493,440 bytes |
| Page size | 4,096 bytes |
| Active logical processors | 20 |
| Enumerated / opened processes | 559 / 388 |
| Access denied / partial or unknown entries | 169 / 171 |
| Observed working sets / private commits | 388 / 388 |
| Observed working-set sum (shared pages can repeat) | 18,778,861,568 bytes |
| Observed private-commit sum | 19,415,982,080 bytes |
| Displayed rows / enumeration truncation | 200 / false |
| Standalone snapshot cost, including PDH initialization | 802.90 ms |

The root status was partial: normal permission gaps are visible. All nine memory
fields were observed. CPU/I/O rates and disk throughput were unknown on the first
sample. GPU adapter and process telemetry reported unsupported with reasons.

A separate optimized `watch --interval-ms 1000 --count 3 --json` run cost
**387.65, 20.76, 23.97 ms** per snapshot. Actual process windows were null,
1.0001633 and 1.0004729 seconds. Three physical-disk instances supplied valid
rates in the second and third samples; their last PDH window was 1.0038639
seconds. No disk labels or per-process values are retained here.

Cost includes enumeration, queries, rate calculation, retained previous data and
top-N projection; it excludes JSON encoding, output, process startup and compiler
time. First-call PDH initialization dominates cold cost. This is a small run on
an active workstation, not a percentile benchmark or an overhead guarantee.

## Test-first receipt and proving checks

Baseline `cargo test --workspace`: 19 existing Windows tests passed. Before native
implementation, `cargo test -p loomward-telemetry` had 2 passes and 3 intended
failures (Windows observation, watch NDJSON and invalid CLI rejection). The three
synthetic rate tests also failed before rate implementation. They now pass.

Final commands and tails:

```text
cargo fmt --all --check
exit 0

cargo test --workspace
loomward-core:      test result: ok. 19 passed; 0 failed
telemetry unit:     test result: ok.  4 passed; 0 failed
telemetry native:   test result: ok.  6 passed; 0 failed
29 Windows tests total; all doc tests passed

cargo clippy --workspace --all-targets -- -D warnings
Finished dev profile [unoptimized + debuginfo]; exit 0

cargo check --workspace --all-targets --target x86_64-linux-android
Finished dev profile [unoptimized + debuginfo]; exit 0
```

The ten telemetry tests cover exact known rates, signed deltas, PID reuse,
missing identity, invalid windows, regressed/missing counters, a real short-lived
child after exit (even with a retained handle), bounded display/totals, parsed
snapshot JSON, NDJSON watch, invalid CLI options, current-process nonzero working
set, denied system-process unknowns, nonnegative two-sample rates, PDH structural
truths and retention of counters for undisplayed processes. The native test child
only lists its own test names and exits; no process is terminated or modified.

## Unverified and remaining limits

Ubuntu execution/hosted CI was not run: the installed Ubuntu has no Rust
toolchain; no toolchain was installed. The existing Android/Linux target proves
non-Windows conditional compilation of all workspace targets, not Ubuntu runtime
behavior. EX fallback on an older Windows build, unavailable PDH/elevation-denied
counter providers, more than 8,192 processes, multi-processor-group machines and
long-run overhead were not measured. Browser/Tauri/Python integration was not
changed or tested by this native-only lane.

GPU budgets are unsupported because windows-sys 0.61 has no DXGI adapter COM
bindings. Optional per-process GPU counters were deferred rather than interpreting
PID-only instances as stable identities. Physical-disk instances can span volumes:
isolated per-volume I/O is unknown. Neither process sums nor cache/kernel counters
are a disjoint physical-memory accounting. Samples are sequential observations,
not an atomic machine image or permission to act.

`HUMAN_TODO.md` q-1 through q-4 remain open and unchanged; no owner confirmation
was inferred. LW-051 is groundwork only; trends UI and diagnosis are not delivered.
No process effects, elevation, commits, pushes, branches, PRs or releases occurred.

## Revision 2 / lane L11a (10 October 2026)

Windows 11 build 26300, rustc/cargo 1.97.1; administrator membership check false.
Branch `feat/telemetry-v2`, base/unchanged HEAD `dcbd1c2`. This is local lane proof
for driver integration, not a merged-state claim. Native source follows
[architecture section 11](../../docs/41-v03-architecture.md) and the telemetry
schema comments in revision 2. The previous section is historical evidence;
its GPU-unsupported description and q-1..q-4 status do not describe this revision.

Changed: schema version 2 uses commit/working-set quantities, machine-normalised
CPU fractions, memory-list counters, separate disk busy/queue values and global
PDH GPU counters. English wildcard paths are localized and re-expanded every
sample with `PDH_REFRESHCOUNTERS`; each value's status and bounded name buffer are
validated. Malformed disk instances are skipped and counted without discarding
valid peers. GPU utilisation sums by LUID and engine type, then takes the maximum
and clamps to one. Shared and dedicated memory remain separate.

### One real snapshot, aggregates only

`cargo build -p loomward-telemetry --release`, then
`target/release/loomward-telemetry.exe snapshot --json`; sample start `2026-10-10T09:33:19.490000+00:00`.
Raw JSON was parsed in memory, never saved. A curated aggregate-only receipt is
in gitignored `.loomward/telemetry-v2-summary.json`; this public evidence contains
no process names, PIDs, paths, adapter LUIDs or disk labels. Labels A/B/C below are
anonymous within this receipt.

| Measurement | Observed value |
|---|---:|
| Physical total / available | 34,064,613,376 / 3,189,833,728 bytes |
| System commit / limit / peak | 41,003,528,192 / 102,784,090,112 / 102,784,090,112 bytes |
| Standby / modified / free and zero | 2,158,632,960 / 806,461,440 / 1,167,319,040 bytes |
| Physical load fraction | 0.9 |
| System cache / kernel paged / nonpaged | 3,263,246,336 / 4,684,939,264 / 2,207,682,560 bytes |
| Page size / active logical CPUs | 4,096 bytes / 20 |
| Enumerated / opened / access denied / partial processes | 591 / 419 / 171 / 172 |
| Working sets / private commits observed | 419 / 419 |
| Observed working-set / private-commit sums | 25,626,333,184 / 25,604,018,176 bytes |
| Display / enumeration truncated | 200 / false |
| Memory-list unknowns / malformed PDH occurrences | 0 / 0 |
| Standalone snapshot cost | 414.79 ms |

The root was partial because inaccessible processes and first-sample rates remain
unknown. All memory-list components were observed. CPU, disk rates/busy/queue and
GPU utilisation require two samples; their first values were null with reasons.
GPU global memory gauges were already observed on the standalone sample:

| Anonymous adapter | Dedicated used bytes | Shared used bytes | Utilisation |
|---|---:|---:|---|
| A | 2,965,872,640 | 1,431,605,248 | unknown: requires two samples |
| B | 0 | 8,192 | unknown: requires two samples |
| C | 0 | 414,687,232 | unknown: requires two samples |

A separate release `watch --interval-ms 1000 --count 3 --json` cost
**397.77, 32.99, 27.10 ms**. Process sample intervals
were null, 1.0004286 and 1.000308 seconds.
On the last sample, 3 physical disks had valid separate read/write,
busy and queue counters; aggregate throughput was 20323727.45
read bytes/s and 160840484.74 write bytes/s. Busy fractions
ranged 0.000000..1.000000; queue lengths
ranged 0.000000..16.431141 outstanding requests.
System CPU busy fraction was 0.618971.

| Anonymous adapter (last watch sample) | Dedicated used bytes | Shared used bytes | Engine types | Max aggregated engine fraction |
|---|---:|---:|---:|---|
| A | 2,977,406,976 | 1,431,605,248 | 7 | 0.026077 |
| B | 0 | 8,192 | 1 | 0.000000 |
| C | 0 | 414,687,232 | 6 | unknown: provider returned an unnamed engine type |

The unnamed engine type is represented by null plus `engine_type_unavailable`;
its adapter's maximum stays unknown, and other adapters remain observed. No
malformed PDH instances occurred in this final run. Dedicated capacities are null
with `capacity_not_provided_by_gpu_usage_counters`; optional process GPU memory
remains unsupported with a reason. Adapter bytes are global usage counters,
never per-process sums or a dedicated+shared total.

Cost includes PDH initialization/re-expansion, enumeration, queries, rate maths,
retained copy and projection. It excludes JSON, output, startup and compilation.
These are four observations on an active workstation with uncontrolled caches,
not cold-cache, percentile, P12 overhead or P13 soak proof. Memory-list and
GlobalMemoryStatusEx observations are sequential and need not form an exact sum.

### Test-first and proving receipt

Baseline: 10 telemetry tests passed. Changing the worked rate oracle to expect
0.125 of machine capacity failed against the old 12.5-percent implementation.
Reinstating the old abort-on-malformed-name behavior also failed the new parser
regression; the mutation was restored before final verification.

Final local Windows commands:

```text
cargo fmt --all --check                         exit 0
cargo test --workspace                          82 passed, 0 failed; doc tests passed
cargo clippy --workspace --all-targets -- -D warnings   exit 0
cargo build -p loomward-telemetry --release      exit 0
```

Telemetry contributes 15 unit tests and 6 native/CLI tests (21 total). New cases
pin machine CPU normalization/capacity, synthetic PDH buffers with null/outside/
header/unterminated/invalid-UTF-16 names, per-value status, valid zeros, exact
64-bit bytes, disk busy vs queue, malformed-name isolation/counting, localized
flexible-array layout, fresh wildcard expansions, LUID joins, engine-type sums
then maximum/clamping, invalid/incomplete GPU aggregates, unnamed engine types,
and adapter churn. Native assertions accept either observed values or explicit
unknown reasons independently of token privilege and available GPU hardware.

### NOT verified / residual risk

No Ubuntu execution, hosted CI, non-English Windows host, unavailable-provider
host, older Windows EX fallback, multi-processor-group hardware, buffer-budget
stress, real adapter hotplug or long-running telemetry soak was measured.
Synthetic parsers cover corruption/churn but do not substitute for physical
hotplug. CPU/process/PDH queries remain sequential observations. The standalone
JSON schema change requires consumers to adopt version 2. Engine leases, pools,
view-service byte-string/identity projection and UI integration remain L11b work.
No file/process effects, privilege change, commit, push, PR or release occurred.

`HUMAN_TODO.md` was read and left unchanged: q-1..q-4 and q-7 are closed;
q-5 remains open for personal teacher disclosure after confinement proof, and
q-6 is not yet needed. This read-only lane adds no owner action.

Final-source smoke receipt (after adding the explicit unnamed-type field reason
and unscaled/uncapped PDH double formatting): release rebuilt successfully;
standalone snapshot **655.92 ms**, three-sample watch
**445.37, 36.83, 28.41 ms**. Memory lists were
2,046,382,080 standby / 136,511,488
modified / 2,022,612,992 free bytes, with no unknowns.
3 GPU adapters and 3 disks were observed; the unnamed type
remained an explicit per-adapter unknown. Formatting uses `PDH_FMT_NOSCALE` and
`PDH_FMT_NOCAP100`, so queue lengths above 100 and byte/rate units are retained.
All three required workspace gates passed again on this final source. The
aggregate-only `.loomward/telemetry-v2-final-summary.json` holds the receipt and
SHA-256 of the four source/test files; no raw identities are saved. Source hashes:

- `crates/loomward-telemetry/src/lib.rs`: `2c32fa30d8b7ce839d5ec3fd62ec378e095e218282d9b7fed5d4cbae2a47eba8`
- `crates/loomward-telemetry/src/windows.rs`: `0450ee027596e669e5231ba3530eec40b01f470ba46d85c480efa74c951a00ce`
- `crates/loomward-telemetry/src/windows/pdh.rs`: `0ca7bf16378fd09da5e01ab5a8abe196d66c14396cecb13e829db36787a6a65f`
- `crates/loomward-telemetry/tests/native.rs`: `b6e412632635ac53c6112f1fe4c9b4f67e09d0cdb254ec4c5e288c5eca6a693c`
