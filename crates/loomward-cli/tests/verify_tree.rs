use std::path::Path;
use std::process::{Command, Output};

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
        String::from_utf8_lossy(&output.stderr).contains("whole user-profile folder"),
        "{output:?}"
    );
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
    let (temp, source, destination) = populated_fixture();
    let state = temp.path().join("state");
    let output = verify(&source, &destination, &state, &[]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
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
