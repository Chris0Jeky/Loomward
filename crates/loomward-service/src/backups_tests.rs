//! Backups stage 1: the open-file dialog's path stays in Rust, at most 256 KiB is read, only
//! normalised evidence is stored, and every refusal names its rule. Synthetic receipts only.
use super::*;
use crate::backups::{read_receipt, CHECK_TITLE, MAX_AGE_SECONDS, SNAPSHOTS_TITLE};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const SNAPSHOT: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const VERSION: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// Answers each file dialog with the next queued path (`None` = the owner cancelled).
struct Files {
    queue: Mutex<Vec<Option<PathBuf>>>,
    titles: Mutex<Vec<String>>,
    asked: AtomicUsize,
}

impl Files {
    fn new(picks: Vec<Option<PathBuf>>) -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(picks.into_iter().rev().collect()),
            titles: Mutex::default(),
            asked: AtomicUsize::new(0),
        })
    }
}

impl NativeDialogs for Files {
    fn pick_folder(&self) -> Option<PathBuf> {
        panic!("backups.import opened the folder picker")
    }
    fn pick_file(&self, title: &str) -> Option<PathBuf> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.titles.lock().unwrap().push(title.to_owned());
        self.queue.lock().unwrap().pop().flatten()
    }
    fn confirm_disclosure(&self, _: &DisclosureSummary) -> Result<bool, ErrorBody> {
        Ok(false)
    }
}

fn snapshots() -> String {
    json!([{"id": SNAPSHOT, "time": "2026-10-09T21:00:00Z", "paths": ["/synthetic-fixture"],
        "hostname": "synthetic-host", "username": "synthetic-owner", "tree": "c".repeat(64)}])
    .to_string()
}

const CLEAN_CHECK: &str = r#"{"message_type":"summary","num_errors":0,"broken_packs":null,"suggest_repair_index":false,"suggest_prune":false}"#;

fn write(dir: &Path, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn payload(label: &str, source: Option<&str>, target: Option<&str>, integrity: bool) -> Value {
    json!({"repository_label": label, "snapshot_id": SNAPSHOT, "source_volume_id": source,
        "target_volume_id": target, "integrity_check": integrity,
        "required": [{"relative_name": "fixture/ledger.ods", "version": VERSION}]})
}

fn import(svc: &Service, files: &Arc<Files>, body: Value) -> ResponseEnvelope {
    let mut req = request("backups.import", body);
    req.deadline_ms = Some(serde_json::from_value(json!(60_000)).unwrap());
    svc.call(req, &CallContext::tauri(files.clone()))
}

fn imported(r: ResponseEnvelope) -> Payload {
    let (result, _) = ok("backups.import", r);
    assert_eq!(result["outcome"], "imported", "{result:?}");
    result
}

fn refusal(r: ResponseEnvelope) -> String {
    let (result, _) = ok("backups.import", r);
    assert_eq!(result["outcome"], "refused", "{result:?}");
    assert_eq!(result["evidence"], Value::Null);
    result["refusal"].as_str().unwrap().to_owned()
}

/// A fresh personal session; the picker cooldown is cleared so tests can pick back to back.
fn personal(dir: &Path) -> Service {
    open(&dir.join("state"), DatasetClass::Personal)
}

fn rearm(svc: &Service) {
    *svc.inner.picker_last.lock().unwrap() = None;
}

fn list(svc: &Service) -> Vec<Value> {
    let (result, _) = ok("backups.list", call(svc, "backups.list", json!({})));
    result["evidence"].as_array().unwrap().clone()
}

fn volume(svc: &Service, key: &str) -> String {
    svc.inner
        .catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: key.into(),
            display_name: "Synthetic".into(),
            display_path: format!("Synthetic {key}"),
            root_file_id: Some(vec![key.len() as u8; 16]),
            filesystem: Some("NTFS".into()),
            origin: "owner_granted".into(),
            granted_via: "cli_flag".into(),
            observed_at_ns: 0,
        }))
        .unwrap();
    let row: i64 = svc
        .inner
        .db()
        .query_row(
            "SELECT id FROM main.volume WHERE volume_key=?1",
            [key],
            |r| r.get(0),
        )
        .unwrap();
    svc.inner.volume_id(row)
}

#[test]
fn import_reads_the_picked_receipt_and_stores_only_normalised_evidence() {
    let tmp = temp_root();
    let svc = personal(tmp.path());
    // A hostile, bidi-bearing file name: it must never reach the response or the state store.
    let picked = write(tmp.path(), "receipt\u{202e}nosj.json", snapshots());
    let files = Files::new(vec![Some(picked)]);
    let before = svc.inner.revisions().unwrap().1;
    let result = imported(import(
        &svc,
        &files,
        payload("synthetic-nas", None, None, false),
    ));
    assert_eq!(*files.titles.lock().unwrap(), [SNAPSHOTS_TITLE]);
    let e = &result["evidence"];
    assert_eq!(e["evidence_id"], "be_1");
    assert_eq!(e["snapshot_taken"], true);
    assert_eq!(e["snapshot_time"], "2026-10-09T21:00:00Z");
    assert_eq!(e["integrity_checked"], false);
    assert_eq!(e["integrity_observed_at"], Value::Null);
    assert_eq!(e["restore_drill_succeeded"], false);
    assert_eq!(e["reported_recovery_evidence"], false);
    assert_eq!(e["restore_coverage"], json!([]));
    assert_eq!(e["topology"], "unknown");
    assert_eq!(e["topology_basis"], "undeclared");
    assert_eq!(e["max_age_seconds"], MAX_AGE_SECONDS);
    assert_eq!(e["proposed_drill"]["executable"], false);
    assert_eq!(
        e["proposed_drill"]["compare_metadata"],
        json!(["acl", "timestamps"])
    );
    for refusal in [
        "version_not_covered",
        "target_not_independent",
        "credentials_unavailable",
        "integrity_not_checked",
        "restore_not_succeeded",
    ] {
        assert!(
            e["refusals"].as_array().unwrap().contains(&json!(refusal)),
            "{refusal}"
        );
    }
    assert!(svc.inner.revisions().unwrap().1 > before, "state_rev moves");
    // #449: identity is stable, but age belongs to each read, not the earlier import reply.
    let observed_ns: i64 = svc
        .inner
        .db()
        .query_row(
            "SELECT snapshot_observed_ns FROM st.backup_evidence WHERE id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let before_list = crate::now_ns() / 1_000_000_000;
    let listed = list(&svc);
    let after_list = crate::now_ns() / 1_000_000_000;
    assert_eq!(listed.len(), 1);
    let age = listed[0]["snapshot_age_seconds"].as_i64().unwrap();
    let earliest = before_list.min(after_list) - observed_ns / 1_000_000_000;
    let latest = before_list.max(after_list) - observed_ns / 1_000_000_000;
    assert!(
        (earliest..=latest).contains(&age),
        "age outside its read window"
    );
    let mut expected = e.clone();
    expected["snapshot_age_seconds"] = json!(age);
    assert_eq!(listed, vec![expected]);
    let stored: String = svc
        .inner
        .db()
        .query_row(
            "SELECT group_concat(evidence_json) FROM st.backup_evidence",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let tmp_text = tmp.path().to_string_lossy().replace('\\', "\\\\");
    for leak in [
        "receipt",
        "nosj",
        "synthetic-host",
        "synthetic-owner",
        "/synthetic-fixture",
        tmp_text.as_str(),
    ] {
        assert!(!stored.contains(leak), "stored {leak}");
        assert!(
            !result.values().any(|v| v.to_string().contains(leak)),
            "echoed {leak}"
        );
    }
}

#[test]
fn integrity_output_with_status_lines_is_extracted_and_never_claims_a_full_data_read() {
    let tmp = temp_root();
    let svc = personal(tmp.path());
    let snaps = write(tmp.path(), "snapshots.json", snapshots());
    let check = write(
        tmp.path(),
        "check.json",
        format!("{{\"message_type\":\"status\",\"percent_done\":0.5}}\n\n{CLEAN_CHECK}\r\n"),
    );
    let files = Files::new(vec![Some(snaps.clone()), Some(check)]);
    let e = &imported(import(
        &svc,
        &files,
        payload("synthetic-nas", None, None, true),
    ))["evidence"];
    assert_eq!(
        *files.titles.lock().unwrap(),
        [SNAPSHOTS_TITLE, CHECK_TITLE]
    );
    assert_eq!(e["integrity_checked"], true);
    assert_eq!(e["integrity_read_all_data"], false);
    assert!(e["integrity_observed_at"].is_string());
    assert!(e["refusals"]
        .as_array()
        .unwrap()
        .contains(&json!("integrity_not_checked")));
    // A restic fatal error in place of the summary keeps the snapshot evidence (#349).
    rearm(&svc);
    let fatal = write(
        tmp.path(),
        "fatal.txt",
        "Fatal: unable to open config file\n",
    );
    let files = Files::new(vec![Some(snaps), Some(fatal)]);
    let e = &imported(import(
        &svc,
        &files,
        payload("synthetic-nas", None, None, true),
    ))["evidence"];
    assert_eq!(
        (e["snapshot_taken"].clone(), e["integrity_checked"].clone()),
        (json!(true), json!(false))
    );
    assert_eq!(
        e["evidence_id"], "be_1",
        "a re-import replaces the repository's record"
    );
    assert_eq!(list(&svc).len(), 1);
}

#[test]
fn malformed_oversized_and_non_regular_picks_are_refused_by_name() {
    let tmp = temp_root();
    let svc = personal(tmp.path());
    let big = write(tmp.path(), "big.json", vec![b' '; 256 * 1024 + 1]);
    let malformed = write(tmp.path(), "bad.json", "[{");
    let wrong = write(tmp.path(), "wrong.json", "{\"snapshots\":[]}");
    let dir = tmp.path().join("folder.json");
    std::fs::create_dir(&dir).unwrap();
    let local = write(tmp.path(), "local.json", snapshots());
    let mut cases = vec![
        (big, "too_large"),
        (malformed, "invalid_json"),
        (wrong, "invalid_receipt"),
        (dir, "not_a_regular_file"),
        (tmp.path().join("missing.json"), "unreadable"),
        // Device names open a device, not a file: refused before anything is opened.
        (tmp.path().join("CON"), "not_a_regular_file"),
        (tmp.path().join("com1 .json"), "not_a_regular_file"),
        (tmp.path().join("COM\u{00b9}.json"), "not_a_regular_file"),
        (tmp.path().join("LPT\u{00b2}.txt"), "not_a_regular_file"),
        (tmp.path().join("com\u{00b3} .json"), "not_a_regular_file"),
        // Network and device namespaces are refused lexically: no network or device I/O.
        (
            PathBuf::from(r"\\synthetic-host.invalid\share\snapshots.json"),
            "not_a_regular_file",
        ),
        (PathBuf::from(r"\\.\pipe\synthetic"), "not_a_regular_file"),
    ];
    #[cfg(windows)]
    {
        // The same local file through the GLOBALROOT namespace is still refused by name.
        let text = local.to_string_lossy().into_owned();
        cases.push((
            PathBuf::from(format!(r"\\?\GLOBALROOT\??\{text}")),
            "not_a_regular_file",
        ));
    }
    let link = tmp.path().join("link.json");
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&local, &link).is_ok();
    #[cfg(not(windows))]
    let linked = std::os::unix::fs::symlink(&local, &link).is_ok();
    if linked {
        cases.push((link, "not_a_regular_file"));
    }
    for (path, expected) in cases {
        rearm(&svc);
        let files = Files::new(vec![Some(path.clone())]);
        let got = refusal(import(
            &svc,
            &files,
            payload("synthetic-nas", None, None, false),
        ));
        assert_eq!(got, expected, "{}", path.display());
    }
    rearm(&svc);
    let files = Files::new(vec![Some(local)]);
    let body = payload("synthetic-nas", None, None, false);
    imported(import(&svc, &files, body));
    assert_eq!(list(&svc).len(), 1, "refusals stored nothing");
}

#[test]
fn the_reader_never_returns_more_than_the_budget() {
    let tmp = temp_root();
    let exact = write(tmp.path(), "exact.json", vec![b' '; 256 * 1024]);
    assert_eq!(read_receipt(&exact).unwrap().0.len(), 256 * 1024);
    let over = write(tmp.path(), "over.json", vec![b' '; 300 * 1024]);
    assert_eq!(
        read_receipt(&over).unwrap_err(),
        BackupImportRefusal::TooLarge
    );
}

#[test]
fn a_receipt_under_a_reparse_parent_is_refused_before_resolution() {
    let tmp = temp_root();
    let control = write(tmp.path(), "control.json", snapshots());
    let target = tmp.path().join("target");
    std::fs::create_dir_all(target.join("nested")).unwrap();
    write(&target, "receipt.json", snapshots());
    write(&target.join("nested"), "receipt.json", snapshots());
    let link = tmp.path().join("linked-parent");
    #[cfg(windows)]
    {
        // Local junctions need no symlink privilege and never contact a remote host.
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J", "linked-parent", "target"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
    }
    #[cfg(not(windows))]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let results = [
        "receipt.json",
        "nested/receipt.json",
        "missing/receipt.json",
    ]
    .map(|relative| read_receipt(&link.join(relative)));
    #[cfg(windows)]
    std::fs::remove_dir(&link).unwrap();
    #[cfg(not(windows))]
    std::fs::remove_file(&link).unwrap();
    for result in results {
        assert_eq!(
            result.unwrap_err(),
            BackupImportRefusal::NotARegularFile,
            "reparse ancestor must be refused even when the descendant is missing"
        );
    }
    assert_eq!(read_receipt(&control).unwrap().0, snapshots().as_bytes());
}

#[test]
fn a_stream_colon_receipt_is_refused_before_open() {
    let tmp = temp_root();
    let regular = write(tmp.path(), "regular.json", snapshots());
    // An NTFS stream suffix is refused by name before anything is opened; the file on disk is
    // untouched and still reads as a normal local receipt.
    assert_eq!(
        read_receipt(&tmp.path().join("regular.json:stream")).unwrap_err(),
        BackupImportRefusal::NotARegularFile
    );
    #[cfg(not(windows))]
    {
        // A colon is a legal file name elsewhere: even a real stream-named file holding a valid
        // receipt is refused without reading it.
        let colon = write(tmp.path(), "colon.json:stream", snapshots());
        assert_eq!(
            read_receipt(&colon).unwrap_err(),
            BackupImportRefusal::NotARegularFile
        );
        assert_eq!(std::fs::read(&colon).unwrap(), snapshots().as_bytes());
    }
    assert_eq!(read_receipt(&regular).unwrap().0, snapshots().as_bytes());
}

#[test]
fn unrepresentable_receipt_times_refuse_without_replacing_evidence() {
    let tmp = temp_root();
    let svc = personal(tmp.path());
    let snaps = write(tmp.path(), "snapshots.json", snapshots());
    let check = write(tmp.path(), "check.json", CLEAN_CHECK);
    let future = std::time::UNIX_EPOCH + Duration::from_secs(10_413_792_000); // 2300-01-01
    let original = imported(import(
        &svc,
        &Files::new(vec![Some(snaps.clone())]),
        payload("synthetic-nas", None, None, false),
    ));
    for (path, integrity) in [(&snaps, false), (&check, true)] {
        let handle = std::fs::File::options().write(true).open(path).unwrap();
        handle.set_modified(future).unwrap();
        drop(handle);
        assert_eq!(std::fs::metadata(path).unwrap().modified().unwrap(), future);
        rearm(&svc);
        let picks = if integrity {
            vec![Some(snaps.clone()), Some(check.clone())]
        } else {
            vec![Some(snaps.clone())]
        };
        assert_eq!(
            refusal(import(
                &svc,
                &Files::new(picks),
                payload("synthetic-nas", None, None, integrity),
            )),
            "unreadable"
        );
        assert_eq!(
            list(&svc)[0]["snapshot_observed_at"],
            original["evidence"]["snapshot_observed_at"]
        );
        assert_eq!(list(&svc).len(), 1);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::SystemTime::now())
            .unwrap();
    }
}

#[test]
fn v1_stores_migrate_to_importable_backup_evidence() {
    let tmp = temp_root();
    let state = tmp.path().join("state");
    std::fs::create_dir(&state).unwrap();
    for (name, schema) in [
        (
            "catalog.db",
            include_str!("../../loomward-catalog/tests/fixtures/catalog-v1.sql"),
        ),
        (
            "state.db",
            include_str!("../../loomward-catalog/tests/fixtures/state-v1.sql"),
        ),
    ] {
        let conn = rusqlite::Connection::open(state.join(name)).unwrap();
        conn.execute_batch(schema).unwrap();
        conn.pragma_update(None, "application_id", loomward_catalog::APPLICATION_ID)
            .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute("INSERT INTO meta VALUES('dataset_class','personal')", [])
            .unwrap();
    }
    let svc = personal(tmp.path());
    assert!(list(&svc).is_empty());
    let snaps = write(tmp.path(), "snapshots.json", snapshots());
    imported(import(
        &svc,
        &Files::new(vec![Some(snaps)]),
        payload("synthetic-nas", None, None, false),
    ));
    assert_eq!(list(&svc).len(), 1);
    drop(svc);
    assert_eq!(list(&personal(tmp.path())).len(), 1);
}

#[test]
fn wrong_class_sessions_and_adapters_never_open_a_dialog() {
    let tmp = temp_root();
    let synthetic = open(&tmp.path().join("synthetic"), DatasetClass::Synthetic);
    let files = Files::new(vec![Some(tmp.path().join("never.json"))]);
    assert_eq!(
        refusal(import(
            &synthetic,
            &files,
            payload("synthetic-nas", None, None, true)
        )),
        "synthetic_session"
    );
    assert_eq!(files.asked.load(Ordering::SeqCst), 0);
    // HTTP has no native dialogs: the command does not exist there.
    let personal = personal(tmp.path());
    let r = personal.call(
        request(
            "backups.import",
            payload("synthetic-nas", None, None, false),
        ),
        &CallContext::http(),
    );
    assert_eq!(err(r).code, ErrorCode::CapabilityUnavailable);
    assert!(list(&synthetic).is_empty() && list(&personal).is_empty());
}

#[test]
fn cancelling_either_dialog_stores_nothing() {
    let tmp = temp_root();
    let svc = personal(tmp.path());
    let snaps = write(tmp.path(), "snapshots.json", snapshots());
    for picks in [vec![None], vec![Some(snaps), None]] {
        rearm(&svc);
        let files = Files::new(picks);
        let (result, _) = ok(
            "backups.import",
            import(&svc, &files, payload("synthetic-nas", None, None, true)),
        );
        assert_eq!(result["outcome"], "cancelled_by_user");
    }
    assert!(list(&svc).is_empty());
}

#[test]
fn topology_comes_from_volume_identity_never_from_text() {
    let tmp = temp_root();
    let svc = personal(tmp.path());
    let (a, b) = (volume(&svc, "vol-a"), volume(&svc, "vol-bb"));
    let snaps = write(tmp.path(), "snapshots.json", snapshots());
    for (source, target, topology, basis) in [
        (
            Some(a.as_str()),
            Some(a.as_str()),
            "same_volume",
            "same_volume_identity",
        ),
        (
            Some(a.as_str()),
            Some(b.as_str()),
            "unknown",
            "different_volumes_device_unknown",
        ),
        (Some(a.as_str()), None, "unknown", "undeclared"),
        (None, None, "unknown", "undeclared"),
    ] {
        rearm(&svc);
        let files = Files::new(vec![Some(snaps.clone())]);
        let e = &imported(import(
            &svc,
            &files,
            payload("synthetic-nas", source, target, false),
        ))["evidence"];
        assert_eq!(
            (e["topology"].as_str(), e["topology_basis"].as_str()),
            (Some(topology), Some(basis))
        );
        assert!(e["refusals"]
            .as_array()
            .unwrap()
            .contains(&json!("target_not_independent")));
    }
    // An unknown or forged volume ID is refused before any dialog opens.
    rearm(&svc);
    let files = Files::new(vec![Some(snaps)]);
    let r = import(
        &svc,
        &files,
        payload("synthetic-nas", Some(&a), Some("vo_forged"), false),
    );
    assert_eq!(err(r).code, ErrorCode::NotFound);
    assert_eq!(files.asked.load(Ordering::SeqCst), 0);
}

#[test]
fn ages_and_staleness_are_recomputed_on_every_read() {
    let tmp = temp_root();
    let svc = personal(tmp.path());
    let snaps = write(tmp.path(), "snapshots.json", snapshots());
    let files = Files::new(vec![Some(snaps)]);
    imported(import(
        &svc,
        &files,
        payload("synthetic-nas", None, None, false),
    ));
    let refusals = |e: &Value| e["refusals"].as_array().unwrap().clone();
    let fresh = list(&svc).pop().unwrap();
    assert!(fresh["snapshot_age_seconds"].as_u64().unwrap() < 3600);
    assert!(!refusals(&fresh).contains(&json!("stale_observation")));
    let eight_days = (MAX_AGE_SECONDS as i64 + 86_400) * 1_000_000_000;
    svc.inner
        .db()
        .execute(
            "UPDATE st.backup_evidence SET snapshot_observed_ns=snapshot_observed_ns-?1",
            [eight_days],
        )
        .unwrap();
    let stale = list(&svc).pop().unwrap();
    assert!(stale["snapshot_age_seconds"].as_u64().unwrap() > MAX_AGE_SECONDS);
    assert_eq!(
        refusals(&stale)
            .iter()
            .filter(|r| **r == json!("stale_observation"))
            .count(),
        1
    );
    svc.inner
        .db()
        .execute(
            "UPDATE st.backup_evidence SET snapshot_observed_ns=snapshot_observed_ns+?1*2",
            [eight_days],
        )
        .unwrap();
    let future = list(&svc).pop().unwrap();
    assert_eq!(future["snapshot_age_seconds"], Value::Null);
    assert!(refusals(&future).contains(&json!("invalid_observation_time")));
    assert!(!refusals(&future).contains(&json!("stale_observation")));
}

#[test]
fn forget_removes_only_loomwards_record() {
    let tmp = temp_root();
    let svc = personal(tmp.path());
    let snaps = write(tmp.path(), "snapshots.json", snapshots());
    let files = Files::new(vec![Some(snaps.clone())]);
    let id = imported(import(
        &svc,
        &files,
        payload("synthetic-nas", None, None, false),
    ))["evidence"]["evidence_id"]
        .clone();
    let before = svc.inner.revisions().unwrap().1;
    let (result, meta) = ok(
        "backups.forget",
        call(&svc, "backups.forget", json!({"evidence_id": id})),
    );
    assert_eq!(result["evidence_id"], id);
    assert!(meta.state_rev.is_some() && svc.inner.revisions().unwrap().1 > before);
    assert!(list(&svc).is_empty());
    assert!(snaps.exists(), "the receipt file is untouched");
    for id in [id, json!("be_01"), json!("be_999")] {
        let r = call(&svc, "backups.forget", json!({"evidence_id": id}));
        assert_eq!(err(r).code, ErrorCode::NotFound, "{id}");
    }
}

#[test]
fn the_repository_limit_refuses_a_65th_label_but_replaces_an_existing_one() {
    let tmp = temp_root();
    let svc = personal(tmp.path());
    for i in 0..64 {
        svc.inner.db().execute(
            "INSERT INTO st.backup_evidence(repository_label,evidence_json,snapshot_observed_ns,imported_at_ns) VALUES(?1,'{}',0,0)",
            [format!("synthetic-{i:02}")],
        ).unwrap();
    }
    let snaps = write(tmp.path(), "snapshots.json", snapshots());
    let files = Files::new(vec![Some(snaps.clone())]);
    assert_eq!(
        refusal(import(
            &svc,
            &files,
            payload("synthetic-new", None, None, false)
        )),
        "repository_limit"
    );
    rearm(&svc);
    let files = Files::new(vec![Some(snaps)]);
    imported(import(
        &svc,
        &files,
        payload("synthetic-07", None, None, false),
    ));
    let count: i64 = svc
        .inner
        .db()
        .query_row("SELECT count(*) FROM st.backup_evidence", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 64);
}

#[test]
fn import_and_folder_picker_share_one_dialog_and_cooldown() {
    let tmp = temp_root();
    let svc = personal(tmp.path());
    let snaps = write(tmp.path(), "snapshots.json", snapshots());
    let files = Files::new(vec![Some(snaps.clone()), Some(snaps)]);
    imported(import(
        &svc,
        &files,
        payload("synthetic-nas", None, None, false),
    ));
    let r = import(&svc, &files, payload("synthetic-nas", None, None, false));
    assert_eq!(err(r).code, ErrorCode::Busy);
    assert_eq!(files.asked.load(Ordering::SeqCst), 1);
}
