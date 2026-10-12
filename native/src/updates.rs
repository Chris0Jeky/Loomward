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

/// The native confirmation text: version, channel, notes, and where the state is kept.
pub fn confirm_text(offer: &Offer, current: &str) -> String {
    let mut notes: String = offer.notes.chars().take(NOTES_IN_BOX).collect();
    if offer.notes.chars().count() > NOTES_IN_BOX {
        notes.push_str(" [...]");
    }
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
    current: Version,
    prefs: Mutex<Prefs>,
    pending: Mutex<Option<(Offer, tauri_plugin_updater::Update)>>,
    /// The native confirmation, `true` only on an explicit OK ([`crate::dialogs::message_box`]).
    ask: Arc<Ask>,
    last_error: Mutex<Option<String>>,
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
            current: Version::parse(current).unwrap_or_else(|_| Version::new(0, 0, 0)),
            prefs: Mutex::new(prefs),
            pending: Mutex::new(None),
            ask: Arc::new(ask),
            last_error: Mutex::new(None),
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
        } else {
            None
        }
    }

    pub fn status(&self) -> Value {
        let offer = lock(&self.pending).as_ref().map(|(o, _)| o.clone());
        json!({
            "current_version": self.current.to_string(),
            "unavailable": self.usable(),
            "prefs": self.prefs(),
            "offer": offer,
            "last_error": *lock(&self.last_error),
        })
    }

    pub fn set_prefs(&self, prefs: Prefs) -> Result<Value, String> {
        let dir = self
            .dir
            .as_deref()
            .ok_or(self.usable().unwrap_or_default())?;
        save_prefs(dir, &prefs)?;
        let old = std::mem::replace(&mut *lock(&self.prefs), prefs);
        if old.channel != prefs.channel {
            *lock(&self.pending) = None; // an offer from the other channel no longer applies
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
        let (channel, url) = self.plan(trigger, previous)?;
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
        *lock(&self.last_error) = result.as_ref().err().cloned();
        let found = result?.map(|update| {
            let offer = Offer {
                version: update.version.clone(),
                channel,
                notes: update.body.clone().unwrap_or_default(),
                date: update.date.map(|d| d.to_string()),
                previous,
            };
            (offer, update)
        });
        *lock(&self.pending) = found;
        Ok(self.status())
    }

    /// Asks natively, then downloads, verifies the signature and runs the installer, which exits
    /// the app. `version` must be the offer the view showed.
    pub async fn install(&self, version: &str, owner: isize) -> Result<Value, String> {
        let Some((offer, update)) = lock(&self.pending).clone() else {
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
        let text = confirm_text(&offer, &self.current.to_string());
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
