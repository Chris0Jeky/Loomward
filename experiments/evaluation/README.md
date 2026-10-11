# Grouped temporal student evaluation (LW-025)

Every numeric result is **synthetic smoke, not real-user accuracy**. The fixture contains invented
metadata and deliberately conflicting teacher labels. No personal corpus or teacher is consulted.

```powershell
cargo run -p loomward-learn --example evaluate -- evidence/v3/eval-calibration-lw025.json
powershell -NoProfile -ExecutionPolicy Bypass -File experiments/evaluation/check-mutations.ps1
cargo test -p loomward-learn
```

The example embeds `fixtures/evaluation/synthetic.json`; its only argument is an optional JSON output
file. Library callers use `evaluation::split_manifest`, `validate_manifest`, `evaluate` and
`fit_isotonic`. Taxonomy and features reuse the existing student validators. The offline corpus is
explicitly synthetic and bounded to 10,000 records. Times are synthetic ordered ticks recording when
the observation and its label are available; train is at or before tick 10, calibration is after 10
through 20, evaluation is after 20. Production timestamps and personal dataset handling are deferred.

Declare `logical_id` for revisions, `project` for project membership and `near_duplicate` for known
duplicate clusters. Links are transitively joined, with namespaced keys. A component spanning any
cutoff is refused: the caller must change the corpus or cutoffs, never silently break its links.
Group keys are the minimum record ID in each component, independent of input order. No automatic
near-duplicate detection is claimed. Each manifest entry records time, revision, label provenance,
teacher label separately, group and split. The supplied manifest is recomputed and compared before
training, fitting or evaluation. Human and teacher records for the same logical item may coexist;
only human records are holdout truth. The student's existing latest-human precedence applies in train.

Teacher accuracy uses teacher predictions paired with human evaluation truth. Student imitation
uses teacher agreement on evaluation rows (including teacher-only rows). Human holdout utility uses
only human evaluation truth. Metrics are observation-weighted, with per-label denominators and
macro-F1 over labels with truth or predictions. Empty rates stay null. Selective coverage/error
sweeps the raw top-score threshold and retains support, familiarity and margin abstention guards;
the unchanged student operating point is also reported. Correction rate and interruption cost are
explicitly unobserved placeholders.

Raw-score diagnostics use the human evaluation split only: multiclass Brier (sum over taxonomy),
top-label ECE and ten equal-width reliability bins. All have deterministic 95% percentile bootstrap
intervals, resampling whole linked groups. Empty bins have null means/accuracy/intervals and carry
the count of defined replicates. The bounded smoke uses 200 replicates and seed 25, not a benchmark.

The optional isotonic candidate uses only human calibration truth and predicts top-label correctness.
It merges tied scores and decreasing adjacent blocks. It is recorded for inspection, never applied
to the UI or used to label scores as probabilities. Its gate is **NOT verified**; defining a promotion
or calibration acceptance gate belongs to LW-027. The report continues to say **relative score, not
a probability**. This candidate does not predict whether an action is safe.

The mutation script temporarily changes only the evaluation source, requires each new test to fail
at runtime, restores the exact bytes even on error, and writes an aggregate synthetic receipt. Do
not run it concurrently with other Cargo checks in this checkout. Gate results and limitations are
in `evidence/v3/eval-calibration-lw025.md`.
