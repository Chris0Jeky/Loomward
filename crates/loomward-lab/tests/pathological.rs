#![cfg(windows)]

use loomward_lab::pathological::{generate, Profile};
use std::{fs, path::PathBuf};

fn temp() -> Option<tempfile::TempDir> {
    let temp = tempfile::tempdir().unwrap();
    if loomward_windows::enumerate::running_elevated() {
        assert!(
            matches!(generate(temp.path(), Profile::LongPath), Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied)
        );
        // LOOMWARD_REQUIRE_NATIVE=1 makes a host that cannot run the native case a failure.
        assert!(
            std::env::var("LOOMWARD_REQUIRE_NATIVE").as_deref() != Ok("1"),
            "LOOMWARD_REQUIRE_NATIVE=1 but native pathological fixtures refuse an elevated token"
        );
        eprintln!("UNVERIFIED: native pathological fixtures refuse an elevated token");
        return None;
    }
    Some(temp)
}

#[test]
fn pathological_profiles_require_an_empty_plain_directory() {
    let Some(temp) = temp() else { return };
    fs::write(temp.path().join("keeper"), b"keep").unwrap();
    assert!(generate(temp.path(), Profile::LongPath).is_err());
    assert_eq!(fs::read(temp.path().join("keeper")).unwrap(), b"keep");
    assert!(generate(&PathBuf::from(r"C:\"), Profile::LongPath).is_err());
}

#[test]
fn churn_profile_has_exact_bounded_file_count() {
    let Some(temp) = temp() else { return };
    assert!(generate(temp.path(), Profile::Churn { files: 0 }).is_err());
    let tree = generate(temp.path(), Profile::Churn { files: 1001 }).unwrap();
    let mut files = 0;
    for directory in fs::read_dir(&tree.root).unwrap() {
        files += fs::read_dir(directory.unwrap().path()).unwrap().count();
    }
    assert_eq!(files, 1001);
    assert!(tree.leaf.is_file());
}

#[test]
fn denied_profile_restores_access_before_its_owner_removes_the_tree() {
    let Some(temp) = temp() else { return };
    let tree = generate(temp.path(), Profile::DeniedDirectory).unwrap();
    let denied = tree.leaf.parent().unwrap().to_owned();
    assert_eq!(
        fs::read_dir(&denied).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    drop(tree);
    assert_eq!(fs::read_dir(denied).unwrap().count(), 1);
    temp.close().unwrap();
}

#[test]
fn depth_profile_reaches_the_writer_limit_including_the_root() {
    let Some(temp) = temp() else { return };
    let tree = generate(temp.path(), Profile::DepthLimit).unwrap();
    let relative = tree.leaf.strip_prefix(&tree.root).unwrap();
    assert_eq!(relative.components().count(), 512); // 511 directories and the leaf file.
    assert_eq!(fs::read(&tree.leaf).unwrap(), b"synthetic");
    drop(tree);
    temp.close().unwrap();
}

#[test]
fn junction_profile_treats_shell_metacharacters_as_path_data() {
    let Some(temp) = temp() else { return };
    let root = temp.path().join("synthetic&names");
    fs::create_dir(&root).unwrap();
    let tree = generate(&root, Profile::JunctionCycle).unwrap();
    assert!(root.join("cycle/synthetic.bin").is_file());
    drop(tree);
    assert!(!root.join("cycle").exists());
    temp.close().unwrap();
}
