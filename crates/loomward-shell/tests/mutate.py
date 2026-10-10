"""On Windows, prove each shell regression fails with a compiled mutation; restore every file."""
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[3]
SRC = ROOT / "crates/loomward-shell/src"
CASES = [
    ("header_flags_and_opaque_id_list", "shortcuts.rs", "flags: u32_at(h, 20)?,", "flags: 0,"),
    ("string_fields_are_separate_unicode_and_inert", "shortcuts.rs", "(32, &mut s.arguments),\n        (64, &mut s.icon_location)", "(32, &mut s.icon_location),\n        (64, &mut s.arguments)"),
    ("ansi_bytes_are_preserved_without_guessing_code_page", "shortcuts.rs", "Text::Ansi(r.take(len)?.to_vec())", "Text::Ansi({ r.take(len)?; vec![0] })"),
    ("malformed_and_oversized_inputs_are_refused", "shortcuts.rs", "if bytes.len() > MAX_FILE_BYTES", "if false"),
    ("local_link_info_and_health_keep_volume_absence_distinct", "shortcuts.rs", "VolumeState::Unavailable => return Health::VolumeUnavailable", "VolumeState::Unavailable => return Health::Missing"),
    ("network_link_info_is_never_probed", "shortcuts.rs", "return Health::NetworkUnreachableUnknown;", "return Health::Unknown;"),
    ("environment_known_folder_tracker_and_unknown_blocks_stay_data", "shortcuts.rs", "birth_droid: b[64..96].try_into().unwrap(),", "birth_droid: [0; 32],"),
    ("relative_opaque_and_device_targets_are_not_probed", "shortcuts.rs", "s.relative_path.is_some() || s.known_folder.is_some()", "false"),
    ("known_folders_preserve_redirection_ownership_unicode_and_failures", "known_folders.rs", "Ownership::Public", "Ownership::PerUser"),
    ("source_has_no_execution_com_initialisation_or_library_loading", "lib.rs", "pub mod known_folders;", "// std::process\npub mod known_folders;"),
    ("windows_known_folder_shape_and_disposable_shortcut_read", "shortcuts.rs", "parse(&bytes).map_err", "parse(&[]).map_err"),
    ("unicode_link_info_paths_volume_and_network_offsets", "shortcuts.rs", "Some(at) if u32_at(b, at)? != 0", "Some(at) if false && u32_at(b, at)? != 0"),
    ("hostile_extra_blocks_utf16_and_counts_are_bounded", "shortcuts.rs", "if blocks > 64", "if blocks > 65"),
    ("health_rechecks_volume_and_refuses_resolver_flags", "shortcuts.rs", "VolumeState::Unavailable => Health::VolumeUnavailable", "VolumeState::Unavailable => Health::Missing"),
    ("windows_reader_cap_local_metadata_and_sharing_refusal", "shortcuts.rs", "Ok(_) => MetadataState::Present", "Ok(_) => MetadataState::Missing"),
    ("unpaired_surrogates_preserve_raw_units", "shortcuts.rs", "Err(_) => Text::Wide(units),", 'Err(_) => return Err(ParseError("invalid UTF-16")),'),
    ("random_and_mutated_links_never_panic", "shortcuts.rs", "let mut r = Cursor { bytes, pos: 0 };", 'if bytes.len() < 76 { panic!("mutation: unchecked short input"); }\n    let mut r = Cursor { bytes, pos: 0 };'),
    ("source_guard_covers_nested_files_and_import_forms", "../tests/shell.rs", "guard_tree(&path)", "true"),
    ("source_guard_covers_nested_files_and_import_forms", "../tests/shell.rs", 'matches!(word, "process" | "Command")', 'matches!(word, "unused")'),
    ("shortcuts::native::tests::removable_and_optical_targets_do_not_touch_media", "shortcuts.rs", "DRIVE_REMOVABLE | DRIVE_CDROM => VolumeState::Unknown,", "DRIVE_REMOVABLE | DRIVE_CDROM => probe(),"),
    ("shortcuts::native::tests::junction_ancestor_is_refused_with_reason", "../../loomward-windows/src/enumerate/native.rs", "FILE_OPEN_REPARSE_POINT | FILE_OPEN_NO_RECALL | FILE_SYNCHRONOUS_IO_NONALERT", "FILE_OPEN_NO_RECALL | FILE_SYNCHRONOUS_IO_NONALERT"),
    ("enumerate::native::cancellation_tests::reader_uses_final_pinned_handle_after_leaf_swap", "../../loomward-windows/src/enumerate/native.rs", "after_open();\n    let mut bytes = Vec::new();", "after_open();\n    let file = open_readonly_child(parent, name, true)?;\n    let mut bytes = Vec::new();"),
]

# Restore the old full-path component walk for a deterministic ancestor-swap regression.
relative_open = '''let file = open_metadata_child(
                files.last().unwrap(),
                &part.encode_utf16().collect::<Vec<_>>(),
            )?;'''
full_path_open = '''let current = Path::new(&root).join(parts[..=i].join("\\\\"));
            let file = {
                use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};
                OpenOptions::new()
                    .access_mode(FILE_READ_ATTRIBUTES)
                    .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
                    .custom_flags(FILE_FLAG_OPEN_NO_RECALL | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
                    .open(current)?
            };'''
CASES.append(("shortcuts::native::tests::walk_stays_relative_after_junction_swap", "shortcuts.rs", relative_open, full_path_open))
for token in ["ShellExecute", "CreateProcess", "WinExec", "NtCreateUserProcess", "CoCreateInstance", "SHGetFileInfo", "ExtractIcon", "SHParseDisplayName", "SHCreateItemFromParsingName", "GetProcAddress"]:
    CASES.append(("source_guard_covers_nested_files_and_import_forms", "../tests/shell.rs", f'        "{token}",', f'        "unused_{token}_guard",'))

windows_native = "../../loomward-windows/src/enumerate/native.rs"
shortcut_guard = "duplicate_reader::tests::shortcut_reader_is_capped_feature_gated_and_shell_only"
case_guard = "enumerate::native::cancellation_tests::shortcut_opens_ignore_case_and_listing_opens_stay_exact"
cap_test = "enumerate::native::cancellation_tests::shortcut_reader_caps_bytes_and_metadata_handles_cannot_read"
CASES.extend([
    (shortcut_guard, windows_native, "pub(crate) fn open_readonly_child(", "pub fn open_readonly_child("),
    (shortcut_guard, windows_native, '#[cfg(feature = "shortcut-read")]\npub fn read_capped_regular_file(', 'pub fn read_capped_regular_file('),
    (shortcut_guard, "../../loomward-engine/Cargo.toml", 'features = ["content-read"]', 'features = ["content-read", "shortcut-read"]'),
    (shortcut_guard, "../../loomward-engine/src/lib.rs", '//!', '// read_capped_regular_file\n//!'),
    (case_guard, windows_native, "        OBJ_CASE_INSENSITIVE,", "        0,"),
    (case_guard, windows_native, "share, asynchronous, 0)", "share, asynchronous, OBJ_CASE_INSENSITIVE)"),
    (cap_test, windows_native, "open_readonly_child(parent, name, false)", "open_readonly_child(parent, name, true)"),
    (cap_test, windows_native, "    read_capped_regular_file_with(parent, name, cap, || {})", "    read_capped_regular_file_with(parent, name, cap.max(8), || {})"),
    ("windows_shortcut_paths_ignore_user_text_case", "shortcuts.rs", "Ok(_) => MetadataState::Present", "Ok(_) => MetadataState::Missing"),
    ("source_has_no_execution_com_initialisation_or_library_loading", "../tests/support/native.rs", "use super::*;", "// ShellExecuteExW\nuse super::*;"),
])
support = (SRC / "../tests/support/native.rs").read_text(encoding="utf-8")
for test in ["junction_ancestor_is_refused_with_reason", "walk_stays_relative_after_junction_swap", "reader_uses_final_pinned_handle_after_leaf_swap"]:
    start = support.index(f"fn {test}() {{")
    end = support.index("if loomward_windows::enumerate::running_elevated()", start)
    prefix = support[start:end]
    CASES.append((f"shortcuts::native::tests::{test}", "../tests/support/native.rs", prefix + "if loomward_windows::enumerate::running_elevated()", prefix + "if true"))

for name, filename, old, new in CASES:
    path = SRC / filename
    original = path.read_bytes()
    source = original.decode("utf-8")
    assert old in source, f"mutation anchor missing: {name}"
    try:
        path.write_text(source.replace(old, new, 1), encoding="utf-8", newline="")
        windows = name.startswith(("enumerate::", "duplicate_reader::"))
        package = "loomward-windows" if windows else "loomward-shell"
        target = ["--lib"] if windows or name.startswith("shortcuts::native::tests::") else ["--test", "shell"]
        result = subprocess.run(
            ["cargo", "test", "-p", package, "--all-features", *target, name, "--", "--exact"],
            cwd=ROOT, capture_output=True, text=True, encoding="utf-8", timeout=60,
        )
        assert result.returncode != 0 and f"test {name} ... FAILED" in result.stdout, (
            f"mutation survived or did not reach test: {name}\n{result.stdout}\n{result.stderr}"
        )
        print(f"KILLED {name}", flush=True)
    finally:
        path.write_bytes(original)

subprocess.run(["cargo", "test", "-p", "loomward-shell", "-p", "loomward-windows", "--all-features"], cwd=ROOT, check=True)
print(f"{len(CASES)}/{len(CASES)} compiled mutations killed; restored suite PASS")
