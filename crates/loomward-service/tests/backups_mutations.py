"""Offline mutation proof for the backups stage 1 guards (service, catalogue migration, native wall).

Each mutant removes or bypasses one guard; the named test must then fail. Every source file is
restored byte for byte in `finally`, and the targeted tests are rerun green afterwards. Logs go to
the gitignored `.loomward/backups-import/`. Run from anywhere: `py -3 crates/loomward-service/tests/backups_mutations.py`.
"""
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / '.loomward/backups-import'
SERVICE = 'crates/loomward-service/src/backups.rs'
SVC = ['cargo', 'test', '--offline', '-p', 'loomward-service', '--lib']
CAT = ['cargo', 'test', '--offline', '-p', 'loomward-catalog', '--test', 'open']
NATIVE_LIB = ['cargo', 'test', '--offline', '--manifest-path', 'native/Cargo.toml', '--lib']
NATIVE_IT = ['cargo', 'test', '--offline', '--manifest-path', 'native/Cargo.toml', '--test', 'desktop']
T = 'tests::backups_tests::'

MUTANTS = [
    ('synthetic-wall', SERVICE, 'return refused(Why::SyntheticSession);', '',
     SVC, T + 'wrong_class_sessions_and_adapters_never_open_a_dialog'),
    ('namespace-refusal', SERVICE, 'if paths::network_path(path) || reserved_device(path) {', 'if reserved_device(path) {',
     SVC, T + 'malformed_oversized_and_non_regular_picks_are_refused_by_name'),
    ('device-name-refusal', SERVICE, 'if paths::network_path(path) || reserved_device(path) {', 'if paths::network_path(path) {',
     SVC, T + 'malformed_oversized_and_non_regular_picks_are_refused_by_name'),
    ('regular-file-before-open', SERVICE,
     '    if !meta.is_file() || paths::unsafe_attributes(&meta) {\n        return Err(Why::NotARegularFile);\n    }\n    let file',
     '    let file', SVC, T + 'malformed_oversized_and_non_regular_picks_are_refused_by_name'),
    ('byte-budget', SERVICE, [('if meta.len() > lb::MAX_INPUT_BYTES as u64 {', 'if false {'),
                              ('if bytes.len() > lb::MAX_INPUT_BYTES {', 'if false {')],
     None, SVC, T + 'the_reader_never_returns_more_than_the_budget'),
    ('summary-extraction', SERVICE, '.rfind(|line| !line.is_empty())', '.find(|line| !line.is_empty())',
     SVC, T + 'integrity_output_with_status_lines_is_extracted_and_never_claims_a_full_data_read'),
    ('no-full-data-claim', SERVICE, 'read_all_data: false,', 'read_all_data: true,',
     SVC, T + 'integrity_output_with_status_lines_is_extracted_and_never_claims_a_full_data_read'),
    ('store-raw-bytes', SERVICE, 'drop(snapshots);\n        let stored = stored_evidence(&evidence, basis);',
     'let mut stored = stored_evidence(&evidence, basis);\n        stored["raw"] = json!(String::from_utf8_lossy(&snapshots));',
     SVC, T + 'import_reads_the_picked_receipt_and_stores_only_normalised_evidence'),
    ('topology-identity', SERVICE, '(Some(a), Some(b)) if a == b =>', '(Some(a), Some(b)) if a == b && a.is_empty() =>',
     SVC, T + 'topology_comes_from_volume_identity_never_from_text'),
    ('read-time-staleness', SERVICE, 'refusals.push(json!("stale_observation"));', '',
     SVC, T + 'ages_and_staleness_are_recomputed_on_every_read'),
    ('read-time-future', SERVICE, 'refusals.push(json!("invalid_observation_time"));', '',
     SVC, T + 'ages_and_staleness_are_recomputed_on_every_read'),
    ('repository-limit', SERVICE, 'if others >= MAX_REPOSITORIES {', 'if false {',
     SVC, T + 'the_repository_limit_refuses_a_65th_label_but_replaces_an_existing_one'),
    ('forget-not-found', SERVICE, '            == 0\n        {\n            return Err(fail(ErrorCode::NotFound, "unknown backup evidence"));',
     '            == usize::MAX\n        {\n            return Err(fail(ErrorCode::NotFound, "unknown backup evidence"));',
     SVC, T + 'forget_removes_only_loomwards_record'),
    ('shared-dialog-turn', SERVICE, 'let _dialog = self.dialog_turn()?;', 'let _dialog = ();',
     SVC, T + 'import_and_folder_picker_share_one_dialog_and_cooldown'),
    ('v6-migration', 'crates/loomward-catalog/src/db.rs', 'tx.execute_batch(include_str!("backup-evidence.sql"))?;', '',
     CAT, 'v5_and_v4_state_stores_gain_backup_evidence_at_v6'),
    ('native-adapter-wall', 'native/src/lib.rs',
     '            (Some(Command::BackupsImport), DatasetClass::Synthetic) => Some(json!({\n                "outcome": "refused", "evidence": null, "refusal": "synthetic_session"\n            })),\n',
     '', NATIVE_IT, 'synthetic_backup_import_never_reaches_the_service'),
    ('native-personal-only-picker', 'native/src/dialogs.rs', '        self.pick_folder_with(|picker| {\n            pick(',
     '        Some(rfd::FileDialog::new()).and_then(|picker| {\n            pick(', NATIVE_LIB, 'dialogs::tests::file_picker_is_personal_only_and_retitled'),
]


def run(command, test):
    return subprocess.run(command + [test, '--', '--exact'], cwd=ROOT, capture_output=True, text=True, timeout=900)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    originals = {}
    rows = []
    try:
        for name, rel, old, new, command, test in MUTANTS:
            path = ROOT / rel
            originals.setdefault(rel, path.read_bytes())
            text = originals[rel].decode('utf-8')
            for o, n in (old if isinstance(old, list) else [(old, new)]):
                assert text.count(o) == 1, (name, 'mutation seam moved', o)
                text = text.replace(o, n)
            path.write_text(text, encoding='utf-8', newline='\n')
            result = run(command, test)
            path.write_bytes(originals[rel])
            output = result.stdout + result.stderr
            (OUT / f'mutation-{name}.log').write_text(output, encoding='utf-8')
            killed = result.returncode == 101 and f'{test} ... FAILED' in output
            rows.append({'mutation': name, 'test': test, 'exit': result.returncode, 'killed': killed})
            print(f'{name}: exit={result.returncode}, killed={killed}', flush=True)
    finally:
        for rel, data in originals.items():
            (ROOT / rel).write_bytes(data)
        (OUT / 'mutations.json').write_text(json.dumps(rows, indent=2) + '\n', encoding='utf-8')
    for command, test in {(tuple(m[4]), m[5]) for m in MUTANTS}:
        result = run(list(command), test)
        assert result.returncode == 0 and ' ... ok' in result.stdout, (test, result.stdout + result.stderr)
    survivors = [r['mutation'] for r in rows if not r['killed']]
    assert not survivors, f'survivors: {survivors}'
    print(f'PASS {len(rows)} killed mutations; sources restored; targeted tests green again', flush=True)


if __name__ == '__main__':
    main()
