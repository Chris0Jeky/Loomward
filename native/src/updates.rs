//! Opt-in updates (HUMAN_TODO q-30, q-31, q-33): a "Check for updates" button, a check-on-start
//! toggle that is off by default, and a channel (`preview`, the default, or `nightly`).
//!
//! AGENTS.md invariant 2 (no silent reach): [`may_request`] is the only gate in front of a network
//! request, and it opens only for an explicit user action or for a start the owner opted into.
//! Nothing here runs a request in a build without an updater public key
//! (`plugins.updater.pubkey` in `tauri.conf.json`): such a build answers "not configured".
//!
//! The webview reaches this through one command, `lw_update`, whose actions take no URL, path or
//! file: the endpoints are compiled in ([`endpoint`]) and the Tauri updater plugin's own commands
//! stay outside the capability (docs/41 section 12). Installing needs a second, native
//! confirmation the webview cannot answer, as the folder picker and the disclosure box do.

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Where each channel's `latest.json` lives (`.github/workflows/release.yml`, `nightly.yml`).
pub const RELEASES: &str = "https://github.com/Chris0Jeky/Loomward/releases/download";
/// Prefs file inside the state directory, so it is copied and restored with the state.
pub const PREFS_FILE: &str = "updates.json";
/// Why the previous nightly is shown but not installed in-app (LW-122).
pub const ROLLBACK_UNAVAILABLE: &str = "rolling back in the app is not available yet: an older build refuses state.db and catalog.db once a newer build migrated them, so the previous nightly is a manual download plus a restore of the matching state copy with catalog.db moved aside (RELEASING.md, Nightly)";
pub const CONFIRM_TITLE: &str = "Install a Loomward update";
/// Release notes longer than this are cut in the native box (MessageBoxW cannot scroll).
const NOTES_IN_BOX: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    #[default]
    Preview,
    Nightly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub check_on_start: bool,
    pub channel: Channel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The app started; only an opted-in start may check.
    Startup,
    /// The owner pressed a button in the Updates view.
    User,
}

/// The one gate in front of every update request.
pub fn may_request(trigger: Trigger, prefs: &Prefs, configured: bool) -> bool {
    configured
        && match trigger {
            Trigger::User => true,
            Trigger::Startup => prefs.check_on_start,
        }
}

/// The channel's `latest.json`; `previous` is the nightly before the current one (rollback).
/// Previews keep every version as its own release, so they have no rolling "previous".
pub fn endpoint(channel: Channel, previous: bool) -> Option<String> {
    let tag = match (channel, previous) {
        (Channel::Preview, false) => "preview-latest",
        (Channel::Nightly, false) => "nightly",
        (Channel::Nightly, true) => "nightly-previous",
        (Channel::Preview, true) => return None,
    };
    Some(format!("{RELEASES}/{tag}/latest.json"))
}

/// The channel a build came from, by its pre-release tag (`0.3.0-preview.1`,
/// `0.3.0-nightly.20261012.7`). A local build (`0.1.0`) belongs to neither.
pub fn channel_of(v: &Version) -> Option<Channel> {
    match v.pre.as_str().split('.').next() {
        Some("preview") => Some(Channel::Preview),
        Some("nightly") => Some(Channel::Nightly),
        _ => None,
    }
}

/// Whether `candidate` is offered to a build at `current` following `selected`. Never an older
/// build: an older binary refuses state a newer one migrated, and a rollback must restore the
/// matching state snapshot (LW-122), which this updater does not do.
/// - Inside one channel: semver-newer (the nightly date and run number compare numerically).
/// - Switching channel (or from a local build): a newer core only. Inside one core the two
///   channels cannot be ordered (a preview may be dispatched after the newest nightly, or before
///   it), so a same-core switch is a manual install.
/// - `previous`: any other version, shown so the owner can find it; [`Updates::install`] refuses it.
pub fn is_offer(current: &Version, candidate: &Version, selected: Channel, previous: bool) -> bool {
    let core = |v: &Version| (v.major, v.minor, v.patch);
    if previous {
        return candidate != current;
    }
    match (channel_of(current), selected) {
        (Some(from), to) if from == to => candidate > current,
        _ => core(candidate) > core(current),
    }
}

/// Missing or unreadable prefs are the defaults: off, preview.
pub fn load_prefs(dir: &Path) -> Prefs {
    std::fs::read(dir.join(PREFS_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_prefs(dir: &Path, prefs: &Prefs) -> Result<(), String> {
    let tmp = dir.join(format!("{PREFS_FILE}.tmp"));
    let bytes = serde_json::to_vec_pretty(prefs).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, dir.join(PREFS_FILE)))
        .map_err(|e| format!("could not save the update settings: {e}"))
}

/// Release notes are network text: line breaks become spaces, the text is cut to
/// [`NOTES_IN_BOX`] characters before escaping (so no escape is split), and hidden characters are
/// escaped as in the grant confirmation while letters stay readable (review of #978).
pub fn release_notes_text(notes: &str) -> String {
    let flat: String = notes
        .chars()
        .map(|c| {
            if matches!(c, '\n' | '\r' | '\t') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let cut: String = flat.chars().take(NOTES_IN_BOX).collect();
    let mut shown = crate::dialogs::readable(&cut);
    if flat.chars().count() > NOTES_IN_BOX {
        shown.push_str(" [...]");
    }
    shown
}

/// The native confirmation text: version, channel, notes, and where the state is kept.
pub fn confirm_text(offer: &Offer, current: &str) -> String {
    let cleaned = release_notes_text(&offer.notes);
    let notes = if cleaned.is_empty() {
        "Release notes: none published.".to_string()
    } else {
        format!("Release notes (text from the release, not checked by Loomward):\n{cleaned}")
    };
    let kind = if offer.previous {
        "the previous nightly"
    } else {
        "an update"
    };
    format!(
        "Install {kind} of Loomward?\n\nVersion: {} ({} channel)\nRunning now: {current}\n\n{notes}\n\n\
         Loomward closes, the installer runs, and Loomward starts again. Your state in \
         %LOCALAPPDATA%\\Loomward Data stays where it is; a build that changes its schema copies \
         state.db first (state.before-v*.db) and migrates catalog.db in place. Unsigned build: \
         install only what you expect.\n\n\
         OK installs. Cancel (the default) changes nothing.",
        offer.version,
        match offer.channel {
            Channel::Preview => "preview",
            Channel::Nightly => "nightly",
        },
    )
}

/// What a check found, kept until the owner installs it or checks again.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Offer {
    pub version: String,
    pub channel: Channel,
    pub notes: String,
    pub date: Option<String>,
    pub previous: bool,
}

/// One `lw_update` request. Nothing here names a URL, a path or a file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Status,
    SetPrefs {
        check_on_start: bool,
        channel: Channel,
    },
    Check {
        previous: bool,
    },
    Install {
        version: String,
    },
}

type Ask = dyn Fn(isize, &str, &str) -> bool + Send + Sync;

/// The updater state the shell manages: where the prefs live and the pending offer.
pub struct Updates {
    /// `None` in `--fixtures` mode, which keeps no state and never updates.
    dir: Option<PathBuf>,
    /// `plugins.updater.pubkey` is set: without it no signature can be checked, so no request.
    configured: bool,
    /// Preserve the reported identity; malformed versions make the updater unavailable.
    current: String,
    prefs: Mutex<Prefs>,
    checks: Mutex<CheckState<(Offer, tauri_plugin_updater::Update)>>,
    /// The native confirmation, `true` only on an explicit OK ([`crate::dialogs::message_box`]).
    ask: Arc<Ask>,
}

impl Updates {
    pub fn new(dir: Option<PathBuf>, configured: bool, current: &str) -> Self {
        Self::with_prompt(dir, configured, current, crate::dialogs::message_box)
    }

    pub fn with_prompt(
        dir: Option<PathBuf>,
        configured: bool,
        current: &str,
        ask: impl Fn(isize, &str, &str) -> bool + Send + Sync + 'static,
    ) -> Self {
        let prefs = dir.as_deref().map(load_prefs).unwrap_or_default();
        Self {
            dir,
            configured,
            current: current.into(),
            prefs: Mutex::new(prefs),
            checks: Mutex::new(CheckState::new()),
            ask: Arc::new(ask),
        }
    }

    pub fn prefs(&self) -> Prefs {
        *lock(&self.prefs)
    }

    fn usable(&self) -> Option<&'static str> {
        if self.dir.is_none() {
            Some("updates are off in --fixtures mode")
        } else if !self.configured {
            Some("updates are not configured in this build")
        } else if Version::parse(&self.current).is_err() {
            Some("the running version is invalid; updates are unavailable")
        } else {
            None
        }
    }

    pub fn status(&self) -> Value {
        let checks = lock(&self.checks);
        let offer = checks.pending.as_ref().map(|(o, _)| o.clone());
        json!({
            "current_version": self.current,
            "unavailable": self.usable(),
            "prefs": self.prefs(),
            "offer": offer,
            "last_error": checks.last_error,
        })
    }

    pub fn set_prefs(&self, prefs: Prefs) -> Result<Value, String> {
        let dir = self
            .dir
            .as_deref()
            .ok_or(self.usable().unwrap_or_default())?;
        save_prefs(dir, &prefs)?;
        {
            let mut checks = lock(&self.checks);
            let old = std::mem::replace(&mut *lock(&self.prefs), prefs);
            if old.channel != prefs.channel {
                checks.invalidate(); // offers and in-flight checks from the other channel no longer apply
            }
        }
        Ok(self.status())
    }

    /// The request a check would make, or why none is made.
    pub fn plan(&self, trigger: Trigger, previous: bool) -> Result<(Channel, String), String> {
        if let Some(why) = self.usable() {
            return Err(why.into());
        }
        let prefs = self.prefs();
        if !may_request(trigger, &prefs, self.configured) {
            return Err("checking on start is off".into());
        }
        let url = endpoint(prefs.channel, previous)
            .ok_or("only the nightly channel keeps a previous build")?;
        Ok((prefs.channel, url))
    }

    pub async fn check<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        trigger: Trigger,
        previous: bool,
    ) -> Result<Value, String> {
        use tauri_plugin_updater::UpdaterExt;
        let (generation, channel, url) = {
            let mut checks = lock(&self.checks);
            let (channel, url) = self.plan(trigger, previous)?;
            (checks.begin(), channel, url)
        };
        let result = async {
            let url = url.parse().map_err(|e| format!("{e}"))?;
            let updater = app
                .updater_builder()
                .endpoints(vec![url])
                .map_err(|e| e.to_string())?
                .version_comparator(move |current, release| {
                    is_offer(&current, &release.version, channel, previous)
                })
                .build()
                .map_err(|e| e.to_string())?;
            updater.check().await.map_err(|e| e.to_string())
        }
        .await;
        let outcome = result.map(|found| {
            found.map(|update| {
                let offer = Offer {
                    version: update.version.clone(),
                    channel,
                    notes: update.body.clone().unwrap_or_default(),
                    date: update.date.map(|d| d.to_string()),
                    previous,
                };
                (offer, update)
            })
        });
        lock(&self.checks).settle(generation, outcome)?;
        Ok(self.status())
    }

    /// Asks natively, then downloads, verifies the signature and runs the installer, which exits
    /// the app. `version` must be the offer the view showed.
    pub async fn install(&self, version: &str, owner: isize) -> Result<Value, String> {
        let Some((offer, update)) = lock(&self.checks).pending.clone() else {
            return Err("check for updates first".into());
        };
        if offer.version != version {
            return Err("the offer changed; check again".into());
        }
        if offer.previous {
            // LW-122: a rollback restores the binary together with its matching pre-migration
            // state snapshot. Until that drill exists the previous nightly is a manual download.
            return Err(ROLLBACK_UNAVAILABLE.into());
        }
        let text = confirm_text(&offer, &self.current);
        let ask = self.ask.clone();
        // The box is modal and blocking: off the async runtime, as `lw_call` runs its dialogs.
        let accepted =
            tauri::async_runtime::spawn_blocking(move || ask(owner, CONFIRM_TITLE, &text))
                .await
                .unwrap_or(false);
        if !accepted {
            return Ok(json!({ "installed": false }));
        }
        // On Windows this exits the process once the installer has started.
        update
            .download_and_install(|_, _| {}, || {})
            .await
            .map_err(|e| e.to_string())?;
        Ok(json!({ "installed": true }))
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Outcomes share one lock with their request generation. A channel change or newer accepted
/// check supersedes every older reply, regardless of the order in which network requests finish.
struct CheckState<T> {
    generation: u64,
    pending: Option<T>,
    last_error: Option<String>,
}

impl<T> CheckState<T> {
    fn new() -> Self {
        Self {
            generation: 0,
            pending: None,
            last_error: None,
        }
    }

    fn begin(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    fn invalidate(&mut self) {
        self.begin();
        self.pending = None;
        self.last_error = None;
    }

    /// A failed current check also clears the pending offer: it is no longer known to be current.
    fn settle(
        &mut self,
        generation: u64,
        outcome: Result<Option<T>, String>,
    ) -> Result<(), String> {
        if generation != self.generation {
            return Ok(());
        }
        match outcome {
            Ok(found) => {
                self.last_error = None;
                self.pending = found;
                Ok(())
            }
            Err(e) => {
                self.last_error = Some(e.clone());
                self.pending = None;
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod check_state_tests {
    use super::CheckState;

    #[test]
    fn a_late_failure_does_not_clear_a_newer_offer() {
        let mut checks = CheckState::new();
        let startup = checks.begin();
        let user = checks.begin();
        checks.settle(user, Ok(Some("new offer"))).unwrap();
        assert!(checks.settle(startup, Err("old timeout".into())).is_ok());
        assert_eq!(checks.pending, Some("new offer"));
        assert_eq!(checks.last_error, None);
    }

    #[test]
    fn a_late_offer_does_not_replace_a_newer_no_update_result() {
        let mut checks = CheckState::new();
        let old = checks.begin();
        let latest = checks.begin();
        checks.settle(latest, Ok(None)).unwrap();
        checks.settle(old, Ok(Some("old offer"))).unwrap();
        assert_eq!(checks.pending, None);
        assert_eq!(checks.last_error, None);
    }

    #[test]
    fn a_late_offer_does_not_hide_a_newer_error() {
        let mut checks = CheckState::new();
        let old = checks.begin();
        let latest = checks.begin();
        assert!(checks
            .settle(latest, Err("current timeout".into()))
            .is_err());
        checks.settle(old, Ok(Some("old offer"))).unwrap();
        assert_eq!(checks.pending, None);
        assert_eq!(checks.last_error, Some("current timeout".into()));
    }

    #[test]
    fn a_channel_change_invalidates_pending_and_in_flight_results() {
        let mut checks = CheckState::new();
        let previous = checks.begin();
        checks
            .settle(previous, Ok(Some("previous channel")))
            .unwrap();
        let in_flight = checks.begin();
        checks.invalidate();
        assert_eq!(checks.pending, None);
        checks.settle(in_flight, Ok(Some("old channel"))).unwrap();
        assert_eq!(checks.pending, None);
        assert!(checks
            .settle(in_flight, Err("old channel failed".into()))
            .is_ok());
        assert_eq!(checks.last_error, None);
    }
}

/// `plugins.updater.pubkey` from the app's config: empty until the owner provides the key.
pub fn configured(config: &tauri::Config) -> bool {
    config
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("pubkey"))
        .and_then(Value::as_str)
        .is_some_and(|k| !k.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn defaults_are_off_and_preview() {
        let p = Prefs::default();
        assert!(!p.check_on_start);
        assert_eq!(p.channel, Channel::Preview);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_prefs(dir.path()), Prefs::default());
        std::fs::write(dir.path().join(PREFS_FILE), b"{not json").unwrap();
        assert_eq!(load_prefs(dir.path()), Prefs::default());
        std::fs::write(dir.path().join(PREFS_FILE), br#"{"channel":"nightly"}"#).unwrap();
        assert_eq!(
            load_prefs(dir.path()),
            Prefs {
                check_on_start: false,
                channel: Channel::Nightly
            }
        );
    }

    #[test]
    fn no_request_without_a_user_action_or_the_opt_in() {
        let off = Prefs::default();
        let on = Prefs {
            check_on_start: true,
            ..off
        };
        assert!(!may_request(Trigger::Startup, &off, true));
        assert!(may_request(Trigger::Startup, &on, true));
        assert!(may_request(Trigger::User, &off, true));
        // No public key: nothing is ever requested.
        for (t, p) in [
            (Trigger::Startup, on),
            (Trigger::User, off),
            (Trigger::User, on),
        ] {
            assert!(!may_request(t, &p, false));
        }
    }

    #[test]
    fn plan_refuses_before_any_request() {
        let dir = tempfile::tempdir().unwrap();
        let ask = |_: isize, _: &str, _: &str| -> bool { panic!("no dialog") };
        let fixtures = Updates::with_prompt(None, true, "0.3.0-nightly.20261012.1", ask);
        assert!(fixtures.plan(Trigger::User, false).is_err());
        let unsigned = Updates::with_prompt(Some(dir.path().into()), false, "0.1.0", ask);
        assert_eq!(
            unsigned.plan(Trigger::User, false).unwrap_err(),
            "updates are not configured in this build"
        );
        let u = Updates::with_prompt(Some(dir.path().into()), true, "0.1.0", ask);
        assert!(u.plan(Trigger::Startup, false).is_err(), "off by default");
        assert!(
            u.plan(Trigger::User, true).is_err(),
            "previews have no previous"
        );
        assert_eq!(
            u.plan(Trigger::User, false).unwrap().1,
            format!("{RELEASES}/preview-latest/latest.json")
        );
        u.set_prefs(Prefs {
            check_on_start: true,
            channel: Channel::Nightly,
        })
        .unwrap();
        assert_eq!(
            u.plan(Trigger::Startup, false).unwrap(),
            (Channel::Nightly, format!("{RELEASES}/nightly/latest.json"))
        );
        assert_eq!(
            u.plan(Trigger::User, true).unwrap().1,
            format!("{RELEASES}/nightly-previous/latest.json")
        );
        // Persisted beside the state, read back by the next start.
        assert_eq!(load_prefs(dir.path()).channel, Channel::Nightly);
        assert!(load_prefs(dir.path()).check_on_start);
    }

    #[test]
    fn a_failed_recheck_clears_the_previous_offer() {
        let offer = Offer {
            version: "0.4.0-nightly.20261012.7".into(),
            channel: Channel::Nightly,
            notes: String::new(),
            date: None,
            previous: false,
        };
        let mut checks = CheckState::new();
        checks.pending = Some((offer.clone(), ()));
        let failed = checks.begin();
        // The owner checked, saw the offer, and a second check failed on the network.
        assert!(checks.settle(failed, Err("timed out".into())).is_err());
        assert!(
            checks.pending.is_none(),
            "a failed re-check left the superseded offer installable"
        );
        assert_eq!(checks.last_error, Some("timed out".to_string()));
        // A later successful check that finds the build offers it afresh and clears the error.
        let success = checks.begin();
        checks
            .settle(success, Ok(Some((offer.clone(), ()))))
            .unwrap();
        assert_eq!(checks.pending.as_ref().map(|(o, _)| o.clone()), Some(offer));
        assert!(checks.last_error.is_none());
    }

    #[test]
    fn switching_channel_invalidates_an_in_flight_check() {
        let dir = tempfile::tempdir().unwrap();
        let u = Updates::with_prompt(Some(dir.path().into()), true, "0.1.0", |_, _, _| false);
        let old = lock(&u.checks).begin();
        u.set_prefs(Prefs {
            check_on_start: false,
            channel: Channel::Nightly,
        })
        .unwrap();
        let mut checks = lock(&u.checks);
        assert!(checks.settle(old, Err("old channel failed".into())).is_ok());
        assert!(checks.pending.is_none());
        assert!(checks.last_error.is_none());
    }

    #[test]
    fn channel_endpoints() {
        assert_eq!(
            endpoint(Channel::Preview, false).unwrap(),
            "https://github.com/Chris0Jeky/Loomward/releases/download/preview-latest/latest.json"
        );
        assert_eq!(
            endpoint(Channel::Nightly, false).unwrap(),
            "https://github.com/Chris0Jeky/Loomward/releases/download/nightly/latest.json"
        );
        assert!(endpoint(Channel::Nightly, true)
            .unwrap()
            .ends_with("/nightly-previous/latest.json"));
        assert_eq!(endpoint(Channel::Preview, true), None);
    }

    #[test]
    fn version_comparison_with_nightly_suffixes() {
        let n = |s| v(&format!("0.3.0-nightly.{s}"));
        let offer = |c: Version, d: Version, ch, prev| is_offer(&c, &d, ch, prev);
        use Channel::{Nightly, Preview};
        assert_eq!(channel_of(&n("20261012.7")), Some(Nightly));
        assert_eq!(channel_of(&v("0.3.0-preview.2")), Some(Preview));
        assert_eq!(channel_of(&v("0.1.0")), None);
        // Numeric identifiers: run 10 is newer than run 9, a later day newer than an earlier one.
        assert!(offer(n("20261012.9"), n("20261012.10"), Nightly, false));
        assert!(offer(n("20261012.9"), n("20261013.1"), Nightly, false));
        assert!(!offer(n("20261013.1"), n("20261012.9"), Nightly, false));
        assert!(!offer(n("20261012.9"), n("20261012.9"), Nightly, false));
        // A newer core wins over any nightly suffix.
        assert!(offer(
            n("20261012.9"),
            v("0.4.0-nightly.20261001.1"),
            Nightly,
            false
        ));
        // Previews compare by their number.
        assert!(offer(
            v("0.3.0-preview.1"),
            v("0.3.0-preview.2"),
            Preview,
            false
        ));
        assert!(!offer(
            v("0.3.0-preview.2"),
            v("0.3.0-preview.1"),
            Preview,
            false
        ));
        // Preview -> nightly of the same core is not offered: a preview dispatched after the
        // newest nightly would otherwise be replaced by older code. A newer core is offered.
        assert!(!offer(
            v("0.3.0-preview.1"),
            n("20261012.9"),
            Nightly,
            false
        ));
        assert!(!offer(
            v("0.3.0-preview.2"),
            n("20261013.40"),
            Nightly,
            false
        ));
        assert!(offer(
            v("0.3.0-preview.1"),
            v("0.4.0-nightly.20261101.1"),
            Nightly,
            false
        ));
        assert!(!offer(
            v("0.4.0-preview.1"),
            n("20261012.9"),
            Nightly,
            false
        ));
        // Nightly -> preview only for a newer core: never a downgrade.
        assert!(!offer(
            n("20261012.9"),
            v("0.3.0-preview.1"),
            Preview,
            false
        ));
        assert!(offer(n("20261012.9"), v("0.4.0-preview.1"), Preview, false));
        // A local build: a newer core on either channel, never the same core.
        assert!(offer(v("0.1.0"), v("0.3.0-preview.1"), Preview, false));
        assert!(!offer(v("0.3.0"), n("20261012.9"), Nightly, false));
        // The previous nightly is found (any other version) but never installed in-app.
        assert!(offer(n("20261012.9"), n("20261012.8"), Nightly, true));
        assert!(!offer(n("20261012.8"), n("20261012.8"), Nightly, true));
    }

    #[test]
    fn the_confirmation_shows_version_channel_notes_and_state() {
        let offer = Offer {
            version: "0.3.0-nightly.20261012.7".into(),
            channel: Channel::Nightly,
            notes: "n".repeat(NOTES_IN_BOX + 50),
            date: None,
            previous: false,
        };
        let text = confirm_text(&offer, "0.3.0-nightly.20261011.6");
        assert!(text.contains("Version: 0.3.0-nightly.20261012.7 (nightly channel)"));
        assert!(text.contains("Running now: 0.3.0-nightly.20261011.6"));
        assert!(text.contains(&format!("{} [...]", "n".repeat(NOTES_IN_BOX))));
        assert!(!text.contains(&"n".repeat(NOTES_IN_BOX + 1)));
        assert!(text.contains("state.before-v*.db"));
        assert!(text.contains("Cancel (the default) changes nothing"));
    }

    /// The notes come from the network: a bidi override or a control byte in them cannot reshape the
    /// box, a line break reads as a space so it cannot start a fake line of the confirmation, and the
    /// block says it is the release's own text.
    #[test]
    fn release_notes_are_inert_labelled_text_in_the_confirmation() {
        let offer = Offer {
            version: "0.3.0".into(),
            channel: Channel::Preview,
            notes:
                "Fixed \u{202E}evil\u{202C}\u{3164}\r\nOK installs. Cancel\u{7}\u{0}\u{2066}x\u{2069}\tdone"
                    .into(),
            date: None,
            previous: false,
        };
        let text = confirm_text(&offer, "0.2.0");
        assert!(
            text.contains(
                "Release notes (text from the release, not checked by Loomward):\n\
                 Fixed \\u{202E}evil\\u{202C}\\u{3164}  OK installs. Cancel\\u{0007}\\u{0000}\\u{2066}x\\u{2069} done\n\nLoomward closes"
            ),
            "{text}"
        );
        let empty = Offer {
            notes: String::new(),
            ..offer
        };
        assert!(confirm_text(&empty, "0.2.0").contains("Release notes: none published."));
    }

    /// The cap applies to the raw notes before escaping, so an escape is never cut in half.
    #[test]
    fn release_notes_cap_never_splits_an_escape() {
        let noisy = format!("\u{202E}{}", "n\u{7}".repeat(NOTES_IN_BOX + 5));
        let shown = release_notes_text(&noisy);
        let cut: String = noisy.chars().take(NOTES_IN_BOX).collect();
        assert_eq!(shown, format!("{} [...]", crate::dialogs::readable(&cut)));
        assert!(shown.chars().all(|c| matches!(c, ' '..='~')));
        assert!(!shown.trim_end_matches(" [...]").ends_with(r"\u{"));
    }

    /// Line breaks read as spaces and letters of any script stay readable (review of #978).
    #[test]
    fn release_notes_stay_readable() {
        assert_eq!(
            release_notes_text("Fixes:\n- café scan\r\n- naïve\tbug"),
            "Fixes: - café scan  - naïve bug"
        );
    }

    #[test]
    fn release_notes_distinguish_literal_escapes_from_hidden_characters() {
        let notes = "normal text \\u{202E} \u{202E}\u{3164}\u{2800}";
        assert_eq!(
            release_notes_text(notes),
            r"normal text \\u{202E} \u{202E}\u{3164}\u{2800}"
        );
        assert_eq!(release_notes_text("normal text"), "normal text");
    }

    #[test]
    fn actions_take_no_url_or_path() {
        let parse = |v: Value| serde_json::from_value::<Action>(v);
        assert_eq!(parse(json!({"action": "status"})).unwrap(), Action::Status);
        assert!(
            parse(json!({"action": "check", "previous": false, "url": "https://evil"})).is_err()
        );
        assert!(
            parse(json!({"action": "set_prefs", "check_on_start": true, "channel": "beta"}))
                .is_err()
        );
        assert!(parse(json!({"action": "download", "path": "C:\\x.exe"})).is_err());
    }
}
