# Scoped closure and reopening evidence — maintenance wave started 20 October 2026

These are the driver's published receipts for the seven retained issue closures and the corrected reopening of #120. Its earlier closure is explicitly superseded below. Historical tests remain historical; the linked hosted jobs are the execution authority. Broader issue scopes remain open as stated.

## Issue #122

[Published completion receipt](https://github.com/Chris0Jeky/Loomward/issues/122#issuecomment-6304120233)

The v3 protocol/DTO/generated-TypeScript milestone is complete in [PR #133](https://github.com/Chris0Jeky/Loomward/pull/133), following the contract in #110. Its receipt covers all 45 commands, 174 DTO definitions, 123 valid and 46 negative complete-envelope examples, exact decoding and deterministic TS output, with the driver-rerun Rust/Python gates recorded.

Current source retains [contract validation and round-trip tests](https://github.com/Chris0Jeky/Loomward/blob/828ef592eaa10388ce99e760d8985fae865facb8/crates/loomward-protocol/tests/contract.rs), the [deterministic generator and --check](https://github.com/Chris0Jeky/Loomward/blob/828ef592eaa10388ce99e760d8985fae865facb8/scripts/gen_contracts_ts.py), and the [generated app contract](https://github.com/Chris0Jeky/Loomward/blob/828ef592eaa10388ce99e760d8985fae865facb8/app/src/lib/contracts.gen.ts).

Closing the original L1 milestone. The older v2 ownership/disposition decision in #10 and aggregate errata/service/provider acceptance in #117 remain separate; this does not establish those broader gates.

## Issue #124

[Published completion receipt](https://github.com/Chris0Jeky/Loomward/issues/124#issuecomment-6304120928)

The Atlas, Observatory and gauge implementation milestone is complete through [PR #136](https://github.com/Chris0Jeky/Loomward/pull/136) and the inspector/retry/backoff/unknown-VRAM follow-up [#155](https://github.com/Chris0Jeky/Loomward/pull/155). The implementation includes canvas modules, text equivalents and unknown/provisional rendering.

Retained [UI evidence](https://github.com/Chris0Jeky/Loomward/blob/828ef592eaa10388ce99e760d8985fae865facb8/evidence/v3/ui-signature-pass.md) records the later browser/accessibility checks, including the 2,500-node Atlas slice. The [native WebView2 receipt](https://github.com/Chris0Jeky/Loomward/blob/828ef592eaa10388ce99e760d8985fae865facb8/evidence/v3/native/README.md) records five synthetic P9 runs with a 17.5 ms median against the 50 ms target.

Closing the original view/module milestone. Manual Narrator, scaling, multiple-monitor and real field acceptance stay in #24, #101 and #414. These are retained execution receipts, not new measurements from this session.

## Issue #176

[Published completion receipt](https://github.com/Chris0Jeky/Loomward/issues/176#issuecomment-6304119002)

Both named spike defects are fixed by [PR #291](https://github.com/Chris0Jeky/Loomward/pull/291): the child publishes its report through `.partial` plus rename, and log read/UTF-8 failures are retained as explicit `read_error` values. [Current example source](https://github.com/Chris0Jeky/Loomward/blob/828ef592eaa10388ce99e760d8985fae865facb8/crates/loomward-lab/examples/teacher_sandbox/windows.rs) contains regression coverage for atomic publication, missing logs and invalid UTF-8.

The [later reconciliation receipt](https://github.com/Chris0Jeky/Loomward/blob/828ef592eaa10388ce99e760d8985fae865facb8/evidence/v3/teacher-followups-293.md) ties both fixes to #291 and records 14 passing standalone Windows example tests. Its blocked Cargo all-targets checks remain explicitly unverified.

Closing these two spike-only items. Production teacher runtime, stage/provenance enforcement, provider qualification and personal-teacher gates remain in #128, #130 and #293; this is not a new live confinement or console-kill qualification.

## Issue #416

[Published completion receipt](https://github.com/Chris0Jeky/Loomward/issues/416#issuecomment-6304118330)

The original q-40 retirement milestone is complete in [PR #424](https://github.com/Chris0Jeky/Loomward/pull/424). Its parity checklist accounts for every retired legacy surface and preserves deliberately delegated gaps in their existing issues. The Python reference is marked frozen, and all 80 admission fixtures remain pinned by `AdmissionOracleFixtureTests`.

The exact PR head `ec8d0e02bf940054d99b259264a7670f58de55b3` has [nine successful hosted jobs](https://github.com/Chris0Jeky/Loomward/actions/runs/38196055909), including Windows desktop build/test/clippy/self-test, both native workspace legs, the four Python variants and app checks. PR #424 also records its local Windows browser suite. The retained checklist is in [docs/30](https://github.com/Chris0Jeky/Loomward/blob/828ef592eaa10388ce99e760d8985fae865facb8/docs/30-interface-and-workspace-spec.md).

Closing this maintenance milestone. Activity/export, durable Decision desk, providers, resource scheduling and manual Windows accessibility remain tracked in #25, #26, #93, #76, #70, #37, #82, #83, #24 and #101. This closure adds no new qualification claim.

## Issue #763

[Published completion receipt](https://github.com/Chris0Jeky/Loomward/issues/763#issuecomment-6304465897)

Both listed items are now accounted for. [PR #807](https://github.com/Chris0Jeky/Loomward/pull/807), merged as f3c19a2208a74f54c267d28ab0cc58cf3f4859ad, forwards settlement through RepairSink and ProbeWriter. Its tests-only [Linux run](https://github.com/Chris0Jeky/Loomward/actions/runs/39513590898/job/118940280426) failed the expected obsolete-directory relist and swallowed-writer-error assertions; the implementation then passed all nine jobs in [run 39514520589](https://github.com/Chris0Jeky/Loomward/actions/runs/39514520589), including both native platforms.

The send-time-rejected listing/chunk fence in item 2 was already addressed by #790 and was checked during the initial source review. This closure is limited to those two sink-wrapper/rejection items.

## Issue #781

[Published completion receipt](https://github.com/Chris0Jeky/Loomward/issues/781#issuecomment-6313080792)

Completed by [#808](https://github.com/Chris0Jeky/Loomward/pull/808) and the reviewed late-cancellation follow-up [#813](https://github.com/Chris0Jeky/Loomward/pull/813).

A scan that drains its configured budget now records a finished partial result while keeping coverage unknown; it does not authorize absence or freshness. Final publication admission is serialized with cancellation, and the admission gate is released before sink I/O. Cancellation that wins first suppresses completion, freshness and finished-partial publication; cancellation arriving after publication admission preserves the admitted decision.

Both new cancellation regressions failed before implementation on Linux and Windows. Final head `484a46808af970cb9083366208008c117655272c`, tree `6bd2f4142db1aa595e8f722eae273cc8282eb751`, passed all nine jobs in [run 39521167880](https://github.com/Chris0Jeky/Loomward/actions/runs/39521167880). Actual CI checkout `21ced095a07b19cb7f4c9b829376303eb3101ca9` and merge `9e33052b1a54ccb6d79a2d7adb88ec532c1230a2` have that same source tree.

This closes the reported budget-completion defect and its reviewed cancellation window. The tests exercise the real scan pipeline and job registry at a MemorySink-backed publication boundary; they do not establish native directory enumeration, sustained 10M service readiness or broader host qualification.

## Issue #785

[Published completion receipt](https://github.com/Chris0Jeky/Loomward/issues/785#issuecomment-6304961813)

Completed by merged [PR #811](https://github.com/Chris0Jeky/Loomward/pull/811), merge `dc783190cdaced24d06da37d3ae7ce32fb193c7f`.

A retry after restart can recover the original label or retraction receipt when a single read-only proof authenticates the same current durable object, effective published reference, grant and visible ancestry. The original fingerprint fast path remains for exact and legacy retries. Conflicts and missing receipts fail before any new reference/event/revision write; dirty catalogue roots are not repaired during replay validation.

The four original regressions failed before implementation. All ten final portable regressions executed and passed on both Linux and Windows, and all nine jobs in [run 39519053157](https://github.com/Chris0Jeky/Loomward/actions/runs/39519053157) succeeded at head `d894c06f69320dc85f05755a9653dc20e7249cde`, source tree `dc5a981a62f9c69744fa24184e87ffedc2b81576`. The merged source tree is identical. Independent review also checked the proof's read-only and ancestry/incarnation boundaries.

This closes the restart/idempotency defect reported here. It introduces no schema or protocol change and makes no new native filesystem, 10M-readiness or capability-gate claim.

## Issue #120 — reopened; earlier closure superseded

The original issue incorporates W0-B measurement acceptance; required cold-cache and full strategy/thread/volume qualification remain unverified. Warm HDD measurements exist in PR118.

### Earlier closure receipt (superseded)

[Historical public receipt](https://github.com/Chris0Jeky/Loomward/issues/120#issuecomment-6304119592)

The enumeration strategy and promotion milestone is complete in [PR #162](https://github.com/Chris0Jeky/Loomward/pull/162). The measured strategy chain is in the production `DirSource`: extended IDs, width-preserving Both fallback, then Find; child opens are handle-relative and post-open identity/attribute checks remain enforced.

The retained [L7 conformance and measurement receipt](https://github.com/Chris0Jeky/Loomward/blob/828ef592eaa10388ce99e760d8985fae865facb8/evidence/v3/scan-l7.md) maps the native tests and mutation checks, records Windows fmt/workspace/clippy/all-feature checks, and contains 20/20 exact S and 20/20 exact M manifest matches with enumeration and sink time reported separately. Raw measurements are linked in that receipt.

Closing this original strategy/promotion milestone. The historical S/M runs used the in-memory sink and warm/uncontrolled conditions; full durable-service 10M/P4, physical ReFS/Cloud Files, hung-volume and broader daily-use qualification remain in #414, #117, #285 and their linked work. No new native measurement is claimed here.

### Reopening correction (current)

[Published reopening receipt](https://github.com/Chris0Jeky/Loomward/issues/120#issuecomment-6313742953)

Reopened after [the final checkpoint review](https://github.com/Chris0Jeky/Loomward/pull/816#discussion_r4335679013). My earlier closure treated this as an implementation/promotion milestone, but this issue explicitly incorporates both W0-B measurement and L7 acceptance. That narrower interpretation was incorrect.

The promoted strategy chain and retained S/M conformance work in [#162](https://github.com/Chris0Jeky/Loomward/pull/162) remain valid implementation progress. The outstanding acceptance is the required per-strategy, 1–16-thread measurement matrix across the specified fast/slow volumes in warm and cold conditions, as stated in [W0-B](https://github.com/Chris0Jeky/Loomward/blob/132e10e8c43a963658d03cbeaec34d8f20a6c0a8/docs/43-v03-lanes.md#L72).

The retained evidence does not complete that matrix: [#111](https://github.com/Chris0Jeky/Loomward/pull/111) leaves cold-cache and HDD qualification unverified; [#118](https://github.com/Chris0Jeky/Loomward/pull/118) adds warm NVMe/HDD measurements but explicitly leaves true cold-cache measurements open. The [L7 receipt](https://github.com/Chris0Jeky/Loomward/blob/132e10e8c43a963658d03cbeaec34d8f20a6c0a8/evidence/v3/scan-l7.md#L96-L106) also retains its warm/uncontrolled qualification.

Both the GitHub issue and LW-101 stay **in progress** until the required measurements exist. The checkpoint will retain the earlier closure receipt as superseded history, remove this issue from current closure totals, and keep the original acceptance criteria intact.
