//! [`NativeDialogs`] for the desktop: the folder picker and the disclosure confirmation, owned
//! by Rust (docs/41 sections 5.3 and 12). The webview cannot open either: the capability file
//! grants no dialog permission, no dialog plugin is registered, and the only way in is a service
//! call that passes the [`CallContext`]. The chosen path never crosses IPC.

use loomward_protocol::{DatasetClass, DisclosureSummary, NativeDialogs};
use serde::Serialize;
use std::fmt::Write;
use std::path::PathBuf;

/// Shows a title and text; `true` only on explicit accept.
type Prompt = dyn Fn(&str, &str) -> bool + Send + Sync;

pub struct DesktopDialogs {
    dataset: DatasetClass,
    /// Shows the confirmation text and returns `true` only on explicit accept. The real one is
    /// [`message_box`]; tests inject a stand-in.
    ask: Box<Prompt>,
}

impl DesktopDialogs {
    pub fn new(dataset: DatasetClass) -> Self {
        Self::with_prompt(dataset, message_box)
    }

    pub fn with_prompt(
        dataset: DatasetClass,
        ask: impl Fn(&str, &str) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            dataset,
            ask: Box::new(ask),
        }
    }
}

pub const DISCLOSURE_TITLE: &str = "Loomward: send these items to the teacher?";

impl NativeDialogs for DesktopDialogs {
    /// Never opens in a synthetic session: an owner-picked folder is personal data
    /// (`synthetic_session_requires_lab_root`, docs/41 section 12).
    fn pick_folder(&self) -> Option<PathBuf> {
        if self.dataset != DatasetClass::Personal {
            return None;
        }
        rfd::FileDialog::new()
            .set_title("Loomward: choose a folder to observe (metadata only)")
            .pick_folder()
    }

    /// Refuses a summary of another dataset class without asking.
    fn confirm_disclosure(&self, summary: &DisclosureSummary) -> bool {
        summary.dataset_class == self.dataset
            && (self.ask)(DISCLOSURE_TITLE, &disclosure_text(summary))
    }
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
            let hidden = c.is_control()
                || matches!(c as u32,
                    0xAD | 0x61C | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E
                    | 0x2060..=0x2064 | 0x2066..=0x2069 | 0xFEFF);
            if hidden {
                format!("\\u{{{:04X}}}", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// A task-modal warning box whose default button is Cancel, so Enter declines.
// ponytail: MessageBoxW does not scroll; 25 items of long names can exceed a small screen. Move
// to a scrolling Rust-owned TaskDialog when personal disclosure is enabled (LW-111).
#[cfg(windows)]
pub fn message_box(title: &str, text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDOK, MB_DEFBUTTON2, MB_ICONWARNING, MB_OKCANCEL, MB_SETFOREGROUND,
        MB_TASKMODAL, MB_TOPMOST,
    };
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (title, text) = (wide(title), wide(text));
    let flags =
        MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2 | MB_TASKMODAL | MB_SETFOREGROUND | MB_TOPMOST;
    // SAFETY: both strings are NUL-terminated UTF-16 buffers that outlive the call.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), flags) == IDOK }
}

#[cfg(not(windows))]
pub fn message_box(_title: &str, _text: &str) -> bool {
    false
}
