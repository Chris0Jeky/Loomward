//! Windows fresh-process probe for #937; never emits catalogue content.
//! Supply LOOMWARD_COLD_PROBE_DIR (a disposable copy), LOOMWARD_COLD_PROBE_DATASET and
//! optionally LOOMWARD_COLD_PROBE_TERM, then run `cargo test -p loomward-catalog --release
//! --lib cold_first_search -- --ignored --nocapture` once per process/cache condition.
//! LOOMWARD_COLD_PROBE_PREPARE splits FTS prepare; LOOMWARD_COLD_PROBE_READER_ONLY bypasses
//! open's integrity-check warm-up. The caller establishes OS cache state before each process.
#![allow(unsafe_code)] // Read-only SQLite counters and Windows process CPU accounting in this ignored probe.
use super::*;
use rusqlite::trace::{TraceEvent, TraceEventCodes};
use std::cell::RefCell;
use std::time::Instant;

thread_local! {
    static PROFILE: RefCell<Vec<(&'static str, f64, i32, i32)>> = const { RefCell::new(Vec::new()) };
}

fn profile(event: TraceEvent<'_>) {
    if let TraceEvent::Profile(stmt, duration) = event {
        let sql = stmt.sql();
        // Fixed categories only: never expanded SQL, bound values, names or paths.
        let class = if sql.contains("FROM dir d JOIN root_grant") {
            "revisions"
        } else if sql.contains("FROM root r JOIN root_grant") {
            "root_states"
        } else if sql.contains("sum(sub_dirs)") {
            "scope_totals"
        } else if sql.contains("MATCH") {
            "fts_pass"
        } else if sql.contains("search_name_data") {
            "fts_data"
        } else if sql.contains("WITH RECURSIVE a") {
            "ancestry"
        } else {
            "other"
        };
        let ms = duration.as_secs_f64() * 1000.0;
        PROFILE.with(|p| {
            p.borrow_mut().push((
                class,
                ms,
                stmt.get_status(rusqlite::StatementStatus::VmStep),
                stmt.get_status(rusqlite::StatementStatus::FullscanStep),
            ))
        });
    }
}

fn cache(conn: &Connection) -> (i32, i32, i32) {
    let get = |op| {
        let (mut now, mut high) = (0, 0);
        // Read counters only; the live connection owns the SQLite handle.
        assert_eq!(
            unsafe { rusqlite::ffi::sqlite3_db_status(conn.handle(), op, &mut now, &mut high, 0) },
            0
        );
        now
    };
    (
        get(rusqlite::ffi::SQLITE_DBSTATUS_CACHE_HIT),
        get(rusqlite::ffi::SQLITE_DBSTATUS_CACHE_MISS),
        get(rusqlite::ffi::SQLITE_DBSTATUS_CACHE_USED),
    )
}

fn cpu_ms() -> f64 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn GetProcessTimes(
            p: *mut std::ffi::c_void,
            c: *mut u64,
            e: *mut u64,
            k: *mut u64,
            u: *mut u64,
        ) -> i32;
    }
    let (mut c, mut e, mut k, mut u) = (0, 0, 0, 0);
    assert_ne!(
        unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) },
        0
    );
    (k + u) as f64 / 10_000.0
}

#[test]
#[ignore = "#937: requires an explicitly supplied disposable store copy; aggregate output only"]
fn cold_first_search() {
    let dir = std::path::PathBuf::from(
        std::env::var_os("LOOMWARD_COLD_PROBE_DIR").expect("supply disposable copy"),
    );
    let dataset = std::env::var("LOOMWARD_COLD_PROBE_DATASET").unwrap_or("synthetic".into());
    let term = std::env::var("LOOMWARD_COLD_PROBE_TERM").unwrap_or("report".into());
    assert!(["report", "jpg", "img", "backup", "zzqqxx"].contains(&term.as_str()));
    let start = Instant::now();
    let cpu = cpu_ms();
    // Reader-only control keeps the open-time integrity check from warming the OS cache.
    let reader_only = std::env::var_os("LOOMWARD_COLD_PROBE_READER_ONLY").is_some();
    let catalog = (!reader_only).then(|| {
        Catalog::open(&dir, &dataset).unwrap_or_else(|_| panic!("open failed (details suppressed)"))
    });
    let open_ms = (!reader_only).then(|| start.elapsed().as_secs_f64() * 1000.0);
    let open_cpu_ms = (!reader_only).then(|| cpu_ms() - cpu);
    let start = Instant::now();
    let mut reader = match &catalog {
        Some(c) => c.reader(),
        None => Reader::open(&dir),
    }
    .unwrap()
    .without_repairs();
    let reader_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let scope: Vec<i64> = reader
        .conn
        .prepare("SELECT id FROM root ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let scope_ms = start.elapsed().as_secs_f64() * 1000.0;
    let req = SearchRequest {
        text: term.clone(),
        kind: "any".into(),
        scope: Some(scope.clone()),
        limit: 20,
        work_budget: 50_000_000,
        time_budget: Some(std::time::Duration::from_millis(250)),
        ..Default::default()
    };
    // Optional split isolates schema/FTS preparation; default leaves the first search untouched.
    let mut prepare_ms = None;
    if std::env::var_os("LOOMWARD_COLD_PROBE_PREPARE").is_some() {
        let start = Instant::now();
        reader
            .conn
            .prepare(&statement(false, false, false, Some(&scope), false))
            .unwrap();
        prepare_ms = Some(start.elapsed().as_secs_f64() * 1000.0);
    }
    reader
        .conn
        .trace_v2(TraceEventCodes::SQLITE_TRACE_PROFILE, Some(profile));
    let mut searches = Vec::new();
    for _ in 0..2 {
        PROFILE.with(|p| p.borrow_mut().clear());
        let before = cache(&reader.conn);
        let cpu = cpu_ms();
        let start = Instant::now();
        let page = reader.search(&req).unwrap_or_else(|e| {
            match e {
                Error::Invalid(reason) => eprintln!("PROBE_INVALID {reason}"),
                Error::Sql(e) => eprintln!("PROBE_SQL {:?}", e.sqlite_error_code()),
                _ => eprintln!("PROBE_ERROR {:?}", std::mem::discriminant(&e)),
            };
            PROFILE.with(|p| eprintln!("PROBE_PROFILE {:?}", p.borrow()));
            panic!("search failed (details suppressed)")
        });
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let cpu_ms = cpu_ms() - cpu;
        let after = cache(&reader.conn);
        let profile = PROFILE.with(|p| p.borrow().clone());
        searches.push(serde_json::json!({"ms": ms, "cpu_ms": cpu_ms, "hits": page.hits.len(), "budget_hit": page.budget_hit,
            "cache_hits": after.0-before.0, "cache_misses": after.1-before.1, "cache_bytes": after.2, "profile": profile}));
    }
    reader.conn.trace_v2(TraceEventCodes::empty(), None);
    let mut plans = Vec::new();
    for sql in [ROOT_STATES_SQL.to_string(), "SELECT d.root_id,r.generation FROM dir d JOIN root_grant g ON g.id=d.root_id JOIN root r ON r.id=d.root_id WHERE d.parent_id IS NULL AND g.state='active' AND d.listing_state!='absent_pending' AND (?1 IS NULL OR d.root_id=?1) ORDER BY d.root_id LIMIT 65".to_string(), statement(false, false, false, Some(&scope), false), statement(false, true, false, Some(&scope), false)] {
        let mut stmt = reader.conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let params = vec![rusqlite::types::Value::Null; stmt.parameter_count()];
        let plan: Vec<String> = stmt.query_map(rusqlite::params_from_iter(params), |r| r.get(3)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        plans.push(plan);
    }
    let counts = reader.entry_counts().unwrap();
    let pages: i64 = reader
        .conn
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    println!(
        "PROBE {}",
        serde_json::json!({"term": term, "open_ms": open_ms, "open_cpu_ms": open_cpu_ms, "reader_ms": reader_ms,
        "scope_ms": scope_ms, "prepare_ms": prepare_ms, "roots": scope.len(), "files": counts.0, "dirs": counts.1, "pages": pages,
        "searches": searches, "plans": plans})
    );
}
