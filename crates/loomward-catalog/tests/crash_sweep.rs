//! Crash injection at every write (q-39, docs/45-daily-use-readiness.md row 1).
//!
//! A child process (this test binary re-run on `crash_child`) drives a synthetic lifecycle through
//! the real writer: scan, durable reference, labels, bulk first scan, rescan, revoke, forget. The
//! `test-hooks` crash points (`src/crashpoint.rs`) count every write boundary: each row change,
//! each commit before and after it reaches the log, each checkpoint, VACUUM, backup and rename.
//! A clean run enumerates them; then one child per boundary N is killed (`exit(86)`: no
//! destructors, no graceful close) at exactly the Nth, and the parent reopens the store and
//! asserts the recovery invariants in `verify`.
//!
//! `cargo test -p loomward-catalog --features test-hooks --test crash_sweep` samples the row
//! boundaries (every commit, checkpoint, VACUUM, backup and rename boundary is always swept);
//! `LOOMWARD_CRASH_SWEEP=full` sweeps every boundary of every scenario.
use loomward_catalog::*;
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};

const A_PATH: &str = r"C:\Synthetic\CrashPathAlpha";
const A_NAME: &str = "CrashFolderAlpha";
const A_SUB: &str = "crashsubalpha";
const A_ROOT_FILES: [&str; 2] = ["crashrootA1.zzqx", "crashrootA2.zzqx"];
const A_NEW: &str = "crashrootA3.zzqx";
const A_DEEP: &str = "crashdeepA.zzqx";
const B_NAME: &str = "KeepFolderBeta";
const B_FILES: [&str; 2] = ["keepB1.yyqx", "keepB2.yyqx"];
/// Names the forget must remove from every live store file.
const A_NAMES: [&str; 6] = [
    "CrashPathAlpha",
    A_NAME,
    A_SUB,
    "crashrootA",
    A_DEEP,
    "crashfeatA",
];

// ---- the child ------------------------------------------------------------------------------

fn ident(name: &str, id: u8, size: u64) -> Observation {
    let mut o = Observation::file(name, size, Some(size));
    o.file_id = Some([id; 16].to_vec());
    o.created_ft = Some(1);
    o.extension = name.rsplit('.').next().map(str::to_owned);
    o
}
fn row_id(store: &Path, sql: &str) -> i64 {
    Connection::open(store.join("catalog.db"))
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}
fn register(w: &Writer, name: &str, path: &str, id: u8) -> (i64, i64) {
    match w
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_key: "vol".into(),
            display_name: name.into(),
            display_path: path.into(),
            root_file_id: Some([id; 16].to_vec()),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 1,
        }))
        .unwrap()
    {
        WriteReply::Root {
            grant_id, dir_id, ..
        } => (grant_id, dir_id),
        other => panic!("{other:?}"),
    }
}
fn begin(w: &Writer, grant: i64, mode: &str) -> i64 {
    match w
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: mode.into(),
            strategy: "fixture".into(),
            started_at_ns: 2,
        })
        .unwrap()
    {
        WriteReply::Run(run) => run,
        other => panic!("{other:?}"),
    }
}
fn listing(w: &Writer, run: i64, dir: i64, files: Vec<Observation>, dirs: Vec<Observation>) {
    w.call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: dir,
        files,
        dirs,
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }))
    .unwrap();
}
/// A listing as the service publishes it (`sink.rs`): a staged chunk, then `ListingDone`.
fn staged(w: &Writer, run: i64, dir: i64, files: Vec<Observation>, dirs: Vec<Observation>) {
    w.call(WriteCommand::StageChunk {
        run_id: run,
        dir_id: dir,
        seq: 0,
        files,
        dirs,
    })
    .unwrap();
    w.call(WriteCommand::ListingDone {
        run_id: run,
        dir_id: dir,
        outcome: ListingOutcome::Complete,
        skipped: 0,
        errors: 0,
    })
    .unwrap();
}
fn end(w: &Writer, run: i64) {
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 3,
    })
    .unwrap();
}

struct Ack(PathBuf);
impl Ack {
    /// One line per step, fsynced before the next step starts: the receipt the caller was given.
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

/// open, scan A (staged chunks, the form the service publishes: a root listing plus a sub directory),
/// durable reference and labels, bulk first scan of B (one-shot listing), rescan of A (one file gone, one new), revoke A, forget A.
fn lifecycle(store: &Path, ack: &Ack) {
    let c = Catalog::open(store, "synthetic").unwrap();
    ack.log("open");
    let w = c.writer();
    let (ga, da) = register(w, A_NAME, A_PATH, 1);
    ack.log(&format!("register-a {ga} {da}"));
    let run = begin(w, ga, "full");
    staged(
        w,
        run,
        da,
        vec![
            ident(A_ROOT_FILES[0], 11, 100),
            ident(A_ROOT_FILES[1], 12, 200),
        ],
        vec![ident(A_SUB, 13, 0)],
    );
    let sub = row_id(store, &format!("SELECT id FROM dir WHERE name='{A_SUB}'"));
    staged(w, run, sub, vec![ident(A_DEEP, 14, 4096)], vec![]);
    end(w, run);
    ack.log("end-a");
    let deep = row_id(store, &format!("SELECT id FROM file WHERE name='{A_DEEP}'"));
    let WriteReply::Reference(reference) = w
        .call(WriteCommand::ObjectReference {
            node: NodeKey::File(deep),
            observed_at_ns: 5,
        })
        .unwrap()
    else {
        panic!()
    };
    ack.log(&format!("ref-a {reference}"));
    {
        let st = Connection::open(store.join("state.db")).unwrap();
        st.execute_batch(
            "INSERT INTO taxonomy VALUES(1,'[]',0);
             INSERT INTO collection(name,kind,created_at_ns) VALUES('mixed','human',0);",
        )
        .unwrap();
        st.execute(
            "INSERT INTO human_feedback(event_id,client_event_id,content_digest,object_ref_id,revision,label,retracted,taxonomy_version,features_json,recorded_at_ns) VALUES ('fe0','ce0','d',?1,1,'Documents',0,1,?2,0)",
            rusqlite::params![reference, format!("{{\"name\":\"crashfeatA {A_DEEP}\"}}")],
        )
        .unwrap();
        st.execute("INSERT INTO collection_member VALUES (1,?1,0)", [reference])
            .unwrap();
    }
    ack.log("labels-a");
    let (gb, db) = register(w, B_NAME, r"C:\Synthetic\KeepPathBeta", 2);
    ack.log(&format!("register-b {gb} {db}"));
    w.set_bulk_first_scan(true);
    let rb = begin(w, gb, "full");
    listing(
        w,
        rb,
        db,
        vec![ident(B_FILES[0], 21, 50), ident(B_FILES[1], 22, 60)],
        vec![],
    );
    end(w, rb);
    w.set_bulk_first_scan(false);
    ack.log("end-b");
    let run = begin(w, ga, "refresh");
    staged(
        w,
        run,
        da,
        vec![ident(A_ROOT_FILES[0], 11, 100), ident(A_NEW, 15, 300)],
        vec![ident(A_SUB, 13, 0)],
    );
    staged(w, run, sub, vec![ident(A_DEEP, 14, 4096)], vec![]);
    end(w, run);
    ack.log("end-a2");
    w.call(WriteCommand::RevokeGrant {
        grant_id: ga,
        revoked_at_ns: 10,
    })
    .unwrap();
    ack.log("revoke-a");
    w.call(WriteCommand::ForgetGrant {
        grant_id: ga,
        forgotten_at_ns: 20,
        expected_state_rev: None,
    })
    .unwrap();
    ack.log("forget-a");
    drop(c);
    ack.log("closed");
}

/// Entry point of the killed process; a no-op in a normal test run.
#[test]
fn crash_child() {
    let Ok(scenario) = std::env::var("LOOMWARD_CRASH_CHILD") else {
        return;
    };
    let work = PathBuf::from(std::env::var("LOOMWARD_CRASH_WORK").unwrap());
    let store = work.join("store");
    let ack = Ack(work.join("acks.log"));
    match scenario.as_str() {
        "lifecycle" => lifecycle(&store, &ack),
        // A migration, or the recovery of a store a killed process left behind.
        "migrate" | "recover" => {
            drop(Catalog::open(&store, "synthetic").unwrap());
            ack.log("opened");
        }
        other => panic!("scenario {other}"),
    }
}

// ---- the parent -----------------------------------------------------------------------------

struct Outcome {
    crashed: bool,
    boundaries: u64,
    acks: Vec<String>,
    trace: BTreeMap<u64, String>,
}

fn prepare(scenario: &str, work: &Path) {
    let store = work.join("store");
    if scenario != "migrate" {
        return;
    }
    // A synthetic v4-shaped store: a seeded active grant, no v6/v7 tables, no identity_mode column.
    let c = Catalog::open(&store, "synthetic").unwrap();
    register(c.writer(), A_NAME, A_PATH, 1);
    drop(c);
    Connection::open(store.join("state.db"))
        .unwrap()
        .execute_batch(
            "DROP TABLE root_forgotten; DROP TABLE backup_evidence;
             ALTER TABLE root_grant DROP COLUMN identity_mode; PRAGMA user_version=4;",
        )
        .unwrap();
    Connection::open(store.join("catalog.db"))
        .unwrap()
        .execute_batch("PRAGMA user_version=4;")
        .unwrap();
}

/// Run the scenario in a fresh directory, killing it at boundary `at` (None: run to the end).
fn run_child(scenario: &str, work: &Path, at: Option<u64>) -> Outcome {
    std::fs::create_dir_all(work).unwrap();
    prepare(scenario, work);
    let trace_path = work.join("trace.tsv");
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["crash_child", "--exact", "--nocapture", "--test-threads=1"])
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
    let crashed = code == Some(CRASH_EXIT);
    // A crash point that is never reached is a missing test, not a recovery: require the exit the
    // run was meant to end with.
    let want = if at.is_some() { CRASH_EXIT } else { 0 };
    assert!(
        code == Some(want),
        "{scenario} at {at:?}: the child exited {code:?}, expected {want} ({})\nlast output:\n{}\n{}",
        if at.is_some() {
            "the crash point was not reached"
        } else {
            "a clean run must finish"
        },
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
    let boundaries = trace.keys().next_back().copied().unwrap_or(0);
    Outcome {
        crashed,
        boundaries,
        acks,
        trace,
    }
}

type Res<T> = std::result::Result<T, String>;
fn one(db: &Connection, sql: &str) -> Res<i64> {
    db.query_row(sql, [], |r| r.get(0))
        .map_err(|e| format!("{sql}: {e}"))
}
fn text(db: &Connection, sql: &str) -> Res<String> {
    db.query_row(sql, [], |r| r.get(0))
        .map_err(|e| format!("{sql}: {e}"))
}
fn on_disk(store: &Path, needle: &str) -> bool {
    let utf16: Vec<u8> = needle.encode_utf16().flat_map(u16::to_le_bytes).collect();
    ["state.db", "state.db-wal", "catalog.db", "catalog.db-wal"]
        .iter()
        .filter_map(|n| std::fs::read(store.join(n)).ok())
        .any(|bytes| {
            [needle.as_bytes(), utf16.as_slice()]
                .iter()
                .any(|n| bytes.windows(n.len()).any(|w| w == *n))
        })
}
fn reader(store: &Path) -> Connection {
    let db = Connection::open(store.join("catalog.db")).unwrap();
    db.execute(
        "ATTACH DATABASE ?1 AS st",
        [store.join("state.db").to_string_lossy().as_ref()],
    )
    .unwrap();
    db
}
/// Row counts that a second recovery of an already-recovered store must not change.
fn snapshot(store: &Path) -> Res<Vec<i64>> {
    let db = reader(store);
    [
        "dir",
        "file",
        "root",
        "scan_run",
        "ext",
        "volume",
        "st.root_grant",
        "st.object_ref",
        "st.root_forgotten",
        "st.human_feedback",
        "st.collection_member",
    ]
    .iter()
    .map(|t| one(&db, &format!("SELECT count(*) FROM {t}")))
    .chain([one(&db, "SELECT state_rev FROM st.revision")])
    .collect()
}

/// Every invariant a recovered store must hold, whatever boundary the process died at.
/// `acks` are the steps whose receipts the caller saw: each must be durable.
fn zero_check(db: &Connection, bad: &mut Vec<String>, what: &str, sql: &str) {
    match one(db, sql) {
        Ok(0) => {}
        Ok(n) => bad.push(format!("{what}: {n}")),
        Err(e) => bad.push(format!("{what}: query failed: {e}")),
    }
}
fn structural(store: &Path) -> Vec<String> {
    let db = reader(store);
    let mut bad = Vec::new();
    for schema in ["main", "st"] {
        match text(&db, &format!("PRAGMA {schema}.integrity_check")) {
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
    zero_check(
        &db,
        &mut bad,
        "running scan_run after recovery",
        "SELECT count(*) FROM scan_run WHERE state='running'",
    );
    zero_check(&db, &mut bad, "root left scanning/repairing or holding a run", "SELECT count(*) FROM root WHERE state IN ('scanning','repairing') OR active_run IS NOT NULL");
    zero_check(
        &db,
        &mut bad,
        "bulk build left behind",
        "SELECT (SELECT count(*) FROM bulk_file)+(SELECT count(*) FROM bulk_dir)",
    );
    zero_check(
        &db,
        &mut bad,
        "a forget's rewrite of the stores still owed after recovery",
        "SELECT count(*) FROM st.meta WHERE key='scrub_pending'",
    );
    zero_check(
        &db,
        &mut bad,
        "staged listing left behind",
        "SELECT count(*) FROM stage_entry",
    );
    zero_check(
        &db,
        &mut bad,
        "pending reference intent left behind",
        "SELECT count(*) FROM st.pending_reference",
    );
    zero_check(&db, &mut bad, "root without a grant", "SELECT count(*) FROM root r WHERE NOT EXISTS(SELECT 1 FROM st.root_grant g WHERE g.id=r.grant_id)");
    zero_check(&db, &mut bad, "active grant without a root", "SELECT count(*) FROM st.root_grant g WHERE g.state='active' AND NOT EXISTS(SELECT 1 FROM root r WHERE r.grant_id=g.id)");
    zero_check(
        &db,
        &mut bad,
        "forgotten grant still has a root",
        "SELECT count(*) FROM st.root_forgotten f JOIN root r ON r.grant_id=f.grant_id",
    );
    zero_check(&db, &mut bad, "forgotten grant not revoked or not scrubbed", "SELECT count(*) FROM st.root_forgotten f JOIN st.root_grant g ON g.id=f.grant_id WHERE g.state!='revoked' OR g.display_path!='forgotten' OR g.volume_key!='forgotten'");
    zero_check(&db, &mut bad, "root without exactly one root directory", "SELECT count(*) FROM root r WHERE (SELECT count(*) FROM dir WHERE root_id=r.id AND parent_id IS NULL)!=1");
    zero_check(&db, &mut bad, "directory unreachable from its root", "WITH RECURSIVE reach(id) AS (SELECT id FROM dir WHERE parent_id IS NULL UNION ALL SELECT d.id FROM dir d JOIN reach r ON d.parent_id=r.id JOIN dir p ON p.id=r.id WHERE d.root_id=p.root_id) SELECT (SELECT count(*) FROM dir)-(SELECT count(*) FROM reach)");
    zero_check(&db, &mut bad, "valid rollup disagrees with its rows", "SELECT count(*) FROM dir d WHERE d.sub_complete=1 AND (d.sub_files != (SELECT count(*) FROM file f WHERE f.dir_id=d.id)+coalesce((SELECT sum(c.sub_files) FROM dir c WHERE c.parent_id=d.id),0) OR d.sub_logical != coalesce((SELECT sum(logical) FROM file f WHERE f.dir_id=d.id),0)+coalesce((SELECT sum(c.sub_logical) FROM dir c WHERE c.parent_id=d.id),0))");
    zero_check(&db, &mut bad, "resolved reference bound to a missing row", "SELECT count(*) FROM st.object_ref o WHERE o.state='resolved' AND NOT ((o.kind='file' AND EXISTS(SELECT 1 FROM file WHERE id=o.row_id)) OR (o.kind='dir' AND EXISTS(SELECT 1 FROM dir WHERE id=o.row_id)))");
    zero_check(&db, &mut bad, "row stamped newer than the catalogue revision", "SELECT coalesce((SELECT max(listing_rev) FROM dir),0) > (SELECT catalog_rev FROM revision)");
    bad
}

/// Recovery invariants plus the durability of every acknowledged step. Returns violations.
fn verify(store: &Path, acks: &[String], scenario: &str) -> Vec<String> {
    let mut bad = Vec::new();
    let acked = |step: &str| acks.iter().any(|a| a.split(' ').next() == Some(step));
    // Recovery is the open: it must succeed, twice, and the second must change nothing.
    match Catalog::open(store, "synthetic") {
        Ok(c) => drop(c),
        Err(e) => return vec![format!("reopen after the crash failed: {e}")],
    }
    if let Err(e) = Catalog::validate_stores(store, "synthetic") {
        bad.push(format!("validate_stores after recovery: {e}"));
    }
    bad.extend(structural(store));
    let first = snapshot(store);
    match Catalog::open(store, "synthetic") {
        Ok(c) => drop(c),
        Err(e) => bad.push(format!("second reopen failed: {e}")),
    }
    if first != snapshot(store) {
        bad.push(format!(
            "second recovery changed the store: {first:?} then {:?}",
            snapshot(store)
        ));
    }
    let db = reader(store);
    let mut want = |ok: Res<bool>, what: &str| match ok {
        Ok(true) => {}
        Ok(false) => bad.push(format!("acknowledged but not durable: {what}")),
        Err(e) => bad.push(format!("{what}: {e}")),
    };
    if scenario == "migrate" {
        for schema in ["st", "main"] {
            want(
                one(&db, &format!("PRAGMA {schema}.user_version")).map(|v| v == SCHEMA_VERSION),
                &format!("{schema} at the current schema version"),
            );
        }
        want(
            one(&db, "SELECT (SELECT count(*) FROM st.sqlite_schema WHERE name IN ('root_forgotten','backup_evidence'))=2 AND (SELECT count(*) FROM pragma_table_info('root_grant','st') WHERE name='identity_mode')=1").map(|n| n == 1),
            "the v5, v6 and v7 additions",
        );
        want(
            one(&db, &format!("SELECT count(*) FROM st.root_grant WHERE id=1 AND display_path='{A_PATH}' AND identity_mode='verified' AND state='active'")).map(|n| n == 1),
            "the seeded grant survives the migration",
        );
        return bad;
    }
    let forgotten = one(
        &db,
        "SELECT count(*) FROM st.root_forgotten WHERE grant_id=1",
    )
    .unwrap_or(-1)
        > 0;
    let file_in = |grant: i64, name: &str| {
        one(&db, &format!("SELECT count(*) FROM file f JOIN dir d ON d.id=f.dir_id JOIN root r ON r.id=d.root_id WHERE r.grant_id={grant} AND f.name='{name}'"))
    };
    if acked("register-a") {
        want(
            one(&db, "SELECT count(*) FROM st.root_grant WHERE id=1").map(|n| n == 1),
            "register-a: grant 1",
        );
    }
    if acked("end-a") && !forgotten {
        want(file_in(1, A_DEEP).map(|n| n == 1), "end-a: the deep file");
        want(one(&db, "SELECT count(*) FROM scan_run s JOIN root r ON r.id=s.root_id WHERE r.grant_id=1 AND s.state='completed'").map(|n| n >= 1), "end-a: a completed run");
    }
    if acked("ref-a") && !forgotten {
        want(
            one(&db, "SELECT count(*) FROM st.object_ref").map(|n| n >= 1),
            "ref-a: the object reference",
        );
    }
    if acked("labels-a") && !forgotten {
        want(
            one(
                &db,
                "SELECT count(*) FROM st.human_feedback WHERE event_id='fe0'",
            )
            .map(|n| n == 1),
            "labels-a: the human label",
        );
    }
    if acked("end-b") {
        want(
            one(
                &db,
                "SELECT count(*) FROM root WHERE grant_id=2 AND state='complete'",
            )
            .map(|n| n == 1),
            "end-b: root B complete",
        );
        for f in B_FILES {
            want(
                file_in(2, f).map(|n| n == 1),
                &format!("end-b: {f} published"),
            );
        }
    }
    if acked("end-a2") && !forgotten {
        want(file_in(1, A_NEW).map(|n| n == 1), "end-a2: the new file");
        want(
            file_in(1, A_ROOT_FILES[1]).map(|n| n == 0),
            "end-a2: the removed file is gone",
        );
    }
    if acked("revoke-a") {
        want(
            one(
                &db,
                "SELECT count(*) FROM st.root_grant WHERE id=1 AND state='revoked'",
            )
            .map(|n| n == 1),
            "revoke-a: grant 1 revoked",
        );
    }
    if acked("forget-a") {
        want(Ok(forgotten), "forget-a: the forget record");
    }
    if forgotten {
        // Forget is all or nothing once recovered: nothing of the root's survives, in rows or bytes.
        for (what, sql) in [
            ("root", "SELECT count(*) FROM root WHERE grant_id=1"),
            ("object references", "SELECT count(*) FROM st.object_ref"),
            ("labels", "SELECT count(*) FROM st.human_feedback"),
            (
                "collection members",
                "SELECT count(*) FROM st.collection_member",
            ),
        ] {
            match one(&db, sql) {
                Ok(0) => {}
                Ok(n) => bad.push(format!("forgotten root still has {n} {what}")),
                Err(e) => bad.push(e),
            }
        }
        drop(db);
        for needle in A_NAMES {
            if on_disk(store, needle) {
                bad.push(format!(
                    "forgotten name {needle:?} still in a live store file"
                ));
            }
        }
    }
    // The recovered store must still take writes: no wedged run, grant or lock.
    let c = match Catalog::open(store, "synthetic") {
        Ok(c) => c,
        Err(e) => {
            bad.push(format!("open for the write probe failed: {e}"));
            return bad;
        }
    };
    let actives = {
        let db = reader(store);
        let mut s = db
            .prepare("SELECT g.id,(SELECT d.id FROM dir d JOIN root r ON r.id=d.root_id WHERE r.grant_id=g.id AND d.parent_id IS NULL) FROM st.root_grant g WHERE g.state='active'")
            .unwrap();
        s.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
    };
    for (grant, dir) in actives {
        let Some(dir) = dir else {
            bad.push(format!("active grant {grant} has no root directory"));
            continue;
        };
        let probe = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let run = begin(c.writer(), grant, "full");
            listing(
                c.writer(),
                run,
                dir,
                vec![ident("probe.zzqx", 99, 1)],
                vec![],
            );
            end(c.writer(), run);
        }));
        if probe.is_err() {
            bad.push(format!("write probe on grant {grant} failed"));
        }
    }
    drop(c);
    bad.extend(
        structural(store)
            .into_iter()
            .map(|b| format!("after the write probe: {b}")),
    );
    bad
}

fn sweep_all() -> bool {
    std::env::var("LOOMWARD_CRASH_SWEEP").is_ok_and(|v| v == "full")
}
/// Boundaries to kill at: all of them in `full` mode; otherwise every one that is not a bare row
/// change, plus every `stride`th row change (a row change between two commits is in the same
/// transaction as its neighbours, so the sampled ones still exercise each statement kind).
fn targets(trace: &BTreeMap<u64, String>, stride: usize) -> Vec<u64> {
    let mut rows = 0;
    trace
        .iter()
        .filter(|(_, label)| {
            if sweep_all() || !label.starts_with("row ") {
                return true;
            }
            rows += 1;
            rows % stride == 0
        })
        .map(|(n, _)| *n)
        .collect()
}

fn sweep(scenario: &str, stride: usize) {
    let root = tempfile::tempdir().unwrap();
    let base = run_child(scenario, &root.path().join("clean"), None);
    assert!(
        !base.crashed && base.boundaries > 0,
        "{scenario}: clean run"
    );
    let bad = verify(
        &root.path().join("clean").join("store"),
        &base.acks,
        scenario,
    );
    assert!(
        bad.is_empty(),
        "{scenario}: the clean run violates {bad:#?}"
    );
    if scenario == "lifecycle" {
        assert_eq!(base.acks.last().map(String::as_str), Some("closed"));
    }
    let todo = targets(&base.trace, stride);
    eprintln!(
        "crash sweep {scenario}: {} boundaries, killing at {} of them",
        base.boundaries,
        todo.len()
    );
    let failures = Mutex::new(Vec::<String>::new());
    parallel(todo, |n| {
        let work = root.path().join(format!("n{n}"));
        let out = run_child(scenario, &work, Some(n));
        let label = base.trace.get(&n).cloned().unwrap_or_default();
        let bad = verify(&work.join("store"), &out.acks, scenario);
        if !bad.is_empty() {
            failures.lock().unwrap().push(format!(
                "N={n} ({label}; crashed={}; last ack {:?}): {bad:#?}
  repro: LOOMWARD_CRASH_SWEEP=full cargo test -p loomward-catalog --features test-hooks --test crash_sweep -- {scenario}_at {n}",
                out.crashed,
                out.acks.last()
            ));
        }
        let _ = std::fs::remove_dir_all(&work);
    });
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

fn copy_store(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
    }
}
/// Runs `f` over `items` on a few threads.
fn parallel<T: Send>(items: Vec<T>, f: impl Fn(T) + Sync) {
    let queue = Mutex::new(items);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                while let Some(item) = {
                    let next = queue.lock().unwrap().pop();
                    next
                } {
                    f(item);
                }
            });
        }
    });
}

/// A second crash inside the recovery of the first: kill the lifecycle at N, then kill the reopen
/// that repairs it at every one of its own boundaries M, and require the third open to recover.
/// `step`: the first-crash boundaries sampled (every `step`th; all of them in `full` mode).
fn sweep_recovery(step: usize) {
    let root = tempfile::tempdir().unwrap();
    let base = run_child("lifecycle", &root.path().join("clean"), None);
    let firsts: Vec<u64> = (1..=base.boundaries)
        .filter(|n| sweep_all() || (*n as usize).is_multiple_of(step))
        .collect();
    let failures = Mutex::new(Vec::<String>::new());
    // (N, acks of the first crash, boundaries of the recovery)
    let crashed = Mutex::new(Vec::new());
    parallel(firsts, |n| {
        let work = root.path().join(format!("first{n}"));
        let first = run_child("lifecycle", &work, Some(n));
        crashed.lock().unwrap().push((n, first.acks, work));
    });
    let mut tasks = Vec::new();
    for (n, acks, work) in crashed.into_inner().unwrap() {
        // Recovery changes the store: count its boundaries on a copy, kill other copies.
        let counted = root.path().join(format!("count{n}"));
        copy_store(&work.join("store"), &counted.join("store"));
        let recovery = run_child("recover", &counted, None).boundaries;
        for m in 1..=recovery {
            tasks.push((n, m, acks.clone(), work.clone()));
        }
    }
    eprintln!("crash sweep recovery: {} double crashes", tasks.len());
    parallel(tasks, |(n, m, acks, work)| {
        let second = root.path().join(format!("first{n}-second{m}"));
        copy_store(&work.join("store"), &second.join("store"));
        let out = run_child("recover", &second, Some(m));
        let bad = verify(&second.join("store"), &acks, "lifecycle");
        if !bad.is_empty() {
            failures.lock().unwrap().push(format!(
                "N={n} then M={m} (crashed={}): {bad:#?}",
                out.crashed
            ));
        }
        let _ = std::fs::remove_dir_all(&second);
    });
    let failures = failures.into_inner().unwrap();
    assert!(
        failures.is_empty(),
        "{} double crashes break recovery
{}",
        failures.len(),
        failures.join(
            "
"
        )
    );
}

#[test]
fn a_second_crash_during_recovery_still_recovers() {
    sweep_recovery(20);
}

#[test]
fn every_write_of_scan_revoke_and_forget_recovers() {
    sweep("lifecycle", 4);
}
#[test]
fn every_write_of_a_schema_migration_recovers() {
    sweep("migrate", 1);
}

/// A single boundary, for reproducing a failure: `-- lifecycle_at 123`.
#[test]
fn lifecycle_at() {
    single("lifecycle");
}
#[test]
fn migrate_at() {
    single("migrate");
}
fn single(scenario: &str) {
    let Some(n) = std::env::args()
        .next_back()
        .and_then(|a| a.parse::<u64>().ok())
    else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let out = run_child(scenario, &root.path().join("one"), Some(n));
    let bad = verify(&root.path().join("one").join("store"), &out.acks, scenario);
    assert!(bad.is_empty(), "N={n} {:?}: {bad:#?}", out.trace.get(&n));
}

/// The checker must be able to fail: damage a recovered store each way and expect a violation.
#[test]
fn the_verifier_catches_planted_damage() {
    let root = tempfile::tempdir().unwrap();
    let out = run_child("lifecycle", &root.path().join("clean"), None);
    let store = root.path().join("clean").join("store");
    let clean = verify(&store, &out.acks, "lifecycle");
    assert!(clean.is_empty(), "{clean:#?}");
    for (what, sql) in [
        ("a deleted acknowledged file", "DELETE FROM file WHERE name='keepB1.yyqx'"),
        ("a leftover running scan", "INSERT INTO scan_run(root_id,mode,strategy,state,started_at_ns,counters_json,budget_json) SELECT id,'full','x','running',0,'{}','{}' FROM root LIMIT 1"),
        ("a bulk row left behind", "INSERT INTO bulk_dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run) SELECT id,NULL,'x','path_observation','none',0,0,'unlisted',0,0 FROM root LIMIT 1"),
        ("a forgotten grant that kept its root", "PRAGMA foreign_keys=OFF; INSERT INTO st.root_forgotten(grant_id,root_number,forgotten_at_ns) VALUES(2,1,0)"),
    ] {
        let copy = root.path().join(format!("copy-{}", what.len()));
        std::fs::create_dir_all(&copy).unwrap();
        for f in ["state.db", "catalog.db"] {
            std::fs::copy(store.join(f), copy.join(f)).unwrap();
        }
        let db = reader(&copy);
        db.execute_batch(sql).unwrap();
        drop(db);
        assert!(
            !verify(&copy, &out.acks, "lifecycle").is_empty(),
            "undetected: {what}"
        );
    }
}

/// A root published through staged chunks (what the service does) must leave none of its names in
/// either store file after a forget (found by this sweep as F1; #391).
#[test]
fn forget_leaves_no_staged_listing_names() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path(), "synthetic").unwrap();
    let w = c.writer();
    let (grant, root) = register(w, A_NAME, A_PATH, 1);
    let run = begin(w, grant, "full");
    w.call(WriteCommand::StageChunk {
        run_id: run,
        dir_id: root,
        seq: 0,
        files: vec![ident(A_ROOT_FILES[0], 11, 100)],
        dirs: vec![],
    })
    .unwrap();
    w.call(WriteCommand::ListingDone {
        run_id: run,
        dir_id: root,
        outcome: ListingOutcome::Complete,
        skipped: 0,
        errors: 0,
    })
    .unwrap();
    end(w, run);
    w.call(WriteCommand::RevokeGrant {
        grant_id: grant,
        revoked_at_ns: 10,
    })
    .unwrap();
    w.call(WriteCommand::ForgetGrant {
        grant_id: grant,
        forgotten_at_ns: 20,
        expected_state_rev: None,
    })
    .unwrap();
    drop(c);
    assert!(
        !on_disk(dir.path(), "crashrootA"),
        "a staged name survived the forget"
    );
}

/// The sweep's guard: a crash point past the last boundary is never reached, and must fail loudly
/// instead of passing as a clean recovery.
#[test]
#[should_panic(expected = "the crash point was not reached")]
fn an_unreached_crash_point_fails_the_sweep() {
    let root = tempfile::tempdir().unwrap();
    let base = run_child("lifecycle", &root.path().join("clean"), None);
    run_child(
        "lifecycle",
        &root.path().join("past"),
        Some(base.boundaries + 1),
    );
}
