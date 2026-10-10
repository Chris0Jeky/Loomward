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
]

for name, filename, old, new in CASES:
    path = SRC / filename
    original = path.read_bytes()
    source = original.decode("utf-8")
    assert old in source, f"mutation anchor missing: {name}"
    try:
        path.write_text(source.replace(old, new, 1), encoding="utf-8", newline="")
        result = subprocess.run(
            ["cargo", "test", "-p", "loomward-shell", "--test", "shell", name, "--", "--exact"],
            cwd=ROOT, capture_output=True, text=True, encoding="utf-8", timeout=60,
        )
        assert result.returncode != 0 and f"test {name} ... FAILED" in result.stdout, (
            f"mutation survived or did not reach test: {name}\n{result.stdout}\n{result.stderr}"
        )
        print(f"KILLED {name}", flush=True)
    finally:
        path.write_bytes(original)

subprocess.run(["cargo", "test", "-p", "loomward-shell", "--test", "shell"], cwd=ROOT, check=True)
print(f"{len(CASES)}/{len(CASES)} compiled mutations killed; restored suite PASS")
