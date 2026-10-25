//! Diagnostic bundles (docs/42 ADR-V3-24): the bundle is an allowlist that never carries a path,
//! name, label or volume identity, and `diagnostics.save` writes exactly the previewed bytes to an
//! owner-picked file, or nothing. Synthetic data and disposable folders only.
use super::*;
#[cfg(windows)]
use crate::diagnostics::write_export;
use crate::diagnostics::{
    canonical, check_target, check_target_for_session, digest, write_to, SAVE_TITLE,
};
use std::path::PathBuf;

/// Answers each save dialog with the next queued path (`None` = the owner cancelled).
struct Saves {
    queue: Mutex<Vec<Option<PathBuf>>>,
    asked: Mutex<Vec<(String, String)>>,
    /// Answers to Loomward's own replace confirmation, in order (none left = decline), and the
    /// file names it was shown.
    replace: Mutex<Vec<bool>>,
    replace_asked: Mutex<Vec<String>>,
    /// A dialog that never opens: answered instead of the queue.
    closed: Mutex<Option<SaveDialog>>,
}

impl Saves {
    fn new(picks: Vec<Option<PathBuf>>) -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(picks.into_iter().rev().collect()),
            asked: Mutex::default(),
            replace: Mutex::default(),
            replace_asked: Mutex::default(),
            closed: Mutex::default(),
        })
    }
    fn never_opens(why: SaveDialog) -> Arc<Self> {
        let saves = Self::new(vec![]);
        *saves.closed.lock().unwrap() = Some(why);
        saves
    }
    fn count(&self) -> usize {
        self.asked.lock().unwrap().len()
    }
}

impl NativeDialogs for Saves {
    fn pick_folder(&self) -> Option<PathBuf> {
        panic!("a diagnostic save opened the folder picker")
    }
    fn pick_file(&self, _: &str) -> Option<PathBuf> {
        panic!("a diagnostic save opened the open-file dialog")
    }
    fn save_file(&self, title: &str, default_name: &str) -> SaveDialog {
        self.asked
            .lock()
            .unwrap()
            .push((title.to_owned(), default_name.to_owned()));
        if let Some(why) = self.closed.lock().unwrap().clone() {
            return why;
        }
        match self.queue.lock().unwrap().pop().flatten() {
            Some(path) => SaveDialog::Chosen(path),
            None => SaveDialog::Cancelled,
        }
    }
    fn confirm_replace(&self, name: &str) -> bool {
        self.replace_asked.lock().unwrap().push(name.to_owned());
        let mut answers = self.replace.lock().unwrap();
        !answers.is_empty() && answers.remove(0)
    }
    fn confirm_disclosure(&self, _: &DisclosureSummary) -> Result<bool, ErrorBody> {
        Ok(false)
    }
}

/// `(bundle, bytes, digest)` of a preview, after checking the advertised size and digest are
/// those of the canonical text of the bundle as received.
fn preview(svc: &Service) -> (Value, Vec<u8>, String) {
    let (r, _) = ok(
        "diagnostics.preview",
        call(svc, "diagnostics.preview", json!({})),
    );
    let bundle: DiagnosticsBundle = serde_json::from_value(r["bundle"].clone()).unwrap();
    let bytes = canonical(&bundle).unwrap();
    // The app reproduces these bytes with JSON.stringify(bundle, null, 2).
    assert_eq!(bytes, serde_json::to_vec_pretty(&r["bundle"]).unwrap());
    assert_eq!(r["bytes"], bytes.len().to_string());
    let d = r["digest"].as_str().unwrap().to_owned();
    assert_eq!(d, digest(&bytes));
    (r["bundle"].clone(), bytes, d)
}

fn save(svc: &Service, saves: &Arc<Saves>, digest: &str) -> Payload {
    *svc.inner.picker_last.lock().unwrap() = None;
    let mut req = request("diagnostics.save", json!({ "digest": digest }));
    req.deadline_ms = Some(serde_json::from_value(json!(60_000)).unwrap());
    let (result, _) = ok(
        "diagnostics.save",
        svc.call(req, &CallContext::tauri(saves.clone())),
    );
    // Never a path: exactly these three members.
    let mut keys: Vec<_> = result.keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["bytes_written", "outcome", "refusal"]);
    result
}

/// Every name in `dir`, so a test can prove nothing (not even a temporary file) was left.
fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// Markers that must never reach a bundle, raw or as JSON escapes.
#[cfg(windows)]
const MARKERS: [&str; 9] = [
    "lw-marker",
    "\u{202E}",
    "\u{200B}",
    "\u{1F9F5}",
    "\\u202e",
    "\\u200b",
    "script",
    "quoted",
    "Users",
];

/// A root folder whose own name and every name below it is hostile: bidi overrides, zero-width
/// marks, quotes, an emoji, a fullwidth `</script>`, percent and ampersand runs, and a nested
/// `Users\owner` path. Windows refuses `<>:"/\|?*` in names, so those reach only as fullwidth forms.
#[cfg(windows)]
fn hostile_tree(path: &Path) {
    let dir = path.join("lw-marker-dir 'quoted' \u{FF1C}\u{FF0F}script\u{FF1E}");
    std::fs::create_dir_all(dir.join("Users").join("lw-marker-owner")).unwrap();
    for (folder, name) in [
        (
            path.to_path_buf(),
            "lw-marker-file\u{200B}zero\u{202E}txt.exe",
        ),
        (dir.clone(), "lw-marker \u{1F9F5} thread.txt"),
        (
            dir.join("Users").join("lw-marker-owner"),
            "lw-marker&amp%2F;.txt",
        ),
    ] {
        std::fs::write(folder.join(name), b"synthetic").unwrap();
    }
}

#[cfg(windows)]
fn assert_redacted(bytes: &[u8], svc: &Service, root: &Path) {
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    for marker in MARKERS {
        assert!(!text.contains(marker), "{marker:?} leaked:\n{text}");
    }
    let path = root.to_string_lossy().into_owned();
    let escaped = serde_json::to_string(&path).unwrap();
    assert!(!text.contains(&path), "the root path leaked");
    assert!(
        !text.contains(escaped.trim_matches('"')),
        "the escaped root path leaked"
    );
    let volumes: Vec<(String, String)> = svc
        .inner
        .db()
        .prepare("SELECT volume_key,display_name FROM main.volume")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(!volumes.is_empty(), "the scan observed no volume");
    for (key, label) in volumes {
        for s in [key, label].into_iter().filter(|s| s.len() >= 3) {
            let escaped = serde_json::to_string(&s).unwrap();
            assert!(
                !text.contains(&s) && !text.contains(escaped.trim_matches('"')),
                "a volume identity leaked"
            );
        }
    }
}

/// LW-055: hostile names under a scanned lab root never reach the bundle; the root is listed by
/// its opaque ID with what the bundle may say about it.
#[cfg(windows)]
#[test]
fn a_scanned_hostile_lab_root_never_reaches_the_bundle() {
    if loomward_windows::enumerate::skip_elevated_test() {
        eprintln!("skipped: elevated token; the native scan refuses elevation (invariant 2)");
        return;
    }
    let tmp = temp_root();
    let state = tmp.path().join("state");
    let path = tmp
        .path()
        .join("lw-marker-root \u{202E}gnp.exe 'C\u{FF1A}\u{FF3C}Users'");
    crate::lab::register(
        &state,
        &path,
        "seed-diagnostics",
        "S",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    let path = path.canonicalize().unwrap();
    hostile_tree(&path);
    let svc = Service::open(Config {
        state_dir: state,
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
        grant_roots: vec![path.clone()],
    })
    .unwrap();
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    let root = roots["roots"][0]["root_id"].as_str().unwrap().to_owned();
    let job = start(&svc, &root, "full");
    assert_eq!(settle(&svc, &job)["state"], "completed");
    // The catalogue does hold the hostile names: the redaction is not vacuous.
    let names: i64 = svc
        .inner
        .db()
        .query_row(
            "SELECT count(*) FROM main.file WHERE name LIKE 'lw-marker%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(names, 3);
    let (bundle, bytes, _) = preview(&svc);
    assert_redacted(&bytes, &svc, &path);
    let r = &bundle["roots"][0];
    assert_eq!(r["root_id"], root);
    assert_eq!(r["origin"], "lab_generated");
    assert_eq!(r["grant_state"], "active");
    assert_eq!(r["scan_state"], "complete");
    assert_eq!(r["filesystem"], "ntfs");
    assert_eq!(r["identity_mode"], "verified");
    assert_eq!(bundle["counts"]["roots_active"], 1);
    assert_eq!(
        bundle["stores"]["catalog_schema_version"],
        loomward_catalog::SCHEMA_VERSION
    );
    assert_eq!(
        bundle["stores"]["state_schema_version"],
        loomward_catalog::SCHEMA_VERSION
    );
    assert_eq!(bundle["host"]["os_family"], "windows");
    assert!(bundle["host"]["os_build"].as_u64().is_some_and(|b| b > 0));
    assert!(bundle["host"]["logical_processors"].as_u64().is_some());
    assert!(bundle["host"]["memory_total_bytes"].is_string());
    assert_eq!(bundle["jobs"]["by_kind_state"][0]["kind"], "scan");
    assert_eq!(bundle["recent_errors"], json!({"unknown": "not_retained"}));
    // The same folder granted to a personal session previews just as redacted.
    let personal = Service::open(Config {
        state_dir: tmp.path().join("personal-state"),
        dataset: DatasetClass::Personal,
        allow_personal: true,
        allow_identity_unverified: false,
        grant_roots: vec![path.clone()],
    })
    .unwrap();
    let (roots, _) = ok("roots.list", call(&personal, "roots.list", json!({})));
    let root = roots["roots"][0]["root_id"].as_str().unwrap().to_owned();
    let job = start(&personal, &root, "full");
    assert_eq!(settle(&personal, &job)["state"], "completed");
    let (bundle, bytes, _) = preview(&personal);
    assert_redacted(&bytes, &personal, &path);
    assert_eq!(bundle["loomward"]["dataset_class"], "personal");
    assert_eq!(bundle["roots"][0]["origin"], "owner_granted");
}

/// Unknowns stay visible and every class previews; over HTTP there is no save.
#[test]
fn both_dataset_classes_preview_and_http_cannot_save() {
    let tmp = temp_root();
    for class in [DatasetClass::Synthetic, DatasetClass::Personal] {
        let svc = open(&tmp.path().join(format!("{class:?}")), class);
        let (bundle, _, d) = preview(&svc);
        assert_eq!(bundle["loomward"]["dataset_class"], json!(class));
        assert_eq!(bundle["loomward"]["adapter"], "http");
        assert_eq!(bundle["session"]["features"]["diagnostics_save"], false);
        assert_eq!(bundle["roots"], json!([]));
        assert_eq!(bundle["counts"]["roots_active"], 0);
        let e = err(call(&svc, "diagnostics.save", json!({ "digest": d })));
        assert_eq!(e.code, ErrorCode::CapabilityUnavailable);
    }
}

#[test]
fn save_writes_exactly_the_previewed_bytes_and_never_returns_the_path() {
    if export_refuses_elevation() {
        return;
    }
    let tmp = temp_root();
    let out = tmp.path().join("out");
    std::fs::create_dir(&out).unwrap();
    for class in [DatasetClass::Synthetic, DatasetClass::Personal] {
        let svc = open(&tmp.path().join(format!("{class:?}")), class);
        let target = out.join(format!("bundle-{class:?}.json"));
        let saves = Saves::new(vec![Some(target.clone())]);
        let (_, bytes, d) = preview(&svc);
        let r = save(&svc, &saves, &d);
        assert_eq!(r["outcome"], "saved", "{r:?}");
        assert_eq!(r["bytes_written"], bytes.len().to_string());
        assert_eq!(r["refusal"], Value::Null);
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        let asked = saves.asked.lock().unwrap();
        assert_eq!(asked[0].0, SAVE_TITLE);
        let name = &asked[0].1;
        assert!(
            name.starts_with("loomward-diagnostics-")
                && name.ends_with(".json")
                && name.len() == "loomward-diagnostics-20261011-120000.json".len(),
            "{name}"
        );
    }
    // No temporary file survives a save.
    assert_eq!(
        names(&out),
        ["bundle-Personal.json", "bundle-Synthetic.json"]
    );
}

#[test]
fn a_changed_or_missing_preview_writes_nothing_and_opens_no_dialog() {
    let tmp = temp_root();
    let svc = open(&tmp.path().join("state"), DatasetClass::Synthetic);
    let saves = Saves::new(vec![]);
    let other = format!("sha256:{}", "0".repeat(64));
    // Nothing previewed yet.
    let r = save(&svc, &saves, &other);
    assert_eq!(r["outcome"], "refused");
    assert_eq!(r["refusal"], "preview_changed");
    assert_eq!(r["bytes_written"], Value::Null);
    let (_, _, first) = preview(&svc);
    std::thread::sleep(Duration::from_millis(5));
    let (_, _, second) = preview(&svc);
    assert_ne!(first, second, "generated_at moves");
    // Only the last preview can be saved: an older digest is refused.
    for stale in [first.as_str(), other.as_str()] {
        let r = save(&svc, &saves, stale);
        assert_eq!(r["refusal"], "preview_changed", "{r:?}");
    }
    assert_eq!(saves.count(), 0, "a refused digest never opens the dialog");
}

#[test]
fn cancel_writes_nothing_and_a_retried_save_never_writes_twice() {
    if export_refuses_elevation() {
        return;
    }
    let tmp = temp_root();
    let out = tmp.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let svc = open(&tmp.path().join("state"), DatasetClass::Synthetic);
    let (_, bytes, d) = preview(&svc);
    let saves = Saves::new(vec![
        None,
        Some(out.join("a.json")),
        Some(out.join("b.json")),
    ]);
    let r = save(&svc, &saves, &d);
    assert_eq!(r["outcome"], "cancelled_by_user");
    assert_eq!(r["bytes_written"], Value::Null);
    assert!(names(&out).is_empty());
    // The same request_id is answered from the idempotency cache: one dialog, one file.
    *svc.inner.picker_last.lock().unwrap() = None;
    let mut req = request("diagnostics.save", json!({ "digest": d }));
    req.deadline_ms = Some(serde_json::from_value(json!(60_000)).unwrap());
    let first = svc.call(req.clone(), &CallContext::tauri(saves.clone()));
    *svc.inner.picker_last.lock().unwrap() = None;
    let again = svc.call(req, &CallContext::tauri(saves.clone()));
    assert_eq!(first, again);
    assert_eq!(saves.count(), 2);
    assert_eq!(names(&out), ["a.json"]);
    assert_eq!(std::fs::read(out.join("a.json")).unwrap(), bytes);
}

/// Loomward asks before replacing a file, whatever the save dialog did: a decline (Cancel is the
/// default) is `cancelled_by_user` and writes nothing; an accept replaces the file whole. A new file
/// is never asked about.
#[test]
fn a_replace_is_confirmed_by_loomward_and_a_decline_writes_nothing() {
    if export_refuses_elevation() {
        return;
    }
    let tmp = temp_root();
    let out = tmp.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let target = out.join("existing.json");
    let old = vec![b'x'; 300_000];
    std::fs::write(&target, &old).unwrap();
    let svc = open(&tmp.path().join("state"), DatasetClass::Synthetic);
    let (_, bytes, d) = preview(&svc);
    let saves = Saves::new(vec![
        Some(target.clone()),
        Some(target.clone()),
        Some(out.join("new.json")),
    ]);
    // Declined.
    let r = save(&svc, &saves, &d);
    assert_eq!(r["outcome"], "cancelled_by_user", "{r:?}");
    assert_eq!(std::fs::read(&target).unwrap(), old);
    assert_eq!(names(&out), ["existing.json"], "no temporary file");
    // Accepted.
    saves.replace.lock().unwrap().push(true);
    let r = save(&svc, &saves, &d);
    assert_eq!(r["outcome"], "saved");
    assert_eq!(
        std::fs::read(&target).unwrap(),
        bytes,
        "no tail of the old file"
    );
    // A new file needs no confirmation.
    let r = save(&svc, &saves, &d);
    assert_eq!(r["outcome"], "saved");
    assert_eq!(
        *saves.replace_asked.lock().unwrap(),
        ["existing.json", "existing.json"]
    );
}

/// The resolved-folder rule alone: a share is refused before its folder is even looked at.
#[test]
fn a_folder_that_resolves_to_a_share_is_refused_unread() {
    use crate::diagnostics::folder_allowed;
    for share in [
        r"\\?\UNC\synthetic-server\share\folder",
        r"\\synthetic-server\share",
    ] {
        assert!(
            !folder_allowed(Path::new(share), |_| panic!("a share was read")),
            "{share}"
        );
    }
    assert!(folder_allowed(Path::new(r"\\?\C:\synthetic"), |_| true));
    assert!(!folder_allowed(Path::new(r"\\?\C:\synthetic"), |_| false));
}

#[cfg(windows)]
#[test]
fn unsupported_targets_are_refused_before_anything_is_written() {
    if export_refuses_elevation() {
        return;
    }
    let tmp = temp_root();
    let out = tmp.path().join("out");
    std::fs::create_dir_all(out.join("a-folder")).unwrap();
    std::fs::write(out.join("regular.json"), b"keep").unwrap();
    // Both would reach the real `out` folder without their lexical guard: a relative path from
    // the test's working directory, and the `\\.\` device namespace naming a local drive.
    let local = out.to_string_lossy();
    let local = local.strip_prefix(r"\\?\").unwrap_or(&local);
    // Up from the working directory to the volume, then down to `out`, wherever the fixture base
    // is (#465).
    let normal = |p: &Path| {
        p.components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .map(|c| c.as_os_str().to_owned())
            .collect::<Vec<_>>()
    };
    let mut relative: PathBuf = normal(&std::env::current_dir().unwrap())
        .iter()
        .map(|_| "..")
        .collect();
    relative.extend(normal(&out));
    let relative = relative.join("relative.json");
    assert!(relative.parent().unwrap().is_dir(), "{relative:?}");
    let mut refused: Vec<PathBuf> = vec![
        relative,
        PathBuf::from(format!(r"\\.\{local}\device.json")),
        PathBuf::from(r"\\synthetic-server\share\bundle.json"),
        PathBuf::from(r"\\?\UNC\synthetic-server\share\bundle.json"),
        PathBuf::from(r"\\.\PhysicalDrive0"),
        PathBuf::from(r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy1\bundle.json"),
        out.join("CON.json"),
        out.join("com1 .json"),
        out.join("regular.json:stream"),
        out.join("a-folder"),
        out.join("missing-folder").join("bundle.json"),
        out.join("regular.json").join("bundle.json"),
    ];
    // A file symlink needs a privilege or developer mode; when it can be made it must be refused.
    let link = out.join("link.json");
    if std::os::windows::fs::symlink_file(out.join("regular.json"), &link).is_ok() {
        refused.push(link.clone());
    }
    let before = names(&out);
    for target in &refused {
        assert_eq!(
            write_export(target, b"{}"),
            Err(DiagnosticsSaveRefusal::UnsupportedTarget),
            "{target:?}"
        );
    }
    assert_eq!(names(&out), before);
    assert_eq!(std::fs::read(out.join("regular.json")).unwrap(), b"keep");
    // A junction to a local folder is followed to where it resolves: still a local write.
    let junction = tmp.path().join("junction");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(out.join("a-folder"))
        .output()
        .is_ok_and(|o| o.status.success());
    if made {
        assert_eq!(write_export(&junction.join("via.json"), b"{}"), Ok(()));
        assert_eq!(
            std::fs::read(out.join("a-folder").join("via.json")).unwrap(),
            b"{}"
        );
        std::fs::remove_file(out.join("a-folder").join("via.json")).unwrap();
    }
    // Through the command: refused, nothing written.
    let svc = open(&tmp.path().join("state"), DatasetClass::Synthetic);
    let (_, _, d) = preview(&svc);
    let r = save(&svc, &Saves::new(vec![Some(out.join("NUL.json"))]), &d);
    assert_eq!(r["outcome"], "refused");
    assert_eq!(r["refusal"], "unsupported_target");
    assert_eq!(names(&out), before);
}

#[test]
fn an_elevated_session_refuses_new_and_existing_exports_without_touching_them() {
    let tmp = temp_root();
    let target = tmp.path().join("existing.json");
    std::fs::write(&target, b"keep").unwrap();
    for path in [tmp.path().join("new.json"), target.clone()] {
        let why = check_target_for_session(&path, true).unwrap_err();
        assert_eq!(
            serde_json::to_value(why).unwrap(),
            json!("elevated_session")
        );
    }
    assert_eq!(std::fs::read(target).unwrap(), b"keep");
    assert_eq!(
        names(tmp.path()),
        ["existing.json"],
        "no new or partial file"
    );
}

/// Hosted runners are elevated and the pinned export folder refuses an elevated token before any
/// write (fails closed, invariant 2): prove the refusal, then skip a test that needs a real write.
#[cfg(windows)]
fn export_refuses_elevation() -> bool {
    if !loomward_windows::enumerate::skip_elevated_test() {
        return false;
    }
    let tmp = temp_root();
    assert_eq!(
        write_export(&tmp.path().join("elevated.json"), b"{}"),
        Err(DiagnosticsSaveRefusal::ElevatedSession)
    );
    eprintln!("skipped: elevated token; the export folder refuses elevation (invariant 2)");
    true
}

#[cfg(not(windows))]
fn export_refuses_elevation() -> bool {
    false
}

/// A failed write leaves the target as it was and no temporary file behind.
#[cfg(windows)]
#[test]
fn a_failed_write_leaves_the_target_and_no_partial_file() {
    if loomward_windows::enumerate::skip_elevated_test() {
        // The pinned export folder refuses an elevated token before any write (fails closed).
        let tmp = temp_root();
        assert_eq!(
            write_export(&tmp.path().join("elevated.json"), b"{}"),
            Err(DiagnosticsSaveRefusal::ElevatedSession)
        );
        eprintln!("skipped: elevated token; the export folder refuses elevation (invariant 2)");
        return;
    }
    let tmp = temp_root();
    let target = tmp.path().join("read-only.json");
    std::fs::write(&target, b"keep").unwrap();
    let mut perms = std::fs::metadata(&target).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&target, perms.clone()).unwrap();
    let result = write_export(&target, b"{}");
    #[allow(clippy::permissions_set_readonly_false)]
    // Windows only: clears the read-only attribute
    perms.set_readonly(false);
    std::fs::set_permissions(&target, perms).unwrap();
    assert_eq!(result, Err(DiagnosticsSaveRefusal::WriteFailed));
    assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    assert_eq!(names(tmp.path()), ["read-only.json"]);
}

/// Only a known user cancellation is `cancelled_by_user` (#437). Dialog failures and a backend
/// result that cannot distinguish failure from cancellation each say so and write nothing.
#[test]
fn a_dialog_that_never_opened_is_not_reported_as_a_cancel() {
    let tmp = temp_root();
    let out = tmp.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let svc = open(&tmp.path().join("state"), DatasetClass::Synthetic);
    let (_, _, d) = preview(&svc);
    for (why, expected) in [
        (SaveDialog::Busy, "dialog_busy"),
        (SaveDialog::Unavailable, "dialog_unavailable"),
        (SaveDialog::NoSelection, "dialog_no_selection"),
    ] {
        let saves = Saves::never_opens(why);
        let r = save(&svc, &saves, &d);
        assert_eq!(r["outcome"], "refused", "{expected}: {r:?}");
        assert_eq!(r["refusal"], expected);
        assert_eq!(r["bytes_written"], Value::Null);
        assert_eq!(saves.count(), 1);
    }
    assert!(names(&out).is_empty());
    // The default adapter dialogs have none: the same honest answer, not a cancel.
    struct NoDialog;
    impl NativeDialogs for NoDialog {
        fn pick_folder(&self) -> Option<PathBuf> {
            None
        }
        fn confirm_disclosure(&self, _: &DisclosureSummary) -> Result<bool, ErrorBody> {
            Ok(false)
        }
    }
    assert_eq!(NoDialog.save_file("t", "n"), SaveDialog::Unavailable);
}

/// More than 64 roots cannot be listed whole: the bundle says so by name instead of cutting the
/// list or calling the store unreadable (#437). Revoked roots still count, so 64 active grants and
/// one revoked-then-regranted make 65.
#[test]
fn more_than_64_roots_are_one_unknown_naming_the_cap() {
    let tmp = temp_root();
    let svc = open(&tmp.path().join("state"), DatasetClass::Personal);
    if !native_identity_available() {
        return;
    }
    let dirs: Vec<PathBuf> = (0..65)
        .map(|i| {
            let d = tmp.path().join(format!("owner-{i}"));
            std::fs::create_dir(&d).unwrap();
            d.canonicalize().unwrap()
        })
        .collect();
    let mut first = 0;
    for (i, d) in dirs[..64].iter().enumerate() {
        let (root, _) = svc.inner.grant(d, "cli_flag", false).unwrap();
        if i == 0 {
            first = root;
        }
    }
    let (bundle, _, _) = preview(&svc);
    assert_eq!(bundle["roots"].as_array().map(Vec::len), Some(64));
    let grant = crate::db::roots(&svc.inner.db(), Some(first)).unwrap()[0].grant;
    svc.inner.revoke(grant).unwrap();
    svc.inner.grant(&dirs[64], "cli_flag", false).unwrap();
    let (bundle, _, _) = preview(&svc);
    assert_eq!(bundle["roots"], json!({"unknown": "root_cap_exceeded"}));
}

/// An online-only cloud file is named (#437), not folded into "not a plain local file".
#[test]
fn cloud_placeholder_attributes_are_recognised() {
    use crate::paths::placeholder_bits;
    const OFFLINE: u32 = 0x1000;
    const RECALL_ON_OPEN: u32 = 0x4_0000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
    const REPARSE_POINT: u32 = 0x400;
    const ARCHIVE: u32 = 0x20;
    for bits in [
        OFFLINE,
        RECALL_ON_OPEN,
        RECALL_ON_DATA_ACCESS,
        ARCHIVE | OFFLINE,
    ] {
        assert!(placeholder_bits(bits), "{bits:#x}");
    }
    // A plain file and a plain link are not placeholders: those keep the general refusal.
    assert!(!placeholder_bits(ARCHIVE));
    assert!(!placeholder_bits(REPARSE_POINT));
}

#[cfg(windows)]
#[test]
fn an_existing_diagnostic_target_is_found_with_windows_name_casing() {
    if export_refuses_elevation() {
        return;
    }
    let tmp = temp_root();
    let original = tmp.path().join("report.json");
    std::fs::write(&original, b"chosen").unwrap();
    let checked = check_target(&tmp.path().join("REPORT.JSON")).unwrap();
    assert!(
        checked.exists,
        "an existing file must require replacement consent"
    );
    assert_eq!(write_to(&checked, b"previewed"), Ok(()));
    assert_eq!(std::fs::read(original).unwrap(), b"previewed");
    assert_eq!(names(tmp.path()).len(), 1);
}

/// A parent or ancestor replaced during confirmation cannot redirect either kind of export.
#[cfg(windows)]
#[test]
fn diagnostic_export_stays_in_the_pinned_folder_after_a_junction_swap() {
    if export_refuses_elevation() {
        return;
    }
    for (existing, readonly) in [(true, false), (false, false), (true, true)] {
        for ancestor in [false, true] {
            let tmp = temp_root();
            let selected = tmp.path().join("selected");
            let moved = tmp.path().join("moved");
            let other = tmp.path().join("other");
            let suffix = if ancestor { "nested" } else { "" };
            let folder = selected.join(suffix);
            let unrelated = other.join(suffix);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::create_dir_all(&unrelated).unwrap();
            if existing {
                std::fs::write(folder.join("report.json"), b"chosen").unwrap();
                std::fs::write(unrelated.join("report.json"), b"unrelated").unwrap();
            }
            let checked = check_target(&folder.join("report.json")).unwrap();
            assert_eq!(checked.exists, existing);
            // The swap happens while the owner's replace confirmation could be open.
            let relocated = match std::fs::rename(&selected, &moved) {
                Ok(()) => {
                    let made = std::process::Command::new("cmd")
                        .args(["/C", "mklink", "/J"])
                        .arg(&selected)
                        .arg(&other)
                        .output()
                        .unwrap();
                    assert!(made.status.success(), "{made:?}");
                    true
                }
                Err(e) => {
                    // Windows refuses to rename an ancestor with an open descendant directory.
                    assert!(
                        ancestor,
                        "the direct parent swap must exercise a real junction"
                    );
                    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
                    false
                }
            };
            let pinned = if relocated { &moved } else { &selected }.join(suffix);
            if readonly {
                let mut perms = std::fs::metadata(pinned.join("report.json"))
                    .unwrap()
                    .permissions();
                perms.set_readonly(true);
                std::fs::set_permissions(pinned.join("report.json"), perms).unwrap();
            }
            let result = write_to(&checked, b"previewed");
            let other_names = names(&unrelated);
            let other_bytes = std::fs::read(unrelated.join("report.json")).ok();
            // Remove only the synthetic junction before the fixture cleans up its directories.
            if relocated {
                std::fs::remove_dir(&selected).unwrap();
            }
            if readonly {
                let mut perms = std::fs::metadata(pinned.join("report.json"))
                    .unwrap()
                    .permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                std::fs::set_permissions(pinned.join("report.json"), perms).unwrap();
            }
            assert_eq!(
                other_bytes.as_deref(),
                existing.then_some(b"unrelated".as_slice()),
                "export escaped the chosen folder (existing={existing}, ancestor={ancestor})"
            );
            assert_eq!(other_names.len(), usize::from(existing));
            assert_eq!(
                result,
                if readonly {
                    Err(DiagnosticsSaveRefusal::WriteFailed)
                } else {
                    Ok(())
                }
            );
            assert_eq!(
                std::fs::read(pinned.join("report.json")).unwrap(),
                if readonly {
                    b"chosen".as_slice()
                } else {
                    b"previewed".as_slice()
                }
            );
            assert_eq!(names(&pinned), ["report.json"]);
        }
    }
}

/// A file that appears between the replace check and the rename was never confirmed: it is left
/// as it was and no temporary file survives (#437).
#[test]
fn a_file_that_appears_after_the_check_is_not_replaced() {
    if export_refuses_elevation() {
        return;
    }
    let tmp = temp_root();
    let target = tmp.path().join("late.json");
    let checked = check_target(&target).unwrap();
    assert!(!checked.exists);
    std::fs::write(&target, b"keep").unwrap();
    assert_eq!(
        write_to(&checked, b"{}"),
        Err(DiagnosticsSaveRefusal::WriteFailed)
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    assert_eq!(names(tmp.path()), ["late.json"]);
}

#[test]
fn a_second_dialog_is_busy_never_stacked() {
    let tmp = temp_root();
    let svc = open(&tmp.path().join("state"), DatasetClass::Synthetic);
    let (_, _, d) = preview(&svc);
    let _open = svc.inner.dialog.lock().unwrap();
    let saves = Saves::new(vec![]);
    let mut req = request("diagnostics.save", json!({ "digest": d }));
    req.deadline_ms = Some(serde_json::from_value(json!(60_000)).unwrap());
    let e = err(svc.call(req, &CallContext::tauri(saves.clone())));
    assert_eq!(e.code, ErrorCode::Busy);
    assert_eq!(saves.count(), 0);
}

/// The contract example's size and digest are those of its bundle's canonical text, so the app
/// and the mock check a preview exactly as they check the engine's.
#[test]
fn the_contract_example_digest_is_the_canonical_text_digest() {
    let example: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../contracts/v3/examples/commands/diagnostics.preview.result.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let bundle: DiagnosticsBundle = serde_json::from_value(example["bundle"].clone()).unwrap();
    let bytes = canonical(&bundle).unwrap();
    assert_eq!(example["bytes"], bytes.len().to_string());
    assert_eq!(example["digest"], digest(&bytes));
}
