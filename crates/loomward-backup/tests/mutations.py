"""Bounded offline mutation proof. Owns only this crate's lib.rs, restores it in finally."""
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[3]
SOURCE = ROOT / 'crates/loomward-backup/src/lib.rs'
OUT = ROOT / '.loomward/lw084'


def run(test=None):
    command = ['cargo', 'test', '--offline', '-p', 'loomward-backup', '--test', 'receipts']
    if test:
        command += [test, '--', '--exact']
    return subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=900)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    original = SOURCE.read_bytes()
    text = original.decode('utf-8')
    mutants = []

    def replace(name, old, new, test):
        assert text.count(old) == 1, (name, 'mutation seam moved')
        mutants.append((name, text.replace(old, new), test))

    def refusal(reason, test):
        replace(reason, f'refusals.push(Refusal::{reason});', '', test)

    refusal('VersionNotCovered', 'green_job_without_matching_version_is_not_recovery')
    refusal('TargetNotIndependent', 'same_volume_is_not_recovery')
    refusal('CredentialsUnavailable', 'expired_or_missing_credentials_are_not_recovery')
    refusal('PartialRestore', 'partial_restore_is_not_recovery')
    refusal('UnsupportedMetadata', 'unsupported_metadata_is_not_recovery')
    refusal('IntegrityNotChecked', 'snapshot_and_integrity_are_distinct_from_restore')
    refusal('StaleObservation', 'stale_and_future_observations_are_not_recovery')
    refusal('InvalidObservationTime', 'stale_and_future_observations_are_not_recovery')
    refusal('SnapshotMissing', 'missing_snapshot_and_unbound_or_failed_observations_are_not_recovery')
    replace('same-device-or-unknown', 'source.topology != Topology::IndependentTarget', 'false',
            'same_device_and_unknown_topology_are_not_recovery')
    replace('successful-control', 'reported_recovery_evidence: refusals.is_empty(),', 'reported_recovery_evidence: false,',
            'complete_restore_is_scoped_and_proposal_is_only_data')
    replace('integrity-summary', '&& summary.num_errors == 0', '&& true',
            'integrity_summary_errors_and_structure_only_check_do_not_verify_recovery')
    replace('duplicate-key', 'if out.contains_key(&key) {', 'if false {',
            'malformed_wrong_types_huge_numbers_and_duplicate_keys_are_rejected')
    replace('byte-budget', 'if raw.len() > MAX_INPUT_BYTES {', 'if false {',
            'byte_and_depth_budgets_are_enforced')
    replace('invalid-utf8', 'let mut deserializer = serde_json::Deserializer::from_slice(raw);',
            'let cleaned: Vec<u8> = raw.iter().copied().map(|b| if b == 0xff { b\'b\' } else { b }).collect();\n'
            '    let mut deserializer = serde_json::Deserializer::from_slice(&cleaned);',
            'invalid_utf8_is_rejected_without_lossy_conversion')
    replace('source-validation', 'validate_source(source)?;', '',
            'source_limits_empty_scope_and_duplicate_paths_are_refused')
    replace('traversal', 'relative_path(&f.path)', 'true',
            'traversal_and_windows_ambiguous_names_are_refused')
    replace('case-fold-identity', 'a.to_lowercase() == b.to_lowercase()', 'a == b',
            'case_variant_paths_name_the_same_required_file')
    # Extra discriminating mutations for the numeric bound and identity binding.
    replace('huge-number', 'if !v.is_finite() || v.abs() >= i64::MAX as f64 {', 'if !v.is_finite() {',
            'malformed_wrong_types_huge_numbers_and_duplicate_keys_are_rejected')
    # #349 fold-ins (backups stage 1).
    replace('summary-shape', '.filter(|s| s.message_type == "summary" && s.num_errors >= 0)', '',
            'unreadable_integrity_documents_refuse_without_losing_the_snapshot')
    replace('bound-only', 'if id != source.snapshot_id {\n            continue;\n        }', '',
            'only_the_bound_snapshot_is_validated_strictly')
    replace('label-charset', '.all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))', '.all(|_| true)',
            'labels_and_snapshot_time_are_restricted_charsets')
    replace('time-charset', '.all(|b| b.is_ascii_alphanumeric() || b":.+-".contains(&b))', '.all(|_| true)',
            'labels_and_snapshot_time_are_restricted_charsets')
    replace('trailing-space-stem', ".trim_end_matches(' ')", '',
            'traversal_and_windows_ambiguous_names_are_refused')
    pattern = r'observation.repository_id == source.repository_id\s*&& observation.snapshot_id == source.snapshot_id\s*&& observation.exit_code == 0'
    bypassed, count = re.subn(pattern, 'true', text)
    assert count == 1, 'binding seam moved'
    mutants.append(('observation-binding', bypassed, 'missing_snapshot_and_unbound_or_failed_observations_are_not_recovery'))
    rows = []
    try:
        green = run()
        assert green.returncode == 0, green.stdout + green.stderr
        for name, mutated, test in mutants:
            SOURCE.write_text(mutated, encoding='utf-8', newline='\n')
            result = run(test)
            output = result.stdout + result.stderr
            (OUT / f'mutation-{name}.log').write_text(output, encoding='utf-8')
            killed = result.returncode == 101 and f'test {test} ... FAILED' in output and 'test result: FAILED' in output
            rows.append({'mutation': name, 'test': test, 'exit': result.returncode, 'killed': killed})
            print(f'{name}: exit={result.returncode}, killed={killed}', flush=True)
            assert killed, output
    finally:
        SOURCE.write_bytes(original)
        (OUT / 'mutations.json').write_text(json.dumps(rows, indent=2) + '\n', encoding='utf-8')
    restored = run()
    (OUT / 'restored.log').write_text(restored.stdout + restored.stderr, encoding='utf-8')
    assert restored.returncode == 0, restored.stdout + restored.stderr
    declared_tests = set(re.findall(r'#\[test\]\s*fn (\w+)', (SOURCE.parent.parent / 'tests/receipts.rs').read_text(encoding='utf-8')))
    assert declared_tests == {row['test'] for row in rows}, 'each new test must kill a mutant'
    print(f'PASS {len(rows)} killed mutations; every test exercised; restored suite green', flush=True)


if __name__ == '__main__':
    main()
