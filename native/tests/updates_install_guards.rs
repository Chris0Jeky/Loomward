//! Public installer preflight canaries after #447. No network, installer or native dialog.
use loomward_desktop::updates::{Channel, Prefs, Updates, PREFS_FILE};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[test]
fn install_without_an_offer_never_prompts_or_changes_state() {
    let tmp = tempfile::tempdir().unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let prompts = asked.clone();
    let updates = Updates::with_prompt(
        Some(tmp.path().into()),
        true,
        "0.3.0-preview.1",
        move |_, _, _| {
            prompts.fetch_add(1, Ordering::SeqCst);
            false
        },
    );
    for channel in [Channel::Preview, Channel::Nightly] {
        updates
            .set_prefs(Prefs {
                check_on_start: true,
                channel,
            })
            .unwrap();
        let before = updates.status();
        let prefs = std::fs::read(tmp.path().join(PREFS_FILE)).unwrap();
        for requested in ["", "unknown", "0.4.0-preview.1", "0.3.0-nightly.20261013.1"] {
            let result = tauri::async_runtime::block_on(updates.install(requested, 0));
            assert_eq!(result.unwrap_err(), "check for updates first");
            assert_eq!(asked.load(Ordering::SeqCst), 0);
            assert_eq!(updates.status(), before);
            assert_eq!(std::fs::read(tmp.path().join(PREFS_FILE)).unwrap(), prefs);
        }
    }
}

#[test]
fn fixtures_and_unconfigured_builds_cannot_install_without_an_offer() {
    let tmp = tempfile::tempdir().unwrap();
    for (dir, configured) in [(None, true), (Some(tmp.path().into()), false)] {
        let updates = Updates::with_prompt(dir, configured, "0.3.0-preview.1", |_, _, _| {
            panic!("an unavailable build must not ask to install")
        });
        let before = updates.status();
        let result = tauri::async_runtime::block_on(updates.install("0.4.0-preview.1", 0));
        assert_eq!(result.unwrap_err(), "check for updates first");
        assert_eq!(updates.status(), before);
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }
}
