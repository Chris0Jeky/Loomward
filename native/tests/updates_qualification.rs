//! Pure updater qualification after #445. No network, dialog, installer or owner data.
use loomward_desktop::updates::{Trigger, Updates};

#[test]
fn malformed_running_versions_refuse_before_planning_a_request() {
    for current in ["", "not-a-version", "0.3", "\n0.3.0"] {
        let tmp = tempfile::tempdir().unwrap();
        let updates = Updates::with_prompt(
            Some(tmp.path().to_path_buf()),
            true,
            current,
            |_, _, _| panic!("planning must not open a dialog"),
        );
        assert!(
            updates.plan(Trigger::User, false).is_err(),
            "invalid running version {current:?} must not become 0.0.0"
        );
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }
}

#[test]
fn valid_configured_version_can_plan_without_side_effects() {
    let tmp = tempfile::tempdir().unwrap();
    let updates = Updates::with_prompt(
        Some(tmp.path().to_path_buf()),
        true,
        "0.3.0-preview.1",
        |_, _, _| panic!("planning must not open a dialog"),
    );
    assert!(updates.plan(Trigger::User, false).is_ok());
    assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
}

#[test]
fn fixture_and_unconfigured_builds_still_refuse_planning() {
    let tmp = tempfile::tempdir().unwrap();
    for (dir, configured) in [(None, true), (Some(tmp.path().to_path_buf()), false)] {
        let updates = Updates::with_prompt(dir, configured, "0.3.0-preview.1", |_, _, _| {
            panic!("planning must not open a dialog")
        });
        assert!(updates.plan(Trigger::User, false).is_err());
    }
}
