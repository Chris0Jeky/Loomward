//! [`NativeDialogs`] for the desktop: the folder picker, the backup-receipt file picker, the
//! diagnostic-bundle save dialog and the disclosure confirmation, owned by Rust (docs/41 sections 5.3 and 12). The webview cannot open either: the capability file
//! grants no dialog permission, no dialog plugin is registered, and the only way in is a service
//! call that passes the [`CallContext`]. The chosen path never crosses IPC.

use loomward_protocol::{
    DatasetClass, DisclosureSummary, ErrorBody, ErrorCode, NativeDialogs, SaveDialog,
};
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
        // No start folder, no picker: Windows would reopen the last folder picked (#373).
        *self.picker.lock().unwrap_or_else(|e| e.into_inner()) = start_directory().map(|start| {
            rfd::FileDialog::new()
                .set_title(PICKER_TITLE)
                .set_directory(start)
                .set_parent(window)
        });
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
    /// Personal sessions only, one dialog at a time, from the profile start folder (#373).
    fn pick_with(&self, pick: impl FnOnce(rfd::FileDialog) -> Option<PathBuf>) -> Option<PathBuf> {
        if self.dataset != DatasetClass::Personal {
            return None;
        }
        let _active = self.active.try_lock().ok()?;
        let picker = self.picker.lock().ok()?.clone()?;
        pick(picker)
    }

    fn pick_folder_with(
        &self,
        pick: impl FnOnce(rfd::FileDialog) -> Option<PathBuf>,
    ) -> Option<PathBuf> {
        // The service reports typed refusals; None means the owner cancelled either dialog.
        let folder = self.pick_with(pick)?;
        (self.ask)(GRANT_TITLE, &grant_text(&folder)).then_some(folder)
    }

    /// The same parented dialog, retitled for one receipt file and filtered to JSON and text. A
    /// receipt is read once and grants nothing, so the folder confirmation does not apply.
    pub fn pick_file_with(
        &self,
        title: &str,
        pick: impl FnOnce(rfd::FileDialog) -> Option<PathBuf>,
    ) -> Option<PathBuf> {
        self.pick_with(|picker| {
            pick(
                picker
                    .set_title(title)
                    .add_filter("restic JSON output", &["json", "txt"]),
            )
        })
    }
}

impl DesktopDialogs {
    /// The same parented dialog from the profile start folder, as a save dialog named for a
    /// diagnostic bundle (docs/42 ADR-V3-24). Every session may save: the bundle is redacted and
    /// holds no personal data. One dialog at a time. Whatever this dialog asks about an existing
    /// file, the service replaces one only after [`REPLACE_TITLE`] is accepted.
    ///
    /// Another dialog open is `Busy`; no picker (no owner window yet, or no start folder) is
    /// `Unavailable` (#437). rfd erases cancellation and errors into `None`, so no returned location
    /// is `NoSelection`, never a claimed user cancellation.
    pub fn save_with(
        &self,
        title: &str,
        default_name: &str,
        save: impl FnOnce(rfd::FileDialog) -> Option<PathBuf>,
    ) -> SaveDialog {
        let _active = match self.active.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => return SaveDialog::Busy,
            Err(std::sync::TryLockError::Poisoned(_)) => return SaveDialog::Unavailable,
        };
        let Some(picker) = self.picker.lock().ok().and_then(|p| p.clone()) else {
            return SaveDialog::Unavailable;
        };
        save(
            picker
                .set_title(title)
                .set_file_name(default_name)
                .add_filter("Loomward diagnostic bundle", &["json"]),
        )
        .map_or(SaveDialog::NoSelection, SaveDialog::Chosen)
    }
}

pub const PICKER_TITLE: &str = "Loomward: choose a folder to observe (metadata only)";
pub const GRANT_TITLE: &str = "Loomward: read this folder?";

/// The picker's starting view: the user's profile folder, with nothing selected. A stray Enter
/// there picks the whole profile, which the grant rules refuse (`protected_profile_or_store`), and
/// the confirmation after it defaults to Cancel. None (no usable profile) keeps the picker shut.
pub fn start_directory() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir())
}

/// Letters and printable ASCII stay readable; anything else (controls, bidi overrides, zero-width
/// marks, the Hangul fillers) is written as `\u{..}`, and literal backslashes are doubled.
pub fn readable(text: &str) -> String {
    text.chars()
        .map(|c| {
            // The Hangul fillers are letters that draw nothing: they stay hidden (LW-145).
            let filler = matches!(c, '\u{115F}' | '\u{1160}' | '\u{3164}' | '\u{FFA0}');
            if c == '\\' {
                "\\\\".to_string()
            } else if (c.is_alphanumeric() && !filler) || matches!(c, ' '..='~') {
                c.to_string()
            } else {
                format!("\\u{{{:04X}}}", c as u32)
            }
        })
        .collect()
}

/// The one-line confirmation after a pick. Letters of any script stay readable (bar the invisible
/// Hangul fillers); everything else outside printable ASCII (controls, bidi overrides, zero-width
/// marks) is written as `\u{..}`, and literal backslashes are doubled.
pub fn grant_text(folder: &Path) -> String {
    let shown = readable(&folder.to_string_lossy());
    format!(
        "Let Loomward read the names, sizes and dates in {shown}?\n\n\
         It never changes your files, and reads no file contents unless you ask it to find \
         duplicates. Cancel (the default) grants nothing."
    )
}

pub const DISCLOSURE_TITLE: &str = "Loomward: send these items to the teacher?";
/// #219: the one-line confirmation for a folder on a FAT, exFAT or unknown filesystem.
pub const UNVERIFIED_TITLE: &str = "Loomward: list this folder without file identity?";
pub const UNVERIFIED_TEXT: &str =
    "This volume cannot verify file identity; Loomward will list it read-only and never act on it";
/// Loomward's own question before a diagnostic save replaces a file (ADR-V3-24): it does not rely
/// on the save dialog's overwrite prompt.
pub const REPLACE_TITLE: &str = "Loomward: replace this file?";

/// Names the file, with hidden characters spelled out, and says what Cancel (the default) does.
pub fn replace_text(name: &str) -> String {
    format!(
        "Replace {} with the diagnostic bundle you previewed?\n\nThe file's current contents are lost. \
         Cancel (the default) writes nothing.",
        visible(name)
    )
}
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

    fn try_pick_folder(&self) -> Result<Option<PathBuf>, ErrorBody> {
        if !self.picker.lock().is_ok_and(|picker| picker.is_some()) {
            return Err(ErrorBody::new(
                ErrorCode::CapabilityUnavailable,
                "The Windows folder picker is unavailable: no owner window or usable USERPROFILE start folder.",
                false,
            ));
        }
        Ok(self.pick_folder())
    }

    /// Never opens in a synthetic session either: an owner-picked receipt is personal data. The
    /// path goes to the service in-process and never crosses IPC.
    fn pick_file(&self, title: &str) -> Option<PathBuf> {
        self.pick_file_with(title, |picker| picker.pick_file())
    }

    /// Any session: the path goes to the service in-process and never crosses IPC.
    fn save_file(&self, title: &str, default_name: &str) -> SaveDialog {
        self.save_with(title, default_name, |dialog| dialog.save_file())
    }

    /// Every session, like the save dialog; Cancel is the default, and a second dialog declines.
    fn confirm_replace(&self, name: &str) -> bool {
        let Ok(_active) = self.active.try_lock() else {
            return false;
        };
        (self.ask)(REPLACE_TITLE, &replace_text(name))
    }

    /// Personal sessions only (the picker never opens in a synthetic one); Cancel is the default.
    fn confirm_identity_unverified(&self, folder: &Path, filesystem: &str) -> bool {
        if self.dataset != DatasetClass::Personal {
            return false;
        }
        let Ok(_active) = self.active.try_lock() else {
            return false;
        };
        (self.ask)(
            UNVERIFIED_TITLE,
            &format!(
                "{} ({}): {UNVERIFIED_TEXT}",
                visible(&folder.to_string_lossy()),
                visible(filesystem)
            ),
        )
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
    fn a_pick_grants_nothing_until_the_owner_confirms_that_folder() {
        let shown = Arc::new(Mutex::new(Vec::new()));
        let log = shown.clone();
        let answer = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reply = answer.clone();
        let dialogs = DesktopDialogs::with_prompt(DatasetClass::Personal, move |title, text| {
            log.lock()
                .unwrap()
                .push((title.to_string(), text.to_string()));
            reply.load(Ordering::SeqCst)
        });
        *dialogs.picker.lock().unwrap() = Some(rfd::FileDialog::new());
        let picked = PathBuf::from("C:/synthetic/caf\u{E9}-\u{202E}gpj.exe");
        // Declined (Cancel is the default button): the service reports cancelled_by_user.
        assert_eq!(dialogs.pick_folder_with(|_| Some(picked.clone())), None);
        answer.store(true, Ordering::SeqCst);
        assert_eq!(
            dialogs.pick_folder_with(|_| Some(picked.clone())),
            Some(picked.clone())
        );
        // A cancelled picker asks nothing.
        assert_eq!(dialogs.pick_folder_with(|_| None), None);
        let shown = shown.lock().unwrap();
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0].0, GRANT_TITLE);
        assert!(
            shown[0].1.starts_with(
                "Let Loomward read the names, sizes and dates in C:/synthetic/caf\u{E9}-\\u{202E}gpj.exe?"
            ),
            "{}",
            shown[0].1
        );
        // With no owner window the real box is never shown and the pick declines.
        let unowned = DesktopDialogs::new(DatasetClass::Personal);
        *unowned.picker.lock().unwrap() = Some(rfd::FileDialog::new());
        assert_eq!(unowned.pick_folder_with(|_| Some(picked.clone())), None);
    }

    #[test]
    fn followup_picker_preserves_refused_selection() {
        let dialogs = DesktopDialogs::with_prompt(DatasetClass::Personal, |_, _| true);
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

    /// #219: the owner sees exactly the one-line text, and only an explicit accept grants.
    #[test]
    fn identity_unverified_confirmation_is_explicit_and_personal_only() {
        for (class, accept, expected) in [
            (DatasetClass::Personal, true, true),
            (DatasetClass::Personal, false, false),
            (DatasetClass::Synthetic, true, false),
        ] {
            let asked = Arc::new(Mutex::new(Vec::new()));
            let seen = asked.clone();
            let dialogs = DesktopDialogs::with_prompt(class, move |title, text| {
                seen.lock()
                    .unwrap()
                    .push((title.to_owned(), text.to_owned()));
                accept
            });
            assert_eq!(
                dialogs.confirm_identity_unverified(Path::new("synthetic-folder"), "exFAT"),
                expected,
                "{class:?}"
            );
            let asked = asked.lock().unwrap();
            if class == DatasetClass::Personal {
                assert_eq!(
                    *asked,
                    [(
                        UNVERIFIED_TITLE.to_owned(),
                        format!("synthetic-folder (exFAT): {UNVERIFIED_TEXT}")
                    )]
                );
                assert!(!UNVERIFIED_TEXT.contains('\n'));
                assert!(asked[0].1.contains("synthetic-folder"));
                assert!(asked[0].1.contains("exFAT"));
            } else {
                assert!(asked.is_empty());
            }
        }
    }

    #[test]
    fn file_picker_is_personal_only_and_retitled() {
        let dialogs = DesktopDialogs::new(DatasetClass::Personal);
        *dialogs.picker.lock().unwrap() = Some(rfd::FileDialog::new());
        let picked = PathBuf::from("C:/synthetic/receipt.json");
        let title = "Loomward: choose a synthetic receipt";
        assert_eq!(
            dialogs.pick_file_with(title, |d| {
                let configured = format!("{d:?}");
                assert!(configured.contains(title), "{configured}");
                assert!(configured.contains("json"), "{configured}");
                Some(picked.clone())
            }),
            Some(picked)
        );
        assert_eq!(dialogs.pick_file_with(title, |_| None), None);
        // A second dialog while one is open is refused, never stacked.
        let active = dialogs.active.lock().unwrap();
        assert_eq!(
            dialogs.pick_file_with(title, |_| panic!("stacked dialog")),
            None
        );
        drop(active);
        let synthetic = DesktopDialogs::new(DatasetClass::Synthetic);
        *synthetic.picker.lock().unwrap() = Some(rfd::FileDialog::new());
        assert_eq!(
            synthetic.pick_file_with(title, |_| panic!("synthetic file picker opened")),
            None
        );
    }

    #[test]
    fn save_dialog_none_does_not_identify_a_user_cancel() {
        let dialogs = DesktopDialogs::new(DatasetClass::Synthetic);
        *dialogs.picker.lock().unwrap() = Some(rfd::FileDialog::new());
        // rfd erases both cancellation and COM errors into None.
        assert_eq!(
            dialogs.save_with("Synthetic save", "bundle.json", |_| None),
            SaveDialog::NoSelection
        );
    }

    #[test]
    fn save_dialog_opens_in_every_session_retitled_and_one_at_a_time() {
        let title = "Loomward: save a synthetic bundle";
        let name = "loomward-diagnostics-20261011-120000.json";
        for class in [DatasetClass::Synthetic, DatasetClass::Personal] {
            let dialogs = DesktopDialogs::new(class);
            // No owner window yet, so no picker: nothing opens, and it is not a cancel (#437).
            assert_eq!(
                dialogs.save_with(title, name, |_| panic!("unparented save dialog")),
                SaveDialog::Unavailable
            );
            *dialogs.picker.lock().unwrap() = Some(rfd::FileDialog::new());
            let chosen = PathBuf::from("C:/synthetic/bundle.json");
            assert_eq!(
                dialogs.save_with(title, name, |d| {
                    let configured = format!("{d:?}");
                    assert!(configured.contains(title), "{configured}");
                    assert!(configured.contains(name), "{configured}");
                    assert!(configured.contains("json"), "{configured}");
                    Some(chosen.clone())
                }),
                SaveDialog::Chosen(chosen)
            );
            // No location: cancellation versus a backend failure is unknown.
            assert_eq!(
                dialogs.save_with(title, name, |_| None),
                SaveDialog::NoSelection
            );
            let active = dialogs.active.lock().unwrap();
            assert_eq!(
                dialogs.save_with(title, name, |_| panic!("stacked dialog")),
                SaveDialog::Busy
            );
            drop(active);
        }
    }

    /// The replace question names the file (hidden characters spelled out), is asked in every
    /// session, accepts only on OK, and is never stacked on another dialog.
    #[test]
    fn replace_confirmation_names_the_file_and_defaults_to_cancel() {
        for (class, accept) in [
            (DatasetClass::Synthetic, true),
            (DatasetClass::Personal, false),
        ] {
            let asked = Arc::new(Mutex::new(Vec::new()));
            let seen = asked.clone();
            let dialogs = DesktopDialogs::with_prompt(class, move |title, text| {
                seen.lock()
                    .unwrap()
                    .push((title.to_owned(), text.to_owned()));
                accept
            });
            assert_eq!(dialogs.confirm_replace("bundle\u{202E}.json"), accept);
            let busy = dialogs.active.lock().unwrap();
            assert!(!dialogs.confirm_replace("bundle.json"), "stacked question");
            drop(busy);
            let asked = asked.lock().unwrap();
            assert_eq!(asked.len(), 1);
            assert_eq!(asked[0].0, REPLACE_TITLE);
            assert!(
                asked[0].1.starts_with("Replace bundle\\u{202E}.json with"),
                "{}",
                asked[0].1
            );
            assert!(asked[0].1.contains("Cancel (the default) writes nothing"));
        }
        // With no owner window the real box is never shown, so nothing is replaced.
        assert!(!DesktopDialogs::new(DatasetClass::Personal).confirm_replace("bundle.json"));
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
        // It opens on the profile folder, never on whatever folder Windows remembers.
        let start = start_directory().expect("USERPROFILE");
        assert!(
            configured.contains(&format!("starting_directory: Some({start:?})")),
            "{configured}"
        );
    }
}

/// The error box shown when the app cannot start and has no console to print to. It has no owner
/// window because none exists yet.
#[cfg(windows)]
pub fn error_box(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND,
    };
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (title, text) = (wide(title), wide(text));
    // SAFETY: both strings are NUL-terminated UTF-16 buffers that outlive the call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

#[cfg(not(windows))]
pub fn error_box(_title: &str, _text: &str) {}

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
