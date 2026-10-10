# Telemetry bug-hunt verification

10 October 2026. Branch `fix/telemetry-bughunt`, review fixes tested over HEAD
`a89962c` with the uncommitted lane diff. Platform: Windows. The original lane
receipt was measured over `cf27a4e` before its commit; this update records the final
behaviour after review of PR #237.
All new regression inputs are synthetic. No benchmark was performed.
This receipt supplements the historical [engine telemetry evidence](telemetry-engine.md).

## Changed

| Finding | Confirmation and repair | Source |
|---|---|---|
| 1: renewal drops queued samples | Confirmed: an unchanged renewal left zero queued events rather than one. Preserve generation, queued and in-flight events when the channel set is unchanged, including reordered channels and interval changes. Preserve the next deadline on keep-alive renewal; a channel change still invalidates the previous projection. | `crates/loomward-engine/src/telemetry/mod.rs:149` |
| 2: malformed GPU engine load appears exact | Review confirmed that retaining `Some(0.1)` while an unidentifiable engine carries 80% load understates utilisation without a wire-level partial marker. Any parser-malformed or unidentifiable engine instance now nulls every adapter's aggregate and per-type engine busy values. Keep identified adapters and their attributable memory values, malformed counts and partial native status. Identifiable invalid/churned instances still affect only their type and adapter. | `crates/loomward-telemetry/src/windows/pdh.rs:654` |
| 3: denial totals disagree with rows | Review narrowed the shared `Process::access_denied` rule to access denial on `process_handle_fields`, where opening the handle failed. Only these rows are Denied and counted in denial totals. A denied individual field remains Limited, retaining available memory and CPU figures. | `crates/loomward-telemetry/src/lib.rs:100`, `crates/loomward-engine/src/telemetry/mapping.rs:78` |
| Review LOW: unbounded malformed-instance reasons | Retain every malformed-instance count but emit at most one `pdh_unknowns` entry per GPU field per snapshot, including mixed parser/name failures. Remove duplicate query-error entries already recorded by `record_array`. | `crates/loomward-telemetry/src/windows/pdh.rs:667` |
| 4: reads race with lease expiry | Confirmed independently for snapshot, list and explanation: all returned success with only expired leases. Their shared accessor now checks for a live lease, expires state and clears the retained sample before returning `partial_coverage`. Normal reads do not traverse the pending-event queue. | `crates/loomward-engine/src/telemetry/mod.rs:66` |
| Process byte sums | Synthetic inputs `u64::MAX`, 1 and 2 reproduced unchecked-addition overflow. Check both sums, saturate as labelled lower bounds and keep row/count coverage. Mark both fresh and cached native snapshots partial when sums have overflow reasons. | `crates/loomward-telemetry/src/lib.rs:127`, `crates/loomward-telemetry/src/windows.rs:358` |

The expiry repair follows the existing no-lease/no-retained-sample design. No stale payload is served
after the last lease expires; observation timestamps while leased remain unchanged. There is no
new stopped-sample DTO or change to `contracts/v3`.

The native snapshot format gains only the additive, default-empty `ProcessTotals.sum_unknowns`
array. On overflow, its field-specific reason says the saturated byte value is a lower bound,
not an exact total. A legacy totals object without the array still deserializes. The
[native field reference](../../crates/loomward-telemetry/README.md) documents this addition.

## Verified

All five added or revised regression tests failed against the pre-fix behaviour and passed
after the fixes. Eight targeted mutations then removed coverage, denial or deduplication guards; each failed by
assertion with exit 101. The runner restored changed source bytes exactly in a `finally` block.

| Regression | Mutation and red result |
|---|---|
| `malformed_gpu_instances_preserve_identified_adapters_with_partial_coverage` | Remove parser-level malformed coverage: an adapter busy fraction stays populated. |
| Same two-adapter test | Remove unidentified-name coverage: an adapter busy fraction stays populated. |
| `malformed_gpu_instance_cannot_become_a_zero_or_partial_total` | Remove both coverage flags: the restored main assertion fails because aggregate busy remains populated. Identified dedicated/shared memory assertions remain. |
| Same two-adapter test | Remove per-type coverage guard: aggregate busy is null but per-type busy remains populated. |
| `handle_denial_is_denied_and_field_denial_is_limited` | Restore broad denial rule: a readable row with a denied field is Denied instead of Limited. Includes a no-handle Denied control. |
| `process_totals_count_only_handle_access_denials` | Restore broad denial rule: field denial increments totals to 1 instead of 0. The no-handle control increments to 1; observed memory sums remain available. |
| `malformed_gpu_unknowns_are_bounded_per_field_per_snapshot` | Remove engine-field deduplication: 34 entries instead of 3. |
| Same bounded-unknowns test | Remove memory-field deduplication: 65 entries instead of 3. Covers 96 malformed rows across three fields, with and without additional parser-level malformed counts. |

Two added tests and three revised tests in this review-fix pass. The original lane also
mutation-tested renewal, pending mapping, read-side lease expiry and both overflow paths;
those implementations are unchanged. The full applicable suites recheck them along with
per-adapter unknown/churn, observation-time, native lease and cadence behaviour.

| Required gate | Result |
|---|---|
| `cargo fmt --all --check` | PASS |
| `cargo test --workspace` | PASS: 456 passed, 0 failed, 0 ignored |
| `cargo test -p loomward-telemetry -p loomward-engine --all-features` | PASS on restored-source retry: 149 passed, 0 failed, 0 ignored; initial scan/watch assertion failure retained below |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS: no warnings |
| `git diff --check` | PASS |

Tested `git diff -- crates` SHA-256:
`b7e35edb5905e56a09f95da33dd78ff541157d7e1985aaac4c0ff75557e2ff85`.
Raw check logs and reversible mutation helpers remain in gitignored local state; they are not
publication artifacts.

The first all-features run failed in the unchanged
`scan::watch::native_tests::concurrent_scan_start_reserves_root_before_touching_watch`:
the cancellation result had length 0 rather than 1. The test passed in isolation;
the full all-features suite passed both on committed HEAD and with the review fixes
restored. This establishes an intermittent result, not its scheduling cause or a
repair. No scan/watch code or test was changed, and the initial failure log is retained.
This is the #228 race (cancellation stopped the watch before collecting jobs), fixed on main by #233;
this branch was measured on a base without that fix.

## Questions and declined changes

- **Scan/watch cancellation follow-up:** declined in this telemetry slice because the
  failing assertion is outside the changed seam. The driver should investigate the
  cancellation-result failure separately; its cause remains unverified.
- **First status of a newly appeared PDH rate instance:** Microsoft defines `PDH_CSTATUS_NEW_DATA`
  as valid changed raw data, including on rate counters; it does not mean a newly appeared
  instance or an invalid first sample. [Counter status documentation](https://learn.microsoft.com/en-us/windows/win32/perfctrs/checking-pdh-interface-return-values).
  Its [formatted-array example](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhgetformattedcounterarrayw)
  collects twice for counters requiring two samples. These documents do not establish the exact
  first-appearance CStatus for disk/GPU providers between wildcard audits. Existing code decodes
  every returned value's status and requests an immediate audit on an invalid value; the precise
  provider churn case remains unverified. Declined rejecting NEW_DATA or increasing audit frequency
  without evidence: the former rejects valid rates and the latter increases P12 work.
- **GetPerformanceInfo products:** refuted as an unchecked-overflow defect. All six page-count
  products already use `checked_mul` in `windows.rs:144-149`; an out-of-range product becomes
  `None`. No extra conversion helper or change to that path is needed.
- **Process sums:** no code-enforced bound on the observed byte values prevents arithmetic
  overflow; synthetic inputs demonstrate it. This does not establish that the native APIs return
  such values on supported hardware. Checked accumulation now handles them honestly.

## NOT verified

No hosted CI, Linux, browser/native desktop acceptance, provider-specific GPU/disk churn
experiment, or P12 measurement. The source keeps the fifteen-second process cadence, native call
count, ten-sample wildcard audits, sampling intervals and worker count unchanged. Keeping those
cost drivers unchanged is source evidence, not measured proof of flat overhead. The recorded
P12 result remains the historical 1.005203% MISS against the <=1% target in issue #160.

## Residual risk

Unidentifiable GPU engine instances cannot be attributed to an adapter: all engine-derived
values remain unknown across adapters, while identified memory values remain attributable.
The existing v3 projection has no per-field PDH coverage/reason channel; this lane adds none.
Native status remains partial and adapter inventory may be incomplete. Process rows can still describe
an exited process until the next enumeration, carrying their original observation time.
Consumers of native totals must read `sum_unknowns` before treating a saturated sum as exact.
The intermittent scan/watch cancellation assertion remains unresolved despite the passing retry.
