//! [`NativeDialogs`] for the desktop: the folder picker and the disclosure confirmation, owned
//! by Rust (docs/41 sections 5.3 and 12). The webview cannot open either: the capability file
//! grants no dialog permission, no dialog plugin is registered, and the only way in is a service
//! call that passes the [`CallContext`]. The chosen path never crosses IPC.

use loomward_protocol::{DatasetClass, DisclosureSummary, ErrorBody, ErrorCode, NativeDialogs};
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
    picker: Mutex<Option<rfd::FileDialog>>,
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

    pub fn set_owner<R: tauri::Runtime>(
        &self,
        window: &tauri::WebviewWindow<R>,
    ) -> tauri::Result<()> {
        #[cfg(windows)]
        self.owner
            .store(window.hwnd()?.0 as isize, Ordering::SeqCst);
        *self.picker.lock().unwrap_or_else(|e| e.into_inner()) = Some(
            rfd::FileDialog::new()
                .set_title("Loomward: choose a folder to observe (metadata only)")
                .set_parent(window),
        );
        Ok(())
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
            picker: Mutex::new(None),
        }
    }
    fn pick_folder_with(
        &self,
        pick: impl FnOnce(rfd::FileDialog) -> Option<PathBuf>,
    ) -> Option<PathBuf> {
        if self.dataset != DatasetClass::Personal {
            return None;
        }
        let _active = self.active.try_lock().ok()?;
        let picker = self.picker.lock().ok()?.clone()?;
        // The service reports typed refusals; None means the owner cancelled the dialog.
        pick(picker)
    }
}

pub const DISCLOSURE_TITLE: &str = "Loomward: send these items to the teacher?";
/// Conservative temporary MessageBox budget; personal use needs LW-111's scrolling dialog.
pub const MAX_DISCLOSURE_CHARS: usize = 2048;
pub const MAX_DISCLOSURE_ITEMS: usize = 4;
/// Include conservative wrapping at 80 characters, rather than counting newlines alone.
pub const MAX_DISCLOSURE_LINES: usize = 20;

impl NativeDialogs for DesktopDialogs {
    /// Never opens in a synthetic session: an owner-picked folder is personal data
    /// (`synthetic_session_requires_lab_root`, docs/41 section 12).
    fn pick_folder(&self) -> Option<PathBuf> {
        self.pick_folder_with(|picker| picker.pick_folder())
    }

    /// Refuses a summary of another dataset class without asking.
    fn confirm_disclosure(&self, summary: &DisclosureSummary) -> Result<bool, ErrorBody> {
        if summary.dataset_class != self.dataset || self.dataset == DatasetClass::Personal {
            return Ok(false);
        }
        let Ok(_active) = self.active.try_lock() else {
            return Ok(false);
        };
        let pages = (0..summary.items.len())
            .step_by(MAX_DISCLOSURE_ITEMS)
            .map(|start| {
                disclosure_page(
                    summary,
                    start,
                    (start + MAX_DISCLOSURE_ITEMS).min(summary.items.len()),
                )
            })
            .collect::<Vec<_>>();
        if pages.iter().any(|text| {
            text.chars().count() > MAX_DISCLOSURE_CHARS
                || text
                    .lines()
                    .map(|line| line.chars().count().div_ceil(80).max(1))
                    .sum::<usize>()
                    > MAX_DISCLOSURE_LINES
        }) {
            return Err(ErrorBody::new(ErrorCode::ResourceBudget,
                "The disclosure exceeds the native display budget; no items were sent. Use shorter fields or a smaller preview.", true));
        }
        Ok(pages.iter().all(|text| (self.ask)(DISCLOSURE_TITLE, text)))
    }
}

/// Same grant-root validator as the service; native selection is the only volume-root grant.
/// The picker additionally fences whole profiles and credential/browser stores.
pub fn validate_picked_folder(root: &Path, profile: &Path) -> Result<PathBuf, String> {
    let root = loomward_service::paths::check_root(root, None, &[], true)
        .map_err(|_| "the selected folder is not an eligible grant root".to_string())?;
    let profile = profile.canonicalize().map_err(|_| "profile unavailable")?;
    loomward_service::paths::validate_profile_root(&root, &profile)?;
    Ok(root)
}

/// The confirmation text: recipient, model, fields and every item, with characters that change
/// how text looks without showing (controls, bidi overrides, zero-width) written as `\u{..}`.
pub fn disclosure_text(s: &DisclosureSummary) -> String {
    disclosure_page(s, 0, s.items.len())
}

fn disclosure_page(s: &DisclosureSummary, start: usize, end: usize) -> String {
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
    if start == 0 && end == s.items.len() {
        let _ = writeln!(t, "Items ({}):", s.items.len());
    } else {
        let _ = writeln!(t, "Items {}-{} of {}:", start + 1, end, s.items.len());
    }
    for (i, item) in s.items[start..end].iter().enumerate() {
        let _ = writeln!(
            t,
            "{:>2}. {}  [{}]  in {}  (size bucket {})",
            start + i + 1,
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
    if end < s.items.len() {
        t.push_str("\nOK reviews the next page. Cancel (the default) sends nothing.");
    } else if start > 0 {
        let _ = write!(
            t,
            "\nOK sends all {} reviewed items once. Cancel (the default) sends nothing.",
            s.items.len()
        );
    } else {
        t.push_str("\nOK sends exactly these items once. Cancel (the default) sends nothing.");
    }
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
            if c == '\\' {
                "\\\\".to_string()
            } else if hidden {
                format!("\\u{{{:04X}}}", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn followup_picker_preserves_refused_selection() {
        let dialogs = DesktopDialogs::new(DatasetClass::Personal);
        *dialogs.picker.lock().unwrap() = Some(rfd::FileDialog::new());
        let picked = PathBuf::from("C:/synthetic/Users/fixture/.ssh");
        assert_eq!(
            dialogs.pick_folder_with(|_| Some(picked.clone())),
            Some(picked)
        );
        assert_eq!(dialogs.pick_folder_with(|_| None), None);
        let synthetic = DesktopDialogs::new(DatasetClass::Synthetic);
        assert_eq!(
            synthetic.pick_folder_with(|_| panic!("synthetic picker opened")),
            None
        );
    }

    #[test]
    fn picker_keeps_the_main_window_handle() {
        let mut context = tauri::generate_context!();
        context.config_mut().app.windows.clear();
        let app = tauri::test::mock_builder().build(context).unwrap();
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let dialogs = DesktopDialogs::new(DatasetClass::Personal);
        assert!(dialogs.picker.lock().unwrap().is_none());
        dialogs.set_owner(&window).unwrap();
        let picker = dialogs.picker.lock().unwrap();
        // rfd's stored parent must carry the mock runtime's nonzero native handle.
        let configured = format!("{:?}", picker.as_ref().unwrap());
        assert!(
            configured.contains(&format!("hwnd: {}", dialogs.owner.load(Ordering::SeqCst))),
            "{configured}"
        );
    }
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
