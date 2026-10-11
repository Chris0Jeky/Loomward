# Each mutation must reach a failing assertion, then restore the exact source bytes.
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$sourcePath = Join-Path $repo 'crates/loomward-learn/src/evaluation.rs'
$originalBytes = [IO.File]::ReadAllBytes($sourcePath)
$original = [Text.Encoding]::UTF8.GetString($originalBytes)
$cases = @(
    @('linked_groups_are_transitive_and_temporal_traps_are_refused', 'if indices.iter().any(|i| split(self.items[*i].time) != role)', 'if false && indices.iter().any(|i| split(self.items[*i].time) != role)', 'allow a group to span temporal cutoffs'),
    @('supplied_leaking_manifest_is_rejected_before_fit_or_report', 'if self.manifest(train_end, calibration_end)? != *manifest', 'if false && self.manifest(train_end, calibration_end)? != *manifest', 'accept a leaking supplied manifest'),
    @('provenance_is_explicit_and_teacher_never_becomes_holdout_truth', '.filter(|r| r.item.source == Source::Human)', '.filter(|_r| true)', 'count teacher rows as human truth'),
    @('macro_metrics_match_a_worked_confusion_matrix_and_unknowns_stay_null', 'ratio(2 * tp, support + predicted)', 'ratio(tp, support + predicted)', 'break the F1 numerator'),
    @('report_preserves_abstention_thresholds_and_missing_correction_observations', '&& p.suggestions[0].score >= threshold', '&& p.suggestions[0].score >= threshold + 0.2', 'break score threshold coverage'),
    @('calibration_fit_refuses_evaluation_and_train_and_is_independent_of_evaluation_labels', 'if split != "calibration"', 'if false && split != "calibration"', 'permit calibration fitting on evaluation'),
    @('calibration_fit_refuses_evaluation_and_train_and_is_independent_of_evaluation_labels', 'while blocks.len() > 1', 'while false && blocks.len() > 1', 'remove isotonic tie and decreasing-block pooling'),
    @('raw_diagnostics_have_deterministic_group_bootstrap_intervals', 'groups.entry(&row.group).or_default().push(row);', 'groups.entry(&row.item.id).or_default().push(row);', 'bootstrap rows instead of linked groups'),
    @('corpus_boundary_rejects_invalid_and_non_synthetic_records', 'if value["synthetic"] != true', 'if false && value["synthetic"] != true', 'accept non-synthetic input'),
    @('evaluation::tests::calibration_matches_hand_computed_brier_ece_and_bin_edges', '(scores[i] - correct[i] as f64).abs() / rows.len() as f64', '(scores[i] - correct[i] as f64).abs() / rows.len() as f64 * 0.0', 'zero the ECE calculation')
)
$results = @()
Push-Location $repo
try {
    foreach ($case in $cases) {
        if (($original.Split(@($case[1]), [StringSplitOptions]::None)).Length -ne 2) {
            throw "Mutation anchor must occur once: $($case[0])"
        }
        [IO.File]::WriteAllText($sourcePath, $original.Replace($case[1], $case[2]), [Text.UTF8Encoding]::new($false))
        $target = @('--test', 'evaluation')
        if ($case[0].StartsWith('evaluation::')) { $target = @('--lib') }
        $ErrorActionPreference = 'Continue' # Windows PowerShell treats native stderr as an error record.
        try {
            $log = (& cargo test -p loomward-learn @target $case[0] -- --exact 2>&1 | Out-String)
            $exitCode = $LASTEXITCODE
        } finally { $ErrorActionPreference = 'Stop' }
        if ($exitCode -eq 0 -or !$log.Contains("test $($case[0]) ... FAILED")) {
            throw "Mutation did not reach the expected failing test: $($case[0])`n$log"
        }
        $results += [ordered]@{ test = $case[0]; mutation = $case[3]; exit_code = $exitCode; result = 'KILLED: runtime assertion failed' }
        Write-Host "KILLED $($case[0])"
        [IO.File]::WriteAllBytes($sourcePath, $originalBytes)
    }
} finally {
    [IO.File]::WriteAllBytes($sourcePath, $originalBytes)
    Pop-Location
}
if ([Convert]::ToBase64String([IO.File]::ReadAllBytes($sourcePath)) -ne [Convert]::ToBase64String($originalBytes)) {
    throw 'Source bytes were not restored'
}
$receipt = [ordered]@{ scope = 'synthetic smoke, not real-user accuracy'; source_restored = $true; results = $results }
$json = ($receipt | ConvertTo-Json -Depth 5) + "`n"
[IO.File]::WriteAllText((Join-Path $repo 'evidence/v3/eval-calibration-lw025-mutations.json'), $json, [Text.UTF8Encoding]::new($false))
