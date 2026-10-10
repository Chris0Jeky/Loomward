# Duplicate reader bug-hunt correction

2026-10-10. Windows Rust target `x86_64-pc-windows-msvc`.
Branch `fix/duplicate-reader-bughunt`, base/HEAD before edits
`0938e5600662f682ae32f1240d06e2724e24c805` (includes #234 and #244).
All fixture names and content are synthetic; native files are disposable.
No personal inspection, benchmark, commit or push was performed.

## Changed

Both hypotheses were confirmed by assertion failures against the original implementation.

1. Comparison errors now carry the failed input index using the existing `Skip` type. Native
   opens, reads and post-read checks attribute failure to their operand. Grouping skips that
   entry, removes a failed representative, and retries the innocent candidate against a
   surviving representative or starts a new partition. Each skip makes coverage partial.
2. The final deadline/cancellation check retains groups already byte-compared and finally
   revalidated, sets partial coverage and records the stop reason. Later unexamined inputs
   remain unknown; final metadata validation still removes changed members.

Engine revocation behavior is unchanged: `scan_cancel_root` sets the shared cancellation flag
and revoked fence; `inspect_duplicates` checks both at its final gate and returns a permission
error instead of any report. This composition was checked by reading the unchanged executable
paths and running the existing grant/cancellation tests. It was not tested with a real in-flight
engine inspection racing revocation.

Included both requested LOW items from [#247](https://github.com/Chris0Jeky/Loomward/issues/247):
remove the unused `Reason::ContentReadGated` (the engine still emits `content_read_gated`), and
clarify the engine doc comment and hardening evidence: revocation requests cancellation and can
return before I/O drains; the root gate is not held across content I/O.

## Verified: regressions and mutations

The original reader run had five assertion failures: changed representative, sharing-failed
representative, final cancellation, final deadline, and preservation of an earlier native group
when the next comparison stops. Seven new tests and one strengthened existing test now cover:

| Test | Result and mutation that makes it fail |
| --- | --- |
| `native_changed_representative_keeps_remaining_equal_pair` | Retains `[1,2]`, skips changed entry 0 only; restore candidate-blaming/no retry |
| `native_sharing_failed_representative_keeps_remaining_equal_pair` | Real exclusive handle blocks representative; retains `[1,2]`, skips entry 0 only; restore candidate-blaming/no retry |
| `native_changed_candidate_keeps_representative` | Retains `[0,2]`, skips changed candidate 1 only; attribute candidate errors to representative |
| `native_compare_finish_attributes_failure_to_its_operand` | Real metadata changes during comparison exercise post-read checks on both operands, preserving the healthy pair; restore candidate-blaming/no retry |
| `native_final_validation_drops_changed_member` | Previously compared member changes while another pair is compared; removes only the changed member; disable final member validation |
| `core_final_cancel_keeps_verified_groups` | Final stop keeps `[0,1]`, partial/cancelled, no spurious entry skip; restore final group clearing |
| `core_final_deadline_keeps_verified_groups` | Final stop keeps `[0,1]`, partial/time-budget, no spurious entry skip; restore final group clearing |
| `native_stop_keeps_earlier_verified_group_and_later_input_unknown` | Cancellation and deadline during the next size group's comparison keep earlier `[0,1]`, partial/stop, 48 bytes; restore final group clearing |

Four distinct mutations were killed with assertion failures (Cargo exit 101), not build/setup
failures. Every new test was demonstrated to fail. Both mutated source files were restored
byte-for-byte after each run. Focused restored run: `cargo test -p loomward-windows duplicate_reader`,
28 passed, 0 failed, 0 ignored. Existing collision partitioning, byte-budget, elevation, sharing,
junction and pending-I/O cancellation/drain controls remain in that suite.

## NOT verified

No hosted CI, independent review, non-Windows runtime, real in-flight engine revocation race,
stalled filesystem driver or real cloud placeholder was exercised. Python, app and native-shell
gates were not run because those surfaces are unchanged. Native open and post-read failures were
injected through disposable filesystem fixtures; a physical device read error was not induced.

Other #247 items are deferred: feature-convention guard expansion, grant-row/root-id wiring,
the real in-flight engine revocation test, and admission lock duration are separate concerns.

The workspace gate retains one pre-existing ignored test,
`verbatim_device_namespace_state_paths_are_refused`, for the device-namespace defect tracked in
#246. Its ignore annotation is present at the starting HEAD and unchanged by this lane.

## Verified: final gates

| Command | Output |
| --- | --- |
| `cargo fmt --all --check` | PASS, exit 0 |
| `cargo test --workspace` | PASS, exit 0; 562 passed, 0 failed, 1 pre-existing ignored |
| `cargo test -p loomward-windows -p loomward-engine --all-features` | PASS, exit 0; 193 passed, 0 failed, 0 ignored (engine 116 unit + 9 integration, Windows reader crate 68) |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, exit 0 |
| `git diff --check` | PASS, exit 0; checked again after completing this receipt |

Local full gate output is retained under gitignored `.loomward/duplicate-bughunt/gate-0.txt`
through `gate-4.txt`. It is not public evidence or a commit input.

## Changed locations and recommended commits

| Item | Source |
| --- | --- |
| Operand-attributed errors | `crates/loomward-windows/src/duplicate_reader/native.rs:312`, `crates/loomward-windows/src/duplicate_reader.rs:209` |
| Representative removal/retry | `crates/loomward-windows/src/duplicate_reader.rs:319` |
| Final stop retains verified groups | `crates/loomward-windows/src/duplicate_reader.rs:367` |
| Regression fixtures | `crates/loomward-windows/src/duplicate_reader/tests.rs:252`, `:384` |
| Dead reason removed | `crates/loomward-windows/src/duplicate_reader.rs:30` |
| Revocation wording | `crates/loomward-engine/src/scan/mod.rs:123`, `evidence/v3/duplicate-reader-hardening-238.md:21` |

Suggested commit messages if the driver splits the fixes:
`Retry duplicate comparisons after representative failures`;
`Preserve verified duplicate groups on reader stops`.
For one combined commit: `Fix duplicate comparison attribution and partial evidence retention`.

## Residual risk

Equality remains point-in-time default-stream evidence, not an atomic snapshot or effect authority.
Metadata operations and cancellation draining still depend on the filesystem driver returning.

## Addendum: failed representatives invalidate their earlier witness role

2026-10-10, follow-up to the confirmed HIGH in the post-merge review of #261.
Starting HEAD `a8d2fbe73030c3a8ee27665511469991ac7c709c`, branch
`fix/duplicate-partition-reverify`. The grouping source at that HEAD is byte-identical to the
`origin/main` snapshot `c662b13e20dfb2ac3acc03d7a996e88ee6ad5df4` used for regression proof.
Windows Rust target `x86_64-pc-windows-msvc`; synthetic inputs and disposable native fixtures only.

### Changed

Removing a failed representative alone was insufficient: separate `equal(R, X)` and
`equal(R, Y)` calls could witness different versions of R. X and Y then survived as a group
without a direct comparison. Metadata validation did not repair that content-equality gap.

At `crates/loomward-windows/src/duplicate_reader.rs:322`, the existing partition loop now uses
a pending queue. A non-budget representative failure removes the entire partition, skips the
failed entry with its original reason, and queues every survivor before the innocent candidate.
Each survivor is placed through exact comparisons again. Further representative failures repeat
this process without recursive calls. All comparisons use the existing shared meter and native
reader; hashes still establish no equality. A budget stop leaves unmatched survivors unexamined
with partial coverage and a stop reason, rather than emitting the former partition.

At `crates/loomward-windows/src/duplicate_reader.rs:357`, a representative that fails final
metadata validation also invalidates its partition. Its surviving members return to exact
comparison. Other finally validated groups are emitted immediately and retained on a later stop.
The existing final cancellation/deadline and native late-stop preservation tests remain green.

### Verified: regression and mutation proof

The original grouping implementation was restored with all new tests present. All six new tests
failed through assertions, not compilation or fixture setup. The fixed source was then restored
byte-for-byte. These tests live in `crates/loomward-windows/src/duplicate_reader/tests.rs`:

| Test and line | Proof |
| --- | --- |
| `core_failed_representative_reverifies_different_survivors` (:293) | Four inputs model R witnessing A then B; X/Y are directly compared and no group is emitted |
| `core_failed_representative_reverifies_equal_survivors` (:308) | Equal X/Y remain grouped only after direct comparison; all comparison calls and 48 total bytes are asserted |
| `core_failed_representative_reverification_budget_is_partial` (:317) | Budget stops the X/Y re-comparison; no group, partial coverage, byte-budget stop, 32 consumed bytes |
| `core_failed_representative_reverification_handles_another_failure` (:328) | Six inputs build a second suspect partition; its failed witness is skipped and its survivors are compared directly |
| `core_final_failed_representative_reverifies_survivors` (:342) | A final representative validation failure cannot leave unequal survivors grouped |
| `native_failed_representative_reverification_uses_shared_budget` (:616) | Real disposable files exercise a four-input partition and changed representative: 64 bytes with sufficient budget; 48 bytes and no group when re-verification exhausts the budget |

Three distinct mutations were killed by assertion failures:

1. Restore the original `origin/main` grouping core: all six new tests fail.
2. Trust surviving members after their representative fails final validation: the final-witness
   test fails.
3. Suppress the byte-budget stop reason: the core and native re-verification budget tests fail.

After restoration, `cargo test -p loomward-windows duplicate_reader` passes: 34 passed,
0 failed, 0 ignored. Local mutation commands and raw outputs remain in the gitignored
`.loomward/duplicate-partition-reverify/` directory; they are not public evidence inputs.

### Verified: final gates

| Command | Output |
| --- | --- |
| `cargo fmt --all --check` | PASS, exit 0 |
| `cargo test --workspace` | PASS, exit 0; 573 passed, 0 failed, 1 pre-existing ignored |
| `cargo test -p loomward-windows -p loomward-engine --all-features` | PASS, exit 0; 204 passed, 0 failed, 0 ignored |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, exit 0 |
| `git diff --check` | PASS, exit 0; repeated after completing the addendum |

The unchanged ignored workspace test is `verbatim_device_namespace_state_paths_are_refused`,
tracked in #246. Gate outputs are retained under the same gitignored directory as `gate-0.txt`
through `gate-4.txt`.

### NOT verified and residual risk

No hosted CI, independent review, non-Windows execution, real changing disk inventory or physical
device read fault was exercised. Python, UI and desktop-shell gates were not run because those
surfaces are unchanged. No long benchmarks, commit or push was performed.

Equality remains point-in-time default-stream evidence, not an atomic snapshot or deletion
authority. Re-verification uses additional bytes within the same budget and can reduce coverage;
unknown inputs remain visible as partial coverage. Filesystem-driver return and cancellation
draining retain their existing limitations. No out-of-scope fixes were folded into this addendum.
