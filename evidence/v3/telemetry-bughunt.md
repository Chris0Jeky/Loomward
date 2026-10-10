# Telemetry bug-hunt verification

10 October 2026. Branch `fix/telemetry-bughunt`, tested HEAD
`cf27a4e` with the lane diff, measured before it was committed. Platform: Windows.
All new regression inputs are synthetic. No benchmark was performed.
This receipt supplements the historical [engine telemetry evidence](telemetry-engine.md).

## Changed

| Finding | Confirmation and repair | Source |
|---|---|---|
| 1: renewal drops queued samples | Confirmed: an unchanged renewal left zero queued events rather than one. Preserve generation, queued and in-flight events when the channel set is unchanged, including reordered channels and interval changes. Preserve the next deadline on keep-alive renewal; a channel change still invalidates the previous projection. | `crates/loomward-engine/src/telemetry/mod.rs:149` |
| 2: malformed GPU instance erases valid adapters | Confirmed: a malformed instance changed a valid adapter's busy fraction from `Some(0.1)` to `None`. Skip unidentifiable instances, count/report them and keep identified values under partial native coverage. Query-wide failures and identifiable invalid/churned instances still make the affected aggregates unknown. | `crates/loomward-telemetry/src/windows/pdh.rs:654` |
| 3: denial totals disagree with rows | Confirmed: a failed field query with access denial produced a Limited row. Native aggregation and row mapping now share `Process::access_denied`, covering any denied field. | `crates/loomward-telemetry/src/lib.rs:100`, `crates/loomward-engine/src/telemetry/mapping.rs:72` |
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

Each new test failed for its intended behavior with its fix removed, and passed after restoration.
The mutation runner restored the changed source bytes exactly, including on failure.

| Regression | Mutation and red result |
|---|---|
| `unchanged_channel_renewal_preserves_queued_samples_and_generation` | Restore original renewal block: queued count 0 instead of 1. |
| `unchanged_renewal_during_mapping_preserves_the_pending_sample` | Restore original renewal block: pending event discarded, count 0 instead of 1. |
| `malformed_gpu_instances_preserve_identified_adapters_with_partial_coverage` | Restore original `gpu_rows`: valid busy fraction becomes `None`. Covers two adapters, all three arrays and both rejected names and parser-level malformed instances. |
| `partial_access_denial_matches_process_totals_and_rows` | Restore original row mapping: Limited instead of Denied for a field query. |
| `expired_leases_make_all_retained_reads_unavailable_before_worker_wakes` | Remove read-side expiry: all three endpoints serve retained data and keep it. |
| `process_sum_overflow_is_a_visible_lower_bound_and_does_not_hide_rows` | Independently remove working-set and private-commit checked addition: each mutation panics on overflow. Also checks normal sums, deduplicated reasons, counts and legacy deserialization. |
| `overflowed_sums_mark_partial_coverage_without_hiding_processes` | Remove overflow coverage marker: status remains observed rather than partial. |

Seven new tests; eight discriminating mutations. The existing changed-channel mapping race,
per-adapter unknown/churn, observation-time, native lease and cadence tests remain green.

| Required gate | Result |
|---|---|
| `cargo fmt --all --check` | PASS |
| `cargo test --workspace` | PASS: 434 passed, 0 failed, 0 ignored |
| `cargo test -p loomward-telemetry -p loomward-engine --all-features` | PASS: 145 passed, 0 failed, 0 ignored |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS: no warnings |
| `git diff --check` | PASS |

Tested `git diff -- crates` SHA-256:
`f186d2e5c15a2c5754757a4e40882aaa6e1825147b7fade98c28642c8f5a2782`.
Raw check logs and reversible mutation helpers remain in gitignored local state; they are not
publication artifacts.

## Questions and declined changes

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

Unidentifiable GPU instances cannot be attributed to an adapter: retained numbers describe identified
instances with partial native coverage, not a complete adapter inventory. The existing v3 projection
has no per-field PDH coverage/reason channel; this lane adds none. Process rows can still describe
an exited process until the next enumeration, carrying their original observation time.
Consumers of native totals must read `sum_unknowns` before treating a saturated sum as exact.

- `Preserve telemetry samples across unchanged lease renewals`
- `Keep valid GPU observations when malformed instances appear`
- `Align process denial rows with native totals`
- `Reject telemetry reads after the last lease expires`
- `Check process byte sums and report overflow coverage`
