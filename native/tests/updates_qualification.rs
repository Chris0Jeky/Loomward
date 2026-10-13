//! Updater qualification after #445. No network, dialog, installer or owner data.
use loomward_desktop::updates::{Channel, Prefs, Trigger, Updates};

const INVALID_VERSION: &str = "the running version is invalid; updates are unavailable";

#[test]
fn malformed_running_versions_refuse_before_planning_a_request() {
    for current in ["", "not-a-version", "0.3", "\n0.3.0", "0.3.0 ", "v0.3.0"] {
        let tmp = tempfile::tempdir().unwrap();
        let updates =
            Updates::with_prompt(Some(tmp.path().to_path_buf()), true, current, |_, _, _| {
                panic!("planning must not open a dialog")
            });
        assert_eq!(
            updates.plan(Trigger::User, false).unwrap_err(),
            INVALID_VERSION,
            "invalid running version {current:?} must not become 0.0.0"
        );
        let status = updates.status();
        assert_eq!(status["current_version"], current);
        assert_eq!(status["unavailable"], INVALID_VERSION);
        assert!(status["offer"].is_null());
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }
}

#[test]
fn valid_configured_version_can_plan_without_side_effects() {
    for current in [
        "0.0.0",
        "0.3.0-preview.1",
        "0.3.0-nightly.20261013.7+fixture",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let updates =
            Updates::with_prompt(Some(tmp.path().to_path_buf()), true, current, |_, _, _| {
                panic!("planning must not open a dialog")
            });
        assert!(updates.plan(Trigger::User, false).is_ok());
        assert_eq!(updates.status()["current_version"], current);
        assert!(updates.status()["unavailable"].is_null());
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }
}

#[test]
fn fixture_and_unconfigured_builds_still_refuse_planning() {
    let tmp = tempfile::tempdir().unwrap();
    for (dir, configured, expected) in [
        (None, true, "updates are off in --fixtures mode"),
        (
            Some(tmp.path().to_path_buf()),
            false,
            "updates are not configured in this build",
        ),
    ] {
        let updates = Updates::with_prompt(dir, configured, "0.3.0-preview.1", |_, _, _| {
            panic!("planning must not open a dialog")
        });
        assert_eq!(updates.plan(Trigger::User, false).unwrap_err(), expected);
    }
}

#[test]
fn opting_in_or_switching_channel_cannot_rehabilitate_an_invalid_version() {
    let tmp = tempfile::tempdir().unwrap();
    let updates = Updates::with_prompt(Some(tmp.path().into()), true, "unknown", |_, _, _| {
        panic!("planning must not open a dialog")
    });
    for channel in [Channel::Preview, Channel::Nightly] {
        let status = updates
            .set_prefs(Prefs {
                check_on_start: true,
                channel,
            })
            .unwrap();
        assert_eq!(status["unavailable"], INVALID_VERSION);
        for trigger in [Trigger::Startup, Trigger::User] {
            for previous in [false, true] {
                assert_eq!(
                    updates.plan(trigger, previous).unwrap_err(),
                    INVALID_VERSION
                );
            }
        }
    }
}
