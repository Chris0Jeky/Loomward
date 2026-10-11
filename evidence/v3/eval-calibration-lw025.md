# LW-025: grouped temporal student evaluation and calibration

**Every numeric result in this document and its JSON artifacts is synthetic smoke, not real-user
accuracy.** Platform: Windows, shared host. No hostname, personal inventory, real paths or hardware
observations are included. Base source: `697b6176735042f956e2612541d8de7f044fce55` on
`feat/eval-calibration-lw025`; changes are uncommitted for the driver. Cargo/rustc: 1.97.1.

## Changed

The Rust student now has offline library functions in `crates/loomward-learn/src/evaluation.rs`:
`split_manifest`, `validate_manifest`, `classification_metrics`, `evaluate` and `fit_isotonic`.
`crates/loomward-learn/examples/evaluate.rs` embeds the bounded synthetic corpus and writes JSON.
No Python parity change was necessary: the existing Rust student and its Python fixture oracle are
used unchanged. No UI or contract field is needed for this offline report.

The synthetic corpus is `fixtures/evaluation/synthetic.json`, SHA-256
`fd01458fb29a9666b188325bc541bda138cd442eeea6a095267fefb18c01d055`.
It has train-only revisions, a duplicate bridge joining two projects transitively, separate
calibration/evaluation periods, shared evaluation projects, deliberately incorrect teacher labels,
teacher-only records and unfamiliar metadata. Tests add cross-cutoff revision/project/duplicate traps
and a supplied manifest that leaks a revision into evaluation. All are refused before fitting.
Group relations are declared by the fixture; near-duplicate detection is not implemented.

Manifests record group keys, item/logical IDs, revisions, time, split, source, human/teacher labels
separately and whether the label is holdout truth. Time is an ordered synthetic observation/label
availability tick. Connected groups cannot span train/calibration/evaluation cutoffs. No earlier
record is silently moved into a future holdout. Caller-supplied manifests are recomputed before use.

The report separates teacher accuracy against human evaluation labels, student agreement with the
teacher, and human holdout utility. Macro-F1, per-label precision/recall/F1, denominator counts,
unconditional top-one accuracy and selective error/coverage are included. Undefined rates are null;
macro-F1 excludes labels with neither truth nor prediction. Correction rate and interruption cost
are null/unobserved, not invented from label agreement. The score sweep retains the existing student
support, familiarity and margin guards. Predictions retain the student's original abstention reasons.

Reliability bins, multiclass Brier and top-label ECE are raw-score diagnostics on human evaluation
truth only, with deterministic group-level percentile bootstrap intervals. Intervals for empty-bin
means/accuracy are null, with counts of defined replicates. Calibration fitting uses only the separate
human calibration split. The optional isotonic candidate merges tied/decreasing blocks and is not
applied to predictions. It has no acceptance/promotion gate evidence: every score remains labelled
**relative score, not a probability**. No inference about action safety is made.

## Synthetic smoke results

The label **synthetic smoke, not real-user accuracy** applies to every value in this table and the
linked JSON receipts.

| Measure | Synthetic smoke, not real-user accuracy |
|---|---|
| Teacher accuracy / macro-F1, human evaluation only | 0.5 / 0.4857142857142857, 6 observations |
| Student imitation accuracy / macro-F1 | 0.42857142857142855 / 0.41666666666666663, 7 observations |
| Human holdout accuracy / macro-F1 | 0.6666666666666666 / 0.6666666666666666, 6 observations |
| Existing abstention operating point | coverage 0.8333333333333334; selective error 0.2 |
| Multiclass Brier on raw scores | 0.4150706806263366; bootstrap interval [0.00011210828024915947, 1.1821502441087568] |
| Top-label ECE on raw scores | 0.24539566466327195; bootstrap interval [0.0074869309316529355, 0.5925149574960547] |
| Diagnostic bootstrap | 200 replicates, seed 25, 5 independent linked groups |
| Isotonic candidate | 4 human calibration observations; ungated, never applied |

Identical coverage at the displayed score thresholds is a property of this tiny smoke corpus, not a
claim that those thresholds are interchangeable. The erroneous high-score prediction deliberately
survives the score sweep. Small/empty samples can give degenerate or undefined intervals; no field
effect, personal utility or real-user model quality follows from these numbers.

## Commands and verification

Commands run from the lane's repository root (`.`), with Cargo build jobs limited to two during the
workspace gates. No long benchmark was run.

```powershell
cargo test -p loomward-learn
cargo run -p loomward-learn --example evaluate -- evidence/v3/eval-calibration-lw025.json
powershell -NoProfile -ExecutionPolicy Bypass -File experiments/evaluation/check-mutations.ps1
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
git diff --check
```

Baseline student checks passed before editing: the existing parity and unit suites. The new tests
pin transitive grouping/time refusal, supplied-manifest leakage refusal, provenance isolation,
worked confusion-matrix metrics, abstention/correction placeholders, held-out-only calibration,
deterministic group bootstrap, corpus input validation and hand-computed Brier/ECE/bin boundaries.

Every new test was proven to fail with its protection/calculation removed or broken. The mutation
runner requires the named test to fail at runtime; compile/setup errors do not qualify. It restores
the exact source bytes even on failure. In particular, accepting a leaking supplied split and
fitting on evaluation each cause a regression test failure. All nine mutants were caught with exit
101, then the restored source passed. A tenth mutant removes isotonic pooling and is caught by the
calibration test's monotonicity assertion. See
[`eval-calibration-lw025-mutations.json`](eval-calibration-lw025-mutations.json) for each named test
and mutation. The initial mutation harness encountered PowerShell native-stderr/array-splat handling;
the harness was fixed and rerun successfully. Those setup errors were not counted as red tests.

All required local Rust gates passed: formatting; the workspace suite (651 passed, 0 failed,
1 pre-existing ignored test); all-targets/all-features Clippy with warnings denied; and whitespace
checks. The ignored test is `verbatim_device_namespace_state_paths_are_refused`, an existing defect
tracked in #246. Full command results and tested source hashes are recorded in
`eval-calibration-lw025-gates.json`. The generated
[`eval-calibration-lw025.json`](eval-calibration-lw025.json) contains the exact split manifest,
predictions, per-label metrics, selective rows, reliability-bin intervals and isotonic candidate.
The reusable experiment instructions are in `experiments/evaluation/README.md`.

## NOT verified and declined

Personal data and real-user accuracy remain unverified; q-5 is open. Teacher execution, provider
behavior and the inference broker are outside this lane (LW-111). A calibrated-map acceptance gate,
map application and model promotion are outside this lane (LW-027). Correction rate and interruption
cost require human interaction observations. No UI/browser/native-shell gate was run for this
offline-only change. Python was not edited, so `py -3 scripts/verify.py` was not required here; the
existing Rust/Python parity fixtures were exercised through Cargo. No commit or push was performed.

## Residual risk

The manifest can protect only declared identity/project/duplicate links and honest provenance/time;
it does not discover missing relations. Metrics are observation-weighted; uncertainty resamples
linked groups rather than treating revisions/project members as independent. This tiny synthetic
corpus is functional evidence, not a calibration distribution or model promotion case. The isotonic
candidate is deliberately ungated and cannot change the student's probability or permission claims.

Recommended commit message: `Add grouped temporal student evaluation and calibration smoke`.
