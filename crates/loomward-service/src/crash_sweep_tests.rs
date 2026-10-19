//! Crash injection at the service's own writers (q-39, docs/45-daily-use-readiness.md row 1).
//!
//! The catalogue sweep (`loomward-catalog/tests/crash_sweep.rs`) covers the catalogue writer. This
//! one drives real service commands in a child process (this test binary re-run on `crash_child`)
//! over the service connection (`catalog.db` with `state.db` attached) and the learning store's:
//! collections and their members and references, a human label, a refit's model commit, backup
//! evidence import, replace and forget, and each service open's own recovery writes. Every row
//! change, commit and post-commit of those connections and of the catalogue writer they call is a
//! boundary (`loomward_catalog::crash_arm`). A clean run enumerates them; one child per boundary is
//! ended there (`exit(86)`); the parent reopens the service and asserts the invariants in `verify`.
//!
//! `cargo test -p loomward-service crash_sweep` samples the row boundaries (every commit boundary is
//! always swept); `LOOMWARD_CRASH_SWEEP=full` sweeps every boundary.
use super::learning_tests::{feedback, finish_refit};
use super::*;
use loomward_catalog::CRASH_EXIT;
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command as Process;

const COLLECTION: &str = "Crash keep";
const LABEL_KEY: &str = "ce_crash";
const REPO: &str = "synthetic-crash-nas";

// ---- the child ------------------------------------------------------------------------------

struct Ack(PathBuf);
impl Ack {
    /// One line per step, fsynced before the next starts: the receipt the caller was given.
    fn log(&self, line: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.0)
            .unwrap();
        writeln!(f, "{line}").unwrap();
        f.sync_all().unwrap();
    }
}

fn store(work: &Path) -> PathBuf {
    work.join("store")
}

/// The three fixture files' node IDs, sealed for this session.
fn nodes(svc: &Service) -> Vec<String> {
    let conn = svc.inner.db();
    let mut stmt = conn
        .prepare("SELECT id,born_run FROM main.file ORDER BY id")
        .unwrap();
    let ids = stmt
        .query_map([], |r| {
            Ok(svc.inner.ids.seal(
                loomward_catalog::NodeKey::File(r.get(0)?),
                r.get(1)?,
                &svc.inner.instance,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    ids
}

fn update(svc: &Service, id: &str, add: &[String], remove: &[String]) -> ResponseEnvelope {
    let mut r = request(
        "collections.update_members",
        json!({"collection_id":id,"add":add,"remove":remove}),
    );
    r.expected_state_rev = Some(Generation(svc.inner.revisions().unwrap().1 as u64));
    svc.call(r, &CallContext::http())
}

/// Answers every receipt dialog with one synthetic receipt.
struct Receipt(PathBuf);
impl NativeDialogs for Receipt {
    fn pick_folder(&self) -> Option<PathBuf> {
        None
    }
    fn pick_file(&self, _: &str) -> Option<PathBuf> {
        Some(self.0.clone())
    }
    fn confirm_disclosure(&self, _: &DisclosureSummary) -> Result<bool, ErrorBody> {
        Ok(false)
    }
}
fn import(svc: &Service, work: &Path) -> ResponseEnvelope {
    *svc.inner.picker_last.lock().unwrap() = None;
    let mut req = request(
        "backups.import",
        json!({"repository_label": REPO, "snapshot_id": "1".repeat(64), "source_volume_id": null,
            "target_volume_id": null, "integrity_check": false,
            "required": [{"relative_name": "fixture/ledger.ods", "version": format!("sha256:{}", "a".repeat(64))}]}),
    );
    req.deadline_ms = Some(Int::new(60_000).unwrap());
    svc.call(
        req,
        &CallContext::tauri(Arc::new(Receipt(work.join("snapshots.json")))),
    )
}

/// Collections (create, add, remove), a human label and a refit, in a synthetic session.
fn meaning(work: &Path, ack: &Ack) {
    let svc = open(&store(work), DatasetClass::Synthetic);
    ack.log("open");
    let ids = nodes(&svc);
    ack.log("begin create");
    let (created, _) = ok(
        "collections.create",
        call(&svc, "collections.create", json!({"name":COLLECTION})),
    );
    let co = created["collection_id"].as_str().unwrap().to_owned();
    ack.log("create");
    ack.log("begin add");
    ok(
        "collections.update_members",
        update(&svc, &co, &ids[..2], &[]),
    );
    ack.log("add");
    ack.log("begin remove");
    ok(
        "collections.update_members",
        update(&svc, &co, &[], &ids[1..2]),
    );
    ack.log("remove");
    // A node no collection references yet: the label takes a new reference through the catalogue writer.
    ack.log("begin label");
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&ids[2], LABEL_KEY, Some("Documents"), false),
        ),
    );
    ack.log("label");
    ack.log("begin refit");
    finish_refit(&svc);
    ack.log("refit");
    drop(svc);
    ack.log("closed");
}

/// Backup evidence: import, replace under the same label, forget; in a personal session.
fn backup(work: &Path, ack: &Ack) {
    let svc = open(&store(work), DatasetClass::Personal);
    ack.log("open");
    ack.log("begin import");
    let (first, _) = ok("backups.import", import(&svc, work));
    assert_eq!(first["outcome"], "imported", "{first:?}");
    ack.log("import");
    ack.log("begin replace");
    let (second, _) = ok("backups.import", import(&svc, work));
    assert_eq!(second["outcome"], "imported", "{second:?}");
    ack.log("replace");
    ack.log("begin forget");
    ok(
        "backups.forget",
        call(
            &svc,
            "backups.forget",
            json!({"evidence_id": second["evidence"]["evidence_id"]}),
        ),
    );
    ack.log("forget");
    drop(svc);
    ack.log("closed");
}

/// Entry point of the killed process; a no-op in a normal test run.
#[test]
fn crash_child() {
    let Ok(scenario) = std::env::var("LOOMWARD_CRASH_CHILD") else {
        return;
    };
    let work = PathBuf::from(std::env::var("LOOMWARD_CRASH_WORK").unwrap());
    let ack = Ack(work.join("acks.log"));
    match scenario.as_str() {
        "meaning" => meaning(&work, &ack),
        "backup" => backup(&work, &ack),
        other => panic!("scenario {other}"),
    }
}

// ---- the parent -----------------------------------------------------------------------------

/// The store a scenario starts from, built outside the swept process.
fn prepare(scenario: &str, work: &Path) {
    std::fs::create_dir_all(work).unwrap();
    if scenario == "backup" {
        drop(open(&store(work), DatasetClass::Personal));
        let receipt = json!([{"id": "1".repeat(64), "time": "2026-10-09T21:00:00Z", "paths": ["/synthetic-fixture"],
            "hostname": "synthetic-host", "username": "synthetic-owner", "tree": "c".repeat(64)}]);
        std::fs::write(work.join("snapshots.json"), receipt.to_string()).unwrap();
        return;
    }
    let svc = open(&store(work), DatasetClass::Synthetic);
    fixture(&svc, 3);
    // Catalogue-only synthetic native identities, as the collection and learning tests use.
    svc.inner
        .db()
        .execute_batch(
            "UPDATE main.file SET file_id=CAST(printf('%016d',id) AS BLOB),created_ft=123;",
        )
        .unwrap();
}

struct Outcome {
    boundaries: u64,
    acks: Vec<String>,
    trace: BTreeMap<u64, String>,
}

fn child_test_name() -> String {
    let module = module_path!();
    let module = module.split_once("::").map_or(module, |(_, rest)| rest);
    format!("{module}::crash_child")
}

/// Run the scenario on a prepared store in `work`, killing it at boundary `at` (None: to the end).
fn run_child(scenario: &str, work: &Path, at: Option<u64>) -> Outcome {
    let trace_path = work.join("trace.tsv");
    let mut cmd = Process::new(std::env::current_exe().unwrap());
    cmd.args([
        child_test_name().as_str(),
        "--exact",
        "--nocapture",
        "--test-threads=1",
    ])
    .env("LOOMWARD_CRASH_CHILD", scenario)
    .env("LOOMWARD_CRASH_WORK", work)
    .env_remove("LOOMWARD_CRASH_AT")
    .env_remove("LOOMWARD_CRASH_TRACE");
    match at {
        Some(n) => cmd.env("LOOMWARD_CRASH_AT", n.to_string()),
        None => cmd.env("LOOMWARD_CRASH_TRACE", &trace_path),
    };
    let output = cmd.output().unwrap();
    let code = output.status.code();
    if at.is_some() && code == Some(0) {
        panic!("the crash point was not reached: {scenario} at {at:?}");
    }
    let want = if at.is_some() { CRASH_EXIT } else { 0 };
    assert!(
        code == Some(want),
        "{scenario} at {at:?}: the child exited {code:?}, expected {want} (unexpected child failure)\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let acks = std::fs::read_to_string(work.join("acks.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    let trace = std::fs::read_to_string(&trace_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .filter_map(|(n, label)| Some((n.parse().ok()?, label.to_owned())))
        .collect::<BTreeMap<u64, String>>();
    Outcome {
        boundaries: trace.keys().next_back().copied().unwrap_or(0),
        acks,
        trace,
    }
}

type Res<T> = std::result::Result<T, String>;
fn reader(store: &Path) -> Connection {
    let dir = loomward_catalog::sqlite_dir(store);
    let db = Connection::open(dir.join("catalog.db")).unwrap();
    db.execute(
        "ATTACH DATABASE ?1 AS st",
        [dir.join("state.db").to_string_lossy().as_ref()],
    )
    .unwrap();
    db
}
fn one(db: &Connection, sql: &str) -> Res<i64> {
    db.query_row(sql, [], |r| r.get(0))
        .map_err(|e| format!("{sql}: {e}"))
}

/// What must hold on both files whatever boundary the process died at.
fn structural(store: &Path) -> Vec<String> {
    let db = reader(store);
    let mut bad = Vec::new();
    for schema in ["main", "st"] {
        match db.query_row(&format!("PRAGMA {schema}.integrity_check"), [], |r| {
            r.get::<_, String>(0)
        }) {
            Ok(s) if s == "ok" => {}
            other => bad.push(format!("{schema} integrity_check: {other:?}")),
        }
        let fk = db
            .prepare(&format!("PRAGMA {schema}.foreign_key_check"))
            .and_then(|mut s| s.query_map([], |_| Ok(())).map(|r| r.count()));
        if !matches!(fk, Ok(0)) {
            bad.push(format!("{schema} foreign_key_check: {fk:?}"));
        }
    }
    for (what, sql) in [
        ("member without its collection", "SELECT count(*) FROM st.collection_member m WHERE NOT EXISTS(SELECT 1 FROM st.collection c WHERE c.id=m.collection_id)"),
        ("member without its reference", "SELECT count(*) FROM st.collection_member m WHERE NOT EXISTS(SELECT 1 FROM st.object_ref o WHERE o.id=m.object_ref_id)"),
        ("label without its reference", "SELECT count(*) FROM st.human_feedback h WHERE NOT EXISTS(SELECT 1 FROM st.object_ref o WHERE o.id=h.object_ref_id)"),
        ("learning node without its reference", "SELECT count(*) FROM st.learning_node n WHERE NOT EXISTS(SELECT 1 FROM st.object_ref o WHERE o.id=n.object_ref_id)"),
        ("label provenance without its label", "SELECT count(*) FROM st.learning_feature_grant p WHERE NOT EXISTS(SELECT 1 FROM st.human_feedback h WHERE h.event_id=p.event_id)"),
        ("label replay receipt without its label", "SELECT count(*) FROM st.learning_receipt r WHERE NOT EXISTS(SELECT 1 FROM st.human_feedback h WHERE h.client_event_id=r.client_event_id)"),
        ("label without its replay receipt", "SELECT count(*) FROM st.human_feedback h WHERE NOT EXISTS(SELECT 1 FROM st.learning_receipt r WHERE r.client_event_id=h.client_event_id)"),
        ("pending reference intent left behind", "SELECT count(*) FROM st.pending_reference"),
        // Cross-file: a state.db reference resolved into catalog.db names a live row of this catalogue.
        ("resolved reference bound to a missing catalogue row", "SELECT count(*) FROM st.object_ref o WHERE o.state='resolved' AND NOT ((o.kind='file' AND EXISTS(SELECT 1 FROM main.file f WHERE f.id=o.row_id AND f.born_run=o.born_run)) OR (o.kind='dir' AND EXISTS(SELECT 1 FROM main.dir d WHERE d.id=o.row_id AND d.born_run=o.born_run)))"),
        ("resolved reference from another catalogue", "SELECT count(*) FROM st.object_ref o WHERE o.state='resolved' AND o.catalog_instance!=(SELECT value FROM main.meta WHERE key='instance_id')"),
        ("more than one candidate model", "SELECT count(*)>1 FROM st.student_model WHERE status='candidate'"),
    ] {
        match one(&db, sql) {
            Ok(0) => {}
            Ok(n) => bad.push(format!("{what}: {n}")),
            Err(e) => bad.push(e),
        }
    }
    bad
}

/// Row counts that a second recovery of an already-recovered store must not change.
fn snapshot(store: &Path) -> Res<Vec<i64>> {
    let db = reader(store);
    [
        "main.file",
        "main.dir",
        "st.collection",
        "st.collection_member",
        "st.object_ref",
        "st.human_feedback",
        "st.learning_receipt",
        "st.learning_node",
        "st.student_model",
        "st.backup_evidence",
    ]
    .iter()
    .map(|t| one(&db, &format!("SELECT count(*) FROM {t}")))
    // Not `catalog_rev`: every catalogue open publishes a new one by design (`db.rs` recovery).
    .chain([one(&db, "SELECT state_rev FROM st.revision")])
    .collect()
}

fn class(scenario: &str) -> DatasetClass {
    if scenario == "backup" {
        DatasetClass::Personal
    } else {
        DatasetClass::Synthetic
    }
}

/// Recovery invariants, durability of every acknowledged step, and a write after recovery.
fn verify(work: &Path, acks: &[String], scenario: &str) -> Vec<String> {
    let store = store(work);
    let reopen = || {
        std::panic::catch_unwind(|| drop(open(&store, class(scenario))))
            .map_err(|_| "the service did not reopen after the crash".to_owned())
    };
    if let Err(e) = reopen() {
        return vec![e];
    }
    let mut bad = structural(&store);
    let first = snapshot(&store);
    if let Err(e) = reopen() {
        bad.push(format!("second reopen: {e}"));
    }
    let second = snapshot(&store);
    if first != second {
        bad.push(format!(
            "second recovery changed the store: {first:?} then {second:?}"
        ));
    }
    let acked = |step: &str| acks.iter().any(|a| a == step);
    let db = reader(&store);
    let mut want = |ok: Res<bool>, what: &str| match ok {
        Ok(true) => {}
        Ok(false) => bad.push(format!("acknowledged but not durable: {what}")),
        Err(e) => bad.push(format!("{what}: {e}")),
    };
    let members = format!("SELECT count(*) FROM st.collection_member m JOIN st.collection c ON c.id=m.collection_id WHERE c.name='{COLLECTION}'");
    if scenario == "meaning" {
        if acked("create") {
            want(
                one(
                    &db,
                    &format!("SELECT count(*) FROM st.collection WHERE name='{COLLECTION}'"),
                )
                .map(|n| n == 1),
                "create: the collection",
            );
        }
        // A removal that began may or may not have landed; one that was acknowledged has.
        let n = one(&db, &members);
        if acked("remove") {
            want(n.clone().map(|n| n == 1), "remove: one member left");
        } else if acked("begin remove") {
            want(n.clone().map(|n| n == 1 || n == 2), "add: both members");
        } else if acked("add") {
            want(n.clone().map(|n| n == 2), "add: both members");
        } else if acked("begin add") {
            want(
                n.clone().map(|n| n == 0 || n == 2),
                "begin add: both members or none",
            );
        }
        if acked("label") {
            want(one(&db, &format!("SELECT count(*) FROM st.human_feedback h JOIN st.learning_receipt r ON r.client_event_id=h.client_event_id WHERE h.client_event_id='{LABEL_KEY}' AND h.label='Documents'")).map(|n| n == 1), "label: the label and its replay receipt");
        }
        if acked("refit") {
            want(
                one(
                    &db,
                    "SELECT count(*) FROM st.student_model WHERE status='candidate'",
                )
                .map(|n| n == 1),
                "refit: the candidate model",
            );
        }
    } else {
        let rows = one(
            &db,
            &format!("SELECT count(*) FROM st.backup_evidence WHERE repository_label='{REPO}'"),
        );
        if acked("forget") {
            want(rows.map(|n| n == 0), "forget: the evidence is gone");
        } else if acked("begin forget") {
            want(rows.map(|n| n <= 1), "one record per label");
        } else if acked("import") {
            want(rows.map(|n| n == 1), "import: the evidence");
        } else {
            want(rows.map(|n| n <= 1), "one record per label");
        }
    }
    drop(db);
    // The recovered store still takes each writer's write. (A label cannot be retried under its
    // old key: node IDs are sealed per session, so the request's content differs.)
    let probe = std::panic::catch_unwind(|| {
        let svc = open(&store, class(scenario));
        if scenario == "meaning" {
            let ids = nodes(&svc);
            let (created, _) = ok(
                "collections.create",
                call(&svc, "collections.create", json!({"name":"Crash probe"})),
            );
            let co = created["collection_id"].as_str().unwrap().to_owned();
            ok("collections.update_members", update(&svc, &co, &ids, &[]));
            ok(
                "feedback.record",
                call(
                    &svc,
                    "feedback.record",
                    feedback(&ids[0], "ce_probe", Some("Archive"), false),
                ),
            );
            finish_refit(&svc);
        } else {
            let (r, _) = ok("backups.import", import(&svc, work));
            assert_eq!(r["outcome"], "imported", "{r:?}");
        }
    });
    if probe.is_err() {
        bad.push("the write probe after recovery failed".into());
    }
    bad.extend(
        structural(&store)
            .into_iter()
            .map(|b| format!("after the write probe: {b}")),
    );
    bad
}

fn sweep_all() -> bool {
    std::env::var("LOOMWARD_CRASH_SWEEP").is_ok_and(|v| v == "full")
}

/// Copies a prepared work directory (store and receipt) for one child.
fn copy_work(from: &Path, to: &Path) {
    std::fs::create_dir_all(store(to)).unwrap();
    for e in std::fs::read_dir(store(from)).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), store(to).join(e.file_name())).unwrap();
        }
    }
    let receipt = from.join("snapshots.json");
    if receipt.exists() {
        std::fs::copy(receipt, to.join("snapshots.json")).unwrap();
    }
}

fn sweep(scenario: &str, stride: usize) {
    let started = Instant::now();
    let root = temp_root();
    let seed = root.path().join("seed");
    prepare(scenario, &seed);
    let clean = root.path().join("clean");
    copy_work(&seed, &clean);
    let base = run_child(scenario, &clean, None);
    assert_eq!(
        base.acks.last().map(String::as_str),
        Some("closed"),
        "{scenario}: clean run"
    );
    let bad = verify(&clean, &base.acks, scenario);
    assert!(
        bad.is_empty(),
        "{scenario}: the clean run violates {bad:#?}"
    );
    let mut rows = 0;
    let todo: Vec<u64> = base
        .trace
        .iter()
        .filter(|(_, label)| {
            if sweep_all() || !label.starts_with("row ") {
                return true;
            }
            rows += 1;
            rows % stride == 0
        })
        .map(|(n, _)| *n)
        .collect();
    let killed = todo.len();
    let failures = Mutex::new(Vec::<String>::new());
    let queue = Mutex::new(todo);
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                while let Some(n) = {
                    let next = queue.lock().unwrap().pop();
                    next
                } {
                    let work = root.path().join(format!("n{n}"));
                    copy_work(&seed, &work);
                    let out = run_child(scenario, &work, Some(n));
                    let bad = verify(&work, &out.acks, scenario);
                    if !bad.is_empty() {
                        failures.lock().unwrap().push(format!(
                            "N={n} ({}; last ack {:?}): {bad:#?}",
                            base.trace.get(&n).cloned().unwrap_or_default(),
                            out.acks.last()
                        ));
                    }
                    let _ = std::fs::remove_dir_all(&work);
                }
            });
        }
    });
    eprintln!(
        "service crash sweep {scenario}: {} boundaries, killed at {killed}, {:.1} s",
        base.boundaries,
        started.elapsed().as_secs_f64()
    );
    let mut failures = failures.into_inner().unwrap();
    failures.sort();
    assert!(
        failures.is_empty(),
        "{scenario}: {} of {} crash points break recovery\n{}",
        failures.len(),
        base.boundaries,
        failures.join("\n")
    );
}

#[test]
fn every_write_of_collections_labels_and_refit_recovers() {
    sweep("meaning", 8);
}

#[test]
fn every_write_of_backup_evidence_recovers() {
    sweep("backup", 4);
}

/// The checker must be able to fail: damage a recovered store each way and expect a violation.
#[test]
fn the_service_verifier_catches_planted_damage() {
    let root = temp_root();
    for (scenario, what, sql, expected) in [
        ("meaning", "an orphan membership", "PRAGMA foreign_keys=OFF; INSERT INTO st.collection_member VALUES((SELECT id FROM st.collection WHERE name='Crash keep'),999999,0)", "member without its reference"),
        ("meaning", "a lost acknowledged collection", "DELETE FROM st.collection WHERE name='Crash keep'", "create: the collection"),
        ("meaning", "a lost replay receipt", "DELETE FROM st.learning_receipt", "label without its replay receipt"),
        ("meaning", "a reference bound across catalogues", "UPDATE st.object_ref SET born_run=born_run+1000", "resolved reference bound to a missing catalogue row"),
        ("backup", "a resurrected forgotten record", "INSERT INTO st.backup_evidence(repository_label,evidence_json,snapshot_observed_ns,imported_at_ns) VALUES('synthetic-crash-nas','{}',0,0)", "forget: the evidence is gone"),
    ] {
        let work = root.path().join(format!("{scenario}-{}", what.len()));
        prepare(scenario, &work);
        let out = run_child(scenario, &work, None);
        reader(&store(&work)).execute_batch(sql).unwrap();
        let violations = verify(&work, &out.acks, scenario);
        eprintln!("planted {what}: {violations:?}");
        assert!(
            violations.iter().any(|v| v.contains(expected)),
            "undetected {what}: {violations:?}"
        );
    }
}

/// An interrupted two-member add may publish neither member or both, never a subset.
#[test]
fn an_interrupted_collection_add_cannot_publish_one_member() {
    let root = temp_root();
    for count in 0..=2 {
        let work = root.path().join(format!("members-{count}"));
        prepare("meaning", &work);
        let svc = open(&store(&work), DatasetClass::Synthetic);
        let ids = nodes(&svc);
        let (created, _) = ok(
            "collections.create",
            call(&svc, "collections.create", json!({"name":COLLECTION})),
        );
        let co = created["collection_id"].as_str().unwrap().to_owned();
        if count > 0 {
            // One member models a mutant that commits the requested inserts separately.
            ok(
                "collections.update_members",
                update(&svc, &co, &ids[..count], &[]),
            );
        }
        drop(svc);
        let acks = ["open", "begin create", "create", "begin add"].map(str::to_owned);
        let violations = verify(&work, &acks, "meaning");
        if count == 1 {
            assert!(
                violations
                    .iter()
                    .any(|v| v.contains("begin add: both members or none")),
                "an interrupted add published one member: {violations:?}"
            );
        } else {
            assert!(
                violations.is_empty(),
                "an atomic interrupted add with {count} members was refused: {violations:?}"
            );
        }
    }
}

/// A crash point past the last boundary is never reached, and must fail instead of passing.
#[test]
#[should_panic(expected = "the crash point was not reached")]
fn an_unreached_service_crash_point_fails_the_sweep() {
    let root = temp_root();
    let seed = root.path().join("seed");
    prepare("backup", &seed);
    let clean = root.path().join("clean");
    copy_work(&seed, &clean);
    let base = run_child("backup", &clean, None);
    let past = root.path().join("past");
    copy_work(&seed, &past);
    run_child("backup", &past, Some(base.boundaries + 1));
}
