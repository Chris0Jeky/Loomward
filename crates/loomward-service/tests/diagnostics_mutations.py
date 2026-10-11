"""Offline mutation proof for the diagnostic-bundle write gate (docs/42 ADR-V3-24).

Each mutant removes or bypasses one guard of `diagnostics.save` (or leaks a path into the bundle);
the named test must then fail. Every source file is restored byte for byte in `finally`, and the
targeted tests are rerun green afterwards. Logs go to the gitignored `.loomward/diagnostics-bundle/`.
Run from anywhere: `py -3 crates/loomward-service/tests/diagnostics_mutations.py`.
"""
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / '.loomward/diagnostics-bundle'
SERVICE = 'crates/loomward-service/src/diagnostics.rs'
SVC = ['cargo', 'test', '--offline', '-p', 'loomward-service', '--lib']
NATIVE_LIB = ['cargo', 'test', '--offline', '--manifest-path', 'native/Cargo.toml', '--lib']
T = 'tests::diagnostics_tests::'
TARGETS = T + 'unsupported_targets_are_refused_before_anything_is_written'

MUTANTS = [
    ('digest-match', SERVICE, '.filter(|(d, _)| d == r.digest.as_str())', '.filter(|_| true)',
     SVC, T + 'a_changed_or_missing_preview_writes_nothing_and_opens_no_dialog'),
    ('absolute-only', SERVICE, '    if !target.is_absolute()\n        || paths::network_path(target)',
     '    if paths::network_path(target)', SVC, TARGETS),
    ('namespace-refusal', SERVICE, '        || paths::network_path(target)\n', '', SVC, TARGETS),
    ('device-name-refusal', SERVICE, '        || paths::reserved_device(target)\n', '', SVC, TARGETS),
    ('stream-name-refusal', SERVICE, "name.as_deref().is_none_or(|n| n.contains(':'))", 'name.is_none()',
     SVC, TARGETS),
    ('parent-is-directory', SERVICE, '    if !std::fs::metadata(dir).is_ok_and(|m| m.is_dir()) {', '    if false {',
     SVC, TARGETS),
    ('existing-target-regular', SERVICE,
     '        Ok(meta) if !meta.is_file() || paths::unsafe_attributes(&meta) => {',
     '        Ok(meta) if meta.is_file() && !meta.is_file() => {', SVC, TARGETS),
    ('partial-file-cleanup', SERVICE, '        let _ = std::fs::remove_file(&temporary);\n', '',
     SVC, T + 'a_failed_write_leaves_the_target_and_no_partial_file'),
    ('shared-dialog-turn', SERVICE, 'let _dialog = self.dialog_turn()?;', 'let _dialog = ();',
     SVC, T + 'a_second_dialog_is_busy_never_stacked'),
    ('write-exact-bytes', SERVICE, 'match write_export(&target, &bytes) {',
     'match write_export(&target, &[bytes.as_slice(), b"\\n"].concat()) {',
     SVC, T + 'save_writes_exactly_the_previewed_bytes_and_never_returns_the_path'),
    ('canonical-text', SERVICE, '    serde_json::to_vec_pretty(&value).map_err',
     '    serde_json::to_vec(&value).map_err',
     SVC, T + 'both_dataset_classes_preview_and_http_cannot_save'),
    ('leak-a-root-path', SERVICE, '                engine_version: info.engine_version,',
     '                engine_version: Text::truncated(&db::roots(&self.db(), None).ok().and_then(|r| r.first().map(|r| r.display_path.clone())).unwrap_or_default()),',
     SVC, T + 'a_scanned_hostile_lab_root_never_reaches_the_bundle'),
    ('native-save-retitled', 'native/src/dialogs.rs', '.set_title(title)\n                    .set_file_name(default_name)',
     '.set_file_name(default_name)', NATIVE_LIB, 'dialogs::tests::save_dialog_opens_in_every_session_retitled_and_one_at_a_time'),
    ('native-save-one-at-a-time', 'native/src/dialogs.rs', '        let _active = self.active.try_lock().ok()?;\n        let picker = self.picker.lock().ok()?.clone()?;\n        save(',
     '        let picker = self.picker.lock().ok()?.clone()?;\n        save(', NATIVE_LIB,
     'dialogs::tests::save_dialog_opens_in_every_session_retitled_and_one_at_a_time'),
]


def run(command, test):
    return subprocess.run(command + [test, '--', '--exact'], cwd=ROOT, capture_output=True, text=True,
                          encoding='utf-8', errors='replace', timeout=900)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    originals = {}
    rows = []
    try:
        for name, rel, old, new, command, test in MUTANTS:
            path = ROOT / rel
            originals.setdefault(rel, path.read_bytes())
            text = originals[rel].decode('utf-8')
            assert text.count(old) == 1, (name, 'mutation seam moved', old)
            path.write_text(text.replace(old, new), encoding='utf-8', newline='\n')
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
