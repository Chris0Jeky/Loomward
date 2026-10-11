//! Service tests: ingress, deadlines, N4 idempotency, cursors, node IDs, provenance, revocation
//! ordering, and a complete-envelope check of every command's answer.

use crate::{Config, Service};
use loomward_catalog::{DirListing, Observation, RootObservation, WriteCommand, WriteReply};
use loomward_engine::scan::{ListingTicket, RunScope, ScanMessage, ScanSink};
use loomward_engine::EngineResult;
use loomward_protocol::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Test seams inside the service: a delay before one command runs, a run counter, the order
/// commands finished their delay in, a one-shot forced mutation-thread spawn failure, an injected
/// profile resolver for the scan-time grant fence, and named
/// gates a handler blocks at while the test holds them closed (`"locked"`: `volumes.declare_tier`
/// holding `Inner::mutation`; `"after"`: any command after its handler returned, locks released).
#[derive(Default)]
pub(crate) struct Hooks {
    pub delay: Mutex<Option<(Command, Duration)>>,
    pub runs: Mutex<HashMap<Command, usize>>,
    pub order: Mutex<Vec<Command>>,
    pub fail_spawn: std::sync::atomic::AtomicBool,
    pub closed: Mutex<Vec<(Command, &'static str)>>,
    pub arrived: Mutex<Vec<(Command, &'static str)>>,
    pub opened: std::sync::Condvar,
    /// (profile, profiles directory) for the scan-time fence instead of the OS folders (#312).
    pub profile: Mutex<Option<(std::path::PathBuf, std::path::PathBuf)>>,
}

impl Hooks {
    pub fn before(&self, c: Command) {
        *self.runs.lock().unwrap().entry(c).or_default() += 1;
        let delay = *self.delay.lock().unwrap();
        if let Some((cmd, d)) = delay {
            if cmd == c {
                std::thread::sleep(d);
            }
        }
        self.order.lock().unwrap().push(c);
    }

    pub fn gate(&self, c: Command, at: &'static str) {
        self.arrived.lock().unwrap().push((c, at));
        let mut closed = self.closed.lock().unwrap();
        while closed.contains(&(c, at)) {
            closed = self.opened.wait(closed).unwrap();
        }
    }

    pub fn open(&self, c: Command, at: &'static str) {
        self.closed.lock().unwrap().retain(|g| *g != (c, at));
        self.opened.notify_all();
    }

    /// Waits (bounded only by a generous safety timeout) until `cond` holds.
    pub fn until(&self, what: &str, cond: impl Fn(&Self) -> bool) {
        let give_up = Instant::now() + Duration::from_secs(60);
        while !cond(self) {
            assert!(Instant::now() < give_up, "never reached: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn open(dir: &Path, dataset: DatasetClass) -> Service {
    Service::open(Config {
        state_dir: dir.to_path_buf(),
        dataset,
        allow_personal: true,
        grant_roots: vec![],
    })
    .unwrap()
}

pub(crate) fn temp_root() -> tempfile::TempDir {
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-roots");
    std::fs::create_dir_all(&target).unwrap();
    tempfile::tempdir_in(target.canonicalize().unwrap()).unwrap()
}

#[test]
fn followup_test_roots_are_outside_source() {
    let tmp = temp_root();
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-roots");
    std::fs::create_dir_all(&target).unwrap();
    assert!(tmp
        .path()
        .canonicalize()
        .unwrap()
        .starts_with(target.canonicalize().unwrap()));
}

#[test]
fn followup_stored_grant_version_errors_fail_closed() {
    let tmp = temp_root();
    let path = tmp.path().join("catalog.db");
    std::fs::write(&path, b"not a sqlite database").unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        crate::stored_grant_catalogue_is_legacy(&conn).unwrap_err(),
        "cannot inspect stored grants: catalogue version unavailable"
    );
    let empty = rusqlite::Connection::open_in_memory().unwrap();
    assert!(!crate::stored_grant_catalogue_is_legacy(&empty).unwrap());
    empty.pragma_update(None, "user_version", 1).unwrap();
    assert!(crate::stored_grant_catalogue_is_legacy(&empty).unwrap());
    let state = tmp.path().join("state");
    drop(loomward_catalog::Catalog::open(&state, "synthetic").unwrap());
    std::fs::write(state.join("catalog.db"), b"not a sqlite database").unwrap();
    let dist = tmp.path().join("dist");
    std::fs::create_dir(&dist).unwrap();
    let error = Service::open_with_static(
        Config {
            state_dir: state,
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            grant_roots: vec![],
        },
        Some(&dist),
    )
    .err()
    .expect("corrupt catalogue must refuse preflight");
    assert_eq!(
        error,
        "cannot inspect stored grants: catalogue version unavailable"
    );
}

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[test]
fn review_empty_state_starts() {
    let tmp = temp_root();
    rusqlite::Connection::open(tmp.path().join("state.db"))
        .unwrap()
        .close()
        .unwrap();
    drop(open(tmp.path(), DatasetClass::Synthetic));
    let db = rusqlite::Connection::open(tmp.path().join("state.db")).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        loomward_catalog::SCHEMA_VERSION
    );
}

fn v1_store(dir: &Path) {
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
        let db = rusqlite::Connection::open(dir.join(name)).unwrap();
        db.execute_batch(schema).unwrap();
        db.pragma_update(None, "application_id", loomward_catalog::APPLICATION_ID)
            .unwrap();
        db.pragma_update(None, "user_version", 1).unwrap();
        db.execute("INSERT INTO meta VALUES('dataset_class','synthetic')", [])
            .unwrap();
    }
}

#[test]
fn review_v1_state_migrates_and_starts() {
    let tmp = temp_root();
    v1_store(tmp.path());
    let db = rusqlite::Connection::open(tmp.path().join("state.db")).unwrap();
    db.execute(
        "INSERT INTO collection VALUES(1,'Fixture collection','human',0)",
        [],
    )
    .unwrap();
    db.close().unwrap();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    assert_eq!(
        svc.inner
            .db()
            .query_row("SELECT name FROM st.collection WHERE id=1", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "Fixture collection"
    );
    assert!(tmp.path().join("state.before-v1.db").exists());
}

#[test]
fn review_stored_overlap_is_refused_before_store_changes() {
    for legacy in [false, true] {
        let tmp = temp_root();
        let state = tmp.path().join("state");
        std::fs::create_dir(&state).unwrap();
        let dist = tmp.path().join("dist");
        std::fs::create_dir(&dist).unwrap();
        let dist = dist.canonicalize().unwrap();
        if legacy {
            v1_store(&state);
            let db = rusqlite::Connection::open(state.join("catalog.db")).unwrap();
            db.execute(
                "INSERT INTO volume VALUES(1,'fixture','Fixture','{}','{}','{}',1,0)",
                [],
            )
            .unwrap();
            db.execute("INSERT INTO root_grant VALUES(1,1,zeroblob(16),?1,'fixture','fixture','active',0,NULL)", [dist.to_str().unwrap()]).unwrap();
        } else {
            drop(loomward_catalog::Catalog::open(&state, "synthetic").unwrap());
            let db = rusqlite::Connection::open(state.join("state.db")).unwrap();
            db.execute("INSERT INTO root_grant(volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES('fixture',zeroblob(16),?1,'fixture','fixture','active',0)", [dist.to_str().unwrap()]).unwrap();
            db.pragma_update(None, "journal_mode", "DELETE").unwrap();
            let db = rusqlite::Connection::open(state.join("catalog.db")).unwrap();
            db.pragma_update(None, "journal_mode", "DELETE").unwrap();
        }
        let before: std::collections::BTreeMap<_, _> = std::fs::read_dir(&state)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_file())
            .map(|p| (p.clone(), std::fs::read(p).unwrap()))
            .collect();
        let error = Service::open_with_static(
            Config {
                state_dir: state.clone(),
                dataset: DatasetClass::Synthetic,
                allow_personal: false,
                grant_roots: vec![],
            },
            Some(&dist),
        )
        .err()
        .expect("stored overlap must be refused");
        assert!(error.contains("overlaps"), "{error}");
        let after: std::collections::BTreeMap<_, _> = std::fs::read_dir(&state)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_file())
            .map(|p| (p.clone(), std::fs::read(p).unwrap()))
            .collect();
        assert!(
            before == after,
            "refused startup changed the store (legacy={legacy})"
        );
    }
}

#[test]
fn review_lab_digest_refusals_leave_no_files() {
    let tmp = temp_root();
    let state = tmp.path().join("state");
    let root = tmp.path().join("lab");
    for digest in ["sha256:00".to_owned(), format!("sha256:{}", "A".repeat(64))] {
        assert_eq!(
            crate::lab::register(&state, &root, "s", "S", &digest).unwrap_err(),
            "invalid lab manifest digest"
        );
        assert!(!state.exists());
        assert!(!root.exists());
    }
}

#[test]
fn hardening_reads_stay_bounded_after_the_caller_times_out() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    svc.inner
        .hooks
        .closed
        .lock()
        .unwrap()
        .push((Command::HealthGet, "after"));
    for _ in 0..8 {
        let mut r = request("health.get", json!({}));
        r.deadline_ms = Some(Int::new(1).unwrap());
        assert_eq!(
            err(svc.call(r, &CallContext::http())).code,
            ErrorCode::DeadlineExceeded
        );
    }
    let overflow = call(&svc, "health.get", json!({}));
    svc.inner.hooks.open(Command::HealthGet, "after");
    assert_eq!(err(overflow).code, ErrorCode::Busy);
    svc.inner.hooks.until("read permits released", |_| {
        *svc.inner.reads.lock().unwrap() == 0
    });
    ok("health.get", call(&svc, "health.get", json!({})));
}

#[test]
fn hardening_grant_alias_cannot_revoke() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 1);
    let response = call(
        &svc,
        "grants.revoke",
        json!({"grant_id": format!("gr_0{grant}")}),
    );
    assert_eq!(err(response).code, ErrorCode::NotFound);
    assert_eq!(
        crate::db::roots(&svc.inner.db(), Some(root)).unwrap()[0].grant_state,
        "active"
    );
    ok(
        "grants.revoke",
        call(
            &svc,
            "grants.revoke",
            json!({"grant_id": format!("gr_{grant}")}),
        ),
    );
}

#[test]
fn hardening_root_and_volume_ids_cannot_be_forged() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, _) = register(&svc, 1);
    for (prefix, row) in [("rt_", root), ("vo_", 1)] {
        let genuine = if prefix == "rt_" {
            svc.inner.root_id(row)
        } else {
            svc.inner.volume_id(row)
        };
        let forged = genuine.replacen(
            &format!("{prefix}{row}_"),
            &format!("{prefix}{}_", row + 1),
            1,
        );
        let read = |id: &str| {
            if prefix == "rt_" {
                svc.inner.root_row(id)
            } else {
                svc.inner.volume_row(id)
            }
        };
        assert_eq!(read(&genuine).unwrap(), row);
        assert!(read(&forged).is_err());
    }
    let ids = crate::ids::NodeIds::new([42; 32]);
    let (first, second) = ("fixture-instance-53847", "fixture-instance-83288");
    let a = ids.row_id("rt_", 1, first);
    let b = ids.row_id("rt_", 1, second);
    assert_eq!(
        a.split('_').nth(2),
        b.split('_').nth(2),
        "synthetic 32-bit hint collision"
    );
    assert!(ids.open_row_id("rt_", &a, second).is_err());
    assert_ne!(a, b);
}

#[test]
fn hardening_synthetic_telemetry_is_refused_by_the_service() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    for (command, payload) in [
        (
            "telemetry.subscribe",
            json!({"subscription_id": null, "channels": ["system"], "interval_ms": 1000}),
        ),
        (
            "telemetry.unsubscribe",
            json!({"subscription_id": "sb_fixture"}),
        ),
        ("telemetry.snapshot", json!({"channels": ["system"]})),
        ("processes.list", json!({"sort": "name_asc", "limit": 10})),
        ("processes.explain", json!({"process_ref": "pc_fixture"})),
    ] {
        let e = err(call(&svc, command, payload));
        assert_eq!(e.code, ErrorCode::CapabilityUnavailable, "{command}: {e:?}");
        assert_eq!(
            e.message.as_str(),
            "process observation requires a personal session"
        );
    }
}

#[test]
fn hardening_invalid_lab_and_startup_leave_no_files() {
    let tmp = temp_root();
    let state = tmp.path().join("state");
    let root = state.join("refused-lab");
    assert!(crate::lab::register(
        &state,
        &root,
        "s",
        "S",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000"
    )
    .is_err());
    assert!(!state.exists());
    if native_identity_available() {
        let unknown = tmp.path().join("unregistered");
        std::fs::create_dir(&unknown).unwrap();
        assert!(Service::open(Config {
            state_dir: state.clone(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            grant_roots: vec![unknown]
        })
        .is_err());
        assert!(!state.exists());
    }
    assert!(Service::open(Config {
        state_dir: state.clone(),
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        grant_roots: vec![tmp.path().join("missing")],
    })
    .is_err());
    assert!(!state.exists());
    assert!(crate::lab::register(
        Path::new(r"\\server\share\state"),
        &tmp.path().join("lab"),
        "s",
        "S",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000"
    )
    .is_err());
    assert!(!tmp.path().join("lab").exists());
}

#[test]
fn hardening_expired_sql_reads_do_not_start_or_resume() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    fixture(&svc, 20);
    let work = crate::ReadWork {
        deadline: Instant::now() + Duration::from_secs(10),
        interrupt: Mutex::new(None),
    };
    let mut reader = work.reader(&svc.inner.catalog).unwrap();
    reader.set_deadline(Instant::now());
    let req = loomward_catalog::SearchRequest {
        root_id: None,
        text: "report".into(),
        extension: None,
        min_bytes: None,
        kind: "any".into(),
        limit: 10,
        cursor: None,
        work_budget: 50_000_000,
    };
    assert!(matches!(
        reader.search(&req),
        Err(loomward_catalog::Error::Cancelled)
    ));
    let dir: i64 = svc
        .inner
        .db()
        .query_row("SELECT id FROM main.dir WHERE parent_id IS NULL", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(matches!(
        reader.breakdown(
            dir,
            "extension",
            loomward_catalog::Basis::Logical,
            10,
            50_000_000
        ),
        Err(loomward_catalog::Error::Cancelled)
    ));
    let expired = crate::ReadWork {
        deadline: Instant::now(),
        interrupt: Mutex::new(None),
    };
    assert!(
        matches!(expired.reader(&svc.inner.catalog), Err(e) if e.code == ErrorCode::DeadlineExceeded)
    );
    reader.set_deadline(Instant::now() + Duration::from_secs(10));
    assert!(!reader.search(&req).unwrap().items.is_empty());
    assert!(!reader
        .breakdown(
            dir,
            "extension",
            loomward_catalog::Basis::Logical,
            10,
            50_000_000
        )
        .unwrap()
        .buckets
        .is_empty());
}

#[test]
fn hardening_picker_cooldown_survives_cancellation_and_new_ids() {
    struct Dialog(std::sync::atomic::AtomicUsize);
    impl NativeDialogs for Dialog {
        fn pick_folder(&self) -> Option<std::path::PathBuf> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            None
        }
        fn confirm_disclosure(&self, _: &DisclosureSummary) -> bool {
            false
        }
    }
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Personal);
    let dialog = Arc::new(Dialog(std::sync::atomic::AtomicUsize::new(0)));
    let ctx = CallContext::tauri(dialog.clone());
    let first = request("roots.request_grant", json!({"purpose": "metadata_scan"}));
    ok("roots.request_grant", svc.call(first.clone(), &ctx));
    ok("roots.request_grant", svc.call(first, &ctx));
    assert_eq!(
        err(svc.call(
            request("roots.request_grant", json!({"purpose": "metadata_scan"})),
            &ctx
        ))
        .code,
        ErrorCode::Busy
    );
    assert_eq!(dialog.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    *svc.inner.picker_last.lock().unwrap() = Some(Instant::now() - Duration::from_secs(3));
    ok(
        "roots.request_grant",
        svc.call(
            request("roots.request_grant", json!({"purpose": "metadata_scan"})),
            &ctx,
        ),
    );
    assert_eq!(dialog.0.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn hardening_grant_checks_the_registered_digest_before_writes() {
    if !native_identity_available() {
        return;
    }
    let tmp = temp_root();
    let state = tmp.path().join("state");
    let root = tmp.path().join("lab");
    crate::lab::register(
        &state,
        &root,
        "s",
        "S",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    let svc = open(&state, DatasetClass::Synthetic);
    svc.inner
        .db()
        .execute("UPDATE st.lab_root SET manifest_digest='invalid'", [])
        .unwrap();
    assert!(svc.inner.grant(&root, "cli_flag").is_err());
    assert_eq!(crate::db::active_grants(&svc.inner.db()).unwrap(), 0);
    svc.inner
        .db()
        .execute("UPDATE st.lab_root SET manifest_digest='sha256:0000000000000000000000000000000000000000000000000000000000000000'", [])
        .unwrap();
    assert!(svc.inner.grant(&root, "cli_flag").is_ok());
}

fn request(command: &str, payload: Value) -> RequestEnvelope {
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    serde_json::from_value(json!({
        "protocol": "loomward/3", "request_id": format!("r{n}"), "command": command, "payload": payload
    }))
    .unwrap()
}

fn call(svc: &Service, command: &str, payload: Value) -> ResponseEnvelope {
    svc.call(request(command, payload), &CallContext::http())
}

#[test]
fn job_counters_round_trip_through_service_handlers() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (ready, started) = std::sync::mpsc::channel();
    svc.inner.engine.register_job_runner(
        JobKind::Scan,
        Arc::new(move |ctx, _| {
            ready.send(()).unwrap();
            while !ctx.is_cancelled() {
                std::thread::yield_now();
            }
            Ok(CoverageState::Cancelled)
        }),
    );
    let submitted = svc
        .inner
        .engine
        .job_submit(loomward_engine::jobs::JobSpec::new(
            JobKind::Scan,
            Some(RootId::new("rt_fixture").unwrap()),
        ))
        .unwrap();
    assert_eq!(submitted.seq.get(), 1);
    assert_eq!(submitted.rev.get(), 0);
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    let (get, _) = ok(
        "jobs.get",
        call(&svc, "jobs.get", json!({"job_id": submitted.job_id})),
    );
    let observed: JobResult = serde_json::from_value(json!(get)).unwrap();
    assert_eq!(observed.job.seq, submitted.seq);
    assert_eq!(observed.job.rev.get(), 1);
    let (list, _) = ok("jobs.list", call(&svc, "jobs.list", json!({"limit": 50})));
    let listed: JobList = serde_json::from_value(json!(list)).unwrap();
    assert_eq!(listed.jobs[0].seq, submitted.seq);
    assert_eq!(listed.jobs[0].rev, observed.job.rev);
    let (cancel, _) = ok(
        "scan.cancel",
        call(&svc, "scan.cancel", json!({"job_id": submitted.job_id})),
    );
    let cancelled: JobResult = serde_json::from_value(json!(cancel)).unwrap();
    assert_eq!(cancelled.job.seq, submitted.seq);
    assert_eq!(cancelled.job.rev.get(), 2);
}

/// Complete-envelope check (semantics section 10): the response round-trips through the wire
/// type, and an ok result decodes exactly as the command's result type.
fn checked(command: &str, r: &ResponseEnvelope) {
    let wire = serde_json::to_value(r).unwrap();
    let back: ResponseEnvelope = serde_json::from_value(wire).unwrap();
    assert_eq!(&back, r);
    if let ResponseEnvelope::Ok(ok) = r {
        ok.validate_for(Command::from_name(command).unwrap())
            .unwrap_or_else(|e| panic!("{command}: {e:?}"));
    }
}

fn ok(command: &str, r: ResponseEnvelope) -> (Payload, ResponseMeta) {
    checked(command, &r);
    match r {
        ResponseEnvelope::Ok(o) => (o.result, o.meta),
        ResponseEnvelope::Err(e) => panic!("{command}: {:?}", e.error),
    }
}

fn err(r: ResponseEnvelope) -> ErrorBody {
    match r {
        ResponseEnvelope::Err(e) => e.error,
        ResponseEnvelope::Ok(o) => panic!("expected an error, got {:?}", o.result),
    }
}

fn reason(e: &ErrorBody) -> Option<&str> {
    e.detail.as_ref()?.get("reason")?.as_str()
}

fn file(name: &str, bytes: u64) -> Observation {
    let mut o = Observation::file(name, bytes, Some(bytes));
    o.extension = name.rsplit_once('.').map(|(_, e)| e.into());
    o.family = "document".into();
    o
}

/// A fixture root (origin `fixture`, no path) with one listing of `files` files and a subdir.
fn fixture(svc: &Service, files: u64) -> (i64, i64) {
    let w = svc.inner.catalog.writer();
    let WriteReply::Root {
        grant_id,
        root_id,
        dir_id,
    } = w
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_key: "fixture".into(),
            display_name: "Synthetic".into(),
            display_path: "Synthetic root".into(),
            root_file_id: Some(vec![1; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))
        .unwrap()
    else {
        panic!()
    };
    relist(svc, grant_id, dir_id, files);
    (root_id, grant_id)
}

/// One complete run over the fixture root: `files` files and one empty subdirectory.
fn relist(svc: &Service, grant: i64, dir: i64, files: u64) {
    let w = svc.inner.catalog.writer();
    let WriteReply::Run(run) = w
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "full".into(),
            strategy: "fixture".into(),
            started_at_ns: 0,
        })
        .unwrap()
    else {
        panic!()
    };
    let mut sub = Observation::file("sub", 0, Some(0));
    sub.family = "none".into();
    w.call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: dir,
        files: (0..files)
            .map(|i| file(&format!("report-{i:03}.txt"), 1000 + i))
            .collect(),
        dirs: vec![sub],
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }))
    .unwrap();
    let sub_id: i64 = svc
        .inner
        .db()
        .query_row("SELECT id FROM main.dir WHERE parent_id=?1", [dir], |r| {
            r.get(0)
        })
        .unwrap();
    w.call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: sub_id,
        files: vec![],
        dirs: vec![],
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }))
    .unwrap();
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 1,
    })
    .unwrap();
}

fn root_slice(svc: &Service, root: i64) -> Payload {
    ok(
        "tree.slice",
        call(svc, "tree.slice", json!({"anchor": {"kind": "root", "root_id": svc.inner.root_id(root)}, "depth": 2, "max_nodes": 64, "min_share": 0.0, "basis": "logical", "include_files": true})),
    )
    .0
}

#[test]
fn every_command_answers_a_complete_envelope_and_core_reads_serve_the_catalogue() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, _) = fixture(&svc, 5);
    let examples =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/v3/examples/commands");
    for &command in Command::ALL {
        let text =
            std::fs::read_to_string(examples.join(format!("{}.request.json", command.as_str())))
                .unwrap();
        let r = call(&svc, command.as_str(), serde_json::from_str(&text).unwrap());
        checked(command.as_str(), &r);
        if command.wave() >= 3 || command == Command::RootsRequestGrant {
            assert_eq!(
                err(r).code,
                ErrorCode::CapabilityUnavailable,
                "{}",
                command.as_str()
            );
        }
    }
    // Reads against the real catalogue, with sealed node IDs.
    let slice = root_slice(&svc, root);
    let nodes = slice["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 7, "root, five files, one subdir");
    assert!(nodes
        .iter()
        .all(|n| n["node_id"].as_str().unwrap().len() == 53));
    let anchor = slice["anchor_node_id"].as_str().unwrap().to_owned();
    for (command, payload) in [
        ("tree.path", json!({"node_id": anchor})),
        ("node.inspect", json!({"node_id": anchor})),
        (
            "stats.breakdown",
            json!({"node_id": anchor, "by": "ext_family", "basis": "logical", "limit": 8}),
        ),
        (
            "tree.children",
            json!({"node_id": anchor, "sort": "size_desc", "basis": "logical", "limit": 10, "cursor": null}),
        ),
        (
            "search.query",
            json!({"root_id": svc.inner.root_id(root), "text": "report", "extension": null, "min_bytes": null, "kind": "file", "limit": 10, "cursor": null}),
        ),
        (
            "tree.slice",
            json!({"anchor": {"kind": "atlas"}, "depth": 3, "max_nodes": 64, "min_share": 0.0, "basis": "allocated", "include_files": false}),
        ),
        ("roots.list", json!({})),
        ("grants.list", json!({})),
        ("volumes.list", json!({})),
        ("health.get", json!({})),
        ("session.hello", json!({})),
        ("jobs.list", json!({"limit": 5})),
        ("budgets.get", json!({})),
    ] {
        let (result, meta) = ok(command, call(&svc, command, payload));
        assert_eq!(meta.dataset_class, DatasetClass::Synthetic);
        if command == "search.query" {
            assert_eq!(result["items"].as_array().unwrap().len(), 5);
        }
        if command == "roots.list" {
            let r = &result["roots"][0];
            assert_eq!(r["origin"], "fixture");
            assert_eq!(r["totals"]["files"], 5);
        }
    }
    // Every effect capability is false and the honest unknowns stay null.
    let (hello, _) = ok("session.hello", call(&svc, "session.hello", json!({})));
    assert!(hello["capabilities"]["effects"]
        .as_object()
        .unwrap()
        .values()
        .all(|v| v == false));
    // Synthetic sessions scan registered lab roots on Windows; personal sessions their own owner grants.
    assert_eq!(
        hello["capabilities"]["observation"]["metadata_scan"],
        cfg!(windows)
    );
    // Atlas and volume slice nodes are anchors too; their sealed IDs open again.
    let slice = |anchor: Value| {
        call(
            &svc,
            "tree.slice",
            json!({"anchor": anchor, "depth": 2, "max_nodes": 64, "min_share": 0.0, "basis": "logical", "include_files": false}),
        )
    };
    let (atlas, _) = ok("tree.slice", slice(json!({"kind": "atlas"})));
    let atlas_id = atlas["anchor_node_id"].as_str().unwrap().to_owned();
    let volume_id = atlas["nodes"][1]["node_id"].as_str().unwrap().to_owned();
    assert_eq!(atlas["nodes"][1]["kind"], "volume");
    ok(
        "tree.slice",
        slice(json!({"kind": "node", "node_id": atlas_id})),
    );
    let (by_volume, _) = ok(
        "tree.slice",
        slice(json!({"kind": "node", "node_id": volume_id})),
    );
    assert_eq!(by_volume["nodes"][1]["kind"], "root");
    let (health, _) = ok("health.get", call(&svc, "health.get", json!({})));
    assert_eq!(health["engine"]["working_set_bytes"], Value::Null);
    if cfg!(windows) {
        // #163: read from the process now, not a retained telemetry sample.
        assert!(health["engine"]["private_commit_bytes"].is_string());
    }
}

#[test]
fn ingress_refuses_before_any_command_runs() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    // Not on this adapter, then wave 3: neither runs.
    let e = err(call(
        &svc,
        "roots.request_grant",
        json!({"purpose": "metadata_scan"}),
    ));
    assert_eq!(e.code, ErrorCode::CapabilityUnavailable);
    assert_eq!(
        err(call(&svc, "taxonomy.get", json!({}))).code,
        ErrorCode::CapabilityUnavailable
    );
    assert_eq!(
        err(call(&svc, "no.such_command", json!({}))).code,
        ErrorCode::UnknownCommand
    );
    assert_eq!(
        err(call(&svc, "roots.list", json!({"extra": 1}))).code,
        ErrorCode::InvalidRequest
    );
    let mut r = request("roots.list", json!({}));
    r.expected_state_rev = Some(Generation(1));
    assert_eq!(
        err(svc.call(r, &CallContext::http())).code,
        ErrorCode::InvalidRequest
    );
    let runs = svc.inner.hooks.runs.lock().unwrap();
    assert!(runs.is_empty(), "no handler ran: {runs:?}");
}

#[test]
fn deadlines_bound_the_wait_and_set_retry_flags() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    *svc.inner.hooks.delay.lock().unwrap() = Some((Command::RootsList, Duration::from_millis(400)));
    let mut r = request("roots.list", json!({}));
    r.deadline_ms = Some(Int::new(50).unwrap());
    let t = Instant::now();
    let e = err(svc.call(r, &CallContext::http()));
    assert!(t.elapsed() < Duration::from_millis(300));
    assert_eq!(
        (e.code, e.retryable),
        (ErrorCode::DeadlineExceeded, true),
        "a read may be retried"
    );
    // A mutation past its deadline: outcome unknown, never blindly retryable.
    *svc.inner.hooks.delay.lock().unwrap() =
        Some((Command::BudgetsSet, Duration::from_millis(400)));
    let mut m = request("budgets.set", json!({"pool": "learning", "max_workers": 1}));
    m.deadline_ms = Some(Int::new(50).unwrap());
    let e = err(svc.call(m, &CallContext::http()));
    assert_eq!((e.code, e.retryable), (ErrorCode::DeadlineExceeded, false));
    // internal_error is retryable at most once, never for teacher.run (#144).
    let body = |c| crate::retry_flag(c, crate::fail(ErrorCode::InternalError, "x")).retryable;
    assert!(body(Command::TreeSlice) && !body(Command::TeacherRun));
}

fn declare(
    svc: &Service,
    id: &str,
    volume: &str,
    tier: u8,
    expected: Option<u64>,
    deadline: Option<i64>,
) -> ResponseEnvelope {
    let mut r: RequestEnvelope = serde_json::from_value(json!({
        "protocol": "loomward/3", "request_id": id, "command": "volumes.declare_tier",
        "payload": {"volume_id": volume, "tier": tier}
    }))
    .unwrap();
    r.expected_state_rev = expected.map(Generation);
    r.deadline_ms = deadline.map(|d| Int::new(d).unwrap());
    svc.call(r, &CallContext::http())
}

/// Errata #117 N4: tier=2 times out, the owner then sets tier=5; recovery must never resend 2.
#[test]
fn n4_recovery_replays_the_retained_outcome_and_never_overwrites_a_later_choice() {
    let tmp = temp_root();
    let svc = Service::open_with(
        Config {
            state_dir: tmp.path().into(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            grant_roots: vec![],
        },
        Duration::from_millis(1500),
        true,
    )
    .unwrap();
    fixture(&svc, 1);
    let (volumes, read) = ok("volumes.list", call(&svc, "volumes.list", json!({})));
    let volume = volumes["volumes"][0]["volume_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let s0 = read.state_rev.unwrap().get();

    *svc.inner.hooks.delay.lock().unwrap() =
        Some((Command::VolumesDeclareTier, Duration::from_millis(300)));
    let e = err(declare(&svc, "act-2", &volume, 2, None, Some(20)));
    assert_eq!((e.code, e.retryable), (ErrorCode::DeadlineExceeded, false));
    *svc.inner.hooks.delay.lock().unwrap() = None;
    // The owner chooses 5 while tier=2's outcome is unknown (it commits first: mutations serialise).
    let (_, five) = ok(
        "volumes.declare_tier",
        declare(&svc, "act-5", &volume, 5, None, None),
    );

    // Step 1: a byte-identical retry replays the original outcome; nothing executes again.
    let (two, _) = ok(
        "volumes.declare_tier",
        declare(&svc, "act-2", &volume, 2, None, None),
    );
    assert_eq!(
        two["volume"]["tier"]["declared_tier"], 2,
        "the retained outcome of the original call"
    );
    let runs = svc.inner.hooks.runs.lock().unwrap()[&Command::VolumesDeclareTier];
    assert_eq!(runs, 2, "tier=2 and tier=5 each ran exactly once");
    let key = svc
        .inner
        .db()
        .query_row("SELECT volume_key FROM main.volume", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap();
    assert_eq!(
        crate::db::declarations(&svc.inner.db(), &key).unwrap(),
        vec![Some(2), Some(5)]
    );
    // Reusing the action key for different content is a conflict, not a new action.
    let e = err(declare(&svc, "act-2", &volume, 3, None, None));
    assert_eq!(
        (e.code, reason(&e)),
        (ErrorCode::InvalidRequest, Some("idempotency_conflict"))
    );

    // Steps 2 and 3, after the key expires: the old intent replays only under the revision of
    // the read it was based on, which the owner's later choice has moved. Nothing is written.
    std::thread::sleep(Duration::from_millis(1600));
    let e = err(declare(&svc, "act-2", &volume, 2, Some(s0), None));
    assert_eq!(
        (e.code, reason(&e), e.retryable),
        (ErrorCode::StaleGeneration, Some("state_rev_changed"), true)
    );
    let (now, current) = ok("volumes.list", call(&svc, "volumes.list", json!({})));
    assert_eq!(
        now["volumes"][0]["tier"]["declared_tier"], 5,
        "the owner's later choice stands"
    );
    assert_eq!(current.state_rev, five.state_rev);
    assert_eq!(
        crate::db::declarations(&svc.inner.db(), &key)
            .unwrap()
            .len(),
        2
    );
    // With the current revision (the owner re-confirmed against what they saw) it applies.
    ok(
        "volumes.declare_tier",
        declare(
            &svc,
            "act-2b",
            &volume,
            2,
            current.state_rev.map(|g| g.get()),
            None,
        ),
    );
}

#[test]
fn cursors_go_stale_when_their_revision_moves_and_never_cross_requests() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = fixture(&svc, 6);
    let anchor = root_slice(&svc, root)["anchor_node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let children = |cursor: Value, sort: &str| {
        call(
            &svc,
            "tree.children",
            json!({"node_id": anchor, "sort": sort, "basis": "logical", "limit": 2, "cursor": cursor}),
        )
    };
    let (first, _) = ok("tree.children", children(Value::Null, "size_desc"));
    let next = first["next_cursor"].clone();
    let (second, _) = ok("tree.children", children(next.clone(), "size_desc"));
    assert_ne!(first["items"][0]["node_id"], second["items"][0]["node_id"]);
    // Another sort with the same handle is refused, not silently re-interpreted.
    assert_eq!(
        err(children(next.clone(), "name_asc")).code,
        ErrorCode::InvalidRequest
    );
    // A forged or expired handle restarts paging.
    let e = err(children(json!("cu0123"), "size_desc"));
    assert_eq!(
        (e.code, reason(&e)),
        (ErrorCode::StaleGeneration, Some("cursor_expired"))
    );
    // Search pages are bound to catalog_rev.
    let search = |cursor: Value| {
        call(
            &svc,
            "search.query",
            json!({"root_id": null, "text": "report", "extension": null, "min_bytes": null, "kind": "any", "limit": 2, "cursor": cursor}),
        )
    };
    let (s1, _) = ok("search.query", search(Value::Null));
    let s_next = s1["next_cursor"].clone();
    assert!(s_next.is_string());
    // A new committed listing moves the subtree and catalogue revisions.
    let dir = svc
        .inner
        .db()
        .query_row(
            "SELECT id FROM main.dir WHERE root_id=?1 AND parent_id IS NULL",
            [root],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    relist(&svc, grant, dir, 7);
    assert_eq!(
        err(children(next, "size_desc")).code,
        ErrorCode::StaleGeneration
    );
    assert_eq!(err(search(s_next)).code, ErrorCode::StaleGeneration);
}

#[test]
fn node_ids_are_sealed_and_tampering_is_not_found() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, _) = fixture(&svc, 2);
    let slice = root_slice(&svc, root);
    let id = slice["nodes"][1]["node_id"].as_str().unwrap().to_owned();
    assert!(
        !id.contains("file") && !id.contains("dir"),
        "no internal row reference crosses the wire"
    );
    ok(
        "node.inspect",
        call(&svc, "node.inspect", json!({"node_id": id})),
    );
    let mut chars: Vec<char> = id.chars().collect();
    let i = chars.len() - 5;
    chars[i] = if chars[i] == 'A' { 'B' } else { 'A' };
    let forged: String = chars.into_iter().collect();
    for bad in [forged.as_str(), "nd_file_1", "nd_dir_1", "nd_0002"] {
        let e = err(call(&svc, "node.inspect", json!({"node_id": bad})));
        assert_eq!(
            (e.code, e.detail.clone()),
            (ErrorCode::NotFound, None),
            "{bad}"
        );
    }
    // Another install's key opens nothing here.
    let other_tmp = temp_root();
    let other = open(other_tmp.path(), DatasetClass::Synthetic);
    let (other_root, _) = fixture(&other, 2);
    let foreign = root_slice(&other, other_root)["nodes"][1]["node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        err(call(&svc, "node.inspect", json!({"node_id": foreign}))).code,
        ErrorCode::NotFound
    );
    // expected_generation guards a root-anchored read.
    let mut r = request("tree.path", json!({"node_id": id}));
    r.expected_generation = Some(Generation(999_999));
    assert_eq!(
        err(svc.call(r, &CallContext::http())).code,
        ErrorCode::StaleGeneration
    );
}

#[test]
fn provenance_synthetic_accepts_only_identity_verified_lab_roots() {
    let tmp = temp_root();
    let state = tmp.path().join("state");
    let owner = tmp.path().join("owner-folder");
    std::fs::create_dir_all(&owner).unwrap();
    let cfg = |roots: Vec<std::path::PathBuf>, dataset, allow| Config {
        state_dir: state.clone(),
        dataset,
        allow_personal: allow,
        grant_roots: roots,
    };
    let refused = Service::open(cfg(
        vec![owner.canonicalize().unwrap()],
        DatasetClass::Synthetic,
        false,
    ))
    .err()
    .unwrap();
    assert!(
        !refused.contains("owner-folder"),
        "refusals never name the path"
    );
    if !native_identity_available() {
        // Unsupported platforms and elevated tokens fail closed before provenance.
        assert!(refused.contains("identity_unavailable"), "{refused}");
        return;
    }
    assert!(
        refused.contains("synthetic_session_requires_lab_root"),
        "{refused}"
    );
    // A registered lab root is accepted, and listed as lab_generated.
    let lab = tmp.path().join("lab-s1");
    crate::lab::register(
        &state,
        &lab,
        "seed-1",
        "S",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    let lab = lab.canonicalize().unwrap();
    let svc = Service::open(cfg(vec![lab.clone()], DatasetClass::Synthetic, false)).unwrap();
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    assert_eq!(roots["roots"][0]["origin"], "lab_generated");
    assert_eq!(roots["roots"][0]["granted_via"], "cli_flag");
    drop(svc);
    // Re-opening with the same root re-uses its grant.
    let svc = Service::open(cfg(vec![lab.clone()], DatasetClass::Synthetic, false)).unwrap();
    assert_eq!(
        ok("roots.list", call(&svc, "roots.list", json!({}))).0["roots"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    drop(svc);
    // Identity, not the path, is what was registered: a directory recreated at the same path is refused.
    std::fs::remove_dir(&lab).unwrap();
    std::fs::create_dir(&lab).unwrap();
    let e = Service::open(cfg(vec![lab.clone()], DatasetClass::Synthetic, false))
        .err()
        .unwrap();
    assert!(e.contains("synthetic_session_requires_lab_root"), "{e}");
    // Registration refuses a non-empty directory.
    std::fs::write(owner.join("x.txt"), b"x").unwrap();
    assert!(crate::lab::register(
        &state,
        &owner,
        "s",
        "S",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000"
    )
    .is_err());
    // A personal session needs the explicit flag and serves no teacher.
    let personal = tmp.path().join("personal");
    assert!(Service::open(Config {
        state_dir: personal.clone(),
        dataset: DatasetClass::Personal,
        allow_personal: false,
        grant_roots: vec![]
    })
    .is_err());
    let p = Service::open(Config {
        state_dir: personal,
        dataset: DatasetClass::Personal,
        allow_personal: true,
        grant_roots: vec![owner.canonicalize().unwrap()],
    })
    .unwrap();
    let (hello, _) = ok("session.hello", call(&p, "session.hello", json!({})));
    assert_eq!(hello["features"]["teacher_available"], false);
    assert_eq!(
        ok("roots.list", call(&p, "roots.list", json!({}))).0["roots"][0]["origin"],
        "owner_granted"
    );
}

/// A scan writer that records what `state.db` said when the service fenced it.
struct FenceProbe {
    state: std::path::PathBuf,
    seen: Mutex<Vec<String>>,
}

impl ScanSink for FenceProbe {
    fn recover_interrupted(&self) -> EngineResult<()> {
        Ok(())
    }
    fn begin_run(&self, _: &RootId, _: u64, _: &RunScope) -> EngineResult<()> {
        Ok(())
    }
    fn prepare_listing(
        &self,
        _: &RootId,
        _: u64,
        _: Option<u64>,
        _: &[u16],
        _: loomward_engine::scan::source::OpenedIdentity,
    ) -> EngineResult<ListingTicket> {
        Err(loomward_engine::EngineError::unavailable(
            loomward_engine::Component::Scan,
        ))
    }
    fn refresh_listing(&self, _: &RootId, _: u64, _: ListingTicket) -> EngineResult<ListingTicket> {
        Err(loomward_engine::EngineError::unavailable(
            loomward_engine::Component::Scan,
        ))
    }
    fn consume(&self, _: &RootId, _: ScanMessage) -> EngineResult<()> {
        Ok(())
    }
    fn finish_run(&self, _: &RootId, _: u64, _: &RunScope, _: bool) -> EngineResult<()> {
        Ok(())
    }
    fn fence_root(&self, root: &RootId) -> EngineResult<()> {
        let conn = rusqlite::Connection::open(self.state.join("state.db")).unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM root_grant ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        self.seen.lock().unwrap().push(format!("{root}:{state}"));
        Ok(())
    }
}

#[test]
fn revocation_commits_first_then_fences_then_emits_and_is_monotone() {
    let tmp = temp_root();
    let svc = Service::open_with(
        Config {
            state_dir: tmp.path().into(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            grant_roots: vec![],
        },
        crate::idem::TTL,
        false,
    )
    .unwrap();
    let probe = Arc::new(FenceProbe {
        state: svc.inner.state_dir.clone(),
        seen: Mutex::new(vec![]),
    });
    svc.inner.engine.set_scan_sink(probe.clone()).unwrap();
    let (root, grant) = fixture(&svc, 3);
    let node = root_slice(&svc, root)["nodes"][1]["node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut events = svc.subscribe(None);
    let (r1, meta) = ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": svc.inner.root_id(root), "purge_catalog": false}),
        ),
    );
    // The fence ran after the durable commit: it already saw the grant revoked.
    assert_eq!(
        *probe.seen.lock().unwrap(),
        vec![format!("{}:revoked", svc.inner.root_id(root))]
    );
    // The event follows the commit and carries its revision.
    let event = loop {
        match events.recv_timeout(Duration::from_secs(2)) {
            RecvOutcome::Event(e) if e.event == EventName::RootsChanged => break e,
            RecvOutcome::Event(_) => continue,
            other => panic!("{other:?}"),
        }
    };
    event.validate().unwrap();
    assert_eq!(event.state_rev, meta.state_rev);
    assert_eq!(event.data["roots"][0]["grant_state"], "revoked");
    // Monotone: re-revoking (by root or by grant) returns the original time.
    let (r2, _) = ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": svc.inner.root_id(root), "purge_catalog": false}),
        ),
    );
    assert_eq!(r1["revoked_at"], r2["revoked_at"]);
    let (g, _) = ok(
        "grants.revoke",
        call(
            &svc,
            "grants.revoke",
            json!({"grant_id": format!("gr_{grant}")}),
        ),
    );
    assert_eq!(g["revoked_at"], r1["revoked_at"]);
    // Nothing under a revoked root is served any more.
    assert_eq!(
        err(call(&svc, "node.inspect", json!({"node_id": node}))).code,
        ErrorCode::NotFound
    );
    assert_eq!(
        probe.seen.lock().unwrap().len(),
        1,
        "a no-op revoke does not fence again"
    );
}

/// A fixture root with its own identity, no listing.
fn register(svc: &Service, file_id: u8) -> (i64, i64) {
    register_as(svc, file_id, "fixture")
}

fn register_as(svc: &Service, file_id: u8, origin: &str) -> (i64, i64) {
    let WriteReply::Root {
        grant_id, root_id, ..
    } = svc
        .inner
        .catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_key: "fixture".into(),
            display_name: format!("Synthetic {file_id}"),
            display_path: format!("Synthetic root {file_id}"),
            root_file_id: Some(vec![file_id; 16]),
            filesystem: Some("NTFS".into()),
            origin: origin.into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))
        .unwrap()
    else {
        panic!()
    };
    (root_id, grant_id)
}

/// H1: a rebuilt catalogue reassigns root and volume rows. An ID minted before the rebuild is
/// refused with `catalog_instance_changed` and never selects the row that now has its number.
#[test]
fn root_and_volume_ids_from_a_rebuilt_catalogue_never_select_a_new_row() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (a, _) = register(&svc, 1);
    register(&svc, 2);
    let old_a = svc.inner.root_id(a);
    let (volumes, _) = ok("volumes.list", call(&svc, "volumes.list", json!({})));
    let old_volume = volumes["volumes"][0]["volume_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let old_volume_row = svc.inner.volume_row(&old_volume).unwrap();
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": old_a, "purge_catalog": false}),
        ),
    );
    drop(svc);
    for name in ["catalog.db", "catalog.db-wal", "catalog.db-shm"] {
        let _ = std::fs::remove_file(tmp.path().join(name));
    }
    assert!(!tmp.path().join("catalog.db").exists());

    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    let roots = roots["roots"].as_array().unwrap();
    assert_eq!(roots.len(), 1, "only the active grant is derived again");
    let new_b = roots[0]["root_id"].as_str().unwrap().to_owned();
    assert_eq!(
        svc.inner.root_row(&new_b).unwrap(),
        a,
        "the surviving root now has revoked root A's row number"
    );
    // A retried revocation of A must not revoke B.
    let e = err(call(
        &svc,
        "roots.revoke",
        json!({"root_id": old_a, "purge_catalog": false}),
    ));
    assert_eq!(
        (e.code, reason(&e)),
        (ErrorCode::NotFound, Some("catalog_instance_changed"))
    );
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    assert_eq!(roots["roots"][0]["grant_state"], "active");
    // An old tier declaration must not change the volume that now has its row number.
    let (volumes, _) = ok("volumes.list", call(&svc, "volumes.list", json!({})));
    let new_volume = volumes["volumes"][0]["volume_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(svc.inner.volume_row(&new_volume).unwrap(), old_volume_row);
    assert_ne!(new_volume, old_volume);
    let e = err(declare(&svc, "old-tier", &old_volume, 2, None, None));
    assert_eq!(
        (e.code, reason(&e)),
        (ErrorCode::NotFound, Some("catalog_instance_changed"))
    );
    assert_eq!(
        crate::db::declarations(&svc.inner.db(), "fixture").unwrap(),
        Vec::<Option<i64>>::new()
    );
    // Bare row numbers and malformed spellings name nothing and carry no reason.
    for bad in [format!("rt_{a}"), "rt_x".into()] {
        let e = err(call(
            &svc,
            "roots.revoke",
            json!({"root_id": bad, "purge_catalog": false}),
        ));
        assert_eq!(
            (e.code, e.detail.clone()),
            (ErrorCode::NotFound, None),
            "{bad}"
        );
    }
    // Catalogue projections carry the bound spelling too.
    let slice = root_slice(&svc, svc.inner.root_row(&new_b).unwrap());
    assert_eq!(slice["root_generations"][0]["root_id"], new_b.as_str());
}

fn declare_with_deadline(svc: &Service, volume: &str, deadline: i64) -> ResponseEnvelope {
    let mut r = request(
        "volumes.declare_tier",
        json!({"volume_id": volume, "tier": 2}),
    );
    r.deadline_ms = Some(Int::new(deadline).unwrap());
    svc.call(r, &CallContext::http())
}

/// SEC-1: a flood of ticketed mutations cannot starve the owner's revocation, and the queue of
/// outstanding tickets is capped.
#[test]
fn revocation_never_queues_behind_a_full_mutation_queue() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 1);
    let (volumes, _) = ok("volumes.list", call(&svc, "volumes.list", json!({})));
    let volume = volumes["volumes"][0]["volume_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let hooks = &svc.inner.hooks;
    const DECLARE: Command = Command::VolumesDeclareTier;
    *hooks.closed.lock().unwrap() = vec![(DECLARE, "locked"), (DECLARE, "after")];
    // The queue head holds `Inner::mutation`; 63 more wait for their tickets; the 65th is busy.
    assert_eq!(
        err(declare_with_deadline(&svc, &volume, 1)).code,
        ErrorCode::DeadlineExceeded
    );
    hooks.until("the head holds the mutation lock", |h| {
        h.arrived.lock().unwrap().contains(&(DECLARE, "locked"))
    });
    for _ in 1..crate::MAX_PENDING {
        assert_eq!(
            err(declare_with_deadline(&svc, &volume, 1)).code,
            ErrorCode::DeadlineExceeded
        );
    }
    let e = err(declare_with_deadline(&svc, &volume, 1));
    assert_eq!(
        (e.code, e.retryable),
        (ErrorCode::Busy, true),
        "65th refused"
    );
    // The revocation waits only for the lock the head holds, never for the queue behind it.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let svc = &svc;
        let tx = tx.clone();
        scope.spawn(move || {
            let mut r = request(
                "roots.revoke",
                json!({"root_id": svc.inner.root_id(root), "purge_catalog": false}),
            );
            r.deadline_ms = Some(Int::new(60_000).unwrap());
            tx.send(svc.call(r, &CallContext::http())).unwrap();
        });
        hooks.until("the revocation is running", |h| {
            h.runs.lock().unwrap().contains_key(&Command::RootsRevoke)
        });
        // The head commits and releases the lock, then stays unsettled at its "after" gate, so
        // no queued ticket can start: only the revocation can take the lock now.
        hooks.open(DECLARE, "locked");
        let revoked = rx.recv_timeout(Duration::from_secs(60)).unwrap();
        ok("roots.revoke", revoked);
        assert!(hooks.arrived.lock().unwrap().contains(&(DECLARE, "after")));
        assert_eq!(
            hooks.runs.lock().unwrap()[&DECLARE],
            1,
            "no queued mutation ran before the revocation"
        );
        assert_eq!(
            crate::db::declarations(&svc.inner.db(), "fixture").unwrap(),
            vec![Some(2)],
            "the head's declaration committed under the lock first"
        );
        ok(
            "grants.revoke",
            call(
                svc,
                "grants.revoke",
                json!({"grant_id": format!("gr_{grant}")}),
            ),
        );
        hooks.open(DECLARE, "after");
    });
}

/// H2: a mutation whose thread could not be spawned never takes a ticket, so a later mutation
/// still waits for an earlier one, and the queue keeps moving.
#[test]
fn a_failed_mutation_spawn_keeps_queue_order_and_liveness() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    *svc.inner.hooks.delay.lock().unwrap() =
        Some((Command::BudgetsSet, Duration::from_millis(300)));
    let mut first = request("budgets.set", json!({"pool": "learning", "max_workers": 1}));
    first.deadline_ms = Some(Int::new(1).unwrap());
    assert_eq!(
        err(svc.call(first, &CallContext::http())).code,
        ErrorCode::DeadlineExceeded
    );
    svc.inner
        .hooks
        .fail_spawn
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        err(declare_with_deadline(&svc, "vo_failed", 5000)).code,
        ErrorCode::ResourceBudget
    );
    // Ran (an unknown volume), within its deadline, and only after the earlier mutation.
    assert_eq!(
        err(declare_with_deadline(&svc, "vo_later", 5000)).code,
        ErrorCode::NotFound
    );
    assert_eq!(
        *svc.inner.hooks.order.lock().unwrap(),
        vec![Command::BudgetsSet, Command::VolumesDeclareTier]
    );
    assert_eq!(
        err(declare_with_deadline(&svc, "vo_last", 5000)).code,
        ErrorCode::NotFound,
        "the queue is still live"
    );
}

/// #185 item 2: a mutation whose thread could not be spawned never ran, so its request_id is
/// free: a protocol-following retry runs instead of replaying `no worker`.
#[test]
fn a_failed_spawn_leaves_the_request_id_free_for_a_retry() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    svc.inner
        .hooks
        .fail_spawn
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let e = err(declare(&svc, "retry-me", "vo_unknown", 2, None, None));
    assert_eq!((e.code, e.retryable), (ErrorCode::ResourceBudget, true));
    assert_eq!(
        err(declare(&svc, "retry-me", "vo_unknown", 2, None, None)).code,
        ErrorCode::NotFound,
        "the retry ran"
    );
    assert_eq!(
        svc.inner.hooks.runs.lock().unwrap()[&Command::VolumesDeclareTier],
        1
    );
}

/// #185 item 1: with every idempotency slot live, other mutations are `busy` but the owner's
/// revocation still runs.
#[test]
fn revocation_runs_with_the_idempotency_slot_cap_full() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 1);
    for _ in 0..crate::idem::MAX_SLOTS {
        assert_eq!(
            err(declare_with_deadline(&svc, "vo_unknown", 5000)).code,
            ErrorCode::NotFound
        );
    }
    let e = err(declare_with_deadline(&svc, "vo_unknown", 5000));
    assert_eq!(
        (e.code, e.retryable),
        (ErrorCode::Busy, true),
        "cap is full"
    );
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": svc.inner.root_id(root), "purge_catalog": false}),
        ),
    );
    ok(
        "grants.revoke",
        call(
            &svc,
            "grants.revoke",
            json!({"grant_id": format!("gr_{grant}")}),
        ),
    );
}

/// #185 item 3: revoked roots keep their rows, so after many grant/revoke cycles `roots.list`
/// still shows every active root (then the newest revoked ones), in id order, at most 64.
#[test]
fn roots_list_keeps_active_roots_past_64_revoked_rows() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let all: Vec<(i64, i64)> = (1..=70).map(|i| register(&svc, i)).collect();
    for (_, grant) in &all[..66] {
        svc.inner.revoke(*grant).unwrap();
    }
    for command in ["roots.list", "grants.list"] {
        let (list, _) = ok(command, call(&svc, command, json!({})));
        let key = if command == "roots.list" {
            "roots"
        } else {
            "grants"
        };
        let ids: Vec<i64> = list[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| svc.inner.root_row(r["root_id"].as_str().unwrap()).unwrap())
            .collect();
        // roots.list is capped at 64 (4 active, then the 60 newest revoked); grants.list at 256.
        let from = if command == "roots.list" { 6 } else { 0 };
        let expected: Vec<i64> = all[from..].iter().map(|(root, _)| *root).collect();
        assert_eq!(ids, expected, "{command}");
    }
}

/// #185 item 3: the contract lists at most 64 roots, so a 65th active grant is refused by name;
/// re-granting an active root and granting after a revocation still work.
#[test]
fn grants_past_64_active_roots_are_refused_by_rule() {
    let tmp = temp_root();
    let state = tmp.path().join("state");
    let svc = open(&state, DatasetClass::Personal);
    let dirs: Vec<std::path::PathBuf> = (0..65)
        .map(|i| {
            let d = tmp.path().join(format!("owner-{i}"));
            std::fs::create_dir(&d).unwrap();
            d.canonicalize().unwrap()
        })
        .collect();
    if !native_identity_available() {
        // Unsupported platforms and elevated tokens fail closed before the cap.
        assert!(svc.inner.grant(&dirs[0], "cli_flag").is_err());
        return;
    }
    let mut granted = Vec::new();
    for d in &dirs[..64] {
        granted.push(svc.inner.grant(d, "cli_flag").unwrap().0);
    }
    assert_eq!(
        svc.inner.grant(&dirs[64], "cli_flag"),
        Err(crate::Refused::RootLimit)
    );
    assert_eq!(
        svc.inner.grant(&dirs[0], "cli_flag"),
        Ok((granted[0], false))
    );
    let grant = crate::db::roots(&svc.inner.db(), Some(granted[0])).unwrap()[0].grant;
    svc.inner.revoke(grant).unwrap();
    assert!(svc.inner.grant(&dirs[64], "cli_flag").unwrap().1);
}

fn native_identity_available() -> bool {
    #[cfg(windows)]
    if loomward_windows::enumerate::running_elevated() {
        assert!(
            matches!(loomward_windows::identity::observe(Path::new("synthetic")),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied)
        );
        return false;
    }
    cfg!(windows)
}

/// #185 item 5: node IDs are bound to the session (`semantics.md` section 5). The key is never
/// persisted, so an ID minted before a reopen of the same catalogue is plain `not_found`.
#[test]
fn node_ids_do_not_survive_a_reopen() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, _) = fixture(&svc, 2);
    let id = root_slice(&svc, root)["nodes"][1]["node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    ok(
        "node.inspect",
        call(&svc, "node.inspect", json!({"node_id": id})),
    );
    let instance = svc.inner.instance.clone();
    drop(svc);
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    assert_eq!(svc.inner.instance, instance, "same catalogue instance");
    let e = err(call(&svc, "node.inspect", json!({"node_id": id})));
    assert_eq!((e.code, e.detail), (ErrorCode::NotFound, None));
    // The same row is reachable again through a fresh read in this session.
    let fresh = root_slice(&svc, root)["nodes"][1]["node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(fresh, id);
    ok(
        "node.inspect",
        call(&svc, "node.inspect", json!({"node_id": fresh})),
    );
}

/// Test gate inside the scan writer: once armed, the first child ticket blocks (after the root
/// listing published) until released, so a test can act mid-scan deterministically.
#[derive(Default)]
pub(crate) struct Gate {
    armed: Mutex<bool>,
    reached: std::sync::atomic::AtomicBool,
}

impl Gate {
    pub fn pass(&self, child: bool) {
        if !child {
            return;
        }
        while *self.armed.lock().unwrap() {
            self.reached
                .store(true, std::sync::atomic::Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[cfg(windows)]
    fn arm(&self) {
        *self.armed.lock().unwrap() = true;
    }
    #[cfg(windows)]
    fn release(&self) {
        *self.armed.lock().unwrap() = false;
    }
    #[cfg(windows)]
    fn wait_reached(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.reached.load(std::sync::atomic::Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "the scan never reached the gate");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// A registered, granted lab root filled with a small synthetic tree:
/// `top.bin` (100), `a/{1,2,3}.bin` (10, 20, 30), `a/deep/x.bin` (5), and an empty `b`.
#[cfg(windows)]
struct Lab {
    _tmp: tempfile::TempDir,
    path: std::path::PathBuf,
    svc: Service,
    root: String,
}

#[cfg(windows)]
fn lab() -> Option<Lab> {
    if loomward_windows::enumerate::running_elevated() {
        eprintln!("skipped: elevated token; the native scan refuses elevation (invariant 2)");
        return None;
    }
    let tmp = temp_root();
    let state = tmp.path().join("state");
    let path = tmp.path().join("lab-scan");
    crate::lab::register(
        &state,
        &path,
        "seed-scan",
        "S",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    let path = path.canonicalize().unwrap();
    std::fs::write(path.join("top.bin"), [0u8; 100]).unwrap();
    std::fs::create_dir_all(path.join("a").join("deep")).unwrap();
    for (n, size) in [(1, 10), (2, 20), (3, 30)] {
        std::fs::write(path.join("a").join(format!("{n}.bin")), vec![0u8; size]).unwrap();
    }
    std::fs::write(path.join("a").join("deep").join("x.bin"), [0u8; 5]).unwrap();
    std::fs::create_dir(path.join("b")).unwrap();
    let svc = Service::open(Config {
        state_dir: state,
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        grant_roots: vec![path.clone()],
    })
    .unwrap();
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    let root = roots["roots"][0]["root_id"].as_str().unwrap().to_owned();
    Some(Lab {
        _tmp: tmp,
        path,
        svc,
        root,
    })
}

#[cfg(windows)]
#[test]
fn service_minted_root_id_round_trips_through_the_engine() {
    let Some(lab) = lab() else { return };
    let inner = &lab.svc.inner;
    let row = inner.root_row(&lab.root).unwrap();
    let grant = crate::db::roots(&inner.db(), Some(row)).unwrap()[0].grant;
    let id = RootId::new(&lab.root).unwrap();
    let root = inner
        .engine
        .granted_root(
            id.clone(),
            grant,
            &loomward_engine::scan::CatalogRootIdentity {
                row,
                instance: inner.instance.clone(),
            },
        )
        .unwrap();
    assert_eq!(root.root_id(), &id);
    assert_eq!(root.path().canonicalize().unwrap(), lab.path);
    assert_eq!(root.dataset_class(), DatasetClass::Synthetic);
}

/// Independent walk: (files, dirs below the root, logical bytes); reparse points are neither
/// followed nor counted, as the scan's exclusion rule says.
#[cfg(windows)]
fn walk(dir: &Path) -> (u64, u64, u64) {
    use std::os::windows::fs::MetadataExt;
    let mut t = (0, 0, 0);
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let m = std::fs::symlink_metadata(e.path()).unwrap();
        if m.file_attributes() & 0x400 != 0 {
            continue;
        }
        if m.is_dir() {
            let (f, d, b) = walk(&e.path());
            t = (t.0 + f, t.1 + d + 1, t.2 + b);
        } else {
            t = (t.0 + 1, t.1, t.2 + m.len());
        }
    }
    t
}

#[cfg(windows)]
fn start(svc: &Service, root: &str, mode: &str) -> String {
    let (job, _) = ok(
        "scan.start",
        call(svc, "scan.start", json!({"root_id": root, "mode": mode})),
    );
    job["job"]["job_id"].as_str().unwrap().to_owned()
}

#[cfg(windows)]
fn settle(svc: &Service, job: &str) -> Payload {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (j, _) = ok("jobs.get", call(svc, "jobs.get", json!({"job_id": job})));
        let state = j["job"]["state"].as_str().unwrap();
        if matches!(state, "completed" | "failed" | "cancelled") {
            return j["job"].as_object().unwrap().clone();
        }
        assert!(Instant::now() < deadline, "{j:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(windows)]
fn lab_slice(svc: &Service, root: &str) -> Payload {
    ok(
        "tree.slice",
        call(svc, "tree.slice", json!({"anchor": {"kind": "root", "root_id": root}, "depth": 3, "max_nodes": 256, "min_share": 0.0, "basis": "logical", "include_files": true})),
    )
    .0
}

/// (files, dirs, logical) of the slice's anchor node.
#[cfg(windows)]
fn slice_totals(slice: &Payload) -> (u64, u64, u64) {
    let n = &slice["nodes"][0];
    (
        n["files"].as_u64().unwrap(),
        n["dirs"].as_u64().unwrap(),
        n["logical_bytes"].as_str().unwrap().parse().unwrap(),
    )
}

#[cfg(windows)]
fn child_named(slice: &Payload, name: &str) -> Option<String> {
    slice["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["parent"] == json!(0) && n["name"] == name)
        .map(|n| n["node_id"].as_str().unwrap().to_owned())
}

#[cfg(windows)]
#[test]
fn scan_start_publishes_a_lab_tree_matching_an_independent_walk_with_live_events() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    let mut events = svc.subscribe(None);
    let job = start(svc, &lab.root, "full");
    let done = settle(svc, &job);
    assert_eq!(done["state"], "completed", "{done:?}");
    assert_eq!(done["coverage"], "complete");
    let slice = lab_slice(svc, &lab.root);
    assert_eq!(slice_totals(&slice), walk(&lab.path));
    assert_eq!(slice_totals(&slice), (5, 3, 165));
    assert_eq!(slice["aggregate_state"], "consistent", "{slice:?}");
    let (roots, _) = ok("roots.list", call(svc, "roots.list", json!({})));
    assert_eq!(roots["roots"][0]["scan"]["coverage"], "complete");
    // Events: catalogue revisions arrive before the job's terminal state, each after its commit.
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "{seen:?}");
        match events.recv_timeout(Duration::from_secs(2)) {
            RecvOutcome::Event(e) => {
                e.validate().unwrap();
                if e.event == EventName::TreeInvalidated {
                    assert!(e.catalog_rev.is_some());
                    assert_eq!(e.data["root_id"], lab.root.as_str());
                }
                let terminal =
                    e.event == EventName::JobState && e.data["job"]["state"] == "completed";
                seen.push(e.event);
                if terminal {
                    break;
                }
            }
            RecvOutcome::Timeout => {}
            other => panic!("{other:?}"),
        }
    }
    assert!(seen.contains(&EventName::ScanProgress), "{seen:?}");
    let last_tree = seen
        .iter()
        .rposition(|e| *e == EventName::TreeInvalidated)
        .expect("tree.invalidated");
    assert!(last_tree < seen.len() - 1);
    // A refresh scans the same root again to the same totals.
    assert_eq!(
        settle(svc, &start(svc, &lab.root, "refresh"))["state"],
        "completed"
    );
    assert_eq!(slice_totals(&lab_slice(svc, &lab.root)), (5, 3, 165));
}

/// #184 MEDIUM-1 for the durable writer: a directory deleted and recreated under the same name
/// (same creation time, new file ID) is a new row; the old row and its subtree are absent.
#[cfg(windows)]
#[test]
fn absence_is_decided_by_identity_when_a_directory_is_replaced_at_its_name() {
    use std::os::windows::fs::{FileTimesExt, OpenOptionsExt};
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    assert_eq!(
        settle(svc, &start(svc, &lab.root, "full"))["state"],
        "completed"
    );
    let old = child_named(&lab_slice(svc, &lab.root), "a").unwrap();
    let a = lab.path.join("a");
    let created = std::fs::metadata(&a).unwrap().created().unwrap();
    std::fs::remove_dir_all(&a).unwrap();
    std::fs::create_dir(&a).unwrap();
    std::fs::write(a.join("new.bin"), [0u8; 7]).unwrap();
    // Same name, same creation time: only the native identity tells the directories apart.
    std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(0x0200_0000) // FILE_FLAG_BACKUP_SEMANTICS opens a directory
        .open(&a)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_created(created))
        .unwrap();
    assert_eq!(std::fs::metadata(&a).unwrap().created().unwrap(), created);
    assert_eq!(
        settle(svc, &start(svc, &lab.root, "refresh"))["state"],
        "completed"
    );
    let slice = lab_slice(svc, &lab.root);
    assert_eq!(slice_totals(&slice), walk(&lab.path));
    assert_eq!(slice_totals(&slice), (2, 2, 107));
    let new = child_named(&slice, "a").unwrap();
    assert_ne!(
        new, old,
        "a replaced directory is a new row, never the old identity"
    );
    assert_eq!(
        err(call(svc, "node.inspect", json!({"node_id": old}))).code,
        ErrorCode::NotFound
    );
}

/// #206 MEDIUM-1, invariant 4: a directory replaced by a junction (a reparse point) between two
/// scans stays visible as an `excluded` entry flagged `reparse_not_followed`. It is never
/// followed and never counted, the replaced directory's old row retires, and the root's coverage
/// is not complete while the job still reports the skip.
#[cfg(windows)]
#[test]
fn a_junction_replacing_a_directory_stays_visible_as_excluded() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    assert_eq!(
        settle(svc, &start(svc, &lab.root, "full"))["state"],
        "completed"
    );
    let old = child_named(&lab_slice(svc, &lab.root), "b").unwrap();
    std::fs::remove_dir(lab.path.join("b")).unwrap();
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J", "b", "a"])
        .current_dir(&lab.path)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    let done = settle(svc, &start(svc, &lab.root, "refresh"));
    assert_eq!(done["state"], "completed", "{done:?}");
    assert!(done["progress"]["skipped"].as_u64().unwrap() >= 1);
    let slice = lab_slice(svc, &lab.root);
    let b = slice["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["parent"] == json!(0) && n["name"] == "b")
        .expect("the junction stays visible");
    assert_eq!(b["coverage"], "excluded", "{b:?}");
    assert_eq!(b["threads"]["permission"]["state"], "excluded", "{b:?}");
    assert_eq!(
        b["threads"]["permission"]["reason"], "reparse_not_followed",
        "{b:?}"
    );
    assert_eq!(
        (b["files"].as_u64(), b["dirs"].as_u64()),
        (Some(0), Some(0))
    );
    assert_ne!(
        slice["nodes"][0]["coverage"], "complete",
        "an excluded child keeps the root from claiming complete coverage"
    );
    let (roots, _) = ok("roots.list", call(svc, "roots.list", json!({})));
    assert_eq!(
        roots["roots"][0]["scan"]["coverage"], "partial",
        "roots.list agrees with the tree"
    );
    // Never followed and never counted: the totals still equal a walk that skips reparse points.
    assert_eq!(slice_totals(&slice), walk(&lab.path));
    assert_eq!(slice_totals(&slice), (5, 2, 165));
    assert_eq!(
        err(call(svc, "node.inspect", json!({"node_id": old}))).code,
        ErrorCode::NotFound,
        "the replaced directory's row retired"
    );
    // A second scan keeps the same excluded row (no churn) and still completes.
    let kept = child_named(&lab_slice(svc, &lab.root), "b").unwrap();
    assert_eq!(
        settle(svc, &start(svc, &lab.root, "refresh"))["state"],
        "completed"
    );
    let again = lab_slice(svc, &lab.root);
    assert_eq!(child_named(&again, "b"), Some(kept));
    assert_eq!(slice_totals(&again), (5, 2, 165));
}

/// #206: a granted folder replaced at its path fails the identity check at scan start; the
/// grant is recorded `identity_changed` in `state.db` and `roots.changed` says so, so the UI
/// stops advertising a dead grant. Nothing is scanned and the grant is never active again.
#[cfg(windows)]
#[test]
fn a_replaced_root_records_identity_changed_and_emits_roots_changed() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    std::fs::rename(&lab.path, lab.path.with_file_name("lab-moved")).unwrap();
    std::fs::create_dir(&lab.path).unwrap();
    let mut events = svc.subscribe(None);
    let e = err(call(
        svc,
        "scan.start",
        json!({"root_id": lab.root, "mode": "full"}),
    ));
    assert_eq!(
        (e.code, e.message.as_str()),
        (ErrorCode::PermissionDenied, "root_identity_changed")
    );
    let state: String = svc
        .inner
        .db()
        .query_row("SELECT state FROM st.root_grant", [], |r| r.get(0))
        .unwrap();
    assert_eq!(state, "identity_changed");
    let event = loop {
        match events.recv_timeout(Duration::from_secs(2)) {
            RecvOutcome::Event(e) if e.event == EventName::RootsChanged => break e,
            RecvOutcome::Event(_) => continue,
            other => panic!("{other:?}"),
        }
    };
    event.validate().unwrap();
    assert_eq!(event.data["roots"][0]["grant_state"], "identity_changed");
    let (roots, _) = ok("roots.list", call(svc, "roots.list", json!({})));
    assert_eq!(roots["roots"][0]["grant_state"], "identity_changed");
    assert_eq!(roots["roots"][0]["scan"]["state"], "stale");
    assert_eq!(
        crate::db::counts(&svc.inner.db()).unwrap().0,
        0,
        "nothing scanned"
    );
    let again = err(call(
        svc,
        "scan.start",
        json!({"root_id": lab.root, "mode": "full"}),
    ));
    assert_eq!(again.code, ErrorCode::PermissionDenied);
}

/// Revocation mid-scan: the grant commits revoked first, the run is cancelled and the writer
/// fenced; nothing more is written for the root and its catalogue run ends cancelled.
#[cfg(windows)]
#[test]
fn revocation_mid_scan_commits_cancels_and_fences_the_writer() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    svc.inner.scan.gate.arm();
    let job = start(svc, &lab.root, "full");
    svc.inner.scan.gate.wait_reached();
    let rows = || crate::db::counts(&svc.inner.db()).unwrap();
    ok(
        "roots.revoke",
        call(
            svc,
            "roots.revoke",
            json!({"root_id": lab.root, "purge_catalog": false}),
        ),
    );
    let at_revoke = rows();
    svc.inner.scan.gate.release();
    let done = settle(svc, &job);
    assert_eq!(done["state"], "cancelled", "{done:?}");
    assert_eq!(rows(), at_revoke, "nothing is written after revocation");
    let run_state: String = svc
        .inner
        .db()
        .query_row(
            "SELECT state FROM main.scan_run ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(run_state, "cancelled");
    let id = RootId::new(lab.root.clone()).unwrap();
    let commands = svc.inner.catalog.writer().timings().commands;
    assert_eq!(
        svc.inner
            .scan
            .begin_run(&id, u64::MAX, &RunScope::FullRoot)
            .unwrap_err()
            .to_body()
            .code,
        ErrorCode::PermissionDenied,
        "the fenced writer refuses the revoked grant"
    );
    assert_eq!(
        svc.inner.catalog.writer().timings().commands,
        commands,
        "the fence refuses before reaching the catalogue writer"
    );
    assert_eq!(
        err(call(
            svc,
            "scan.start",
            json!({"root_id": lab.root, "mode": "full"})
        ))
        .code,
        ErrorCode::PermissionDenied
    );
}

#[cfg(windows)]
#[test]
fn scan_cancel_is_acknowledged_and_never_publishes_a_complete_generation() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    svc.inner.scan.gate.arm();
    let job = start(svc, &lab.root, "full");
    svc.inner.scan.gate.wait_reached();
    let (ack, _) = ok(
        "scan.cancel",
        call(svc, "scan.cancel", json!({"job_id": job})),
    );
    assert_eq!(ack["job"]["state"], "cancel_requested");
    svc.inner.scan.gate.release();
    assert_eq!(settle(svc, &job)["state"], "cancelled");
    let (roots, _) = ok("roots.list", call(svc, "roots.list", json!({})));
    assert_ne!(roots["roots"][0]["scan"]["coverage"], "complete");
    assert!(roots["roots"][0]["scan"]["generation"].is_null());
    // The root scans to completion afterwards.
    assert_eq!(
        settle(svc, &start(svc, &lab.root, "full"))["state"],
        "completed"
    );
}

/// A synthetic session scans registered lab roots only (personal sessions have their own tests),
/// and a synthetic fixture root (no path, no lab registration) is refused.
#[test]
fn scan_start_is_refused_outside_registered_lab_roots() {
    let tmp = temp_root();
    let synthetic = open(&tmp.path().join("synthetic"), DatasetClass::Synthetic);
    let (root, _) = fixture(&synthetic, 1);
    let e = err(call(
        &synthetic,
        "scan.start",
        json!({"root_id": synthetic.inner.root_id(root), "mode": "full"}),
    ));
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert_eq!(reason(&e), Some("synthetic_session_requires_lab_root"));
}

/// A personal session scans only its own active owner grants: an unknown root, a lab root and
/// a revoked grant are refused before the engine sees them.
#[test]
fn personal_scan_start_refuses_unknown_lab_and_revoked_roots() {
    let tmp = temp_root();
    let personal = open(&tmp.path().join("personal"), DatasetClass::Personal);
    let scan = |root: &str| {
        err(call(
            &personal,
            "scan.start",
            json!({"root_id": root, "mode": "full"}),
        ))
    };
    let e = scan("rt_x");
    assert_eq!(e.code, ErrorCode::NotFound, "{e:?}");
    // The catalogue never registers a lab root in a personal store; a row that says so anyway
    // (a planted or corrupted catalogue) is still refused by the scan gate.
    let lab_root = RootObservation {
        volume_key: "fixture".into(),
        display_name: "Lab".into(),
        display_path: "Lab root".into(),
        root_file_id: Some(vec![1; 16]),
        filesystem: Some("NTFS".into()),
        origin: "lab_generated".into(),
        granted_via: "cli_flag".into(),
        observed_at_ns: 0,
    };
    assert!(personal
        .inner
        .catalog
        .writer()
        .call(WriteCommand::RegisterRoot(lab_root))
        .is_err());
    let (lab, lab_grant) = register_as(&personal, 1, "owner_granted");
    personal
        .inner
        .db()
        .execute(
            "UPDATE st.root_grant SET origin='lab_generated' WHERE id=?1",
            [lab_grant],
        )
        .unwrap();
    let e = scan(&personal.inner.root_id(lab));
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert_eq!(reason(&e), Some("personal_session_requires_owner_grant"));
    assert!(
        !e.message.contains("  ") && !e.message.contains("#"),
        "plain words on the wire: {}",
        e.message.as_str()
    );
    let (owner, grant) = register_as(&personal, 2, "owner_granted");
    personal.inner.revoke(grant).unwrap();
    let e = scan(&personal.inner.root_id(owner));
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert_eq!(e.message.as_str(), "the root's grant is not active");
}

/// An active owner grant for `path` made without today's fences: through the grant itself where
/// native identity opens (as an older build recorded it), else as a fixture row naming the path.
fn unfenced_owner_grant(svc: &Service, path: &Path) -> i64 {
    if let Ok((root, _)) = svc.inner.grant(path, "cli_flag") {
        return root;
    }
    let (root, grant) = register_as(svc, 7, "owner_granted");
    svc.inner
        .db()
        .execute(
            "UPDATE st.root_grant SET display_path=?1, granted_via='cli_flag' WHERE id=?2",
            rusqlite::params![path.to_str().unwrap(), grant],
        )
        .unwrap();
    root
}

/// #312: a stored grant that today's fences refuse is refused at scan start, whatever fences
/// held when it was made: no job, no catalogue row. (a) a sibling profile under a profiles
/// directory with an unrecognised name (#286); (b) a deny-listed store (`.ollama`).
#[test]
fn personal_scan_start_refuses_a_stored_grant_todays_fences_refuse() {
    let tmp = temp_root();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("Home");
    let sibling = home.join("me.OLDPC");
    let store = base.join("fixture-profile").join(".ollama").join("models");
    for d in [home.join("me"), sibling.clone(), store.clone()] {
        std::fs::create_dir_all(d).unwrap();
    }
    for (path, profile) in [
        (sibling, Some((home.join("me"), home.clone()))),
        (store, None),
    ] {
        let svc = open(
            &tmp.path().join(format!("state-{}", profile.is_some())),
            DatasetClass::Personal,
        );
        *svc.inner.hooks.profile.lock().unwrap() = profile;
        let root = unfenced_owner_grant(&svc, &path);
        let rows = crate::db::counts(&svc.inner.db()).unwrap();
        let e = err(call(
            &svc,
            "scan.start",
            json!({"root_id": svc.inner.root_id(root), "mode": "full"}),
        ));
        assert_eq!(e.code, ErrorCode::PermissionDenied, "{e:?}");
        assert_eq!(reason(&e), Some("protected_profile_or_store"));
        assert!(
            !e.message.contains(&*base.to_string_lossy()),
            "no path on the wire"
        );
        let (list, _) = ok("jobs.list", call(&svc, "jobs.list", json!({"limit": 50})));
        assert_eq!(list["jobs"], json!([]), "no job started");
        assert_eq!(
            crate::db::counts(&svc.inner.db()).unwrap(),
            rows,
            "no catalogue row"
        );
    }
}

/// Personal scans are open (#184, #206, #240, #269 closed): an owner folder granted through the
/// CLI path scans to completion, metadata only. No file in the root is modified or rewritten.
#[cfg(windows)]
#[test]
fn personal_session_scans_a_granted_folder_metadata_only() {
    let tmp = temp_root();
    let path = tmp.path().join("owner-folder");
    std::fs::create_dir_all(path.join("a").join("deep")).unwrap();
    std::fs::create_dir(path.join("b")).unwrap();
    let files = [
        ("top.bin", 100usize),
        ("a/1.bin", 10),
        ("a/2.bin", 20),
        ("a/deep/x.bin", 5),
    ];
    for (i, (name, size)) in files.iter().enumerate() {
        std::fs::write(path.join(name), vec![i as u8 + 1; *size]).unwrap();
    }
    let path = path.canonicalize().unwrap();
    let snapshot = || -> Vec<(Vec<u8>, std::time::SystemTime)> {
        files
            .iter()
            .map(|(name, _)| {
                let f = path.join(name);
                (
                    std::fs::read(&f).unwrap(),
                    f.metadata().unwrap().modified().unwrap(),
                )
            })
            .collect()
    };
    let before = snapshot();
    let opened = Service::open(Config {
        state_dir: tmp.path().join("state"),
        dataset: DatasetClass::Personal,
        allow_personal: true,
        grant_roots: vec![path.clone()],
    });
    if loomward_windows::enumerate::running_elevated() {
        // Hosted CI: an elevated token cannot open native identity, so the grant is refused.
        let e = opened.err().expect("an elevated grant is refused");
        assert!(e.contains("identity_unavailable"), "{e}");
        return;
    }
    let svc = opened.unwrap();
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    assert_eq!(roots["roots"][0]["origin"], "owner_granted");
    let root = roots["roots"][0]["root_id"].as_str().unwrap().to_owned();
    let done = settle(&svc, &start(&svc, &root, "full"));
    assert_eq!(done["state"], "completed", "{done:?}");
    assert_eq!(done["coverage"], "complete");
    let slice = lab_slice(&svc, &root);
    assert_eq!(slice_totals(&slice), walk(&path));
    assert_eq!(slice_totals(&slice), (4, 3, 135));
    assert_eq!(
        snapshot(),
        before,
        "a metadata scan never writes the files it lists"
    );
}

fn entry(name: &str, bytes: u64, dir: bool) -> loomward_engine::scan::Entry {
    loomward_engine::scan::Entry {
        name: name.encode_utf16().collect(),
        file_id: None,
        identity_eligible: false,
        attributes: if dir { 0x10 } else { 0x20 },
        reparse_tag: None,
        logical: bytes,
        allocated: Some(bytes),
        creation: None,
        last_write: None,
        change: None,
        last_access: None,
        excluded: false,
        traversal_error: None,
    }
}

/// N1 in the durable writer: a refreshed listing makes its old ticket obsolete; the obsolete
/// listing's chunks and publication are refused, what it staged publishes upsert-only, and only
/// the current listing can complete (and so establish absence).
#[test]
fn obsolete_listing_revisions_are_refused_and_never_complete() {
    use loomward_engine::scan::source::{IdBasis, ListOutcome, OpenedIdentity};
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 7);
    let dir = crate::db::active_root_dir(&svc.inner.db(), root)
        .unwrap()
        .unwrap();
    let id = RootId::new(svc.inner.root_id(root)).unwrap();
    let sink = &svc.inner.scan;
    sink.bind(&id, grant, dir);
    sink.begin_run(&id, 41, &RunScope::FullRoot).unwrap();
    let identity = OpenedIdentity {
        id: None,
        basis: IdBasis::None,
        volume_serial: None,
        attributes: 0x10,
        reparse_tag: None,
    };
    let old = sink.prepare_listing(&id, 41, None, &[], identity).unwrap();
    let listing = |ticket, names: &[&str]| ScanMessage::DirListing {
        run: 41,
        ticket,
        entries: names.iter().map(|n| entry(n, 1, false)).collect(),
    };
    sink.consume(&id, listing(old, &["stale.bin"])).unwrap();
    let new = sink.refresh_listing(&id, 41, old).unwrap();
    assert_ne!(new.input_revision, old.input_revision);
    let obsolete = sink.consume(&id, listing(old, &["late.bin"])).unwrap_err();
    assert_eq!(
        obsolete.to_body().detail.unwrap().get("reason").unwrap(),
        "listing_revision_changed"
    );
    let done = |ticket| ScanMessage::ListingDone {
        run: 41,
        ticket,
        outcome: ListOutcome::Complete,
    };
    assert!(sink.consume(&id, done(old)).is_err());
    let files = || -> Vec<String> {
        let conn = svc.inner.db();
        let mut s = conn
            .prepare("SELECT name FROM main.file ORDER BY name")
            .unwrap();
        let names = s.query_map([], |r| r.get(0)).unwrap();
        names.map(Result::unwrap).collect()
    };
    // The refresh published the obsolete stage upsert-only.
    assert_eq!(files(), vec!["stale.bin".to_string()]);
    sink.consume(&id, listing(new, &["kept.bin"])).unwrap();
    sink.consume(&id, done(new)).unwrap();
    assert_eq!(files(), vec!["kept.bin".to_string()]);
    sink.finish_run(&id, 41, &RunScope::FullRoot, true).unwrap();
    assert_eq!(
        sink.consume(&id, listing(new, &["after.bin"]))
            .unwrap_err()
            .to_body()
            .code,
        ErrorCode::StaleGeneration,
        "an ended run accepts nothing"
    );
}

/// Parallel workers over a wider tree: every child ticket follows its parent's publication, so
/// the published totals still equal an independent walk.
#[cfg(windows)]
#[test]
fn a_wide_lab_tree_matches_the_walk_under_parallel_workers() {
    let Some(lab) = lab() else { return };
    for i in 0..12 {
        for j in 0..12 {
            let d = lab.path.join(format!("w{i}")).join(format!("v{j}"));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("f.bin"), vec![0u8; i * 12 + j]).unwrap();
        }
    }
    let svc = &lab.svc;
    let (job, _) = ok(
        "scan.start",
        call(
            svc,
            "scan.start",
            json!({"root_id": lab.root, "mode": "full", "budget": {"threads": 8}}),
        ),
    );
    let done = settle(svc, job["job"]["job_id"].as_str().unwrap());
    assert_eq!(done["state"], "completed", "{done:?}");
    assert_eq!(done["coverage"], "complete");
    assert_eq!(slice_totals(&lab_slice(svc, &lab.root)), walk(&lab.path));
}

/// #206 LOW: a failed `EndRun` aborts the catalogue run, so the root is released instead of
/// answering busy to every later scan until a restart.
#[test]
fn a_failed_end_run_releases_the_root() {
    use loomward_engine::scan::source::{IdBasis, ListOutcome, OpenedIdentity};
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 9);
    let dir = crate::db::active_root_dir(&svc.inner.db(), root)
        .unwrap()
        .unwrap();
    let id = RootId::new(svc.inner.root_id(root)).unwrap();
    let sink = &svc.inner.scan;
    sink.bind(&id, grant, dir);
    svc.inner
        .db()
        .execute_batch(
            "CREATE TRIGGER main.fail_end BEFORE UPDATE OF state ON scan_run
             WHEN NEW.state='completed' BEGIN SELECT RAISE(ABORT,'injected'); END;",
        )
        .unwrap();
    sink.begin_run(&id, 51, &RunScope::FullRoot).unwrap();
    let identity = OpenedIdentity {
        id: None,
        basis: IdBasis::None,
        volume_serial: None,
        attributes: 0x10,
        reparse_tag: None,
    };
    let ticket = sink.prepare_listing(&id, 51, None, &[], identity).unwrap();
    sink.consume(
        &id,
        ScanMessage::ListingDone {
            run: 51,
            ticket,
            outcome: ListOutcome::Complete,
        },
    )
    .unwrap();
    let mut events = svc.subscribe(None);
    assert!(sink.finish_run(&id, 51, &RunScope::FullRoot, true).is_err());
    let announced = loop {
        match events.recv_timeout(Duration::from_millis(500)) {
            RecvOutcome::Event(e) if e.event == EventName::TreeInvalidated => break true,
            RecvOutcome::Event(_) => continue,
            _ => break false,
        }
    };
    assert!(announced, "a failed end still announces what was published");
    let (run_state, active): (String, Option<i64>) = svc
        .inner
        .db()
        .query_row(
            "SELECT s.state,r.active_run FROM main.scan_run s JOIN main.root r ON r.id=s.root_id",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((run_state.as_str(), active), ("failed", None));
    svc.inner
        .db()
        .execute_batch("DROP TRIGGER main.fail_end")
        .unwrap();
    sink.begin_run(&id, 52, &RunScope::FullRoot)
        .expect("the root is not busy after a failed end");
}

/// #206 LOW: no `tree.invalidated` follows a revocation that fenced the root's writer.
#[test]
fn no_tree_invalidated_after_the_writer_is_fenced() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 11);
    let dir = crate::db::active_root_dir(&svc.inner.db(), root)
        .unwrap()
        .unwrap();
    let id = RootId::new(svc.inner.root_id(root)).unwrap();
    svc.inner.scan.bind(&id, grant, dir);
    let mut events = svc.subscribe(None);
    let mut next_tree = || loop {
        match events.recv_timeout(Duration::from_millis(500)) {
            RecvOutcome::Event(e) if e.event == EventName::TreeInvalidated => return true,
            RecvOutcome::Event(_) => continue,
            RecvOutcome::Timeout => return false,
            other => panic!("{other:?}"),
        }
    };
    svc.inner.scan.invalidated(&id, true);
    assert!(next_tree(), "an unfenced root publishes");
    svc.inner.scan.fence_root(&id).unwrap();
    svc.inner.scan.invalidated(&id, true);
    assert!(!next_tree(), "a fenced root never publishes");
}

/// #206: a root swapped after `granted_root` passed is caught by the scan itself; the writer
/// records `identity_changed` and `roots.changed` follows, exactly as at scan start.
#[test]
fn a_scan_that_finds_the_root_swapped_records_identity_changed() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 12);
    let dir = crate::db::active_root_dir(&svc.inner.db(), root)
        .unwrap()
        .unwrap();
    let id = RootId::new(svc.inner.root_id(root)).unwrap();
    svc.inner.scan.bind(&id, grant, dir);
    let mut events = svc.subscribe(None);
    svc.inner.scan.root_identity_changed(&id);
    let state: String = svc
        .inner
        .db()
        .query_row(
            "SELECT state FROM st.root_grant WHERE id=?1",
            [grant],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "identity_changed");
    let event = loop {
        match events.recv_timeout(Duration::from_secs(2)) {
            RecvOutcome::Event(e) if e.event == EventName::RootsChanged => break e,
            RecvOutcome::Event(_) => continue,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(event.data["roots"][0]["grant_state"], "identity_changed");
    assert_eq!(
        svc.inner
            .scan
            .begin_run(&id, 1, &RunScope::FullRoot)
            .unwrap_err()
            .to_body()
            .code,
        ErrorCode::PermissionDenied,
        "the writer is fenced"
    );
}

#[test]
fn followup_all_catalogue_reads_observe_deadlines() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, _) = fixture(&svc, 3);
    let node = root_slice(&svc, root)["anchor_node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let expired = crate::ReadWork {
        deadline: Instant::now(),
        interrupt: Mutex::new(None),
    };
    let active = crate::ReadWork {
        deadline: Instant::now() + Duration::from_secs(10),
        interrupt: Mutex::new(None),
    };
    for (command, payload) in [
        ("roots.list", json!({})),
        (
            "tree.slice",
            json!({"anchor":{"kind":"atlas"},"depth":1,"max_nodes":64,"min_share":0.0,"basis":"logical","include_files":true}),
        ),
        (
            "tree.children",
            json!({"node_id":node,"sort":"name_asc","basis":"logical","limit":10,"cursor":null}),
        ),
        ("tree.path", json!({"node_id":node})),
        ("node.inspect", json!({"node_id":node})),
    ] {
        let req = request(command, payload);
        let cmd = Command::from_name(command).unwrap();
        let control = svc
            .inner
            .handle(&req, cmd, &CallContext::http(), Some(&active));
        assert!(control.is_ok(), "{command} control: {:?}", control.err());
        assert_eq!(
            svc.inner
                .handle(&req, cmd, &CallContext::http(), Some(&expired))
                .err()
                .expect(command)
                .code,
            ErrorCode::DeadlineExceeded,
            "{command}"
        );
    }
}

#[test]
fn followup_picker_refusal_reaches_the_user() {
    struct Pick(std::path::PathBuf);
    impl NativeDialogs for Pick {
        fn pick_folder(&self) -> Option<std::path::PathBuf> {
            Some(self.0.clone())
        }
        fn confirm_disclosure(&self, _: &DisclosureSummary) -> bool {
            false
        }
    }
    let tmp = temp_root();
    let picked = tmp.path().join("Users/fixture/.ssh");
    std::fs::create_dir_all(&picked).unwrap();
    let svc = open(&tmp.path().join("state"), DatasetClass::Personal);
    let ctx = CallContext::tauri(Arc::new(Pick(picked)));
    let (result, _) = ok(
        "roots.request_grant",
        svc.call(
            request("roots.request_grant", json!({"purpose":"metadata_scan"})),
            &ctx,
        ),
    );
    assert_eq!(result["outcome"], "refused");
    assert_eq!(result["refusal"], "protected_profile_or_store");
}

/// #224: the picker refuses a folder that contains a stored active grant whose path no longer
/// resolves, exactly as `--static` does (one shared rule), instead of skipping the stale grant.
#[test]
fn a_stale_stored_grant_still_blocks_a_picked_parent() {
    struct Pick(std::path::PathBuf);
    impl NativeDialogs for Pick {
        fn pick_folder(&self) -> Option<std::path::PathBuf> {
            Some(self.0.clone())
        }
        fn confirm_disclosure(&self, _: &DisclosureSummary) -> bool {
            false
        }
    }
    let tmp = temp_root();
    let parent = tmp.path().join("ws");
    std::fs::create_dir_all(&parent).unwrap();
    let parent = parent.canonicalize().unwrap();
    let svc = open(&tmp.path().join("state"), DatasetClass::Personal);
    let gone = parent.join("gone-grant");
    svc.inner.db().execute("INSERT INTO st.root_grant(volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES('fixture',zeroblob(16),?1,'fixture','fixture','active',0)", [gone.to_str().unwrap()]).unwrap();
    let ctx = CallContext::tauri(Arc::new(Pick(parent)));
    let (result, _) = ok(
        "roots.request_grant",
        svc.call(
            request("roots.request_grant", json!({"purpose":"metadata_scan"})),
            &ctx,
        ),
    );
    assert_eq!(result["outcome"], "refused");
    #[cfg(windows)]
    if loomward_windows::enumerate::running_elevated() {
        // Native identity refuses elevation (invariant 2, #217) before any overlap check runs.
        assert_eq!(result["refusal"], "identity_unavailable");
        return;
    }
    assert_eq!(result["refusal"], "already_granted");
}

#[test]
fn followup_inspect_uses_the_roots_actual_grant() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    svc.inner.db().execute("INSERT INTO st.root_grant(id,volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES(20,'unused',zeroblob(16),'Unrelated grant','fixture','fixture','revoked',0)", []).unwrap();
    let (root, grant) = fixture(&svc, 1);
    assert_ne!(root, grant);
    let node = root_slice(&svc, root)["anchor_node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (result, _) = ok(
        "node.inspect",
        call(&svc, "node.inspect", json!({"node_id":node})),
    );
    assert_eq!(result["root_id"], svc.inner.root_id(root));
    assert_eq!(result["display_path"]["text"], "Synthetic root");
}

#[path = "duplicates_tests.rs"]
mod duplicates_tests;
