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
