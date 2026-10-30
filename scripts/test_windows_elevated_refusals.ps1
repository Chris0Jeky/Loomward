# Run, under the hosted runner's real elevated token, only the tests that pin the elevated-token
# refusal (AGENTS.md invariant 2). The standard-user step (test_windows_standard_user.ps1) runs the
# rest of the native tests unelevated, so these refusal branches would otherwise never run in CI.
# Each listed test calls skip_elevated_test(), which prints `SKIPPED (elevated): <test name>` when the
# token is elevated, then asserts the refusal. A test that did not run, or did not take that branch,
# fails the step. This is not the workspace: one `--exact` filter per name, nothing else executes.
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'This check requires Windows.' }
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'The elevated-refusal tests need an elevated token; run them from the hosted bootstrap.'
}
# A host that cannot run the native case must not turn a refusal into a failure here.
Remove-Item Env:LOOMWARD_REQUIRE_NATIVE -ErrorAction SilentlyContinue

# Exact libtest names (module path included) of the tests whose elevated branch asserts a refusal,
# one or two per seam: windows, engine, service, http, lab and shell. Add a name when a new seam grows
# such a branch; a renamed test makes this step red rather than silently shrinking it.
$tests = @(
    'duplicate_reader::native::tests::named_pipe_handle_is_a_visible_special_file_refusal',
    'duplicate_reader::tests::native::native_equal_and_same_size_different_bytes',
    'effects::exec::crash_tests::a_failed_journal_append_stops_the_executor_and_recovers_under_the_oracle',
    'effects::tests::staging_entry_limit_is_inclusive',
    'enumerate::native::cancellation_tests::shortcut_reader_caps_bytes_and_metadata_handles_cannot_read',
    'enumerate::native::cancellation_tests::unverified_filesystems_open_only_under_an_unverified_grant',
    'enumerate::native::containment_tests::native_containment_normalizes_verbatim_and_short_root_aliases',
    'enumerate::tests::junction_never_opened_or_followed',
    'linkcount::tests::linkcount_disposable_hardlinks_match_identity_and_refuse_replacement',
    'tests::windows::volume_bounds_are_real_or_named_unknowns',
    'watch::native::tests::native_fixture_latency_and_continuous_drain',
    'watch::native::tests::pending_read_early_exit_drains_before_storage_is_reused',
    'duplicates::tests::duplicate_job_cancel_and_root_revocation_reach_the_worker_and_terminal_state',
    'placement::tests::placement_native_link_pass_feeds_verified_relief_on_disposable_hardlinks',
    'scan::pipeline::w2_tests::native::revocation_after_scan_finish_cannot_publish_complete',
    'scan::tests::duplicate_grant_returns_evidence_for_its_own_class',
    'scan::tests::followup_grant_gate_cancels_and_fences_every_wire_alias',
    'scan::tests::native_scan_job_reports_exact_final_progress',
    'scan::watch::native_tests::watcher_start_error_preserves_native_cause',
    'tests::diagnostics_tests::a_failed_write_leaves_the_target_and_no_partial_file',
    'tests::duplicates_tests::duplicates_follow_a_fresh_grant_after_the_same_folder_was_revoked',
    'tests::open_stores_the_canonical_spelling_of_a_non_canonical_root',
    'tests::personal_session_scans_a_granted_folder_metadata_only',
    'tests::placement_registered_lab_simulation_preserves_every_file_and_mtime',
    'tests::http_options_consume_exactly_the_validated_canonical_roots',
    'followup_proposed_static_overlap_leaves_no_active_grant',
    'followup_stored_grants_refuse_static_without_recording_new_grants',
    'refused_start_creates_no_state_directory',
    'windows::tests::root_log_writes_through_validated_handle',
    'pathological_profiles_require_an_empty_plain_directory',
    'shortcuts::native::tests::junction_ancestor_is_refused_with_reason',
    'shortcuts::native::tests::reader_uses_final_pinned_handle_after_leaf_swap',
    'shortcuts::native::tests::walk_stays_relative_after_junction_swap',
    'windows_known_folder_shape_and_disposable_shortcut_read',
    'windows_reader_cap_local_metadata_and_sharing_refusal',
    'windows_shortcut_paths_ignore_user_text_case'
)

# Same feature set and target directory as the later workspace steps, so the build is shared; libs,
# bins and test targets only, so no doctest or example is built for a name filter.
$cargoArguments = @('test', '--locked', '--workspace', '--lib', '--bins', '--tests', '--', '--exact', '--show-output') + $tests
Write-Host "::group::elevated cargo test: $($tests.Count) refusal tests"
& cargo +stable @cargoArguments 2>&1 | Tee-Object -Variable cargoOutput
$cargoExit = $LASTEXITCODE
Write-Host '::endgroup::'
if ($cargoExit -ne 0) { exit $cargoExit }
$outputText = $cargoOutput -join "`n"

# Count check, as Invoke-QualificationCargo's MinimumPassed: a renamed test or a dead filter is red.
$passed = 0
foreach ($result in [regex]::Matches($outputText, 'test result: ok\. (\d+) passed; 0 failed; 0 ignored;')) {
    $passed += [int]$result.Groups[1].Value
}
if ($passed -ne $tests.Count) { throw "Expected $($tests.Count) passing refusal tests, ran $passed." }
foreach ($test in $tests) {
    $name = [regex]::Escape($test)
    if ($outputText -notmatch "(?m)^test $name \.\.\. ok") { throw "$test did not pass." }
    if ($outputText -notmatch "SKIPPED \(elevated\): $name(?:\r?\n|$)") {
        throw "$test did not take its elevated-refusal branch (no SKIPPED (elevated) line)."
    }
}
Write-Host "Elevated-refusal tests that ran and took their refusal branch: $($tests.Count)"
