use std::path::Path;
use std::process::{Command, Output};

/// Hosted CI runs elevated, and grants refuse elevated processes; native coverage then skips.
fn elevated() -> bool {
    #[cfg(windows)]
    return loomward_windows::enumerate::skip_elevated_test();
    #[cfg(not(windows))]
    false
}

fn verify(source: &Path, destination: &Path, state: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_loomward-native"))
        .arg("verify-tree")
        .arg(source)
        .arg(destination)
        .arg("--state-dir")
        .arg(state)
        .args(extra)
        .output()
        .unwrap()
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&destination).unwrap();
    (temp, source, destination)
}

#[test]
fn unsafe_roots_are_refused_before_state_or_tree_reads() {
    if elevated() {
        return;
    }
    let (temp, source, destination) = fixture();
    let state = temp.path().join("state");
    let nested = source.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let volume = source.ancestors().last().unwrap();
    for (a, b, s, reason) in [
        (
            volume,
            destination.as_path(),
            state.as_path(),
            "volume_root",
        ),
        (source.as_path(), volume, state.as_path(), "volume_root"),
        (
            source.as_path(),
            source.as_path(),
            state.as_path(),
            "already_granted",
        ),
        (
            source.as_path(),
            nested.as_path(),
            state.as_path(),
            "already_granted",
        ),
        (
            nested.as_path(),
            source.as_path(),
            state.as_path(),
            "already_granted",
        ),
        (
            source.as_path(),
            destination.as_path(),
            nested.as_path(),
            "contains_state_dir",
        ),
        (
            nested.as_path(),
            destination.as_path(),
            source.as_path(),
            "inside_state_dir",
        ),
    ] {
        let output = verify(a, b, s, &[]);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(reason),
            "{output:?}"
        );
        assert!(!state.exists());
        assert!(!source.join("state.db").exists());
        assert!(!nested.join("state.db").exists());
    }
}

#[test]
fn whole_environment_profile_is_refused_with_a_plain_reason() {
    if elevated() {
        return;
    }
    let (temp, source, destination) = fixture();
    let state = temp.path().join("state");
    let output = Command::new(env!("CARGO_BIN_EXE_loomward-native"))
        .env("USERPROFILE", &source)
        .arg("verify-tree")
        .arg(&source)
        .arg(&destination)
        .arg("--state-dir")
        .arg(&state)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("the whole user-profile folder is not eligible (q-4); choose a child folder"),
        "{output:?}"
    );
    assert!(output.stdout.is_empty());
    assert!(!state.exists());
}

#[test]
fn usage_errors_exit_two_and_show_usage() {
    for args in [
        vec!["verify-tree"],
        vec!["verify-tree", "source", "destination"],
        vec!["verify-tree", "source", "destination", "--state-dir"],
        vec![
            "verify-tree",
            "source",
            "destination",
            "--state-dir",
            "state",
            "--budget-bytes",
            "bad",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_loomward-native"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Usage: loomward-native"),
            "{output:?}"
        );
    }
}

#[cfg(windows)]
fn align_times(source: &Path, destination: &Path) {
    use std::os::windows::fs::{FileTimesExt, OpenOptionsExt};
    let meta = std::fs::metadata(source).unwrap();
    // Fixture setup only: the verifier itself never requests this access.
    std::fs::OpenOptions::new()
        .access_mode(0x100) // FILE_WRITE_ATTRIBUTES
        .custom_flags(0x02000000) // FILE_FLAG_BACKUP_SEMANTICS (directories too)
        .open(destination)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_created(meta.created().unwrap())
                .set_modified(meta.modified().unwrap()),
        )
        .unwrap();
}

#[cfg(windows)]
fn populated_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let (temp, source, destination) = fixture();
    for root in [&source, &destination] {
        std::fs::write(root.join("fixture"), b"same").unwrap();
        std::fs::create_dir(root.join("empty")).unwrap();
    }
    // C1 requires inherited destination ACLs; temporary-directory creation can add explicit ACEs.
    let acl = Command::new("icacls")
        .arg(temp.path())
        .args(["/reset", "/T", "/Q"])
        .output()
        .unwrap();
    assert!(acl.status.success(), "{acl:?}");
    for name in ["fixture", "empty"] {
        align_times(&source.join(name), &destination.join(name));
    }
    align_times(&source, &destination);
    (temp, source, destination)
}

#[cfg(windows)]
#[test]
fn identical_trees_exit_zero_with_a_json_report() {
    if elevated() {
        return;
    }
    let (temp, source, destination) = populated_fixture();
    let state = temp.path().join("state");
    let output = verify(&source, &destination, &state, &[]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for (side, root) in [("source", &source), ("destination", &destination)] {
        use loomward_windows::enumerate::{DirSource, FileIdObs, NativeSource};
        let (_, identity) = NativeSource::default().open_root(root).unwrap();
        let Some(FileIdObs::Id128(id)) = identity.id else {
            panic!("128-bit fixture identity")
        };
        assert_eq!(
            report["roots"][side]["volume_guid"],
            format!("{:032x}", identity.volume_guid.unwrap())
        );
        let hex: String = id.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(report["roots"][side]["root_file_id"], hex);
    }
    assert_eq!(report["differences"], serde_json::json!([]));
    assert_eq!(report["unknowns"], serde_json::json!([]));
    assert!(report["reported_metadata"].as_array().unwrap().len() > 1);
    assert!(String::from_utf8_lossy(&output.stderr).contains("equal"));
    assert_eq!(std::fs::read(source.join("fixture")).unwrap(), b"same");
    assert_eq!(std::fs::read(destination.join("fixture")).unwrap(), b"same");
    let db = rusqlite::Connection::open_with_flags(
        state.join("state.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        db.query_row(
            "SELECT value FROM meta WHERE key='dataset_class'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "personal"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM root_grant WHERE state='active' AND origin='owner_granted' AND granted_via='cli_flag'", [], |r| r.get::<_, i64>(0)).unwrap(),
        2
    );
}

#[cfg(windows)]
#[test]
fn changed_missing_and_extra_files_exit_one_with_the_difference() {
    if elevated() {
        return;
    }
    for (change, kind) in [
        ("changed", "Content"),
        ("missing", "Missing"),
        ("extra", "Extra"),
    ] {
        let (temp, source, destination) = populated_fixture();
        let name = if change == "extra" {
            "extra"
        } else {
            "fixture"
        };
        match change {
            "changed" => std::fs::write(destination.join(name), b"diff").unwrap(),
            "missing" => std::fs::remove_file(destination.join(name)).unwrap(),
            "extra" => std::fs::write(destination.join(name), b"extra").unwrap(),
            _ => unreachable!(),
        }
        let output = verify(&source, &destination, &temp.path().join("state"), &[]);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            report["differences"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["path"] == name && d["kind"] == kind),
            "{report}"
        );
        assert_eq!(report["unknowns"], serde_json::json!([]));
        assert_eq!(std::fs::read(source.join("fixture")).unwrap(), b"same");
    }
}

#[cfg(windows)]
#[test]
fn byte_time_and_entry_budget_exhaustion_exit_two() {
    if elevated() {
        return;
    }
    for option in ["--budget-bytes", "--budget-secs", "--max-entries"] {
        let (temp, source, destination) = populated_fixture();
        let output = verify(
            &source,
            &destination,
            &temp.path().join("state"),
            &[option, "0"],
        );
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            !report["unknowns"].as_array().unwrap().is_empty(),
            "{report}"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("unknown"));
    }
}

#[test]
fn capabilities_describe_native_read_only_verification() {
    let output = Command::new(env!("CARGO_BIN_EXE_loomward-native"))
        .arg("capabilities")
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["native_content_hash"], cfg!(windows));
    assert_eq!(report["file_mutation"], false);
    assert_eq!(report["process_mutation"], false);
}

#[cfg(windows)]
#[test]
fn reused_state_with_stale_same_path_grant_requires_fresh_state() {
    if elevated() {
        return;
    }
    let (temp, source, destination) = populated_fixture();
    let state = temp.path().join("state");
    assert_eq!(
        verify(&source, &destination, &state, &[]).status.code(),
        Some(0)
    );
    let db = rusqlite::Connection::open(state.join("state.db")).unwrap();
    // Keep the stale active grant and let the next open grant the new live identity.
    db.execute(
        "UPDATE root_grant SET root_file_id=zeroblob(16) WHERE id=(SELECT min(id) FROM root_grant)",
        [],
    )
    .unwrap();
    drop(db);
    let output = verify(&source, &destination, &state, &[]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("use a fresh --state-dir"),
        "{output:?}"
    );
}

#[cfg(windows)]
#[test]
fn reparse_children_and_partial_listings_exit_two_with_json_unknowns() {
    if elevated() {
        return;
    }
    let (temp, source, destination) = populated_fixture();
    let target = temp.path().join("target");
    std::fs::create_dir(&target).unwrap();
    // Junction fixture setup; the verifier must not follow it.
    let output = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(source.join("link"))
        .arg(&target)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let output = verify(&source, &destination, &temp.path().join("state"), &[]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["unknowns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["reason"] == "reparse_point"),
        "{report}"
    );
    let output = verify(
        &source,
        &destination,
        &temp.path().join("partial-state"),
        &["--max-entries", "1"],
    );
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["unknowns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["reason"] == "entry_budget"),
        "{report}"
    );
}

/// Manifest root (#875) of the populated fixture's destination holding `fixture` = `content` and an
/// empty folder, computed independently of the CLI from the documented encoding.
#[cfg(windows)]
fn expected_root(content: &[u8]) -> String {
    use loomward_operations::{manifest_root, CustodyEntry, EntryKind, COLLATION_V1};
    use sha2::{Digest, Sha256};
    let empty = manifest_root(&[], COLLATION_V1).unwrap();
    let root = manifest_root(
        &[
            CustodyEntry::from_utf8_name(
                "fixture",
                EntryKind::File,
                content.len() as u64,
                Sha256::digest(content).into(),
            ),
            CustodyEntry::from_utf8_name("empty", EntryKind::Directory, 0, empty),
        ],
        COLLATION_V1,
    )
    .unwrap();
    root.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(windows)]
fn manifest_dir<'a>(report: &'a serde_json::Value, path: &str) -> &'a serde_json::Value {
    report["manifest_roots"]["directories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["path"] == path)
        .unwrap_or_else(|| panic!("no manifest entry for {path:?}: {report}"))
}

#[cfg(windows)]
fn verify_json(
    temp: &tempfile::TempDir,
    source: &Path,
    destination: &Path,
    state: &str,
    extra: &[&str],
) -> (Option<i32>, serde_json::Value) {
    let output = verify(source, destination, &temp.path().join(state), extra);
    let report = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{output:?}"));
    (output.status.code(), report)
}

#[cfg(windows)]
#[test]
fn manifest_roots_are_absent_unless_asked_for() {
    if elevated() {
        return;
    }
    let (temp, source, destination) = populated_fixture();
    let (code, report) = verify_json(&temp, &source, &destination, "state", &[]);
    assert_eq!(code, Some(0));
    assert!(report.get("manifest_roots").is_none(), "{report}");
}

#[cfg(windows)]
#[test]
fn manifest_roots_cover_the_destination_and_are_stable_across_runs() {
    if elevated() {
        return;
    }
    let (temp, source, destination) = populated_fixture();
    let mut roots = Vec::new();
    for state in ["state-one", "state-two"] {
        let (code, report) =
            verify_json(&temp, &source, &destination, state, &["--manifest-roots"]);
        assert_eq!(code, Some(0), "{report}");
        assert_eq!(report["manifest_roots"]["collation_version"], 1);
        assert_eq!(report["manifest_roots"]["digest_algorithm"], "sha256");
        assert_eq!(manifest_dir(&report, "")["root"], expected_root(b"same"));
        assert_eq!(manifest_dir(&report, "")["reason"], serde_json::Value::Null);
        assert!(manifest_dir(&report, "empty")["root"].is_string());
        roots.push(manifest_dir(&report, "")["root"].clone());
    }
    assert_eq!(roots[0], roots[1]);
}

#[cfg(windows)]
#[test]
fn manifest_root_changes_when_one_byte_changes() {
    if elevated() {
        return;
    }
    let (temp, source, destination) = populated_fixture();
    // The same single-byte edit on both sides keeps the trees equal, so the root is emitted.
    for root in [&source, &destination] {
        std::fs::write(root.join("fixture"), b"sam3").unwrap();
    }
    align_times(&source.join("fixture"), &destination.join("fixture"));
    align_times(&source, &destination);
    let (code, report) = verify_json(&temp, &source, &destination, "state", &["--manifest-roots"]);
    assert_eq!(code, Some(0), "{report}");
    let root = manifest_dir(&report, "")["root"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(root, expected_root(b"sam3"));
    assert_ne!(root, expected_root(b"same"));
}

#[cfg(windows)]
#[test]
fn directories_with_a_differing_or_unknown_entry_report_null_with_a_reason() {
    if elevated() {
        return;
    }
    let (temp, source, destination) = populated_fixture();
    std::fs::write(destination.join("fixture"), b"diff").unwrap();
    let (code, report) = verify_json(
        &temp,
        &source,
        &destination,
        "state-diff",
        &["--manifest-roots"],
    );
    assert_eq!(code, Some(1), "{report}");
    let top = manifest_dir(&report, "");
    assert_eq!(top["root"], serde_json::Value::Null);
    assert_eq!(top["reason"], "differing_entry");
    // A sibling folder with nothing wrong still has its root: no all-or-nothing blanking.
    assert!(manifest_dir(&report, "empty")["root"].is_string());

    let (temp, source, destination) = populated_fixture();
    let (code, report) = verify_json(
        &temp,
        &source,
        &destination,
        "state-budget",
        &["--manifest-roots", "--budget-bytes", "0"],
    );
    assert_eq!(code, Some(2), "{report}");
    let top = manifest_dir(&report, "");
    assert_eq!(top["root"], serde_json::Value::Null);
    assert!(
        top["reason"].as_str().is_some_and(|r| !r.is_empty()),
        "{top}"
    );
    drop(temp);
}

#[cfg(windows)]
#[test]
fn only_mode_emits_no_roots() {
    if elevated() {
        return;
    }
    let (temp, source, destination) = populated_fixture();
    let list = temp.path().join("plan.txt");
    std::fs::write(&list, "fixture\nempty\n").unwrap();
    let list = list.to_str().unwrap();
    let (code, report) = verify_json(
        &temp,
        &source,
        &destination,
        "state",
        &["--manifest-roots", "--only", list],
    );
    assert_eq!(code, Some(0), "{report}");
    assert_eq!(
        report["manifest_roots"]["directories"],
        serde_json::Value::Null
    );
    assert_eq!(report["manifest_roots"]["reason"], "selected_set");
    assert!(!report.to_string().contains("\"root\""), "{report}");
}
