//! Diagnostic bundles (docs/42 ADR-V3-24): the bundle is an allowlist that never carries a path,
//! name, label or volume identity, and `diagnostics.save` writes exactly the previewed bytes to an
//! owner-picked file, or nothing. Synthetic data and disposable folders only.
use super::*;
#[cfg(windows)]
use crate::diagnostics::write_export;
use crate::diagnostics::{canonical, digest, SAVE_TITLE};
use std::path::PathBuf;

/// Answers each save dialog with the next queued path (`None` = the owner cancelled).
struct Saves {
    queue: Mutex<Vec<Option<PathBuf>>>,
    asked: Mutex<Vec<(String, String)>>,
}

impl Saves {
    fn new(picks: Vec<Option<PathBuf>>) -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(picks.into_iter().rev().collect()),
            asked: Mutex::default(),
        })
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
    fn save_file(&self, title: &str, default_name: &str) -> Option<PathBuf> {
        self.asked
            .lock()
            .unwrap()
            .push((title.to_owned(), default_name.to_owned()));
        self.queue.lock().unwrap().pop().flatten()
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
    if loomward_windows::enumerate::running_elevated() {
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
    assert_eq!(bundle["stores"]["catalog_schema_version"], 7);
    assert_eq!(bundle["stores"]["state_schema_version"], 7);
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

/// An existing regular file is replaced only because the native dialog asked the owner first.
#[test]
fn an_owner_confirmed_replace_overwrites_a_regular_file_whole() {
    let tmp = temp_root();
    let target = tmp.path().join("existing.json");
    std::fs::write(&target, vec![b'x'; 300_000]).unwrap();
    let svc = open(&tmp.path().join("state"), DatasetClass::Synthetic);
    let (_, bytes, d) = preview(&svc);
    let r = save(&svc, &Saves::new(vec![Some(target.clone())]), &d);
    assert_eq!(r["outcome"], "saved");
    assert_eq!(
        std::fs::read(&target).unwrap(),
        bytes,
        "no tail of the old file"
    );
}

#[cfg(windows)]
#[test]
fn unsupported_targets_are_refused_before_anything_is_written() {
    let tmp = temp_root();
    let out = tmp.path().join("out");
    std::fs::create_dir_all(out.join("a-folder")).unwrap();
    std::fs::write(out.join("regular.json"), b"keep").unwrap();
    // Both would reach the real `out` folder without their lexical guard: a relative path from
    // the test's working directory, and the `\\.\` device namespace naming a local drive.
    let local = out.to_string_lossy();
    let local = local.strip_prefix(r"\\?\").unwrap_or(&local);
    let relative = Path::new("../../target/test-roots")
        .join(tmp.path().file_name().unwrap())
        .join("out")
        .join("relative.json");
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

/// A failed write leaves the target as it was and no temporary file behind.
#[cfg(windows)]
#[test]
fn a_failed_write_leaves_the_target_and_no_partial_file() {
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
