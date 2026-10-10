//! [`NativeDialogs`] for the desktop: the folder picker and the disclosure confirmation, owned
//! by Rust (docs/41 sections 5.3 and 12). The webview cannot open either: the capability file
//! grants no dialog permission, no dialog plugin is registered, and the only way in is a service
//! call that passes the [`CallContext`]. The chosen path never crosses IPC.

use loomward_protocol::{DatasetClass, DisclosureSummary, NativeDialogs};
use serde::Serialize;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};

/// Shows a title and text; `true` only on explicit accept.
type Prompt = dyn Fn(&str, &str) -> bool + Send + Sync;

pub struct DesktopDialogs {
    dataset: DatasetClass,
    /// Shows the confirmation text and returns `true` only on explicit accept. The real one is
    /// [`message_box`]; tests inject a stand-in.
    ask: Box<Prompt>,
    active: Mutex<()>,
    owner: Arc<AtomicIsize>,
}

impl DesktopDialogs {
    pub fn new(dataset: DatasetClass) -> Self {
        let owner = Arc::new(AtomicIsize::new(0));
        let handle = owner.clone();
        let mut dialogs = Self::with_prompt(dataset, move |title, text| {
            message_box(handle.load(Ordering::SeqCst), title, text)
        });
        dialogs.owner = owner;
        dialogs
    }

    pub fn set_owner(&self, hwnd: isize) {
        self.owner.store(hwnd, Ordering::SeqCst);
    }

    pub fn with_prompt(
        dataset: DatasetClass,
        ask: impl Fn(&str, &str) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            dataset,
            ask: Box::new(ask),
            active: Mutex::new(()),
            owner: Arc::new(AtomicIsize::new(0)),
        }
    }
}

pub const DISCLOSURE_TITLE: &str = "Loomward: send these items to the teacher?";
/// Conservative temporary MessageBox budget; personal use needs LW-111's scrolling dialog.
pub const MAX_DISCLOSURE_CHARS: usize = 2048;

impl NativeDialogs for DesktopDialogs {
    /// Never opens in a synthetic session: an owner-picked folder is personal data
    /// (`synthetic_session_requires_lab_root`, docs/41 section 12).
    fn pick_folder(&self) -> Option<PathBuf> {
        if self.dataset != DatasetClass::Personal {
            return None;
        }
        let _active = self.active.try_lock().ok()?;
        let picked = rfd::FileDialog::new()
            .set_title("Loomward: choose a folder to observe (metadata only)")
            .pick_folder()?;
        let profile = std::env::var_os("USERPROFILE")?;
        validate_picked_folder(&picked, Path::new(&profile)).ok()
    }

    /// Refuses a summary of another dataset class without asking.
    fn confirm_disclosure(&self, summary: &DisclosureSummary) -> bool {
        if summary.dataset_class != self.dataset || self.dataset == DatasetClass::Personal {
            return false;
        }
        let Ok(_active) = self.active.try_lock() else {
            return false;
        };
        let text = disclosure_text(summary);
        text.chars().count() <= MAX_DISCLOSURE_CHARS && (self.ask)(DISCLOSURE_TITLE, &text)
    }
}

/// Same grant-root validator as the service; native selection is the only volume-root grant.
/// The picker additionally fences whole profiles and credential/browser stores.
pub fn validate_picked_folder(root: &Path, profile: &Path) -> Result<PathBuf, String> {
    let root = loomward_service::paths::check_root(root, None, &[], true)
        .map_err(|_| "the selected folder is not an eligible grant root".to_string())?;
    let profile = profile.canonicalize().map_err(|_| "profile unavailable")?;
    let folded = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
    let (r, p) = (folded(&root), folded(&profile));
    // A volume root is the explicit native exception; other ancestors expose the whole profile.
    if root.parent().is_some() && (r == p || p.starts_with(&(r.clone() + "\\"))) {
        return Err("whole profiles are not eligible".into());
    }
    let profiles = profile.parent().ok_or("profile unavailable")?;
    if root
        .parent()
        .is_some_and(|parent| folded(parent) == folded(profiles))
    {
        return Err("whole profiles are not eligible".into());
    }
    for component in root.strip_prefix(profiles).unwrap_or(&root).components() {
        if [
            ".ssh", ".aws", ".azure", ".gnupg", ".kube", ".codex", ".claude", "appdata",
        ]
        .contains(
            &component
                .as_os_str()
                .to_string_lossy()
                .to_lowercase()
                .as_str(),
        ) {
            return Err("credential and browser stores are not eligible".into());
        }
    }
    Ok(root)
}

/// The confirmation text: recipient, model, fields and every item, with characters that change
/// how text looks without showing (controls, bidi overrides, zero-width) written as `\u{..}`.
pub fn disclosure_text(s: &DisclosureSummary) -> String {
    let fields: Vec<String> = s.fields.iter().map(wire).collect();
    let mut t = String::new();
    let _ = writeln!(
        t,
        "Recipient: {} (model {}, reasoning {})",
        wire(&s.recipient),
        wire(&s.model),
        wire(&s.reasoning_effort)
    );
    let _ = writeln!(t, "Dataset: {}", wire(&s.dataset_class));
    let _ = writeln!(t, "Fields sent: {}", fields.join(", "));
    let _ = writeln!(t, "Items ({}):", s.items.len());
    for (i, item) in s.items.iter().enumerate() {
        let _ = writeln!(
            t,
            "{:>2}. {}  [{}]  in {}  (size bucket {})",
            i + 1,
            visible(&item.name),
            visible(&item.extension),
            visible(&item.context),
            item.size_bucket.get()
        );
    }
    let _ = writeln!(t, "Excluded by screening: {}", s.excluded_count.get());
    let _ = writeln!(t, "Payload: {}", s.payload_digest.as_str());
    let _ = writeln!(t, "Runner profile: {}", s.runner_profile_digest.as_str());
    let _ = writeln!(t, "Expires: {}", s.expires_at.as_str());
    t.push_str("\nOK sends exactly these items once. Cancel (the default) sends nothing.");
    t
}

/// A DTO enum or constant as its wire string.
fn wire(v: &impl Serialize) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        other => format!("{other:?}"),
    }
}

/// Escapes characters that hide or reorder text.
pub fn visible(s: &str) -> String {
    s.chars()
        .map(|c| {
            let hidden = !matches!(c, ' '..='~');
            if hidden {
                format!("\\u{{{:04X}}}", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// An app-owned warning box whose default button is Cancel, so Enter declines.
// shortcut: MessageBoxW cannot scroll; keep personal disclosure disabled until LW-111.
#[cfg(windows)]
pub fn message_box(owner: isize, title: &str, text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDOK, MB_APPLMODAL, MB_DEFBUTTON2, MB_ICONWARNING, MB_OKCANCEL,
        MB_SETFOREGROUND,
    };
    if owner == 0 {
        return false;
    }
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (title, text) = (wide(title), wide(text));
    let flags = MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2 | MB_APPLMODAL | MB_SETFOREGROUND;
    // SAFETY: both strings are NUL-terminated UTF-16 buffers that outlive the call.
    unsafe { MessageBoxW(owner as _, text.as_ptr(), title.as_ptr(), flags) == IDOK }
}

#[cfg(not(windows))]
pub fn message_box(_owner: isize, _title: &str, _text: &str) -> bool {
    false
}
