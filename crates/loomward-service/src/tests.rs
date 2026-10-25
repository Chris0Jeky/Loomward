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

#[test]
fn hello_reports_duplicate_content_reads_separately_from_effects() {
    let dir = temp_root();
    for dataset in [DatasetClass::Synthetic, DatasetClass::Personal] {
        let state = dir.path().join(format!("{dataset:?}"));
        let svc = open(&state, dataset);
        let (hello, _) = ok("session.hello", call(&svc, "session.hello", json!({})));
        assert_eq!(
            hello["capabilities"]["observation"]["duplicate_content_read"],
            cfg!(windows)
        );
        assert!(hello["capabilities"]["effects"]
            .as_object()
            .unwrap()
            .values()
            .all(|v| v == false));
    }
}

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
    pub fail_job_list: std::sync::atomic::AtomicBool,
    pub before_generation_read: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    pub closed: Mutex<Vec<(Command, &'static str)>>,
    pub arrived: Mutex<Vec<(Command, &'static str)>>,
    pub opened: std::sync::Condvar,
    /// #369: attempt a weak upgrade after Drop's final ownership check, before releasing Inner.
    pub after_drop_check: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// How far ahead of the wall clock the collection page loop reads time (`Inner::past`).
    pub skew: Mutex<Duration>,
    /// How far ahead of the wall clock a read's receiver reads time once it starts waiting.
    pub read_ahead: Mutex<Duration>,
    /// #758: a manually advanced service read clock; None keeps production timing.
    pub read_clock: Mutex<Option<Instant>>,
    /// (profile, profiles directory) for the scan-time fence instead of the OS folders (#312).
    pub profile: Mutex<Option<(std::path::PathBuf, std::path::PathBuf)>>,
    /// #219: the filesystem name grant-time probes observe instead of the volume's.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub filesystem: Mutex<Option<&'static str>>,
    /// The error the old-path probe of a renamed unverified grant meets instead of opening it.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub probe_error: Mutex<Option<loomward_windows::enumerate::SourceError>>,
    /// #519 review: a personal space read for one volume key answers this instead of the OS.
    pub space: Mutex<Vec<(String, SpaceFake)>>,
    /// #530: the personal space read budget a test sets instead of the production 500 ms.
    pub personal_space_wait: Mutex<Option<Duration>>,
    /// #569: the next explicit scan.start that has just cancelled an automatic run fails on retry.
    pub fail_explicit_retry: std::sync::atomic::AtomicBool,
    #[cfg(windows)]
    pub fail_automatic_start: std::sync::atomic::AtomicBool,
    /// One-shot: the next automatic start is refused by the resource budget (refunded, not failed).
    #[cfg(windows)]
    pub refuse_automatic_budget: std::sync::atomic::AtomicBool,
    #[cfg(windows)]
    pub fail_watch_start: std::sync::atomic::AtomicBool,
    #[cfg(windows)]
    pub fail_freshness_checkpoint: std::sync::atomic::AtomicBool,
    #[cfg(windows)]
    pub busy_freshness_checkpoint: std::sync::atomic::AtomicBool,
    #[cfg(windows)]
    pub before_freshness_finish: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

/// What a personal space read answers in tests: wait on an explicit gate, or report fixed space.
/// Runs on the worker thread, as the OS read does.
#[derive(Clone, Copy)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum SpaceFake {
    Stall,
    Fixed { capacity: u64, free: u64 },
}

impl SpaceFake {
    pub(crate) fn answer(self, hooks: &Hooks) -> Option<(loomward_windows::volumes::Space, i64)> {
        match self {
            Self::Stall => {
                hooks.gate(Command::VolumesList, "space_read");
                None
            }
            Self::Fixed { capacity, free } => Some((
                loomward_windows::volumes::Space {
                    capacity_bytes: capacity.to_string(),
                    free_bytes: free.to_string(),
                    read_only: Some(false),
                    cluster_bytes: None,
                },
                crate::now_ns(),
            )),
        }
    }
}

impl Hooks {
    pub fn receive_read<T>(
        &self,
        rx: &std::sync::mpsc::Receiver<T>,
        deadline: Instant,
        now: Instant,
    ) -> Result<T, std::sync::mpsc::RecvTimeoutError> {
        if self.read_clock.lock().unwrap().is_none() {
            return rx.recv_timeout(deadline.saturating_duration_since(now));
        }
        let safety = Instant::now() + Duration::from_secs(60);
        loop {
            let now = self.read_clock.lock().unwrap().expect("read clock removed");
            let left = deadline.saturating_duration_since(now);
            match rx.recv_timeout(left.min(Duration::from_millis(10))) {
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if !left.is_zero() => {
                    assert!(Instant::now() < safety, "read worker never answered");
                }
                reply => return reply,
            }
        }
    }

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
        // These gates also run from Drop: preserve the first test panic after poisoning.
        self.arrived
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((c, at));
        let mut closed = self.closed.lock().unwrap_or_else(|e| e.into_inner());
        while closed.contains(&(c, at)) {
            closed = self.opened.wait(closed).unwrap_or_else(|e| e.into_inner());
        }
    }

    pub fn open(&self, c: Command, at: &'static str) {
        self.closed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|g| *g != (c, at));
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
        allow_identity_unverified: false,
        grant_roots: vec![],
    })
    .unwrap()
}

#[test]
fn normal_start_checks_each_database_once() {
    let dir = temp_root();
    let dist = dir.path().join("dist");
    std::fs::create_dir(&dist).unwrap();
    for dataset in [DatasetClass::Synthetic, DatasetClass::Personal] {
        let state = dir.path().join(format!("{dataset:?}"));
        drop(open(&state, dataset));
        for static_dir in [None, Some(dist.as_path())] {
            loomward_catalog::take_integrity_checks();
            let service = Service::open_with_static(
                Config {
                    state_dir: state.clone(),
                    dataset,
                    allow_personal: true,
                    allow_identity_unverified: false,
                    grant_roots: vec![],
                },
                static_dir,
            )
            .unwrap();
            let checks = loomward_catalog::take_integrity_checks();
            for name in ["catalog.db", "state.db"] {
                assert_eq!(
                    checks.iter().filter(|path| path.ends_with(name)).count(),
                    1,
                    "{dataset:?}: {name}: {checks:?}"
                );
            }
            drop(service);
        }
    }
}

fn damage_data_page(path: &Path) -> Vec<u8> {
    // Leave the header and schema readable: only the full integrity check sees this damage.
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch("CREATE TABLE integrity_probe(value TEXT); INSERT INTO integrity_probe VALUES('synthetic'); PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE").unwrap();
    let page: u32 = conn
        .query_row(
            "SELECT rootpage FROM sqlite_schema WHERE name='integrity_probe'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let page_size: u32 = conn
        .pragma_query_value(None, "page_size", |r| r.get(0))
        .unwrap();
    drop(conn);
    let mut corrupt = std::fs::read(path).unwrap();
    corrupt[((page - 1) * page_size) as usize] = 0;
    std::fs::write(path, &corrupt).unwrap();
    corrupt
}

#[test]
fn corrupt_catalogue_is_preserved_and_rebuilt_at_normal_start() {
    let dir = temp_root();
    drop(open(dir.path(), DatasetClass::Synthetic));
    let corrupt = damage_data_page(&dir.path().join("catalog.db"));
    loomward_catalog::take_integrity_checks();
    drop(open(dir.path(), DatasetClass::Synthetic));
    assert_eq!(
        loomward_catalog::take_integrity_checks()
            .iter()
            .filter(|path| path.ends_with("catalog.db"))
            .count(),
        1
    );
    let preserved = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("catalog.corrupt-")
        })
        .expect("corrupt derived catalogue must be preserved");
    assert_eq!(std::fs::read(preserved).unwrap(), corrupt);
}

#[test]
fn state_integrity_failure_still_precedes_static_overlap() {
    let dir = temp_root();
    drop(open(dir.path(), DatasetClass::Synthetic));
    let corrupt = damage_data_page(&dir.path().join("state.db"));
    let error = Service::open_with_static(
        Config {
            state_dir: dir.path().to_path_buf(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![],
        },
        Some(dir.path()),
    )
    .err()
    .expect("corruption must refuse startup");
    assert_eq!(error, "state store: CorruptDatabase");
    assert_eq!(std::fs::read(dir.path().join("state.db")).unwrap(), corrupt);
}

#[test]
fn corrupt_catalogue_is_refused_at_static_start_without_rewriting() {
    let dir = temp_root();
    let state = dir.path().join("state");
    drop(open(&state, DatasetClass::Synthetic));
    let corrupt = b"synthetic corrupt catalogue";
    std::fs::write(state.join("catalog.db"), corrupt).unwrap();
    let dist = dir.path().join("dist");
    std::fs::create_dir(&dist).unwrap();
    let error = Service::open_with_static(
        Config {
            state_dir: state.clone(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![],
        },
        Some(&dist),
    )
    .err()
    .expect("corrupt catalogue must refuse startup");
    assert_eq!(
        error,
        "cannot inspect stored grants: catalogue version unavailable"
    );
    assert_eq!(std::fs::read(state.join("catalog.db")).unwrap(), corrupt);
}

pub(crate) fn temp_root() -> tempfile::TempDir {
    tempfile::tempdir_in(crate::paths::test_roots_base()).unwrap()
}

#[test]
fn an_injected_read_clock_expires_the_receiver_and_catalogue_open() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let deadline = Instant::now() + Duration::from_secs(3);
    *svc.inner.hooks.read_clock.lock().unwrap() = Some(deadline + Duration::from_secs(1));
    let (_tx, rx) = std::sync::mpsc::channel::<()>();
    assert_eq!(
        svc.inner.hooks.receive_read(&rx, deadline, Instant::now()),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    );
    let work = crate::ReadWork {
        deadline,
        interrupt: Mutex::new(None),
    };
    assert_eq!(
        svc.inner.read_reader(Some(&work)).err().unwrap().code,
        ErrorCode::DeadlineExceeded
    );
}

/// #730: a failed assertion holding a test gate must not panic again during cleanup.
#[test]
fn poisoned_test_gates_preserve_the_original_panic() {
    const CHILD: &str = "LOOMWARD_730_POISONED_GATE";
    if let Some(gate) = std::env::var_os(CHILD) {
        let tmp = temp_root();
        let svc = open(tmp.path(), DatasetClass::Synthetic);
        struct Release(Arc<Hooks>);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.open(Command::VolumesList, "space_read");
            }
        }
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let svc = svc;
            let _release = Release(svc.inner.hooks.clone());
            let _lock = if gate == "arrived" {
                svc.inner.hooks.arrived.lock().unwrap()
            } else {
                svc.inner.hooks.closed.lock().unwrap()
            };
            panic!("original test failure");
        }));
        let payload = failure.expect_err("the original panic must still be reported");
        assert_eq!(
            payload.downcast_ref::<&str>(),
            Some(&"original test failure")
        );
        return;
    }
    for gate in ["arrived", "closed"] {
        // A second panic aborts the process, so catch_unwind alone cannot test this regression.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::poisoned_test_gates_preserve_the_original_panic",
                "--nocapture",
            ])
            .env(CHILD, gate)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{gate}: cleanup aborted: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        // A renamed test would match nothing and exit 0 while proving nothing.
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{gate}: the child ran no test"
        );
    }
}

/// Fixtures stay outside the source tree and inside the profile fence wherever the checkout is
/// (#465: a `%TEMP%` worktree sits under the protected `AppData` store).
#[test]
fn followup_test_roots_are_outside_source() {
    let tmp = temp_root();
    let path = tmp.path().canonicalize().unwrap();
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    assert!(!path.starts_with(crates.canonicalize().unwrap()));
    assert!(path.starts_with(crate::paths::test_roots_base()));
    assert_eq!(crate::paths::check_profile(&path), Ok(()));
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
            allow_identity_unverified: false,
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

/// #390: SQLite opens `state.db` by its plain drive path, never `\\?\` (which its Windows VFS takes
/// for UNC and then leaks shared WAL read locks), while the grant guards keep comparing the
/// canonical state directory.
#[test]
fn state_store_opens_by_plain_path_and_guards_keep_the_canonical_one() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    assert_eq!(svc.inner.state_dir, tmp.path().canonicalize().unwrap());
    let db = svc.inner.db();
    for schema in ["main", "st"] {
        let file: String = db
            .query_row(
                "SELECT file FROM pragma_database_list WHERE name=?1",
                [schema],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !file.is_empty() && !file.starts_with(r"\\"),
            "{schema}: {file}"
        );
    }
}

#[test]
fn health_job_list_failure_is_an_error_not_idle() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (health, _) = ok("health.get", call(&svc, "health.get", json!({})));
    assert_eq!(health["jobs_running"], 0);
    svc.inner
        .hooks
        .fail_job_list
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let error = err(call(&svc, "health.get", json!({})));
    assert_eq!(error.code, ErrorCode::InternalError);
    assert_eq!(error.message.as_str(), "synthetic job list failure");
}

#[test]
fn health_counts_active_jobs_beyond_the_history_page() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (release, held) = std::sync::mpsc::channel();
    let held = Mutex::new(held);
    svc.inner.engine.register_job_runner(
        JobKind::Scan,
        Arc::new(move |_, _| {
            held.lock().unwrap().recv().unwrap();
            Ok(CoverageState::Complete)
        }),
    );
    let active = svc
        .inner
        .engine
        .job_submit(loomward_engine::jobs::JobSpec::new(
            JobKind::Scan,
            Some(RootId::new("rt_fixture").unwrap()),
        ))
        .unwrap();
    svc.inner.engine.register_job_runner(
        JobKind::Teacher,
        Arc::new(|_, _| Ok(CoverageState::Complete)),
    );
    for _ in 0..50 {
        let job = svc
            .inner
            .engine
            .job_submit(loomward_engine::jobs::JobSpec::new(JobKind::Teacher, None))
            .unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while !matches!(
            svc.inner.engine.job_status(&job.job_id).unwrap().state,
            JobState::Completed
        ) {
            assert!(Instant::now() < until, "synthetic job did not finish");
            std::thread::yield_now();
        }
    }
    let (history, _) = ok("jobs.list", call(&svc, "jobs.list", json!({"limit":50})));
    assert_eq!(history["jobs"].as_array().unwrap().len(), 50);
    assert!(history["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|j| j["job_id"] != json!(active.job_id)));
    let (health, _) = ok("health.get", call(&svc, "health.get", json!({})));
    // Release before asserting, so even a red regression never strands its runner.
    svc.inner.engine.job_cancel(&active.job_id).unwrap();
    let (cancelling, _) = ok("health.get", call(&svc, "health.get", json!({})));
    release.send(()).unwrap();
    assert_eq!(health["jobs_running"], 1);
    assert_eq!(cancelling["jobs_running"], 1);
    let until = Instant::now() + Duration::from_secs(10);
    while svc.inner.engine.job_status(&active.job_id).unwrap().state != JobState::Cancelled {
        assert!(Instant::now() < until, "synthetic scan did not finish");
        std::thread::yield_now();
    }
    let (idle, _) = ok("health.get", call(&svc, "health.get", json!({})));
    assert_eq!(idle["jobs_running"], 0);
}

fn generation_read_race(command: &str) {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, _) = fixture(&svc, 2);
    let slice = root_slice(&svc, root);
    let node = slice["nodes"][0]["node_id"].clone();
    let payload = match command {
        "tree.slice" => {
            json!({"anchor":{"kind":"root","root_id":svc.inner.root_id(root)},"depth":2,"max_nodes":64,"min_share":0.0,"basis":"logical","include_files":true})
        }
        "tree.children" => {
            json!({"node_id":node,"sort":"name_asc","basis":"logical","limit":50,"cursor":null})
        }
        "stats.breakdown" => json!({"node_id":node,"by":"extension","basis":"logical","limit":10}),
        _ => json!({"node_id":node}),
    };
    let generation = svc
        .inner
        .db()
        .query_row("SELECT generation FROM root WHERE id=?1", [root], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap() as u64;
    let mut req = request(command, payload);
    req.expected_generation = Some(Generation(generation));
    ok(command, svc.call(req.clone(), &CallContext::http()));
    let path = loomward_catalog::sqlite_dir(&svc.inner.state_dir).join("catalog.db");
    *svc.inner.hooks.before_generation_read.lock().unwrap() = Some(Box::new(move || {
        rusqlite::Connection::open(path)
            .unwrap()
            .execute(
                "UPDATE root SET generation=generation+1 WHERE id=?1",
                [root],
            )
            .unwrap();
    }));
    let error = err(svc.call(req.clone(), &CallContext::http()));
    assert_eq!(error.code, ErrorCode::StaleGeneration, "{command}");
    assert_eq!(reason(&error), Some("generation_changed"));
    req.expected_generation = Some(Generation(generation + 1));
    ok(command, svc.call(req, &CallContext::http()));
}

#[test]
fn generation_race_node_inspect() {
    generation_read_race("node.inspect");
}
#[test]
fn generation_race_tree_path() {
    generation_read_race("tree.path");
}
#[test]
fn generation_race_tree_slice() {
    generation_read_race("tree.slice");
}
#[test]
fn generation_race_tree_children() {
    generation_read_race("tree.children");
}
#[test]
fn generation_race_breakdown() {
    generation_read_race("stats.breakdown");
}

#[test]
fn roots_list_keeps_a_root_mid_scan_with_unknown_totals() {
    // Every scan dirties its root's aggregates; the list must stay available with an
    // honest unknown for that root, not fail for every root (review of #358).
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = fixture(&svc, 2);
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    assert!(!roots["roots"][0]["totals"].is_null());
    svc.inner
        .catalog
        .writer()
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "full".into(),
            strategy: "fixture".into(),
            started_at_ns: 0,
        })
        .unwrap();
    svc.inner
        .db()
        .execute(
            "UPDATE main.dir SET dirty_rev=agg_valid_rev+1 WHERE root_id=?1 AND parent_id IS NULL",
            [root],
        )
        .unwrap();
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    assert!(roots["roots"][0]["totals"].is_null());
    assert_eq!(roots["roots"][0]["grant_state"], "active");
}

/// #722: several large roots cannot push `roots.list` past its deadline. Each root's unique
/// totals get a share of the budget; a root past its share reports them unknown, the stored
/// aggregates stay exact, and a small root is still measured exactly.
#[test]
fn roots_list_degrades_slow_roots_to_unknown_totals_inside_the_deadline() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let sizes = [200u64, 70_000, 70_000, 70_000];
    for (i, &files) in sizes.iter().enumerate() {
        let (root, grant) = register(&svc, i as u8 + 1);
        let dir: i64 = svc
            .inner
            .db()
            .query_row(
                "SELECT id FROM main.dir WHERE root_id=?1 AND parent_id IS NULL",
                [root],
                |r| r.get(0),
            )
            .unwrap();
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
        // Listings are capped, so each 1000 files sit in their own subdirectory.
        let chunks = files.div_ceil(1000);
        let subdirs = (0..chunks)
            .map(|d| {
                let mut o = Observation::file(format!("d{d}"), 0, Some(0));
                o.attrs = 0x10;
                o.file_id = Some(vec![0xD0; 15].into_iter().chain([d as u8 + 1]).collect());
                o.created_ft = Some(1);
                o
            })
            .collect();
        w.call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id: dir,
            files: vec![],
            dirs: subdirs,
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        }))
        .unwrap();
        let children: Vec<i64> = svc
            .inner
            .db()
            .prepare("SELECT id FROM (SELECT id,parent_id FROM main.dir UNION ALL SELECT id,parent_id FROM main.bulk_dir) WHERE parent_id=?1 ORDER BY id")
            .unwrap()
            .query_map([dir], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for (d, child) in children.into_iter().enumerate() {
            let listed = (d as u64 * 1000..files.min((d as u64 + 1) * 1000))
                .map(|n| {
                    let mut o = file(&format!("report-{n}.txt"), 1000);
                    o.file_id = Some(((n + 1) as u128).to_le_bytes().to_vec());
                    o
                })
                .collect();
            w.call(WriteCommand::DirListing(DirListing {
                run_id: run,
                dir_id: child,
                files: listed,
                dirs: vec![],
                state: "complete".into(),
                skipped: 0,
                errors: 0,
            }))
            .unwrap();
        }
        w.call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 1,
        })
        .unwrap();
    }
    // A 5% share of a one-second budget (about 50 ms for all four roots) is far below what the
    // big roots need on any host, so they are cut deterministically while the small one is not.
    let work = crate::ReadWork {
        deadline: Instant::now() + Duration::from_secs(1),
        interrupt: Mutex::new(None),
    };
    let started = Instant::now();
    let roots = svc.inner.root_list_sharing(Some(&work), 5).unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    let roots = roots["roots"].as_array().unwrap();
    assert_eq!(roots.len(), sizes.len());
    let mut cut = 0;
    for (root, files) in roots.iter().zip(sizes) {
        let totals = &root["totals"];
        assert_eq!(totals["files"], files, "stored aggregates stay exact");
        match totals["unique_objects"].as_u64() {
            Some(n) => assert_eq!(n, files),
            None => {
                cut += 1;
                assert!(totals["unique_allocated_bytes"].is_null());
            }
        }
    }
    assert!(cut >= 1, "at least one large root must exceed its share");
    assert_eq!(
        roots[0]["totals"]["unique_objects"], 200,
        "the small root is exact"
    );
}

fn placement_fixture(svc: &Service) -> (i64, i64, String) {
    let (root, grant) = fixture(svc, 3);
    svc.inner.db().execute("UPDATE main.volume SET capabilities_json=?1,bytes_per_cluster=4096", [json!({"space":{"capacity_bytes":"1048576","free_bytes":"65536","read_only":false}}).to_string()]).unwrap();
    let volume = svc.inner.volume_id(
        svc.inner
            .db()
            .query_row("SELECT volume_id FROM main.root WHERE id=?1", [root], |r| {
                r.get(0)
            })
            .unwrap(),
    );
    ok(
        "volumes.declare_tier",
        call(
            svc,
            "volumes.declare_tier",
            json!({"volume_id": volume, "tier": 0}),
        ),
    );
    (root, grant, volume)
}

fn placement_request(volume: &str, save: bool) -> Value {
    json!({"source_volume_id":volume,"target_free_bytes":"262144","max_transfer_bytes":"1048576","heat_policy":"unknown_is_ineligible","relief_policy":"verified_only","candidate_basis":"largest_dirs","max_groups":50,"overrides":[],"node_budget":1000,"save":save})
}

#[test]
fn volume_inventory_past_its_bound_is_refused_not_truncated() {
    // Codex P2 on #316: a 65th volume must surface as a resource bound, never vanish silently.
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    for i in 0..65 {
        svc.inner.db().execute("INSERT INTO main.volume(volume_key,display_name,identity_json,capabilities_json,device_json,online,observed_at_ns) VALUES(?1,'Synthetic volume','{}','{}','{}',1,0)", [format!("vk{i}")]).unwrap();
    }
    for command in ["volumes.list", "tiers.model"] {
        assert_eq!(
            err(call(&svc, command, json!({}))).code,
            ErrorCode::ResourceBudget,
            "{command}"
        );
    }
}

#[test]
fn placement_revoked_only_volumes_stay_visible_but_are_not_targets() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (_, _, volume) = placement_fixture(&svc);
    let WriteReply::Root {
        grant_id: grant,
        root_id,
        ..
    } = svc
        .inner
        .catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "revoked".into(),
            display_name: "Revoked target".into(),
            display_path: "Synthetic target".into(),
            root_file_id: Some(vec![2; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))
        .unwrap()
    else {
        panic!("root registration");
    };
    let conn = svc.inner.db();
    let target: i64 = conn
        .query_row(
            "SELECT volume_id FROM main.root WHERE id=?1",
            [root_id],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute("UPDATE main.volume SET capabilities_json=(SELECT capabilities_json FROM main.volume WHERE volume_key='fixture') WHERE id=?1", [target]).unwrap();
    conn.execute(
        "UPDATE st.root_grant SET state='revoked' WHERE id=?1",
        [grant],
    )
    .unwrap();
    let target_id = svc.inner.volume_id(target);
    conn.execute(
        "INSERT INTO st.tier_declaration(volume_key,tier,declared_at_ns) VALUES('revoked',1,0)",
        [],
    )
    .unwrap();
    drop(conn);
    for command in ["volumes.list", "tiers.model"] {
        let (result, _) = ok(command, call(&svc, command, json!({})));
        let row = result["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["volume_id"] == target_id)
            .unwrap();
        assert_eq!(row["online"], true);
        assert!(row["tier"]["note"]
            .as_str()
            .unwrap()
            .contains("No active grant"));
        if command == "tiers.model" {
            assert_eq!(row["source_eligible"], false);
            assert_eq!(result["volumes"][0]["source_eligible"], true);
        }
    }
    let (plan, _) = ok(
        "placement.simulate",
        call(&svc, "placement.simulate", placement_request(&volume, true)),
    );
    assert!(plan["excluded_volumes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["volume_id"] == target_id && v["reason"] == "no_active_grant"));
    assert!(plan["projected_free_bytes"].get(&target_id).is_none());
    // Candidate estimates disclose the same omission (Codex P2 on #350).
    let (candidates, _) = ok(
        "placement.candidates",
        call(
            &svc,
            "placement.candidates",
            json!({"source_volume_id":volume,"basis":"largest_dirs","max_groups":50,"min_bytes":"0"}),
        ),
    );
    assert!(candidates["excluded_volumes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["volume_id"] == target_id && v["reason"] == "no_active_grant"));
    let (detail, _) = ok(
        "proposals.get",
        call(
            &svc,
            "proposals.get",
            json!({"proposal_id":plan["proposal_id"]}),
        ),
    );
    assert_eq!(detail["plan"]["excluded_volumes"], plan["excluded_volumes"]);
    let refused = err(call(
        &svc,
        "placement.simulate",
        placement_request(&target_id, false),
    ));
    assert_eq!(refused.code, ErrorCode::InvalidRequest);
    assert_eq!(refused.detail.unwrap()["reason"], "no_active_grant");
    svc.inner
        .db()
        .execute(
            "UPDATE st.root_grant SET state='active' WHERE id=?1",
            [grant],
        )
        .unwrap();
    let (model, _) = ok("tiers.model", call(&svc, "tiers.model", json!({})));
    assert_eq!(model["volumes"][1]["source_eligible"], true);
    assert!(!model["volumes"][1]["tier"]["note"]
        .as_str()
        .unwrap()
        .contains("No active grant"));
    ok(
        "placement.simulate",
        call(
            &svc,
            "placement.simulate",
            placement_request(&target_id, false),
        ),
    );
}

#[test]
fn placement_physics_drift_alone_keeps_the_historical_scenario() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (_, _, volume) = placement_fixture(&svc);
    let (plan, _) = ok(
        "placement.simulate",
        call(&svc, "placement.simulate", placement_request(&volume, true)),
    );
    svc.inner.db().execute("UPDATE main.volume SET capabilities_json=?1,observed_at_ns=(SELECT max(created_at_ns)+1 FROM st.proposal)", [json!({"space":{"capacity_bytes":"2097152","free_bytes":"131072","read_only":false}}).to_string()]).unwrap();
    let (model, _) = ok("tiers.model", call(&svc, "tiers.model", json!({})));
    assert_eq!(model["volumes"][0]["free_bytes"], "131072");
    assert_eq!(model["volumes"][0]["capacity_bytes"], "2097152");
    let (detail, _) = ok(
        "proposals.get",
        call(
            &svc,
            "proposals.get",
            json!({"proposal_id":plan["proposal_id"]}),
        ),
    );
    assert_eq!(detail["summary"]["status"], "simulation_only");
    assert_eq!(detail["plan"], Value::Object(plan.clone()));
    let (listed, _) = ok("proposals.list", call(&svc, "proposals.list", json!({})));
    assert_eq!(listed["proposals"][0]["status"], "simulation_only");
    svc.inner
        .db()
        .execute(
            "UPDATE main.revision SET catalog_rev=catalog_rev+1 WHERE id=1",
            [],
        )
        .unwrap();
    let (detail, _) = ok(
        "proposals.get",
        call(
            &svc,
            "proposals.get",
            json!({"proposal_id":plan["proposal_id"]}),
        ),
    );
    assert_eq!(detail["summary"]["status"], "stale");
}

#[test]
fn placement_save_deduplicates_only_placement_kind() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (_, _, volume) = placement_fixture(&svc);
    let (plan, _) = ok(
        "placement.simulate",
        call(&svc, "placement.simulate", placement_request(&volume, true)),
    );
    {
        // Simulate a migrated/corrupted row of another kind; today's schema only permits placement.
        let conn = svc.inner.db();
        conn.pragma_update(None, "ignore_check_constraints", true)
            .unwrap();
        conn.execute("UPDATE st.proposal SET kind='other_simulation'", [])
            .unwrap();
        conn.pragma_update(None, "ignore_check_constraints", false)
            .unwrap();
    }
    let (next, _) = ok(
        "placement.simulate",
        call(&svc, "placement.simulate", placement_request(&volume, true)),
    );
    assert_ne!(next["proposal_id"], plan["proposal_id"]);
    let (again, _) = ok(
        "placement.simulate",
        call(&svc, "placement.simulate", placement_request(&volume, true)),
    );
    assert_eq!(again["proposal_id"], next["proposal_id"]);
}

#[cfg(windows)]
#[test]
fn placement_lab_space_timestamp_is_fresh() {
    let Some(lab) = lab() else {
        return;
    };
    lab.svc
        .inner
        .db()
        .execute("UPDATE main.volume SET observed_at_ns=0", [])
        .unwrap();
    let before = crate::now_ns();
    let (volumes, _) = ok("volumes.list", call(&lab.svc, "volumes.list", json!({})));
    let after = crate::now_ns();
    assert!(!volumes["volumes"][0]["free_bytes"].is_null());
    let observed: Timestamp =
        serde_json::from_value(volumes["volumes"][0]["observed_at"].clone()).unwrap();
    assert!(observed >= crate::ts_ns(before) && observed <= crate::ts_ns(after));
}

/// A personal session whose only owner grant is a real folder (#519). Returns the folder so a test
/// can change it; the caller keeps `tmp` alive for the grant's lifetime.
#[cfg(windows)]
fn personal_grant_session(tmp: &tempfile::TempDir) -> Option<(Service, std::path::PathBuf)> {
    if loomward_windows::enumerate::skip_elevated_test() {
        eprintln!("skipped: elevated token; the native space read refuses elevation (invariant 2)");
        return None;
    }
    let folder = tmp.path().join("owner-folder");
    std::fs::create_dir_all(&folder).unwrap();
    let folder = folder.canonicalize().unwrap();
    let svc = Service::open(Config {
        state_dir: tmp.path().join("state"),
        dataset: DatasetClass::Personal,
        allow_personal: true,
        allow_identity_unverified: false,
        grant_roots: vec![folder.clone()],
    })
    .unwrap();
    Some((svc, folder))
}

/// #519: a volume holding an active owner grant reports its observed capacity and free space.
#[cfg(windows)]
#[test]
fn volumes_list_observes_space_for_an_active_personal_grant() {
    let tmp = temp_root();
    let Some((svc, _folder)) = personal_grant_session(&tmp) else {
        return;
    };
    // This test requires an OS observation, so allow a generous read budget on a loaded host.
    *svc.inner.hooks.personal_space_wait.lock().unwrap() = Some(Duration::from_secs(30));
    let (volumes, _) = ok("volumes.list", call(&svc, "volumes.list", json!({})));
    let rows = volumes["volumes"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{:?}", volumes);
    let capacity: u64 = rows[0]["capacity_bytes"].as_str().unwrap().parse().unwrap();
    let free: u64 = rows[0]["free_bytes"].as_str().unwrap().parse().unwrap();
    assert!(capacity > 0 && free <= capacity, "{}", rows[0]);
}

/// #519: revoking the grant ends the observation; the volume reports unknown, never zero.
#[cfg(windows)]
#[test]
fn volumes_list_space_is_unknown_once_the_personal_grant_is_revoked() {
    let tmp = temp_root();
    let Some((svc, _folder)) = personal_grant_session(&tmp) else {
        return;
    };
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    let root_id = roots["roots"][0]["root_id"].clone();
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": root_id, "purge_catalog": false}),
        ),
    );
    let (volumes, _) = ok("volumes.list", call(&svc, "volumes.list", json!({})));
    assert!(volumes["volumes"][0]["capacity_bytes"].is_null());
    assert!(volumes["volumes"][0]["free_bytes"].is_null());
}

/// #519: a granted root that can no longer be observed stays unknown and the list still answers.
#[cfg(windows)]
#[test]
fn volumes_list_space_is_unknown_not_zero_when_the_personal_root_is_unobservable() {
    let tmp = temp_root();
    let Some((svc, folder)) = personal_grant_session(&tmp) else {
        return;
    };
    std::fs::remove_dir(&folder).unwrap();
    let (volumes, _) = ok("volumes.list", call(&svc, "volumes.list", json!({})));
    assert_eq!(volumes["volumes"][0]["capacity_bytes"], Value::Null);
    assert_eq!(volumes["volumes"][0]["free_bytes"], Value::Null);
}

/// #553: an unobservable first active root must not hide a second observable root on the same volume.
#[cfg(windows)]
#[test]
fn volumes_list_observes_space_through_a_second_root_when_the_first_is_unobservable() {
    let tmp = temp_root();
    let Some((svc, folder)) = personal_grant_session(&tmp) else {
        return;
    };
    *svc.inner.hooks.personal_space_wait.lock().unwrap() = Some(Duration::from_secs(30));
    let volume_key: String = svc
        .inner
        .db()
        .query_row("SELECT volume_key FROM main.volume", [], |r| r.get(0))
        .unwrap();
    // The second grant lives on the same real volume, so its identity matches the volume key.
    let second = tmp.path().join("second-folder");
    std::fs::create_dir_all(&second).unwrap();
    let (second_volume_key, second_file_id) = crate::lab::identity(&second).unwrap();
    assert_eq!(second_volume_key, volume_key);
    let WriteReply::Root {
        grant_id, root_id, ..
    } = svc
        .inner
        .catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key,
            display_name: "Second".into(),
            display_path: second.canonicalize().unwrap().display().to_string(),
            root_file_id: Some(second_file_id),
            filesystem: Some("NTFS".into()),
            origin: "owner_granted".into(),
            granted_via: "cli_flag".into(),
            observed_at_ns: 0,
        }))
        .unwrap()
    else {
        panic!("expected registered root");
    };
    // #796: force the identity check that the background pump may otherwise reach only under
    // load. A made-up file ID retires this grant before the space reader can try the second root.
    svc.inner
        .engine
        .granted_root(
            RootId::new(svc.inner.root_id(root_id)).unwrap(),
            grant_id,
            &loomward_engine::scan::CatalogRootIdentity {
                row: root_id,
                instance: svc.inner.instance.clone(),
            },
        )
        .expect("the second root must remain valid when the freshness pump checks its identity");
    // The first (lowest-id) root is now unobservable; the second must still answer.
    std::fs::remove_dir(&folder).unwrap();
    let (volumes, _) = ok("volumes.list", call(&svc, "volumes.list", json!({})));
    assert_eq!(
        volumes["volumes"].as_array().unwrap().len(),
        1,
        "{volumes:?}"
    );
    assert!(
        !volumes["volumes"][0]["capacity_bytes"].is_null(),
        "{volumes:?}"
    );
    assert!(
        !volumes["volumes"][0]["free_bytes"].is_null(),
        "{volumes:?}"
    );
}

/// #519 review: one stalled personal volume degrades alone. `volumes.list` and the alerts view still
/// answer, the healthy volume is observed, and changing grant roots never exhausts the space readers.
#[cfg(windows)]
#[test]
fn a_stalled_personal_volume_shares_one_reader_across_distinct_grant_roots() {
    let tmp = temp_root();
    let Some((svc, _folder)) = personal_grant_session(&tmp) else {
        return;
    };
    // This fixture repoints a grant between verified folders while one injected volume read
    // stalls. Keep automatic watches off: the catalogue itself is not being refreshed here.
    svc.inner.freshness.stop();
    // The stalled volume gets a key of its own, so its injected stall cannot hold up other tests
    // that read the real volume of this disk.
    svc.inner
        .db()
        .execute(
            "UPDATE main.volume SET volume_key='vsn_stalled' WHERE id=(SELECT min(id) FROM main.volume)",
            [],
        )
        .unwrap();
    let stalled_key = "vsn_stalled";
    // A second online personal volume, read by injection rather than by the OS.
    let healthy = tmp.path().join("healthy-folder");
    std::fs::create_dir_all(&healthy).unwrap();
    svc.inner
        .catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "vsn_healthy".into(),
            display_name: "Healthy".into(),
            display_path: healthy.display().to_string(),
            root_file_id: Some(vec![2; 16]),
            filesystem: Some("NTFS".into()),
            origin: "owner_granted".into(),
            granted_via: "cli_flag".into(),
            observed_at_ns: 0,
        }))
        .unwrap();
    // Keep the stall in force through every refresh, and release it even if an assertion fails.
    struct Release(Arc<Hooks>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.open(Command::VolumesList, "space_read");
        }
    }
    let _release = Release(svc.inner.hooks.clone());
    svc.inner
        .hooks
        .closed
        .lock()
        .unwrap()
        .push((Command::VolumesList, "space_read"));
    svc.inner.hooks.space.lock().unwrap().extend([
        (stalled_key.to_owned(), SpaceFake::Stall),
        (
            "vsn_healthy".to_owned(),
            SpaceFake::Fixed {
                capacity: 1 << 30,
                free: 1 << 29,
            },
        ),
    ]);
    let wire_id = |key: &str| -> String {
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
    };
    let (stalled_id, healthy_id) = (wire_id(stalled_key), wire_id("vsn_healthy"));
    for round in 0..10 {
        let root = tmp.path().join(format!("replacement-folder-{round}"));
        std::fs::create_dir_all(&root).unwrap();
        let (_, identity) = svc.inner.root_observation(&root).unwrap();
        let (_, file_id, _) = crate::Inner::identity_triple(&identity).unwrap();
        svc.inner.db().execute(
            "UPDATE st.root_grant SET display_path=?1,root_file_id=?2 WHERE id=(SELECT r.grant_id FROM main.root r JOIN main.volume v ON v.id=r.volume_id WHERE v.volume_key='vsn_stalled')",
            rusqlite::params![root.display().to_string(), file_id],
        ).unwrap();
        let (volumes, _) = ok("volumes.list", call(&svc, "volumes.list", json!({})));
        let rows = volumes["volumes"].as_array().unwrap();
        let row = |id: &str| {
            rows.iter()
                .find(|r| r["volume_id"] == json!(id))
                .unwrap_or_else(|| panic!("round {round}: {volumes:?}"))
        };
        assert!(
            row(&stalled_id)["capacity_bytes"].is_null(),
            "round {round}: {volumes:?}"
        );
        assert!(
            row(&stalled_id)["free_bytes"].is_null(),
            "round {round}: {volumes:?}"
        );
        assert_eq!(
            row(&healthy_id)["capacity_bytes"],
            "1073741824",
            "round {round}: {volumes:?}"
        );
        assert_eq!(
            row(&healthy_id)["free_bytes"],
            "536870912",
            "round {round}: {volumes:?}"
        );
    }
    // The alerts view reads the same observation and must still answer while the volume stalls.
    ok("alerts.list", call(&svc, "alerts.list", json!({})));
    assert_eq!(
        svc.inner
            .hooks
            .arrived
            .lock()
            .unwrap()
            .iter()
            .filter(|g| **g == (Command::VolumesList, "space_read"))
            .count(),
        1,
        "distinct grant roots must share the same stalled volume reader"
    );
    // A replacement root shares the stall, but can retry once the volume's reader finishes.
    let replacement = tmp.path().join("replacement-folder");
    std::fs::create_dir_all(&replacement).unwrap();
    let (_, identity) = svc.inner.root_observation(&replacement).unwrap();
    let (_, file_id, _) = crate::Inner::identity_triple(&identity).unwrap();
    svc.inner.db().execute(
        "UPDATE st.root_grant SET display_path=?1,root_file_id=?2 WHERE id=(SELECT r.grant_id FROM main.root r JOIN main.volume v ON v.id=r.volume_id WHERE v.volume_key='vsn_stalled')",
        rusqlite::params![replacement.display().to_string(), file_id],
    ).unwrap();
    svc.inner.hooks.space.lock().unwrap()[0].1 = SpaceFake::Fixed {
        capacity: 4096,
        free: 1024,
    };
    let (volumes, _) = ok("volumes.list", call(&svc, "volumes.list", json!({})));
    let replacement = volumes["volumes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["volume_id"] == stalled_id)
        .unwrap();
    assert!(replacement["capacity_bytes"].is_null());
    assert!(replacement["free_bytes"].is_null());
    svc.inner.hooks.open(Command::VolumesList, "space_read");
    svc.inner.hooks.until(
        "replacement root observes space after the stalled read finishes",
        |_| {
            let (volumes, _) = ok("volumes.list", call(&svc, "volumes.list", json!({})));
            let replacement = volumes["volumes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["volume_id"] == stalled_id)
                .unwrap();
            replacement["capacity_bytes"] == "4096" && replacement["free_bytes"] == "1024"
        },
    );
}

#[test]
fn placement_wire_preserves_unknowns_and_exclusions() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (_, _, volume) = placement_fixture(&svc);
    svc.inner.db().execute("INSERT INTO main.volume(volume_key,display_name,identity_json,capabilities_json,device_json,online,observed_at_ns) VALUES('other','Unknown target','{}','{}','{}',1,0)", []).unwrap();
    let (model, _) = ok("tiers.model", call(&svc, "tiers.model", json!({})));
    assert_eq!(model["volumes"][0]["tier"]["tier"], 0);
    assert_eq!(model["volumes"][0]["capacity_bytes"], "1048576");
    assert_eq!(model["volumes"][1]["pressure"], "unknown");
    let (candidates, _) = ok(
        "placement.candidates",
        call(
            &svc,
            "placement.candidates",
            json!({"source_volume_id":volume,"basis":"largest_dirs","max_groups":50,"min_bytes":"0"}),
        ),
    );
    assert_eq!(candidates["groups"].as_array().unwrap().len(), 1);
    let g = &candidates["groups"][0];
    assert!(g["estimated_relief_bytes"].is_null());
    assert_eq!(g["relief_basis"], "unknown");
    assert!(g["heat"].is_null() && g["pinned"].is_null());
    let (plan, _) = ok(
        "placement.simulate",
        call(
            &svc,
            "placement.simulate",
            placement_request(&volume, false),
        ),
    );
    assert_eq!(plan["filesystem_changed"], false);
    assert_eq!(plan["pre_rejected"][0]["reason"], "relief_unknown");
    assert_eq!(plan["excluded_volumes"][0]["reason"], "tier_unknown");
    assert!(plan["proposal_id"].is_null());
    assert_eq!(
        svc.inner
            .db()
            .query_row("SELECT count(*) FROM st.proposal", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn placement_oversized_catalogue_count_is_a_budget_error() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (_, _, volume) = placement_fixture(&svc);
    svc.inner
        .db()
        .execute("UPDATE main.dir SET sub_files=9007199254740992", [])
        .unwrap();
    let response = call(
        &svc,
        "placement.simulate",
        placement_request(&volume, false),
    );
    assert_eq!(err(response).code, ErrorCode::ResourceBudget);
}

#[test]
fn placement_saved_scenarios_survive_restart_and_hide_revoked_roots() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant, volume) = placement_fixture(&svc);
    let (saved, _) = ok(
        "placement.simulate",
        call(&svc, "placement.simulate", placement_request(&volume, true)),
    );
    let old_id = saved["proposal_id"].clone();
    let (again, _) = ok(
        "placement.simulate",
        call(&svc, "placement.simulate", placement_request(&volume, true)),
    );
    assert_eq!(again["proposal_id"], old_id);
    let (detail, _) = ok(
        "proposals.get",
        call(&svc, "proposals.get", json!({"proposal_id":old_id})),
    );
    assert_eq!(detail["plan"], json!(saved));
    assert_eq!(detail["summary"]["status"], "simulation_only");
    let digest = detail["inputs_digest"].clone();
    drop(svc);
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    assert_eq!(
        err(call(&svc, "proposals.get", json!({"proposal_id":old_id}))).code,
        ErrorCode::NotFound
    );
    let volume = svc.inner.volume_id(
        svc.inner
            .db()
            .query_row("SELECT volume_id FROM main.root WHERE id=?1", [root], |r| {
                r.get(0)
            })
            .unwrap(),
    );
    let (saved, _) = ok(
        "placement.simulate",
        call(&svc, "placement.simulate", placement_request(&volume, true)),
    );
    let (detail, _) = ok(
        "proposals.get",
        call(
            &svc,
            "proposals.get",
            json!({"proposal_id":saved["proposal_id"]}),
        ),
    );
    assert_eq!(detail["inputs_digest"], digest);
    assert_eq!(
        svc.inner
            .db()
            .query_row("SELECT count(*) FROM st.proposal", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let dir = svc
        .inner
        .db()
        .query_row(
            "SELECT id FROM main.dir WHERE root_id=?1 AND parent_id IS NULL",
            [root],
            |r| r.get(0),
        )
        .unwrap();
    relist(&svc, grant, dir, 4);
    let (list, _) = ok("proposals.list", call(&svc, "proposals.list", json!({})));
    assert_eq!(list["proposals"][0]["status"], "stale");
    drop(svc);
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    assert_eq!(
        err(call(&svc, "proposals.get", json!({"proposal_id":old_id}))).code,
        ErrorCode::NotFound
    );
    let (list, _) = ok("proposals.list", call(&svc, "proposals.list", json!({})));
    let id = list["proposals"][0]["proposal_id"].clone();
    let (detail, _) = ok(
        "proposals.get",
        call(&svc, "proposals.get", json!({"proposal_id":id})),
    );
    assert_eq!(detail["summary"]["status"], "stale");
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id":svc.inner.root_id(root),"purge_catalog":false}),
        ),
    );
    let (list, _) = ok("proposals.list", call(&svc, "proposals.list", json!({})));
    assert!(list["proposals"].as_array().unwrap().is_empty());
    assert_eq!(
        err(call(&svc, "proposals.get", json!({"proposal_id":id}))).code,
        ErrorCode::NotFound
    );
}

#[test]
fn placement_reads_do_not_take_mutation_tickets_and_expired_save_writes_nothing() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (_, _, volume) = placement_fixture(&svc);
    let mut req = request("placement.simulate", placement_request(&volume, false));
    req.deadline_ms = Some(Int::new(1).unwrap());
    *svc.inner.hooks.delay.lock().unwrap() =
        Some((Command::PlacementSimulate, Duration::from_millis(30)));
    let issued = svc.inner.turn.0.lock().unwrap().issued;
    let read_error = err(svc.call(req, &CallContext::http()));
    assert_eq!(read_error.code, ErrorCode::DeadlineExceeded);
    assert!(read_error.retryable);
    assert_eq!(svc.inner.turn.0.lock().unwrap().issued, issued);
    let mut req = request("placement.simulate", placement_request(&volume, true));
    req.deadline_ms = Some(Int::new(1).unwrap());
    let save_error = err(svc.call(req, &CallContext::http()));
    assert_eq!(save_error.code, ErrorCode::DeadlineExceeded);
    assert!(!save_error.retryable);
    svc.inner.hooks.until("expired save settled", |h| {
        h.arrived
            .lock()
            .unwrap()
            .contains(&(Command::PlacementSimulate, "after"))
    });
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        svc.inner
            .db()
            .query_row("SELECT count(*) FROM st.proposal", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    *svc.inner.hooks.delay.lock().unwrap() = None;
    let req = request("placement.simulate", placement_request(&volume, true));
    let saved = svc.call(req.clone(), &CallContext::http());
    ok("placement.simulate", saved.clone());
    assert_eq!(svc.call(req.clone(), &CallContext::http()), saved);
    let mut conflict = req;
    conflict.payload.insert("node_budget".into(), json!(999));
    assert_eq!(
        err(svc.call(conflict, &CallContext::http()))
            .detail
            .unwrap()["reason"],
        "idempotency_conflict"
    );
}

#[test]
fn placement_personal_catalogue_scope_and_migrated_proposal_table() {
    let tmp = temp_root();
    v1_store(tmp.path());
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (_, _, volume) = placement_fixture(&svc);
    ok(
        "placement.simulate",
        call(&svc, "placement.simulate", placement_request(&volume, true)),
    );
    assert_eq!(
        svc.inner
            .db()
            .query_row(
                "SELECT count(*) FROM st.proposal WHERE kind='placement_simulation'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    let personal = temp_root();
    let svc = open(personal.path(), DatasetClass::Personal);
    let (root, _, volume) = placement_fixture(&svc);
    let (plan, meta) = ok(
        "placement.simulate",
        call(
            &svc,
            "placement.simulate",
            placement_request(&volume, false),
        ),
    );
    assert_eq!(meta.dataset_class, DatasetClass::Personal);
    assert_eq!(
        plan["root_generations"][0]["root_id"],
        svc.inner.root_id(root)
    );
    svc.inner
        .db()
        .execute("UPDATE main.volume SET capabilities_json='{}'", [])
        .unwrap();
    let e = err(call(
        &svc,
        "placement.simulate",
        placement_request(&volume, false),
    ));
    assert_eq!(e.code, ErrorCode::CapabilityUnavailable);
    assert_eq!(
        e.detail.unwrap()["reason"],
        "personal_placement_physics_unobserved"
    );
}

#[test]
fn placement_wire_discloses_unknown_clusters_hidden_descendants_and_bad_source() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (_, _, volume) = placement_fixture(&svc);
    svc.inner
        .db()
        .execute(
            "UPDATE main.dir SET sub_logical=1,sub_allocated=1 WHERE parent_id IS NOT NULL",
            [],
        )
        .unwrap();
    svc.inner.db().execute("INSERT INTO main.volume(volume_key,display_name,identity_json,capabilities_json,device_json,online,observed_at_ns) VALUES('target','Synthetic target','{}',?1,'{}',1,0)", [json!({"space":{"capacity_bytes":"1048576","free_bytes":"524288","read_only":false}}).to_string()]).unwrap();
    let target = svc.inner.volume_id(svc.inner.db().last_insert_rowid());
    ok(
        "volumes.declare_tier",
        call(
            &svc,
            "volumes.declare_tier",
            json!({"volume_id":target,"tier":1}),
        ),
    );
    let (plan, _) = ok(
        "placement.simulate",
        call(
            &svc,
            "placement.simulate",
            placement_request(&volume, false),
        ),
    );
    let assumptions = plan["assumptions"].as_array().unwrap();
    assert!(assumptions
        .iter()
        .any(|a| a.as_str().unwrap().contains("unknown cluster size")));
    assert!(assumptions
        .iter()
        .any(|a| a.as_str().unwrap().contains("descendant")));
    let (candidates, _) = ok(
        "placement.candidates",
        call(
            &svc,
            "placement.candidates",
            json!({"source_volume_id":volume,"basis":"largest_dirs","max_groups":50,"min_bytes":"0"}),
        ),
    );
    let mut whatif = placement_request(&volume, false);
    whatif["relief_policy"] = json!("entry_allocation_whatif");
    whatif["heat_policy"] = json!("assumed_only");
    whatif["overrides"] = json!([{"group_id":candidates["groups"][0]["group_id"],"heat":0.1,"pinned":false,"active":false,"protected":false,"days_since_move":400}]);
    let (plan, _) = ok(
        "placement.simulate",
        call(&svc, "placement.simulate", whatif),
    );
    assert_eq!(plan["proposals"].as_array().unwrap().len(), 1);
    assert_eq!(plan["proposals"][0]["transfer_bytes"], "3003");
    assert_eq!(plan["proposals"][0]["executable"], false);
    assert_eq!(plan["filesystem_changed"], false);
    ok(
        "volumes.declare_tier",
        call(
            &svc,
            "volumes.declare_tier",
            json!({"volume_id":volume,"tier":null}),
        ),
    );
    let e = err(call(
        &svc,
        "placement.simulate",
        placement_request(&volume, false),
    ));
    assert_eq!(e.code, ErrorCode::InvalidRequest);
    assert_eq!(e.detail.unwrap()["reason"], "source_tier_unknown");
}

#[test]
fn placement_save_refuses_same_generation_changes_and_unknown_overlap() {
    let tmp = temp_root();
    let svc = Arc::new(open(tmp.path(), DatasetClass::Synthetic));
    let (_, _, volume) = placement_fixture(&svc);
    svc.inner
        .hooks
        .closed
        .lock()
        .unwrap()
        .push((Command::PlacementSimulate, "placement_read"));
    let worker = svc.clone();
    let req = placement_request(&volume, true);
    let pending = std::thread::spawn(move || call(&worker, "placement.simulate", req));
    svc.inner.hooks.until("placement snapshot", |h| {
        h.arrived
            .lock()
            .unwrap()
            .contains(&(Command::PlacementSimulate, "placement_read"))
    });
    svc.inner
        .db()
        .execute("UPDATE main.revision SET catalog_rev=catalog_rev+1", [])
        .unwrap();
    svc.inner
        .hooks
        .open(Command::PlacementSimulate, "placement_read");
    assert_eq!(
        err(pending.join().unwrap()).code,
        ErrorCode::StaleGeneration
    );
    assert_eq!(
        svc.inner
            .db()
            .query_row("SELECT count(*) FROM st.proposal", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    svc.inner.db().execute("INSERT INTO st.root_grant(volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES('fixture',zeroblob(16),'Synthetic root\\unobserved-child','fixture','fixture','active',0)",[]).unwrap();
    let grant = svc.inner.db().last_insert_rowid();
    svc.inner
        .db()
        .execute(
            "INSERT INTO main.root(grant_id,volume_id,state) VALUES(?1,1,'never_scanned')",
            [grant],
        )
        .unwrap();
    let e = err(call(
        &svc,
        "placement.simulate",
        placement_request(&volume, false),
    ));
    assert_eq!(e.code, ErrorCode::PartialCoverage);
    assert_eq!(e.message.as_str(), "overlapping_grant_ancestry_unknown");
    svc.inner
        .db()
        .execute(
            "UPDATE st.root_grant SET display_path='Synthetic second root' WHERE id=?1",
            [grant],
        )
        .unwrap();
    svc.inner
        .hooks
        .closed
        .lock()
        .unwrap()
        .push((Command::PlacementSimulate, "placement_read"));
    let worker = svc.clone();
    let req = placement_request(&volume, true);
    let pending = std::thread::spawn(move || call(&worker, "placement.simulate", req));
    svc.inner.hooks.until("unscanned root snapshot", |h| {
        h.arrived
            .lock()
            .unwrap()
            .iter()
            .filter(|g| **g == (Command::PlacementSimulate, "placement_read"))
            .count()
            >= 3
    });
    svc.inner
        .db()
        .execute(
            "UPDATE st.root_grant SET state='revoked',revoked_at_ns=0 WHERE id=?1",
            [grant],
        )
        .unwrap();
    svc.inner
        .hooks
        .open(Command::PlacementSimulate, "placement_read");
    assert_eq!(
        err(pending.join().unwrap()).code,
        ErrorCode::StaleGeneration
    );
}

#[cfg(windows)]
#[test]
fn placement_registered_lab_simulation_preserves_every_file_and_mtime() {
    if loomward_windows::enumerate::skip_elevated_test() {
        let tmp = temp_root();
        let refusal = crate::lab::register(
            &tmp.path().join("state"),
            &tmp.path().join("lab"),
            "synthetic",
            "S",
            &format!("sha256:{}", "0".repeat(64)),
        )
        .unwrap_err();
        assert!(refusal.contains("identity_unavailable"));
        return;
    }
    let lab = lab().unwrap();
    assert_eq!(
        settle(&lab.svc, &start(&lab.svc, &lab.root, "full"))["state"],
        "completed"
    );
    // #561: a background watcher publication would otherwise race the saved simulation.
    f3_wait_live(&lab.svc);
    fn contents(
        root: &Path,
    ) -> std::collections::BTreeMap<std::path::PathBuf, (Vec<u8>, std::time::SystemTime)> {
        let mut files = std::collections::BTreeMap::new();
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(contents(&path).into_iter().map(|(relative, value)| {
                    (
                        std::path::PathBuf::from(path.file_name().unwrap()).join(relative),
                        value,
                    )
                }));
            } else {
                files.insert(
                    std::path::PathBuf::from(path.file_name().unwrap()),
                    (
                        std::fs::read(&path).unwrap(),
                        std::fs::metadata(path).unwrap().modified().unwrap(),
                    ),
                );
            }
        }
        files
    }
    let before = contents(&lab.path);
    let (volumes, _) = ok("volumes.list", call(&lab.svc, "volumes.list", json!({})));
    let volume = volumes["volumes"][0]["volume_id"].as_str().unwrap();
    ok(
        "volumes.declare_tier",
        call(
            &lab.svc,
            "volumes.declare_tier",
            json!({"volume_id":volume,"tier":0}),
        ),
    );
    let (model, _) = ok("tiers.model", call(&lab.svc, "tiers.model", json!({})));
    assert!(!model["volumes"][0]["capacity_bytes"].is_null());
    let (candidates, _) = ok(
        "placement.candidates",
        call(
            &lab.svc,
            "placement.candidates",
            json!({"source_volume_id":volume,"basis":"root_children","max_groups":50,"min_bytes":"0"}),
        ),
    );
    assert!(!candidates["groups"].as_array().unwrap().is_empty());
    let mut req = placement_request(volume, true);
    req["target_free_bytes"] = model["volumes"][0]["free_bytes"].clone();
    let (plan, _) = ok(
        "placement.simulate",
        call(&lab.svc, "placement.simulate", req),
    );
    assert_eq!(plan["filesystem_changed"], false);
    assert_eq!(before, contents(&lab.path));
}

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
                allow_identity_unverified: false,
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
            allow_identity_unverified: false,
            grant_roots: vec![unknown]
        })
        .is_err());
        assert!(!state.exists());
    }
    assert!(Service::open(Config {
        state_dir: state.clone(),
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
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
        max_bytes: None,
        modified_after: None,
        modified_before: None,
        kind: "any".into(),
        limit: 10,
        cursor: None,
        work_budget: 50_000_000,
        time_budget: None,
        scope: None,
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
    assert!(!reader.search(&req).unwrap().hits.is_empty());
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
        fn confirm_disclosure(
            &self,
            _: &DisclosureSummary,
        ) -> Result<bool, loomward_protocol::ErrorBody> {
            Ok(false)
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
    assert!(svc.inner.grant(&root, "cli_flag", false).is_err());
    assert_eq!(crate::db::active_grants(&svc.inner.db()).unwrap(), 0);
    svc.inner
        .db()
        .execute("UPDATE st.lab_root SET manifest_digest='sha256:0000000000000000000000000000000000000000000000000000000000000000'", [])
        .unwrap();
    assert!(svc.inner.grant(&root, "cli_flag", false).is_ok());
}

/// Requests carry the protocol maximum deadline: a test that pins a deadline sets its own, and the
/// 2 to 5 s defaults turned every handler into a race on a loaded host (#320).
fn request(command: &str, payload: Value) -> RequestEnvelope {
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    serde_json::from_value(json!({
        "protocol": "loomward/3", "request_id": format!("r{n}"), "command": command, "payload": payload,
        "deadline_ms": 60_000
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
pub(super) fn fixture(svc: &Service, files: u64) -> (i64, i64) {
    let w = svc.inner.catalog.writer();
    let WriteReply::Root {
        grant_id,
        root_id,
        dir_id,
    } = w
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "fixture".into(),
            display_name: "Synthetic".into(),
            display_path: "Synthetic root".into(),
            root_file_id: Some(vec![1; 16]),
            filesystem: Some("NTFS".into()),
            origin: if svc.dataset() == DatasetClass::Personal {
                "owner_granted"
            } else {
                "fixture"
            }
            .into(),
            granted_via: if svc.dataset() == DatasetClass::Personal {
                "cli_flag"
            } else {
                "fixture"
            }
            .into(),
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
    // A first full scan builds in bulk until EndRun; the sink resolves children the same way.
    let sub_id: i64 = svc
        .inner
        .db()
        .query_row(
            "SELECT id FROM main.dir WHERE parent_id=?1 UNION ALL SELECT id FROM main.bulk_dir WHERE parent_id=?1",
            [dir],
            |r| r.get(0),
        )
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

/// #356: makes file row 2 a hard link of row 1, then rescans the fixture root keeping only row 2's
/// name and the subdirectory, so the publication drops rows 1 and 3 and row 1's identity survives
/// as row 2. The listing runs on its own thread and is returned paused inside the writer's
/// pending-publication window (precious staging committed, catalogue transaction not begun);
/// the caller acts, then releases the window and joins.
fn rescan_paused_in_publication_window(svc: &Arc<Service>) -> std::thread::JoinHandle<()> {
    let (identity, created, name): (Vec<u8>, i64, String) = svc
        .inner
        .db()
        .query_row(
            "SELECT a.file_id,a.created_ft,b.name FROM main.file a, main.file b WHERE a.id=1 AND b.id=2",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    svc.inner
        .db()
        .execute(
            "UPDATE main.file SET file_id=?1,created_ft=?2 WHERE id=2",
            rusqlite::params![identity, created],
        )
        .unwrap();
    let writer = svc.inner.catalog.writer();
    let WriteReply::Run(run) = writer
        .call(WriteCommand::BeginRun {
            grant_id: 1,
            mode: "full".into(),
            strategy: "fixture".into(),
            started_at_ns: 0,
        })
        .unwrap()
    else {
        panic!()
    };
    let mut kept = file(&name, 1001);
    kept.file_id = Some(identity);
    kept.created_ft = Some(created);
    let mut sub = Observation::file("sub", 0, Some(0));
    sub.family = "none".into();
    writer.publication_window().arm();
    let worker = svc.clone();
    let listing = std::thread::spawn(move || {
        worker
            .inner
            .catalog
            .writer()
            .call(WriteCommand::DirListing(DirListing {
                run_id: run,
                dir_id: 1,
                files: vec![kept],
                dirs: vec![sub],
                state: "complete".into(),
                skipped: 0,
                errors: 0,
            }))
            .unwrap();
    });
    writer.publication_window().wait_paused();
    listing
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
        if (command.wave() >= 3
            && !matches!(
                command,
                Command::FeedbackRecord
                    | Command::LearningStatus
                    | Command::LearningQueue
                    | Command::LearningRefit
                    | Command::ThreadsMeaning
                    | Command::CollectionsList
                    | Command::CollectionsCreate
                    | Command::CollectionsUpdateMembers
                    | Command::CollectionsMembers
                    | Command::BackupsList
                    | Command::BackupsImport
                    | Command::BackupsForget
                    | Command::DiagnosticsPreview
                    | Command::DiagnosticsSave
            )
            // docs/47 C5: synthetic sessions serve copy on Windows (copy_tests.rs pins both classes).
            && !(cfg!(windows) && command.as_str().starts_with("copy."))
            // LW-118: Ask is served (ask_tests.rs pins its refusals).
            && !command.as_str().starts_with("ask."))
            || command == Command::RootsRequestGrant
        {
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
            json!({"root_id": svc.inner.root_id(root), "text": "report", "extension": null, "min_bytes": null, "max_bytes": null, "modified_after": null, "modified_before": null, "kind": "file", "limit": 10, "cursor": null}),
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
            assert_eq!(result["hits"].as_array().unwrap().len(), 5);
            assert_eq!(result["index_state"], "current");
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
    // docs/46 F1: the federated search fields are contract only; the engine says so and refuses them
    // rather than ignoring them (every field alone, even `sources: ["own"]`).
    assert_eq!(hello["features"]["federated_search"], false);
    for extra in [
        json!({"sources": ["own"]}),
        json!({"terms": [{"text": "report", "mode": "token", "negated": false}]}),
        json!({"extensions": ["docx"]}),
        json!({"collection_id": "co_synth1"}),
        json!({"under_node_id": anchor}),
    ] {
        let mut payload = json!({"root_id": svc.inner.root_id(root), "text": "report", "extension": null, "min_bytes": null, "max_bytes": null, "modified_after": null, "modified_before": null, "kind": "file", "limit": 10, "cursor": null});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            err(call(&svc, "search.query", payload)).code,
            ErrorCode::CapabilityUnavailable
        );
    }
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
            allow_identity_unverified: false,
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
    // Search pages are a keyset over the live index (LW-113): a commit does not stale them.
    let search = |cursor: Value| {
        call(
            &svc,
            "search.query",
            json!({"root_id": null, "text": "report", "extension": null, "min_bytes": null, "max_bytes": null, "modified_after": null, "modified_before": null, "kind": "any", "limit": 2, "cursor": cursor}),
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
    let (s2, _) = ok("search.query", search(s_next));
    assert!(s2["hits"].as_array().is_some_and(|h| !h.is_empty()));
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

// #669: simulate a clone by changing only the registry GUID; serial and file ID stay native.
fn lab_registration_guid_case(binding: &str) {
    if !native_identity_available() {
        return;
    }
    let tmp = temp_root();
    let state = tmp.path().join("state");
    let root = tmp.path().join("lab");
    crate::lab::register(
        &state,
        &root,
        "42",
        "S",
        &format!("sha256:{}", "00".repeat(32)),
    )
    .unwrap();
    let root = root.canonicalize().unwrap();
    let svc = open(&state, DatasetClass::Synthetic);
    let guid = format!(
        "{:032x}",
        svc.inner
            .root_observation(&root)
            .unwrap()
            .1
            .volume_guid
            .unwrap()
    );
    let conn = svc.inner.db();
    let stored_guid: Option<String> = conn
        .query_row("SELECT volume_guid FROM st.lab_root", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        stored_guid.as_deref(),
        Some(guid.as_str()),
        "new registrations persist their native GUID"
    );
    let registered_guid = match binding {
        "matching" => Some(guid.clone()),
        "clone" => Some(format!(
            "{:032x}",
            u128::from_str_radix(&guid, 16).unwrap() ^ 1
        )),
        "legacy" | "unmigrated" => None,
        _ => unreachable!(),
    };
    conn.execute("UPDATE st.lab_root SET volume_guid=?1", [registered_guid])
        .unwrap();
    drop(conn);
    let result = svc.inner.grant(&root, "cli_flag", false);
    if binding == "matching" {
        assert!(result.is_ok(), "matching GUID must authorise: {result:?}");
    } else {
        assert_eq!(
            result,
            Err(crate::Refused::Rule(
                RootGrantResultRefusal::SyntheticSessionRequiresLabRoot
            )),
            "{binding} must require explicit lab re-registration"
        );
        assert_eq!(crate::db::active_grants(&svc.inner.db()).unwrap(), 0);
    }
    drop(svc);
    if binding == "unmigrated" {
        let conn = rusqlite::Connection::open(state.join("state.db")).unwrap();
        conn.execute_batch("ALTER TABLE lab_root DROP COLUMN volume_guid; PRAGMA user_version=10")
            .unwrap();
    }
    let reopened = Service::open(Config {
        state_dir: state,
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
        grant_roots: vec![root],
    });
    if binding == "matching" {
        assert!(reopened.is_ok());
    } else {
        let error = reopened
            .err()
            .expect("CLI preflight must refuse the registration");
        assert!(
            error.contains("synthetic_session_requires_lab_root"),
            "{error}"
        );
    }
}

#[test]
fn lab_registration_guid_clone_is_refused() {
    lab_registration_guid_case("clone");
}

#[test]
fn lab_registration_guid_legacy_is_refused() {
    lab_registration_guid_case("legacy");
}

#[test]
fn lab_registration_guid_unmigrated_preflight_is_refused() {
    lab_registration_guid_case("unmigrated");
}

#[test]
fn lab_registration_guid_matching_is_accepted() {
    lab_registration_guid_case("matching");
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
        allow_identity_unverified: false,
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
        allow_identity_unverified: false,
        grant_roots: vec![]
    })
    .is_err());
    let p = Service::open(Config {
        state_dir: personal,
        dataset: DatasetClass::Personal,
        allow_personal: true,
        allow_identity_unverified: false,
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
fn both_grant_endings_call_the_scan_writer_fence_after_commit() {
    for ending in ["revoke", "identity_changed", "identity_changed_from_scan"] {
        let tmp = temp_root();
        let svc = Service::open_with(
            Config {
                state_dir: tmp.path().into(),
                dataset: DatasetClass::Synthetic,
                allow_personal: false,
                allow_identity_unverified: false,
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
        let (root, grant) = register(&svc, 3);
        let id = RootId::new(svc.inner.root_id(root)).unwrap();
        let state = if ending == "revoke" {
            svc.inner.revoke(grant).unwrap();
            "revoked"
        } else {
            svc.inner
                .identity_changed(grant, &id, ending == "identity_changed");
            "identity_changed"
        };
        assert_eq!(
            *probe.seen.lock().unwrap(),
            vec![format!("{id}:{state}")],
            "{ending}"
        );
    }
}

#[cfg(windows)]
#[test]
fn identity_change_cancels_an_active_scan_before_returning() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    svc.inner.scan.gate.arm();
    let job = start(svc, &lab.root, "full");
    svc.inner.scan.gate.wait_reached();
    let grant = svc
        .inner
        .db()
        .query_row("SELECT grant_id FROM main.root WHERE id=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    svc.inner
        .identity_changed(grant, &RootId::new(lab.root.clone()).unwrap(), true);
    // Observe cancellation while the writer is still held, before it can fail on its own fence.
    let job_id = JobId::new(job.clone()).unwrap();
    let state = svc.inner.engine.job_status(&job_id).unwrap().state;
    svc.inner.scan.gate.release();
    assert!(
        matches!(state, JobState::CancelRequested | JobState::Cancelled),
        "{state:?}"
    );
    assert_eq!(settle(svc, &job)["state"], "cancelled");
}

#[test]
fn revocation_commits_first_then_fences_then_emits_and_is_monotone() {
    let tmp = temp_root();
    let svc = Service::open_with(
        Config {
            state_dir: tmp.path().into(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
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
            volume_guid: None,
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

/// A worker answers its caller before its closure ends, so for a moment after any call it still
/// holds the service and its catalogue connections. The gate holds that moment open; dropping the
/// service must wait for it rather than return with `catalog.db` still open.
#[test]
fn dropping_the_service_waits_for_workers_that_already_replied() {
    use std::sync::atomic::{AtomicBool, Ordering};
    for command in [Command::RootsList, Command::RootsRevoke] {
        let tmp = temp_root();
        let svc = open(tmp.path(), DatasetClass::Synthetic);
        let (a, _) = register(&svc, 1);
        let root = svc.inner.root_id(a);
        svc.inner
            .hooks
            .closed
            .lock()
            .unwrap()
            .push((command, "worker_exit"));
        let reply = if command == Command::RootsList {
            call(&svc, "roots.list", json!({}))
        } else {
            call(
                &svc,
                "roots.revoke",
                json!({"root_id": root, "purge_catalog": false}),
            )
        };
        ok(command.as_str(), reply);
        let released = Arc::new(AtomicBool::new(false));
        let (flag, weak) = (released.clone(), Arc::downgrade(&svc.inner));
        let opener = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::SeqCst);
            weak.upgrade()
                .expect("the held worker keeps the service alive")
                .hooks
                .open(command, "worker_exit");
        });
        drop(svc);
        assert!(
            released.load(Ordering::SeqCst),
            "{command:?}: drop returned while a replied worker still held the service"
        );
        std::fs::remove_file(tmp.path().join("catalog.db")).unwrap();
        opener.join().unwrap();
    }
}

#[test]
fn drop_rejects_a_weak_upgrade_after_its_last_check() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let hooks = svc.inner.hooks.clone();
    let weak = Arc::downgrade(&svc.inner);
    let pump = weak.clone();
    let (attempted, attempt) = std::sync::mpsc::sync_channel(1);
    *hooks.after_drop_check.lock().unwrap() = Some(Box::new(move || {
        // Keep any successful upgrade alive beyond Drop's return, as a paused pump would.
        attempted.send(pump.upgrade()).unwrap();
    }));

    drop(svc);

    let transient = attempt
        .try_recv()
        .expect("Drop must run the last-check hook");
    assert!(
        transient.is_none(),
        "a Weak upgrade after Drop's last check kept Inner alive past its return"
    );
    assert!(weak.upgrade().is_none());
    std::fs::remove_file(tmp.path().join("catalog.db")).unwrap();
}

#[test]
fn drop_waits_for_a_weak_upgrade_after_its_last_count_check() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let hooks = svc.inner.hooks.clone();
    struct Release(Arc<Hooks>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.open(Command::TelemetrySubscribe, "drop_ready");
        }
    }
    let _release = Release(hooks.clone());
    hooks
        .closed
        .lock()
        .unwrap()
        .push((Command::TelemetrySubscribe, "drop_ready"));
    let weak = Arc::downgrade(&svc.inner);
    let (done, finished) = std::sync::mpsc::channel();
    let dropper = std::thread::spawn(move || {
        drop(svc);
        done.send(()).unwrap();
    });
    hooks.until("drop ready", |h| {
        h.arrived
            .lock()
            .unwrap()
            .contains(&(Command::TelemetrySubscribe, "drop_ready"))
    });
    // The pump can acquire this same transient reference after the count check.
    let transient = weak.upgrade().unwrap();
    hooks.open(Command::TelemetrySubscribe, "drop_ready");
    let escaped = finished.recv_timeout(Duration::from_millis(200)).is_ok();
    drop(transient);
    if !escaped {
        finished.recv_timeout(Duration::from_secs(15)).unwrap();
    }
    dropper.join().unwrap();
    assert!(
        !escaped,
        "Drop returned while a transient upgrade held Inner"
    );
    assert!(weak.upgrade().is_none());
    std::fs::remove_file(tmp.path().join("catalog.db")).unwrap();
}

#[test]
#[cfg(windows)]
fn drop_closes_the_catalogue_with_an_active_telemetry_lease() {
    for _ in 0..20 {
        let tmp = temp_root();
        // No roots or process channel: only aggregate system counters leave the sampler.
        let svc = open(tmp.path(), DatasetClass::Personal);
        let mut events = svc.subscribe(None);
        ok(
            "telemetry.subscribe",
            call(
                &svc,
                "telemetry.subscribe",
                json!({"subscription_id": null, "channels": ["system"], "interval_ms": 1000}),
            ),
        );
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            if matches!(events.recv_timeout(Duration::from_millis(250)), RecvOutcome::Event(e) if e.event == EventName::TelemetrySample)
            {
                break;
            }
            assert!(
                Instant::now() < until,
                "the telemetry pump never published a sample"
            );
        }
        drop(events);
        let weak = Arc::downgrade(&svc.inner);
        drop(svc);
        assert!(weak.upgrade().is_none());
        std::fs::remove_file(tmp.path().join("catalog.db")).unwrap();
    }
}

#[test]
fn revoke_distinguishes_missing_grants_from_store_failures() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    assert_eq!(svc.inner.revoke(1).unwrap_err().code, ErrorCode::NotFound);
    let (_, grant) = register(&svc, 1);
    // Only this service connection loses the store; the catalogue writer remains intact.
    svc.inner.db().execute_batch("DETACH DATABASE st").unwrap();
    assert_eq!(
        svc.inner.revoke(grant).unwrap_err().code,
        ErrorCode::InternalError
    );
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
    // Dropping the service closed every connection, so the catalogue is removable (#320 follow-up).
    std::fs::remove_file(tmp.path().join("catalog.db")).unwrap();
    for name in ["catalog.db-wal", "catalog.db-shm"] {
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

#[test]
fn grants_list_caps_retained_and_forgotten_rows_without_negative_limit() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    // More historical roots than either listing cap, plus a forgotten audit record.
    svc.inner.db().execute_batch(
        "WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<301)
         INSERT INTO st.root_grant(id,volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns,revoked_at_ns)
         SELECT id,'fixture',zeroblob(16),'Synthetic history','fixture','fixture','revoked',0,1 FROM n;
         INSERT INTO main.root(id,grant_id,state) SELECT id,id,'never_scanned' FROM st.root_grant;
         INSERT INTO st.root_forgotten(grant_id,root_number,forgotten_at_ns) VALUES(1,1,2);"
    ).unwrap();
    assert_eq!(crate::db::grant_rows(&svc.inner.db()).unwrap().len(), 256);
    assert!(crate::db::forgotten_grants(&svc.inner.db(), -1)
        .unwrap()
        .is_empty());
    let (list, _) = ok("grants.list", call(&svc, "grants.list", json!({})));
    assert_eq!(list["grants"].as_array().unwrap().len(), 256);
    // Once fewer retained rows remain, only the remaining slots go to forgotten records.
    svc.inner
        .db()
        .execute("DELETE FROM main.root WHERE id>255", [])
        .unwrap();
    let (list, _) = ok("grants.list", call(&svc, "grants.list", json!({})));
    assert_eq!(list["grants"].as_array().unwrap().len(), 255);
    assert_eq!(
        list["grants"][254]["forgotten_at"],
        crate::ts_ns(2).as_str()
    );
}

/// #198: the hard root cap requires owner action; narrower transient budgets can be retried.
#[test]
fn root_limit_requires_owner_action_but_transient_budgets_are_retryable() {
    for command in [Command::RootsRequestGrant, Command::RootsList] {
        for (reason, retryable) in [
            (Some("root_limit_reached"), false),
            (Some("sort_requires_index"), true),
            (None, true),
        ] {
            let body = crate::fail(ErrorCode::ResourceBudget, "budget exhausted");
            let body = if let Some(reason) = reason {
                crate::with_reason(body, reason)
            } else {
                body
            };
            let error = crate::retry_flag(command, body);
            assert_eq!(error.code, ErrorCode::ResourceBudget);
            assert_eq!(error.retryable, retryable, "{command:?}: {reason:?}");
            assert_eq!(
                error
                    .detail
                    .as_ref()
                    .and_then(|d| d.get("reason"))
                    .and_then(Value::as_str),
                reason
            );
        }
    }
}

/// #185 item 3: the contract lists at most 64 roots, so a 65th active grant is refused by name;
/// re-granting an active root and granting after a revocation still work.
#[test]
fn grants_past_64_active_roots_are_refused_by_rule() {
    struct Pick(std::path::PathBuf);
    impl NativeDialogs for Pick {
        fn pick_folder(&self) -> Option<std::path::PathBuf> {
            Some(self.0.clone())
        }
        fn confirm_disclosure(&self, _: &DisclosureSummary) -> Result<bool, ErrorBody> {
            Ok(false)
        }
    }
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
        assert!(svc.inner.grant(&dirs[0], "cli_flag", false).is_err());
        return;
    }
    let mut granted = Vec::new();
    for d in &dirs[..64] {
        granted.push(svc.inner.grant(d, "cli_flag", false).unwrap().0);
    }
    assert_eq!(
        svc.inner.grant(&dirs[64], "cli_flag", false),
        Err(crate::Refused::RootLimit)
    );
    let error = err(svc.call(
        request("roots.request_grant", json!({"purpose": "metadata_scan"})),
        &CallContext::tauri(Arc::new(Pick(dirs[64].clone()))),
    ));
    assert_eq!(
        (error.code, error.retryable),
        (ErrorCode::ResourceBudget, false)
    );
    assert_eq!(reason(&error), Some("root_limit_reached"));
    assert_eq!(
        svc.inner.grant(&dirs[0], "cli_flag", false),
        Ok((granted[0], false))
    );
    let grant = crate::db::roots(&svc.inner.db(), Some(granted[0])).unwrap()[0].grant;
    svc.inner.revoke(grant).unwrap();
    assert!(svc.inner.grant(&dirs[64], "cli_flag", false).unwrap().1);
}

/// Invalid revisions stay unknown in both events and responses.
#[test]
fn publish_clamps_negative_revision_to_unknown() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let mut events = svc.subscribe(None);
    match events.recv_timeout(Duration::from_secs(5)) {
        RecvOutcome::Event(e) => assert_eq!(e.event, EventName::StreamHello),
        other => panic!("expected hello, got {other:?}"),
    }
    let warning =
        || json!({"code": "scan_throttled", "message": "test", "at": "2026-01-01T00:00:00Z"});
    let next_warning = |events: &mut Box<dyn EventStream>| loop {
        match events.recv_timeout(Duration::from_secs(5)) {
            RecvOutcome::Event(e) if e.event == EventName::HealthWarning => break e,
            RecvOutcome::Event(_) => continue,
            other => panic!("{other:?}"),
        }
    };
    svc.inner
        .publish(EventName::HealthWarning, warning(), Some(-1), Some(-1));
    let event = next_warning(&mut events);
    event.validate().unwrap();
    assert_eq!(event.catalog_rev, None);
    assert_eq!(event.state_rev, None);
    // ResponseMeta agrees on the same negative input.
    let meta = svc.inner.meta(
        std::time::Instant::now(),
        &Payload::new(),
        Some(-1),
        Some(-1),
    );
    assert_eq!(meta.catalog_rev, None);
    assert_eq!(meta.state_rev, None);
    assert_eq!(event.catalog_rev, meta.catalog_rev);
    assert_eq!(event.state_rev, meta.state_rev);
    // Non-negative values pass through unchanged.
    svc.inner
        .publish(EventName::HealthWarning, warning(), Some(3), Some(7));
    let event = next_warning(&mut events);
    event.validate().unwrap();
    assert_eq!(event.catalog_rev, Some(Generation(3)));
    assert_eq!(event.state_rev, Some(Generation(7)));
}

pub(crate) fn native_identity_available() -> bool {
    #[cfg(windows)]
    if loomward_windows::enumerate::skip_elevated_test() {
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
pub(crate) struct Lab {
    _tmp: tempfile::TempDir,
    pub(crate) path: std::path::PathBuf,
    pub(crate) svc: Service,
    pub(crate) root: String,
}

#[cfg(windows)]
pub(crate) fn lab() -> Option<Lab> {
    lab_in(temp_root())
}

#[cfg(windows)]
pub(crate) fn lab_in(tmp: tempfile::TempDir) -> Option<Lab> {
    if loomward_windows::enumerate::skip_elevated_test() {
        eprintln!("skipped: elevated token; the native scan refuses elevation (invariant 2)");
        return None;
    }
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
        allow_identity_unverified: false,
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

#[cfg(windows)]
#[test]
fn scan_start_refuses_a_grant_revoked_after_root_mint() {
    for cancel_fence in [false, true] {
        let Some(lab) = lab() else { return };
        let inner = &lab.svc.inner;
        inner.freshness.pause_pump();
        let row = inner.root_row(&lab.root).unwrap();
        let stored = crate::db::roots(&inner.db(), Some(row)).unwrap().remove(0);
        let id = RootId::new(&lab.root).unwrap();
        inner.scan.bind(&id, stored.grant, stored.dir.unwrap());
        let root = inner
            .engine
            .granted_root(
                id,
                stored.grant,
                &loomward_engine::scan::CatalogRootIdentity {
                    row,
                    instance: inner.instance.clone(),
                },
            )
            .unwrap();
        if cancel_fence {
            inner.revoke(stored.grant).unwrap();
        } else {
            // Pause revocation after its durable commit, before the engine cancellation fence.
            inner
                .catalog
                .writer()
                .call(WriteCommand::RevokeGrant {
                    grant_id: stored.grant,
                    revoked_at_ns: crate::now_ns(),
                })
                .unwrap();
        }
        for refresh in [false, true] {
            let answer = if refresh {
                inner.engine.scan_refresh(&root, None)
            } else {
                inner.engine.scan_start(&root, None)
            };
            if let Ok(job) = &answer {
                settle(&lab.svc, job.job_id.as_str());
            }
            assert!(
                matches!(
                    answer,
                    Err(loomward_engine::EngineError::PermissionDenied { .. })
                ),
                "revoked mint admitted native work (cancel fence={cancel_fence}, refresh={refresh}): {answer:?}"
            );
        }
    }
}

/// #731: a revoke that commits while a full scan is held mid-walk is refused by the durable
/// writer at the scan's final EndRun, on a real catalogue. The job ends cancelled or failed, its
/// coverage is never complete, and no sweep publishes: the root's generation and the count of
/// completed runs stay where the baseline scan left them.
#[cfg(windows)]
#[test]
fn revocation_before_the_final_end_run_is_refused_by_the_durable_writer() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    let inner = &svc.inner;
    // A completed baseline scan fixes the generation and run count a refused sweep must not move.
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    let root_row = inner.root_row(&lab.root).unwrap();
    let grant = crate::db::roots(&inner.db(), Some(root_row))
        .unwrap()
        .remove(0)
        .grant;
    let scalar =
        |sql: &str| -> i64 { inner.db().query_row(sql, [root_row], |r| r.get(0)).unwrap() };
    let generation = scalar("SELECT generation FROM main.root WHERE id=?1");
    let completed =
        scalar("SELECT count(*) FROM main.scan_run WHERE root_id=?1 AND state='completed'");
    inner.scan.gate.arm();
    let job = start(svc, &lab.root, "full");
    inner.scan.gate.wait_reached();
    // The durable revoke commits while the walk is held: the run's remaining writer commands,
    // including its final EndRun, now meet a revoked grant.
    let revoked = inner.catalog.writer().call(WriteCommand::RevokeGrant {
        grant_id: grant,
        revoked_at_ns: crate::now_ns(),
    });
    inner.scan.gate.release();
    assert!(revoked.is_ok(), "{revoked:?}");
    let done = settle(svc, &job);
    assert!(
        matches!(
            done.get("state").and_then(|v| v.as_str()),
            Some("cancelled" | "failed")
        ),
        "{done:?}"
    );
    assert_ne!(
        done.get("coverage").and_then(|v| v.as_str()),
        Some("complete"),
        "{done:?}"
    );
    assert_eq!(
        scalar("SELECT count(*) FROM main.scan_run WHERE root_id=?1 AND state='completed'"),
        completed,
        "a refused EndRun must not publish a completed run"
    );
    assert_eq!(
        scalar("SELECT generation FROM main.root WHERE id=?1"),
        generation,
        "a refused EndRun must not publish a sweep"
    );
    f3_wait_live(svc);
}

/// #731: a half-applied revoke, whose durable grant commit landed but whose catalogue apply failed
/// (the live cancel of the run was refused), leaves the run running and claiming its root. The
/// run's final EndRun must still be refused by the grant-state guard in `active_run`: no completed
/// run and no sweep. #733: the refused finish must also settle the run and release its root.
#[cfg(windows)]
#[test]
fn a_half_applied_revoke_still_refuses_the_final_end_run() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    let inner = &svc.inner;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    let root_row = inner.root_row(&lab.root).unwrap();
    let grant = crate::db::roots(&inner.db(), Some(root_row))
        .unwrap()
        .remove(0)
        .grant;
    let scalar =
        |sql: &str| -> i64 { inner.db().query_row(sql, [root_row], |r| r.get(0)).unwrap() };
    let generation = scalar("SELECT generation FROM main.root WHERE id=?1");
    let completed =
        scalar("SELECT count(*) FROM main.scan_run WHERE root_id=?1 AND state='completed'");
    inner.scan.gate.arm();
    let job = start(svc, &lab.root, "full");
    inner.scan.gate.wait_reached();
    // Test-local: the trigger lives on this lab's own catalogue and is dropped right after the
    // revoke, so it only fails the revoke's own cancel of the run.
    inner
        .db()
        .execute_batch(
            "CREATE TRIGGER main.half_applied_cancel_refusal BEFORE UPDATE OF state ON scan_run
             WHEN NEW.state='cancelled' BEGIN SELECT RAISE(ABORT,'injected'); END;",
        )
        .unwrap();
    let revoked = inner.catalog.writer().call(WriteCommand::RevokeGrant {
        grant_id: grant,
        revoked_at_ns: crate::now_ns(),
    });
    inner
        .db()
        .execute_batch("DROP TRIGGER main.half_applied_cancel_refusal")
        .unwrap();
    // The half-applied state: the grant is revoked durably, the run and its root claim are not.
    assert!(
        revoked.is_err(),
        "the injected cancel must fail the revoke: {revoked:?}"
    );
    let grant_state: String = inner
        .db()
        .query_row(
            "SELECT state FROM st.root_grant WHERE id=?1",
            [grant],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(grant_state, "revoked");
    assert_eq!(
        scalar("SELECT count(*) FROM main.scan_run WHERE root_id=?1 AND state='running'"),
        1
    );
    assert_eq!(
        scalar("SELECT count(*) FROM main.root WHERE id=?1 AND active_run IS NOT NULL"),
        1
    );
    inner.scan.gate.release();
    let done = settle(svc, &job);
    assert!(
        matches!(
            done.get("state").and_then(|v| v.as_str()),
            Some("cancelled" | "failed")
        ),
        "{done:?}"
    );
    assert_ne!(
        done.get("coverage").and_then(|v| v.as_str()),
        Some("complete"),
        "{done:?}"
    );
    assert_eq!(
        scalar("SELECT count(*) FROM main.scan_run WHERE root_id=?1 AND state='completed'"),
        completed,
        "a refused EndRun must not publish a completed run"
    );
    assert_eq!(
        scalar("SELECT generation FROM main.root WHERE id=?1"),
        generation,
        "a refused EndRun must not publish a sweep"
    );
    assert_eq!(
        scalar("SELECT count(*) FROM main.scan_run WHERE root_id=?1 AND state='running'"),
        0,
        "a refused finish must settle the revoked run"
    );
    assert_eq!(
        scalar("SELECT count(*) FROM main.scan_run WHERE root_id=?1 AND state IN ('cancelled','failed') AND json_extract(error_json,'$.code')='grant_not_active' AND finished_at_ns IS NOT NULL"),
        1,
        "the settled run must record why it could not finish"
    );
    assert_eq!(
        scalar("SELECT count(*) FROM main.root WHERE id=?1 AND active_run IS NOT NULL"),
        0,
        "a refused finish must release the root without restart"
    );
    assert_eq!(
        scalar("SELECT count(*) FROM main.root WHERE id=?1 AND state='scanning'"),
        0
    );
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
pub(crate) fn start(svc: &Service, root: &str, mode: &str) -> String {
    let (job, _) = ok(
        "scan.start",
        call(svc, "scan.start", json!({"root_id": root, "mode": mode})),
    );
    job["job"]["job_id"].as_str().unwrap().to_owned()
}

#[cfg(windows)]
pub(crate) fn settle(svc: &Service, job: &str) -> Payload {
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
    assert_eq!(done["state"], "completed", "initial scan state: {done:?}");
    assert_eq!(done["coverage"], "complete", "initial scan coverage");
    let slice = lab_slice(svc, &lab.root);
    assert_eq!(
        slice_totals(&slice),
        walk(&lab.path),
        "initial catalogue totals versus independent walk"
    );
    assert_eq!(
        slice_totals(&slice),
        (5, 3, 165),
        "initial catalogue totals versus fixture"
    );
    assert_eq!(
        slice["aggregate_state"], "consistent",
        "initial catalogue aggregate state: {slice:?}"
    );
    let (roots, _) = ok("roots.list", call(svc, "roots.list", json!({})));
    assert_eq!(
        roots["roots"][0]["scan"]["coverage"], "complete",
        "roots.list scan coverage"
    );
    assert_eq!(
        roots["roots"][0]["scan"]["last_job_id"],
        json!(job),
        "roots.list last_job_id"
    );
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
                    assert_eq!(
                        e.data["root_id"],
                        lab.root.as_str(),
                        "tree.invalidated root_id"
                    );
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
        "completed",
        "refresh scan state"
    );
    assert_eq!(
        slice_totals(&lab_slice(svc, &lab.root)),
        (5, 3, 165),
        "refreshed catalogue totals versus fixture"
    );
}

/// #719: progress is observable before the writer finishes, and final counters remain exact.
#[cfg(windows)]
#[test]
fn scan_jobs_get_reports_progress_while_a_lab_scan_is_running() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    svc.inner.freshness.pause_pump();
    svc.inner.scan.gate.arm();
    let id = start(svc, &lab.root, "full");
    svc.inner.scan.gate.wait_reached();
    let (during, _) = ok("jobs.get", call(svc, "jobs.get", json!({"job_id":id})));
    let (roots, _) = ok("roots.list", call(svc, "roots.list", json!({})));
    // Release before assertions so even the red test leaves no blocked worker.
    svc.inner.scan.gate.release();
    let done = settle(svc, &id);
    assert_eq!(during["job"]["state"], "running");
    assert_eq!(during["job"]["progress"]["phase"], "enumerating");
    assert_eq!(roots["roots"][0]["scan"]["last_job_id"], id);
    assert_eq!(done["progress"]["phase"], "done");
    assert_eq!(done["progress"]["indexed_files"], 5);
    assert_eq!(done["progress"]["indexed_dirs"], 3);
    assert_eq!(done["progress"]["logical_bytes"], "165");
}

/// #719/#808: a directory-budget stop remains a named, finished partial scan at the service boundary.
#[cfg(windows)]
#[test]
fn scan_directory_cap_reports_a_named_limit_and_partial_history() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    svc.inner.freshness.pause_pump();
    let (started, _) = ok(
        "scan.start",
        call(
            svc,
            "scan.start",
            json!({"root_id":lab.root,"mode":"full","budget":{"max_dirs":1,"threads":1}}),
        ),
    );
    let id = started["job"]["job_id"].as_str().unwrap();
    let done = settle(svc, id);
    assert_eq!(done["state"], "completed");
    assert_eq!(done["coverage"], "partial");
    assert_eq!(done["progress"]["limit_hit"], true);
    assert_eq!(done["progress"]["limit_reason"], "directory_limit");
    let (roots, _) = ok("roots.list", call(svc, "roots.list", json!({})));
    assert_eq!(roots["roots"][0]["scan"]["last_job_id"], id);
    assert_eq!(roots["roots"][0]["scan"]["coverage"], "partial");
    assert!(!roots["roots"][0]["scan"]["finished_at"].is_null());
}

#[cfg(windows)]
fn search_freshness_page(lab: &Lab) -> Value {
    let (page, _) = ok(
        "search.query",
        call(
            &lab.svc,
            "search.query",
            json!({"root_id":lab.root,"text":"bin","extension":null,"min_bytes":null,"max_bytes":null,"modified_after":null,"modified_before":null,"kind":"file","limit":100,"cursor":null}),
        ),
    );
    Value::Object(page)
}

#[cfg(windows)]
#[test]
fn search_freshness_preserves_never_scanned_coverage() {
    let Some(lab) = lab() else { return };
    lab.svc.inner.freshness.pause_pump();
    let page = search_freshness_page(&lab);
    assert_eq!(page["roots"][0]["scan_state"], "never_scanned", "{page:?}");
    assert_eq!(page["roots"][0]["coverage"], "unscanned", "{page:?}");
    assert_eq!(page["index_state"], "current", "{page:?}");
    assert_eq!(page["partial"], true);
}

#[cfg(windows)]
#[test]
fn search_freshness_first_scan_builds_then_becomes_current() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    svc.inner.freshness.pause_pump();
    svc.inner.scan.gate.arm();
    let job = start(svc, &lab.root, "full");
    svc.inner.scan.gate.wait_reached();
    let during = search_freshness_page(&lab);
    // Always release the writer before asserting, including on the unfixed implementation.
    svc.inner.scan.gate.release();
    assert_eq!(settle(svc, &job)["state"], "completed");
    let after = search_freshness_page(&lab);
    assert_eq!(during["index_state"], "building", "{during:?}");
    assert_eq!(during["roots"][0]["scan_state"], "scanning");
    assert_eq!(during["roots"][0]["coverage"], "partial");
    assert_eq!(during["partial"], true);
    assert_eq!(after["index_state"], "current", "{after:?}");
    assert_eq!(after["roots"][0]["scan_state"], "complete");
    assert_eq!(after["roots"][0]["coverage"], "complete");
    assert_eq!(after["partial"], false);
}

#[cfg(windows)]
#[test]
fn search_freshness_rescan_keeps_existing_index_stale() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.freshness.pause_pump();
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    let before = search_freshness_page(&lab);
    svc.inner.scan.gate.arm();
    let job = start(svc, &lab.root, "full");
    svc.inner.scan.gate.wait_reached();
    let during = search_freshness_page(&lab);
    svc.inner.scan.gate.release();
    assert_eq!(settle(svc, &job)["state"], "completed");
    let after = search_freshness_page(&lab);
    for page in [&before, &during] {
        assert_eq!(page["index_state"], "stale", "{page:?}");
        assert_eq!(page["roots"][0]["scan_state"], "stale");
        assert_eq!(page["roots"][0]["coverage"], "stale");
        assert_eq!(page["partial"], true);
    }
    assert_eq!(after["index_state"], "current", "{after:?}");
    assert_eq!(after["partial"], false);
}

#[cfg(windows)]
#[test]
fn search_freshness_uses_prior_scan_state_from_its_page_snapshot() {
    const SEARCH: (Command, &str) = (Command::SearchQuery, "search_page_captured");
    const PUBLISHED: (Command, &str) = (Command::ScanStart, "full_catalogue_published");
    // Release both workers before scoped-thread cleanup, including after a failed assertion.
    struct Release<'a>(&'a Service);
    impl Drop for Release<'_> {
        fn drop(&mut self) {
            self.0.inner.scan.gate.release();
            self.0.inner.hooks.open(SEARCH.0, SEARCH.1);
            self.0.inner.hooks.open(PUBLISHED.0, PUBLISHED.1);
        }
    }
    for rescan in [false, true] {
        let Some(lab) = lab() else { return };
        let svc = &lab.svc;
        if rescan {
            settle(svc, &start(svc, &lab.root, "full"));
            f3_wait_live(svc);
        }
        svc.inner.freshness.pause_pump();
        svc.inner.hooks.arrived.lock().unwrap().clear();
        svc.inner
            .hooks
            .closed
            .lock()
            .unwrap()
            .extend([SEARCH, PUBLISHED]);
        svc.inner.scan.gate.arm();
        let job = start(svc, &lab.root, "full");
        let during = std::thread::scope(|scope| {
            let _release = Release(svc);
            svc.inner.scan.gate.wait_reached();
            let searching = scope.spawn(|| search_freshness_page(&lab));
            svc.inner
                .hooks
                .until("search page captured while scanning", |h| {
                    h.arrived.lock().unwrap().contains(&SEARCH)
                });
            svc.inner.scan.gate.release();
            svc.inner
                .hooks
                .until("EndRun committed before freshness finish", |h| {
                    h.arrived.lock().unwrap().contains(&PUBLISHED)
                });
            assert!(svc.inner.freshness.stale(1));
            svc.inner.hooks.open(SEARCH.0, SEARCH.1);
            searching.join().unwrap()
        });
        assert_eq!(settle(svc, &job)["state"], "completed");
        assert_eq!(
            during["index_state"],
            if rescan { "stale" } else { "building" },
            "rescan={rescan}: {during:?}"
        );
        assert_eq!(
            during["roots"][0]["scan_state"],
            if rescan { "stale" } else { "scanning" }
        );
        assert_eq!(during["partial"], true);
    }
}

#[cfg(windows)]
#[test]
fn f3_search_reports_freshness_without_enabling_federated_search() {
    let Some(lab) = lab() else { return };
    assert_eq!(
        settle(&lab.svc, &start(&lab.svc, &lab.root, "full"))["state"],
        "completed"
    );
    let (page, _) = ok(
        "search.query",
        call(
            &lab.svc,
            "search.query",
            json!({"root_id":null,"text":"top","extension":null,"min_bytes":null,"max_bytes":null,"modified_after":null,"modified_before":null,"kind":"any","limit":20,"cursor":null}),
        ),
    );
    assert_eq!(page["hits"].as_array().unwrap().len(), 1);
    let freshness = &page["hits"][0]["freshness"];
    assert!(freshness.is_object(), "missing per-hit freshness: {page:?}");
    serde_json::from_value::<SearchFreshness>(freshness.clone()).unwrap();
    let (hello, _) = ok("session.hello", call(&lab.svc, "session.hello", json!({})));
    assert_eq!(hello["features"]["federated_search"], false);
    // The ranked token index follows the same switch (#520): no index, no backfill, until it flips.
    let ranked: bool = rusqlite::Connection::open(lab._tmp.path().join("state").join("catalog.db"))
        .unwrap()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name IN ('search_token','search_dirty','search_backfill'))",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ranked, crate::FEDERATED_SEARCH);
}

#[cfg(windows)]
fn f3_hits(svc: &Service) -> Value {
    let (page, _) = ok(
        "search.query",
        call(
            svc,
            "search.query",
            json!({"root_id":null,"text":"bin","extension":null,"min_bytes":null,"max_bytes":null,"modified_after":null,"modified_before":null,"kind":"file","limit":100,"cursor":null}),
        ),
    );
    Value::Object(page)
}

#[cfg(windows)]
pub(crate) fn f3_wait_live(svc: &Service) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let page = f3_hits(svc);
        let (jobs, _) = ok("jobs.list", call(svc, "jobs.list", json!({"limit":50})));
        let settled = jobs["jobs"].as_array().unwrap().iter().all(|j| {
            !matches!(
                j["state"].as_str(),
                Some("queued" | "running" | "cancel_requested")
            )
        });
        if !svc.inner.freshness.stale(1)
            && settled
            && page["index_state"] == "current"
            && page["hits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|h| h["freshness"]["state"] != "stale")
        {
            return page;
        }
        assert!(Instant::now() < deadline, "never caught up: {page:?}; tracker={}; runs={:?}",svc.inner.freshness.debug(1),svc.inner.db().query_row("SELECT group_concat(mode||':'||state||':'||coalesce(counters_json,''),';') FROM main.scan_run WHERE root_id=1",[],|r|r.get::<_,Option<String>>(0)));
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(windows)]
fn f3_names(page: &Value) -> std::collections::BTreeSet<String> {
    page["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["path"]["text"].as_str().unwrap().to_owned())
        .collect()
}

#[cfg(windows)]
#[test]
fn f3_restart_rename_delete_and_create_catch_up_matches_fresh_scan() {
    let Some(lab) = lab() else { return };
    let Lab {
        _tmp,
        path,
        svc,
        root,
    } = lab;
    assert_eq!(
        settle(&svc, &start(&svc, &root, "full"))["coverage"],
        "complete"
    );
    f3_wait_live(&svc);
    let saved: (Vec<u8>, i64) = svc
        .inner
        .db()
        .query_row(
            "SELECT journal_id,next_usn FROM st.root_freshness",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(saved.0.len(), 8);
    drop(svc);
    std::fs::rename(path.join("top.bin"), path.join("renamed.bin")).unwrap();
    std::fs::remove_file(path.join("a/deep/x.bin")).unwrap();
    std::fs::create_dir(path.join("new-directory")).unwrap();
    std::fs::write(path.join("new-directory/created.bin"), [1; 12]).unwrap();
    // A same-volume sibling is not a granted search scope.
    std::fs::create_dir(_tmp.path().join("sibling")).unwrap();
    std::fs::write(_tmp.path().join("sibling/outside.bin"), [2; 7]).unwrap();
    let svc = open(&_tmp.path().join("state"), DatasetClass::Synthetic);
    let caught = f3_wait_live(&svc);
    assert_eq!(
        slice_totals(&lab_slice(&svc, &svc.inner.root_id(1))),
        walk(&path)
    );
    let names = f3_names(&caught);
    assert!(names.contains("renamed.bin"), "{caught:?}");
    assert!(
        names.contains("new-directory\\created.bin") || names.contains("new-directory/created.bin"),
        "{caught:?}"
    );
    assert!(!names
        .iter()
        .any(|n| n.ends_with("x.bin") || n.ends_with("outside.bin") || n.ends_with("top.bin")));
    let targeted: i64 = svc
        .inner
        .db()
        .query_row(
            "SELECT count(*) FROM main.scan_run WHERE mode='targeted'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        targeted > 0,
        "journal hints never selected targeted refresh scopes"
    );
    let root = svc.inner.root_id(1);
    settle(&svc, &start(&svc, &root, "full"));
    assert_eq!(names, f3_names(&f3_hits(&svc)));
}

#[cfg(windows)]
#[test]
fn f3_watch_survives_scan_completion_and_reconciles() {
    let Some(lab) = lab() else { return };
    settle(&lab.svc, &start(&lab.svc, &lab.root, "full"));
    std::fs::rename(lab.path.join("top.bin"), lab.path.join("live.bin")).unwrap();
    std::fs::remove_file(lab.path.join("a/3.bin")).unwrap();
    let stale = f3_hits(&lab.svc);
    assert_eq!(stale["hits"][0]["freshness"]["state"], "stale", "{stale:?}");
    assert!(stale["hits"][0]["freshness"]["at"].is_string());
    let live = f3_wait_live(&lab.svc);
    assert!(f3_names(&live).contains("live.bin"), "{live:?}");
    assert!(
        !f3_names(&live).iter().any(|n| n.ends_with("3.bin")),
        "{live:?}"
    );
}

/// #717: NTFS reports the scan's own directory listing (a last-access update) as MODIFIED, which
/// dirtied the root and started a second, automatic scan about a second after the pressed one.
#[cfg(windows)]
#[test]
fn f3_issue717_a_scan_does_not_dirty_its_own_root_through_reads() {
    let Some(lab) = lab() else { return };
    // The fixture's ChangeTimes must precede the scan by more than the clock-tick slack.
    std::thread::sleep(Duration::from_millis(400));
    settle(&lab.svc, &start(&lab.svc, &lab.root, "full"));
    // Lazily flushed notifications arrive within about a second; the pump runs every 200 ms.
    std::thread::sleep(Duration::from_millis(2500));
    let (jobs, _) = ok(
        "jobs.list",
        call(
            &lab.svc,
            "jobs.list",
            json!({"kinds": ["scan"], "limit": 50}),
        ),
    );
    assert_eq!(jobs["jobs"].as_array().unwrap().len(), 1, "{jobs:?}");
    assert!(
        !lab.svc.inner.freshness.stale(1),
        "tracker={}",
        lab.svc.inner.freshness.debug(1)
    );
}

#[cfg(windows)]
#[test]
fn f3_journal_wrap_dirties_scope_until_explicit_full_refresh() {
    for gap in [
        "UPDATE st.root_freshness SET journal_id=zeroblob(8)",
        "UPDATE st.root_freshness SET next_usn=-1",
    ] {
        let Some(lab) = lab() else { return };
        let Lab {
            _tmp,
            path: _,
            svc,
            root,
        } = lab;
        settle(&svc, &start(&svc, &root, "full"));
        f3_wait_live(&svc);
        assert_eq!(svc.inner.db().execute(gap, []).unwrap(), 1);
        drop(svc);
        let svc = open(&_tmp.path().join("state"), DatasetClass::Synthetic);
        let page = f3_hits(&svc);
        assert!(page["hits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h["freshness"]["state"] == "stale"));
        assert!(svc.inner.freshness.stale(1));
        let root = svc.inner.root_id(1);
        settle(&svc, &start(&svc, &root, "full"));
        f3_wait_live(&svc);
    }
}

#[cfg(windows)]
#[test]
fn f3_completed_partial_full_scan_is_current_without_claiming_complete_coverage() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    svc.inner.freshness.pause_pump();
    struct Denied(std::path::PathBuf);
    impl Drop for Denied {
        fn drop(&mut self) {
            assert!(std::process::Command::new("icacls")
                .arg(&self.0)
                .args(["/remove:d", "*S-1-1-0"])
                .output()
                .unwrap()
                .status
                .success());
        }
    }
    let denied = Denied(lab.path.join("a"));
    assert!(std::process::Command::new("icacls")
        .arg(&denied.0)
        .args(["/deny", "*S-1-1-0:(R)"])
        .output()
        .unwrap()
        .status
        .success());
    assert!(std::fs::read_dir(&denied.0).is_err());
    let job = settle(svc, &start(svc, &lab.root, "full"));
    assert_eq!(job["state"], "completed");
    assert_eq!(job["coverage"], "partial");
    let page = f3_hits(svc);
    assert_eq!(page["index_state"], "current", "{page:?}");
    assert_eq!(page["partial"], true);
    assert_eq!(page["roots"][0]["coverage"], "partial");
    assert!(!page["hits"].as_array().unwrap().is_empty());
    assert!(page["hits"]
        .as_array()
        .unwrap()
        .iter()
        .all(|hit| { hit["freshness"]["state"] == "live" && hit["freshness"]["at"].is_null() }));
    let (roots, _) = ok("roots.list", call(svc, "roots.list", json!({})));
    assert_eq!(roots["roots"][0]["scan"]["coverage"], "partial");
    assert_eq!(roots["roots"][0]["totals"]["complete"], false);
    assert_eq!(job["progress"]["indexed_dirs"], 1);
    assert_eq!(roots["roots"][0]["totals"]["dirs"], 2);
    assert_eq!(job["progress"]["failed"], 1);
}

/// #774: a full scan that ran to the end around a denied folder is a finished (partial) run, not a
/// cancelled one: roots.list and alerts see it, and the denied folder's catalogued rows survive.
#[cfg(windows)]
#[test]
fn finished_partial_full_scan_is_recorded_without_reconciling_the_denied_subtree() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    assert_eq!(
        settle(svc, &start(svc, &lab.root, "full"))["coverage"],
        "complete"
    );
    struct Denied(std::path::PathBuf);
    impl Drop for Denied {
        fn drop(&mut self) {
            let _ = std::process::Command::new("icacls")
                .arg(&self.0)
                .args(["/remove:d", "*S-1-1-0"])
                .output();
        }
    }
    let denied = Denied(lab.path.join("a"));
    assert!(std::process::Command::new("icacls")
        .arg(&denied.0)
        .args(["/deny", "*S-1-1-0:(R)"])
        .output()
        .unwrap()
        .status
        .success());
    assert!(std::fs::read_dir(&denied.0).is_err());
    let job = settle(svc, &start(svc, &lab.root, "full"));
    assert_eq!(job["state"], "completed");
    assert_eq!(job["coverage"], "partial");
    let db = svc.inner.db();
    let (state, complete): (String, Option<bool>) = db
        .query_row(
            "SELECT state,json_extract(counters_json,'$.complete') FROM main.scan_run ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((state.as_str(), complete), ("completed", Some(false)));
    // The denied folder's rows from the first scan are not reconciled as absent.
    let kept: (i64, i64) = db
        .query_row(
            "SELECT (SELECT count(*) FROM main.dir WHERE name IN ('a','deep') AND listing_state!='absent_pending'),
                    (SELECT count(*) FROM main.file f JOIN main.dir d ON d.id=f.dir_id WHERE d.name IN ('a','deep'))",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(kept, (2, 4));
    drop(db);
    let (roots, _) = ok("roots.list", call(svc, "roots.list", json!({})));
    let scan = &roots["roots"][0]["scan"];
    assert_eq!(scan["coverage"], "partial");
    assert!(!scan["finished_at"].is_null(), "{scan:?}");
    assert!(!scan["generation"].is_null(), "{scan:?}");
    let (alerts, _) = ok("alerts.list", call(svc, "alerts.list", json!({})));
    let growth = alerts["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "fast_growth")
        .unwrap();
    assert_eq!(growth["coverage"], "partial", "{growth:?}");
    assert!(
        !growth["note"]
            .as_str()
            .unwrap()
            .contains("No completed scan"),
        "{growth:?}"
    );
}

/// #781: limits produce a finished partial sample, while freshness still requires an unlimited pass.
#[cfg(windows)]
#[test]
fn f3_limited_full_scan_stays_stale_until_an_unlimited_pass() {
    for budget in [json!({"max_entries": 1}), json!({"max_dirs": 1})] {
        let Some(lab) = lab() else { return };
        let svc = &lab.svc;
        let claim = svc.inner.freshness.explicit(1);
        let (started, _) = ok(
            "scan.start",
            call(
                svc,
                "scan.start",
                json!({"root_id": lab.root, "mode": "full", "budget": budget}),
            ),
        );
        let job = settle(svc, started["job"]["job_id"].as_str().unwrap());
        assert_eq!(job["state"], "completed");
        assert_eq!(job["coverage"], "partial");
        assert_eq!(job["progress"]["limit_hit"], true);
        assert_eq!(f3_hits(svc)["index_state"], "stale");
        let (state, complete): (String, Option<bool>) = svc
            .inner
            .db()
            .query_row(
                "SELECT state,json_extract(counters_json,'$.complete') FROM main.scan_run ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((state.as_str(), complete), ("completed", Some(false)));
        let (roots, _) = ok("roots.list", call(svc, "roots.list", json!({})));
        let scan = &roots["roots"][0]["scan"];
        assert_eq!(scan["coverage"], "partial");
        assert!(!scan["finished_at"].is_null(), "{scan:?}");
        assert!(!scan["generation"].is_null(), "{scan:?}");
        assert_eq!(roots["roots"][0]["totals"]["complete"], false);
        let (alerts, _) = ok("alerts.list", call(svc, "alerts.list", json!({})));
        let growth = alerts["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["kind"] == "fast_growth")
            .unwrap();
        assert_eq!(growth["coverage"], "partial", "{growth:?}");
        assert!(
            !growth["note"]
                .as_str()
                .unwrap()
                .contains("No completed scan"),
            "{growth:?}"
        );
        drop(claim);
        assert_eq!(
            settle(svc, &start(svc, &lab.root, "full"))["coverage"],
            "complete"
        );
        assert_eq!(f3_wait_live(svc)["index_state"], "current");
    }
}

#[cfg(windows)]
#[test]
fn f3_cancelled_full_scan_stays_stale_until_a_finished_pass() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    let claim = svc.inner.freshness.explicit(1);
    svc.inner.scan.gate.arm();
    let job = start(svc, &lab.root, "full");
    svc.inner.scan.gate.wait_reached();
    let cancelled = call(svc, "scan.cancel", json!({"job_id": job}));
    svc.inner.scan.gate.release();
    ok("scan.cancel", cancelled);
    assert_eq!(settle(svc, &job)["state"], "cancelled");
    assert_eq!(f3_hits(svc)["index_state"], "stale");
    drop(claim);
    assert_eq!(
        settle(svc, &start(svc, &lab.root, "full"))["coverage"],
        "complete"
    );
    assert_eq!(f3_wait_live(svc)["index_state"], "current");
}

#[cfg(windows)]
#[test]
fn f3_overflow_marks_hits_and_root_stale_until_full_refresh() {
    let Some(lab) = lab() else { return };
    settle(&lab.svc, &start(&lab.svc, &lab.root, "full"));
    f3_wait_live(&lab.svc);
    // Same root-dirty hint as a zero-byte/1022 kernel completion or a dropped batch.
    lab.svc
        .inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    let page = f3_hits(&lab.svc);
    assert_eq!(page["index_state"], "stale");
    assert_eq!(page["partial"], true);
    assert!(page["hits"]
        .as_array()
        .unwrap()
        .iter()
        .all(|h| h["freshness"]["state"] == "stale" && h["freshness"]["at"].is_string()));
    let before = f3_names(&page);
    assert_eq!(before, f3_names(&f3_wait_live(&lab.svc)));
}

#[cfg(windows)]
#[test]
fn f3_unchanged_restart_reports_caught_up_and_revoke_stops_watch() {
    let Some(lab) = lab() else { return };
    let Lab {
        _tmp,
        path: _,
        svc,
        root,
    } = lab;
    settle(&svc, &start(&svc, &root, "full"));
    let names = f3_names(&f3_wait_live(&svc));
    drop(svc);
    let svc = open(&_tmp.path().join("state"), DatasetClass::Synthetic);
    let page = f3_wait_live(&svc);
    assert_eq!(names, f3_names(&page));
    assert!(
        page["hits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h["freshness"]["state"] == "caught_up" && h["freshness"]["at"].is_string()),
        "{page:?}"
    );
    let root = svc.inner.root_id(1);
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id":root,"purge_catalog":false}),
        ),
    );
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    assert!(!svc.inner.freshness.stale(1));
    assert!(f3_hits(&svc)["hits"].as_array().unwrap().is_empty());
    ok(
        "roots.forget",
        call(&svc, "roots.forget", json!({"root_id":root})),
    );
    let remaining: i64 = svc
        .inner
        .db()
        .query_row("SELECT count(*) FROM st.root_freshness", [], |r| r.get(0))
        .unwrap();
    assert_eq!(remaining, 0);
}

#[cfg(windows)]
#[test]
fn f3_revoke_fences_a_watch_still_being_armed() {
    let Some(lab) = lab() else { return };
    let Lab {
        _tmp,
        path: _,
        svc,
        root: _,
    } = lab;
    let svc = Arc::new(svc);
    let phase = (Command::ScanStart, "persistent_watch_resolved");
    svc.inner.hooks.arrived.lock().unwrap().clear();
    svc.inner.hooks.closed.lock().unwrap().push(phase);
    svc.inner.freshness.remove(1);
    let opening = {
        let svc = svc.clone();
        std::thread::spawn(move || crate::freshness::ensure_roots(&svc.inner))
    };
    svc.inner
        .hooks
        .until("persistent watch root resolved", |h| {
            h.arrived.lock().unwrap().contains(&phase)
        });
    let grant = crate::db::roots(&svc.inner.db(), Some(1)).unwrap()[0].grant;
    let (sent, ack) = std::sync::mpsc::channel();
    let revoking = {
        let svc = svc.clone();
        std::thread::spawn(move || {
            sent.send(svc.inner.revoke(grant)).unwrap();
        })
    };
    svc.inner.hooks.until("durable revocation", |_| {
        svc.inner
            .db()
            .query_row(
                "SELECT state='revoked' FROM st.root_grant WHERE id=?1",
                [grant],
                |r| r.get::<_, bool>(0),
            )
            .unwrap()
    });
    // Release the test gate before asserting, so a red regression cannot strand a native thread.
    let early = ack.recv_timeout(Duration::from_secs(1)).ok();
    svc.inner.hooks.open(phase.0, phase.1);
    if early.is_none() {
        ack.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    }
    let opened = opening.join();
    revoking.join().unwrap();
    assert!(early.is_some(), "revoke waited for the slow watch arm");
    early.unwrap().unwrap();
    opened.unwrap();
    assert!(svc.inner.freshness.debug(1).is_empty());
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    assert!(!svc.inner.freshness.stale(1));
}

#[cfg(windows)]
#[test]
fn f3_watch_event_between_start_and_journal_read_cannot_be_caught_up() {
    let Some(lab) = lab() else { return };
    settle(&lab.svc, &start(&lab.svc, &lab.root, "full"));
    f3_wait_live(&lab.svc);
    let svc = Arc::new(lab.svc);
    let phase = (Command::ScanStart, "persistent_watch_started");
    svc.inner.hooks.arrived.lock().unwrap().clear();
    svc.inner.hooks.closed.lock().unwrap().push(phase);
    svc.inner.freshness.remove(1);
    let opening = {
        let svc = svc.clone();
        std::thread::spawn(move || crate::freshness::ensure_roots(&svc.inner))
    };
    svc.inner
        .hooks
        .until("watch started before journal read", |h| {
            h.arrived.lock().unwrap().contains(&phase)
        });
    // A watch hint can precede its USN record (for example, while a renamed handle is open).
    // Hold automatic retries so the assertion observes catch-up, not a subsequent full scan.
    svc.inner.freshness.exhaust_budget(1);
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    svc.inner.hooks.open(phase.0, phase.1);
    opening.join().unwrap();
    assert!(
        svc.inner.freshness.stale(1),
        "lost the in-window watch event"
    );
    assert!(svc.inner.freshness.debug(1).contains("full=true"));
    assert!(f3_hits(&svc)["hits"]
        .as_array()
        .unwrap()
        .iter()
        .all(|h| h["freshness"]["state"] == "stale"));
}

#[cfg(windows)]
#[test]
fn f3_journal_replay_does_not_hold_db_or_watch_setup() {
    let Some(lab) = lab() else { return };
    settle(&lab.svc, &start(&lab.svc, &lab.root, "full"));
    f3_wait_live(&lab.svc);
    let grant = crate::db::roots(&lab.svc.inner.db(), Some(1)).unwrap()[0].grant;
    let svc = Arc::new(lab.svc);
    let phase = (Command::ScanStart, "persistent_journal_read");
    svc.inner.hooks.arrived.lock().unwrap().clear();
    svc.inner.hooks.closed.lock().unwrap().push(phase);
    svc.inner.freshness.remove(1);
    let opening = {
        let svc = svc.clone();
        std::thread::spawn(move || crate::freshness::ensure_roots(&svc.inner))
    };
    svc.inner.hooks.until("journal replay", |h| {
        h.arrived.lock().unwrap().contains(&phase)
    });
    let read_deadline = Instant::now() + Duration::from_secs(1);
    let read_available = loop {
        if svc.inner.db.try_lock().is_ok() {
            break true;
        }
        if Instant::now() >= read_deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let (sent, ack) = std::sync::mpsc::channel();
    let revoking = {
        let svc = svc.clone();
        std::thread::spawn(move || sent.send(svc.inner.revoke(grant)).unwrap())
    };
    let early = ack.recv_timeout(Duration::from_secs(2)).ok();
    // Always release before asserting, including on the old blocking implementation.
    svc.inner.hooks.open(phase.0, phase.1);
    opening.join().unwrap();
    revoking.join().unwrap();
    assert!(
        read_available,
        "journal replay held the service database mutex"
    );
    assert!(
        early.is_some_and(|r| r.is_ok()),
        "revoke waited on journal replay"
    );
    assert!(!svc.inner.freshness.stale(1));
}

#[cfg(windows)]
#[test]
fn f3_completed_foreground_job_does_not_mean_background_catalogue_is_quiescent() {
    let Some(lab) = lab() else { return };
    let foreground = start(&lab.svc, &lab.root, "full");
    assert_eq!(settle(&lab.svc, &foreground)["coverage"], "complete");
    f3_wait_live(&lab.svc);
    let full_runs = || -> i64 {
        lab.svc
            .inner
            .db()
            .query_row(
                "SELECT count(*) FROM main.scan_run WHERE mode!='targeted'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    let before = full_runs();
    let phase = (Command::ScanStart, "targeted_catalogue_published");
    lab.svc.inner.hooks.closed.lock().unwrap().push(phase);
    lab.svc.inner.freshness.record(
        1,
        vec![loomward_windows::watch::Change::Name {
            action: 1,
            name: "a\\new.bin".encode_utf16().collect(),
        }],
    );
    lab.svc.inner.freshness.ready_targeted_hint(1);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !lab.svc.inner.hooks.arrived.lock().unwrap().contains(&phase) {
        assert!(
            Instant::now() < deadline,
            "no targeted publication: {}",
            lab.svc.inner.freshness.debug(1)
        );
        std::thread::yield_now();
    }
    let complete: bool = lab
        .svc
        .inner
        .db()
        .query_row(
            "SELECT sub_complete FROM main.dir WHERE parent_id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let stale = lab.svc.inner.freshness.stale(1);
    let page = f3_hits(&lab.svc);
    let (job, _) = ok(
        "jobs.get",
        call(&lab.svc, "jobs.get", json!({"job_id":foreground})),
    );
    // Release before asserting so a regression cannot strand the scan worker.
    lab.svc.inner.hooks.open(phase.0, phase.1);
    assert_eq!(job["job"]["state"], "completed");
    assert_eq!(job["job"]["coverage"], "complete");
    assert!(
        !complete,
        "targeted pass should honestly leave root coverage partial"
    );
    assert!(stale, "background publication must remain visibly stale");
    assert_eq!(page["partial"], true);
    assert!(page["hits"]
        .as_array()
        .unwrap()
        .iter()
        .all(|hit| hit["freshness"]["state"] == "stale"));
    let current = f3_wait_live(&lab.svc);
    assert_eq!(current["index_state"], "current");
    assert_eq!(current["partial"], true);
    assert_eq!(full_runs(), before, "settled hints started a full fallback");
    assert!(
        !lab.svc
            .inner
            .db()
            .query_row(
                "SELECT sub_complete FROM main.dir WHERE parent_id IS NULL",
                [],
                |r| r.get::<_, bool>(0),
            )
            .unwrap(),
        "targeted freshness must not establish complete root coverage"
    );
}

#[cfg(windows)]
#[test]
fn f3_successful_full_scan_with_late_hints_restarts_refresh_budget() {
    let Some(lab) = lab() else { return };
    settle(&lab.svc, &start(&lab.svc, &lab.root, "full"));
    f3_wait_live(&lab.svc);
    lab.svc.inner.scan.gate.arm();
    let job = start(&lab.svc, &lab.root, "full");
    lab.svc.inner.scan.gate.wait_reached();
    // Simulate three earlier refresh attempts, then a buffered hint during a successful scan.
    lab.svc.inner.freshness.exhaust_budget(1);
    lab.svc
        .inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    lab.svc.inner.scan.gate.release();
    assert_eq!(settle(&lab.svc, &job)["coverage"], "complete");
    f3_wait_live(&lab.svc);
}

#[cfg(windows)]
#[test]
fn f3_automatic_start_records_the_job_under_the_claim_lock() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.freshness.pause_pump();
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    svc.inner.freshness.ready_targeted_hint(1);
    let held = (Command::ScanStart, "automatic_claim_checked");
    svc.inner
        .hooks
        .arrived
        .lock()
        .unwrap()
        .retain(|at| *at != held);
    svc.inner.hooks.closed.lock().unwrap().push(held);
    let (fenced, auto) = std::thread::scope(|s| {
        let refreshing = s.spawn(|| crate::freshness::refresh(&svc.inner));
        svc.inner.hooks.until("automatic claim checked", |h| {
            h.arrived.lock().unwrap().contains(&held)
        });
        let fenced = !svc.inner.freshness.unclaimed_try(1);
        let claiming = s.spawn(|| svc.inner.freshness.explicit(1));
        svc.inner.hooks.open(held.0, held.1);
        refreshing.join().unwrap();
        (fenced, claiming.join().unwrap().auto.clone())
    });
    assert!(
        fenced,
        "claim lock was released between the check and automatic start"
    );
    let auto = auto.expect("explicit claim missed the automatic job");
    assert_eq!(settle(svc, auto.as_str())["state"], "completed");
}

#[cfg(windows)]
#[test]
fn f3_completed_automatic_start_preserves_the_reset_budget() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.freshness.pause_pump();
    let list = serde_json::from_value(json!({"limit":50})).unwrap();
    let before = svc.inner.engine.job_list(&list).unwrap();
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    svc.inner.freshness.ready_targeted_hint(1);
    let held = (Command::ScanStart, "automatic_submitted");
    svc.inner
        .hooks
        .arrived
        .lock()
        .unwrap()
        .retain(|at| *at != held);
    svc.inner.hooks.closed.lock().unwrap().push(held);
    let state = std::thread::scope(|s| {
        let refreshing = s.spawn(|| crate::freshness::refresh(&svc.inner));
        svc.inner.hooks.until("automatic job submitted", |h| {
            h.arrived.lock().unwrap().contains(&held)
        });
        let jobs = svc.inner.engine.job_list(&list).unwrap();
        let latest = jobs
            .jobs
            .iter()
            .find(|j| !before.jobs.iter().any(|old| old.job_id == j.job_id))
            .unwrap();
        let state = settle(svc, latest.job_id.as_str())["state"].clone();
        svc.inner.hooks.open(held.0, held.1);
        refreshing.join().unwrap();
        state
    });
    assert_eq!(state, "completed");
    assert_eq!(
        svc.inner.freshness.attempts(1),
        0,
        "late start bookkeeping overwrote the successful pass's reset"
    );
}

#[cfg(windows)]
#[test]
fn f3_issue617_completed_catchup_retries_its_checkpoint() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.freshness.pause_pump();
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    for _ in 0..2 {
        svc.inner.freshness.ready_targeted_hint(1);
        svc.inner
            .hooks
            .fail_automatic_start
            .store(true, std::sync::atomic::Ordering::SeqCst);
        crate::freshness::refresh(&svc.inner);
    }
    svc.inner.freshness.ready_targeted_hint(1);
    svc.inner
        .hooks
        .fail_freshness_checkpoint
        .store(true, std::sync::atomic::Ordering::SeqCst);
    crate::freshness::refresh(&svc.inner);
    let auto = svc.inner.freshness.explicit(1).auto.clone().unwrap();
    assert_eq!(settle(svc, auto.as_str())["coverage"], "complete");
    assert!(
        svc.inner.freshness.stale(1),
        "unconfirmed coverage must stay stale"
    );
    assert_eq!(svc.inner.freshness.attempts(1), 3);
    crate::freshness::refresh(&svc.inner);
    assert!(
        !svc.inner.freshness.stale(1),
        "completed catch-up lost its settle witness"
    );
    assert_eq!(svc.inner.freshness.attempts(1), 0);
    assert!(f3_hits(svc)["hits"]
        .as_array()
        .unwrap()
        .iter()
        .all(|h| h["freshness"]["state"] == "live"));
}

#[cfg(windows)]
#[test]
fn issue776_default_scan_shares_workers_with_an_owner_scan_and_freshness() {
    if loomward_windows::enumerate::skip_elevated_test() {
        eprintln!("skipped: elevated token; native scans refuse elevation");
        return;
    }
    for threads in [1, 32] {
        let tmp = temp_root();
        let state = tmp.path().join("state");
        let mut paths = Vec::new();
        for name in ["lab-first", "lab-second", "lab-refresh"] {
            let path = tmp.path().join(name);
            crate::lab::register(
                &state,
                &path,
                name,
                "S",
                "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            )
            .unwrap();
            std::fs::write(path.join("synthetic.bin"), b"synthetic").unwrap();
            paths.push(path.canonicalize().unwrap());
        }
        let svc = Service::open(Config {
            state_dir: state,
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: paths,
        })
        .unwrap();
        svc.inner.freshness.pause_pump();
        let rows = crate::db::roots(&svc.inner.db(), None).unwrap();
        let roots: Vec<_> = rows.iter().map(|r| svc.inner.root_id(r.root)).collect();
        for root in &roots {
            assert_eq!(
                settle(&svc, &start(&svc, root, "full"))["coverage"],
                "complete"
            );
        }
        let held = (Command::ScanStart, "full_catalogue_published");
        svc.inner
            .hooks
            .arrived
            .lock()
            .unwrap()
            .retain(|at| *at != held);
        svc.inner.hooks.closed.lock().unwrap().push(held);
        let first = start(&svc, &roots[0], "full");
        svc.inner.hooks.until("first scan holds its workers", |h| {
            h.arrived.lock().unwrap().contains(&held)
        });
        let second = call(
            &svc,
            "scan.start",
            json!({"root_id":roots[1],"mode":"full","budget":{"threads":threads}}),
        );
        let third = if threads == 1 {
            svc.inner.freshness.record(
                rows[2].root,
                vec![loomward_windows::watch::Change::RootDirty],
            );
            svc.inner.freshness.ready_targeted_hint(rows[2].root);
            crate::freshness::refresh(&svc.inner);
            None
        } else {
            Some(call(
                &svc,
                "scan.start",
                json!({"root_id":roots[2],"mode":"full"}),
            ))
        };
        let auto = svc.inner.freshness.explicit(rows[2].root).auto.clone();
        let workers = svc.inner.engine.budgets_get().unwrap().pools[0]
            .current_workers
            .get();
        let first_state = svc
            .inner
            .engine
            .job_status(&JobId::new(&first).unwrap())
            .unwrap()
            .state;
        // Always unblock before asserting: a red regression must not strand a worker on drop.
        svc.inner.hooks.open(held.0, held.1);
        assert_eq!(settle(&svc, &first)["coverage"], "complete");
        let (second, _) = ok("scan.start", second);
        let second = settle(&svc, second["job"]["job_id"].as_str().unwrap());
        assert_eq!(second["coverage"], "complete", "{second:?}");
        assert_eq!(first_state, JobState::Running);
        assert_eq!(workers, 8, "all three jobs share the bounded pool");
        if let Some(third) = third {
            let refused = err(third);
            assert_eq!((refused.code, refused.retryable), (ErrorCode::Busy, true));
            assert!(refused.message.contains("retry"));
            assert!(auto.is_none());
        } else {
            let auto = auto.expect("another root refreshes before the owner scans finish");
            assert_eq!(settle(&svc, auto.as_str())["coverage"], "complete");
        }
        // With the pool released, another root can start normally.
        assert_eq!(
            settle(&svc, &start(&svc, &roots[2], "full"))["coverage"],
            "complete"
        );
    }
}

#[cfg(windows)]
#[test]
fn f3_issue617_explicit_scan_preempts_another_roots_pool() {
    if loomward_windows::enumerate::skip_elevated_test() {
        return;
    }
    let tmp = temp_root();
    let state = tmp.path().join("state");
    let mut paths = Vec::new();
    for name in ["lab-first", "lab-second"] {
        let path = tmp.path().join(name);
        crate::lab::register(
            &state,
            &path,
            name,
            "S",
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        std::fs::create_dir(path.join("child")).unwrap();
        std::fs::write(path.join("child/file.bin"), b"synthetic").unwrap();
        paths.push(path.canonicalize().unwrap());
    }
    let svc = Service::open(Config {
        state_dir: state,
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
        grant_roots: paths,
    })
    .unwrap();
    svc.inner.freshness.pause_pump();
    // Exercise actual pool exhaustion even though default scans now leave headroom.
    svc.inner
        .engine
        .budgets_set(
            &serde_json::from_value(json!({"pool":"scan_enumerate","max_workers":1})).unwrap(),
        )
        .unwrap();
    let rows = crate::db::roots(&svc.inner.db(), None).unwrap();
    let first = svc.inner.root_id(rows[0].root);
    let second = svc.inner.root_id(rows[1].root);
    for root in [&first, &second] {
        assert_eq!(
            settle(&svc, &start(&svc, root, "full"))["coverage"],
            "complete"
        );
    }
    svc.inner.freshness.record(
        rows[0].root,
        vec![loomward_windows::watch::Change::RootDirty],
    );
    svc.inner.freshness.ready_targeted_hint(rows[0].root);
    svc.inner.scan.gate.arm();
    crate::freshness::refresh(&svc.inner);
    svc.inner.scan.gate.wait_reached();
    let auto = svc
        .inner
        .freshness
        .explicit(rows[0].root)
        .auto
        .clone()
        .unwrap();
    let held = (Command::ScanStart, "full_catalogue_published");
    svc.inner
        .hooks
        .arrived
        .lock()
        .unwrap()
        .retain(|at| *at != held);
    svc.inner.hooks.closed.lock().unwrap().push(held);
    let (answer, stale) = std::thread::scope(|s| {
        let asking = s.spawn(|| call(&svc, "scan.start", json!({"root_id":second,"mode":"full"})));
        svc.inner
            .hooks
            .until("explicit response or automatic cancellation", |_| {
                asking.is_finished()
                    || svc.inner.engine.job_status(&auto).unwrap().state
                        == JobState::CancelRequested
            });
        let stale = svc.inner.freshness.stale(rows[0].root);
        svc.inner.scan.gate.release();
        (asking.join().unwrap(), stale)
    });
    let (job, _) = ok("scan.start", answer);
    assert!(stale, "pre-emption cannot make the other root fresh");
    assert_eq!(settle(&svc, auto.as_str())["state"], "cancelled");
    svc.inner.hooks.until("owner scan holds the pool", |h| {
        h.arrived.lock().unwrap().contains(&held)
    });
    for _ in 0..3 {
        crate::freshness::refresh(&svc.inner);
    }
    let attempts = svc.inner.freshness.attempts(rows[0].root);
    svc.inner.hooks.open(held.0, held.1);
    assert_eq!(
        settle(&svc, job["job"]["job_id"].as_str().unwrap())["coverage"],
        "complete"
    );
    assert!(svc.inner.freshness.stale(rows[0].root));
    assert_eq!(
        attempts, 0,
        "pool pressure spent the deferred catch-up's retries"
    );
    crate::freshness::refresh(&svc.inner);
    let retry = svc
        .inner
        .freshness
        .explicit(rows[0].root)
        .auto
        .clone()
        .unwrap();
    assert_eq!(settle(&svc, retry.as_str())["coverage"], "complete");
    assert!(!svc.inner.freshness.stale(rows[0].root));
}

#[cfg(windows)]
#[test]
fn f3_issue617_deferred_checkpoint_preserves_newer_hints() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.freshness.pause_pump();
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    svc.inner.freshness.ready_targeted_hint(1);
    svc.inner
        .hooks
        .fail_freshness_checkpoint
        .store(true, std::sync::atomic::Ordering::SeqCst);
    crate::freshness::refresh(&svc.inner);
    let auto = svc.inner.freshness.explicit(1).auto.clone().unwrap();
    assert_eq!(settle(svc, auto.as_str())["coverage"], "complete");
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    let claim = svc.inner.freshness.explicit(1);
    crate::freshness::refresh(&svc.inner);
    assert!(
        svc.inner.freshness.stale(1),
        "old completion cleared a newer hint"
    );
    drop(claim);
    svc.inner.freshness.ready_targeted_hint(1);
    crate::freshness::refresh(&svc.inner);
    let auto = svc.inner.freshness.explicit(1).auto.clone().unwrap();
    assert_eq!(settle(svc, auto.as_str())["coverage"], "complete");
    assert!(!svc.inner.freshness.stale(1));
}

#[cfg(windows)]
#[test]
fn f3_search_and_scan_completion_serialize_watch_checkpoints() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.freshness.pause_pump();
    svc.inner.scan.gate.arm();
    let job = start(svc, &lab.root, "full");
    svc.inner.scan.gate.wait_reached();
    let held = (Command::ScanStart, "freshness_checkpoint_claimed");
    let waiting = (Command::ScanStart, "freshness_checkpoint_waiting");
    svc.inner
        .hooks
        .arrived
        .lock()
        .unwrap()
        .retain(|at| *at != held && *at != waiting);
    svc.inner.hooks.closed.lock().unwrap().push(held);
    let fenced = std::thread::scope(|s| {
        let searching = s.spawn(|| svc.inner.hit_freshness(1));
        svc.inner.hooks.until("search checkpoint held", |h| {
            h.arrived.lock().unwrap().contains(&held)
        });
        svc.inner.scan.gate.release();
        svc.inner.hooks.until("completion checkpoint waiting", |h| {
            h.arrived
                .lock()
                .unwrap()
                .iter()
                .filter(|at| **at == waiting)
                .count()
                == 2
        });
        let fenced = svc.inner.freshness.checkpoint_fenced(1);
        svc.inner.hooks.open(held.0, held.1);
        searching.join().unwrap();
        fenced
    });
    assert_eq!(settle(svc, &job)["coverage"], "complete");
    assert!(
        fenced,
        "search and completion can overfill the native checkpoint queue"
    );
    assert!(!svc.inner.freshness.stale(1));
}

#[cfg(windows)]
fn search_checkpoint_failure_during_full_scan(new_hint: bool) {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.freshness.pause_pump();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let (arrived, waiting) = std::sync::mpsc::sync_channel(1);
    let worker = barrier.clone();
    *svc.inner.hooks.before_freshness_finish.lock().unwrap() = Some(Box::new(move || {
        arrived.send(()).unwrap();
        worker.wait();
    }));
    let job = start(svc, &lab.root, "full");
    waiting.recv_timeout(Duration::from_secs(30)).unwrap();
    let mut pages = Vec::new();
    for _ in 0..3 {
        svc.inner
            .hooks
            .fail_freshness_checkpoint
            .store(true, std::sync::atomic::Ordering::SeqCst);
        pages.push(f3_hits(svc));
    }
    if new_hint {
        svc.inner
            .freshness
            .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    }
    barrier.wait();
    assert_eq!(settle(svc, &job)["coverage"], "complete");
    for page in pages {
        assert_eq!(page["hits"][0]["freshness"]["state"], "stale");
    }
    assert_eq!(
        svc.inner.freshness.stale(1),
        new_hint,
        "only a real change may invalidate the completed scan"
    );
}

#[cfg(windows)]
#[test]
fn f3_search_checkpoint_failure_does_not_burn_completed_scan_epoch() {
    search_checkpoint_failure_during_full_scan(false);
}

#[cfg(windows)]
#[test]
fn f3_search_checkpoint_failure_preserves_real_generation_change() {
    search_checkpoint_failure_during_full_scan(true);
}

#[cfg(windows)]
#[test]
fn f3_dead_watcher_searches_never_reopen_refresh_budget() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.freshness.pause_pump();
    svc.inner.freshness.stop_watch(1);
    for attempt in 1..=3 {
        assert_eq!(f3_hits(svc)["hits"][0]["freshness"]["state"], "stale");
        svc.inner.freshness.ready_targeted_hint(1);
        crate::freshness::refresh(&svc.inner);
        let claim = svc.inner.freshness.explicit(1);
        let job = claim.auto.as_ref().expect("missing initial refresh");
        assert_eq!(settle(svc, job.as_str())["state"], "completed");
        assert_eq!(svc.inner.freshness.attempts(1), attempt);
        assert!(svc.inner.freshness.stale(1));
    }
    let list = serde_json::from_value(json!({"limit":50})).unwrap();
    let jobs = svc.inner.engine.job_list(&list).unwrap().jobs.len();
    for _ in 0..3 {
        svc.inner.freshness.expire_retry(1);
        assert_eq!(f3_hits(svc)["hits"][0]["freshness"]["state"], "stale");
        svc.inner.freshness.ready_targeted_hint(1);
        crate::freshness::refresh(&svc.inner);
        assert_eq!(
            svc.inner.freshness.attempts(1),
            3,
            "search against a dead watcher reopened the budget"
        );
        assert_eq!(svc.inner.engine.job_list(&list).unwrap().jobs.len(), jobs);
    }
}

#[cfg(windows)]
#[test]
fn f3_new_hint_reopens_failed_refresh_budget_only_after_backoff() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.freshness.pause_pump();
    for _ in 0..3 {
        svc.inner
            .freshness
            .record(1, vec![loomward_windows::watch::Change::RootDirty]);
        svc.inner.freshness.ready_targeted_hint(1);
        svc.inner
            .hooks
            .fail_automatic_start
            .store(true, std::sync::atomic::Ordering::SeqCst);
        crate::freshness::refresh(&svc.inner);
    }
    assert_eq!(svc.inner.freshness.attempts(1), 3);
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    assert_eq!(svc.inner.freshness.attempts(1), 3, "hint bypassed backoff");
    svc.inner.freshness.expire_retry(1);
    svc.inner.freshness.ready_targeted_hint(1);
    crate::freshness::refresh(&svc.inner);
    assert_eq!(
        svc.inner.freshness.attempts(1),
        3,
        "backoff expiry alone retried"
    );
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    assert_eq!(
        svc.inner.freshness.attempts(1),
        2,
        "new hint must reopen exactly one attempt"
    );
    assert!(
        svc.inner.freshness.stale(1),
        "a retry grant is not fresh evidence"
    );
    svc.inner.freshness.ready_targeted_hint(1);
    svc.inner
        .hooks
        .fail_automatic_start
        .store(true, std::sync::atomic::Ordering::SeqCst);
    crate::freshness::refresh(&svc.inner);
    assert_eq!(svc.inner.freshness.attempts(1), 3);
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    svc.inner.freshness.ready_targeted_hint(1);
    crate::freshness::refresh(&svc.inner);
    assert_eq!(svc.inner.freshness.attempts(1), 3, "retry bypassed backoff");
    svc.inner.freshness.expire_retry(1);
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    svc.inner.freshness.ready_targeted_hint(1);
    crate::freshness::refresh(&svc.inner);
    let claim = svc.inner.freshness.explicit(1);
    let job = claim.auto.as_ref().expect("new hint did not start refresh");
    assert_eq!(settle(svc, job.as_str())["state"], "completed");
    assert!(!svc.inner.freshness.stale(1));
}

#[cfg(windows)]
#[test]
fn f3_owner_preemption_does_not_spend_refresh_attempt() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.freshness.pause_pump();
    svc.inner.scan.gate.arm();
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    svc.inner.freshness.ready_targeted_hint(1);
    crate::freshness::refresh(&svc.inner);
    svc.inner.scan.gate.wait_reached();
    let claim = svc.inner.freshness.explicit(1);
    let job = claim.auto.as_ref().unwrap();
    let before = svc.inner.freshness.attempts(1);
    svc.inner.engine.scan_cancel(job).unwrap();
    svc.inner.scan.gate.release();
    assert_eq!(settle(svc, job.as_str())["state"], "cancelled");
    crate::freshness::refresh(&svc.inner);
    assert_eq!(before, 1);
    assert_eq!(
        svc.inner.freshness.attempts(1),
        0,
        "owner cancellation consumed an attempt"
    );
    assert!(svc.inner.freshness.stale(1));
}

#[cfg(windows)]
#[test]
fn f3_duplicates_leave_refresh_running_when_a_worker_is_available() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.freshness.pause_pump();
    svc.inner.scan.gate.arm();
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    svc.inner.freshness.ready_targeted_hint(1);
    crate::freshness::refresh(&svc.inner);
    svc.inner.scan.gate.wait_reached();
    let auto = svc.inner.freshness.explicit(1).auto.clone().unwrap();
    // A default refresh already leaves headroom for hashing at the unchanged pool cap.
    let (answer, state) = std::thread::scope(|s| {
        let asking = s.spawn(|| {
            call(
                svc,
                "duplicates.inspect",
                json!({"root_id":lab.root,
            "budget":{"max_candidates":200,"max_bytes":"1024","max_ms":5000}}),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !asking.is_finished()
            && svc.inner.engine.job_status(&auto).unwrap().state == JobState::Running
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        let state = svc.inner.engine.job_status(&auto).unwrap().state;
        svc.inner.scan.gate.release();
        (asking.join().unwrap(), state)
    });
    let (job, _) = ok("duplicates.inspect", answer);
    assert_eq!(
        settle(svc, job["job"]["job_id"].as_str().unwrap())["state"],
        "completed"
    );
    assert_eq!(settle(svc, auto.as_str())["state"], "completed");
    assert_eq!(
        state,
        JobState::Running,
        "inspection needlessly cancelled refresh"
    );
}

/// #561: an explicit scan.start is never refused because an automatic watcher refresh holds the
/// root. It pre-empts that refresh, the root stays stale across the hand-over, and a second
/// explicit scan is still refused while the first runs.
#[cfg(windows)]
#[test]
fn f3_explicit_scan_preempts_a_running_watcher_refresh() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.scan.gate.arm();
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    svc.inner.freshness.ready_targeted_hint(1);
    svc.inner.scan.gate.wait_reached();
    let held = (Command::ScanStart, "full_catalogue_published");
    svc.inner.hooks.closed.lock().unwrap().push(held);
    let (jobs, _) = ok("jobs.list", call(svc, "jobs.list", json!({"limit":50})));
    let auto = jobs["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["state"] == "running")
        .expect("the watcher refresh holds the root")["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let state = |job: &str| {
        let (j, _) = ok("jobs.get", call(svc, "jobs.get", json!({"job_id": job})));
        j["job"]["state"].as_str().unwrap().to_owned()
    };
    let (explicit, stale_in_hand_over) = std::thread::scope(|s| {
        let asking = s.spawn(|| {
            call(
                svc,
                "scan.start",
                json!({"root_id": lab.root, "mode": "full"}),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !asking.is_finished() && state(&auto) == "running" && Instant::now() < deadline {
            std::thread::yield_now();
        }
        let stale = svc.inner.freshness.stale(1);
        // Release before asserting so a regression cannot strand the scan worker.
        svc.inner.scan.gate.release();
        (asking.join().unwrap(), stale)
    });
    let (job, _) = ok("scan.start", explicit);
    let explicit = job["job"]["job_id"].as_str().unwrap().to_owned();
    assert_ne!(explicit, auto);
    assert!(stale_in_hand_over, "the hand-over must stay visibly stale");
    assert_eq!(settle(svc, &auto)["state"], "cancelled");
    // The cancelled run has ended and the explicit pass has not yet reported: still stale.
    svc.inner
        .hooks
        .until("explicit pass held before freshness", |h| {
            h.arrived.lock().unwrap().contains(&held)
        });
    let stale_between = svc.inner.freshness.stale(1);
    let hits_between = f3_hits(svc);
    svc.inner.hooks.open(held.0, held.1);
    assert!(stale_between, "a cancelled run must not make the root live");
    assert!(hits_between["hits"]
        .as_array()
        .unwrap()
        .iter()
        .all(|h| h["freshness"]["state"] == "stale"));
    assert_eq!(settle(svc, &explicit)["coverage"], "complete");
    f3_wait_live(svc);

    // An explicit scan is never pre-empted: a second one is refused at once.
    svc.inner.scan.gate.arm();
    let first = start(svc, &lab.root, "full");
    let second = call(
        svc,
        "scan.start",
        json!({"root_id": lab.root, "mode": "full"}),
    );
    svc.inner.scan.gate.release();
    assert_eq!(err(second).code, ErrorCode::Busy);
    assert_eq!(settle(svc, &first)["state"], "completed");
    f3_wait_live(svc);
}

/// #568: hashing waits out an automatic refresh before reading its catalogue candidates.
#[cfg(windows)]
#[test]
fn f3_duplicates_preempt_a_running_watcher_refresh() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    // Exercise actual pool exhaustion even though default scans now leave headroom.
    svc.inner
        .engine
        .budgets_set(
            &serde_json::from_value(json!({"pool":"scan_enumerate","max_workers":1})).unwrap(),
        )
        .unwrap();
    for name in ["copy.bin", "other.bin"] {
        std::fs::write(lab.path.join(name), b"same").unwrap();
    }
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.scan.gate.arm();
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    svc.inner.freshness.ready_targeted_hint(1);
    svc.inner.scan.gate.wait_reached();
    let (jobs, _) = ok("jobs.list", call(svc, "jobs.list", json!({"limit":50})));
    let auto = jobs["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["state"] == "running")
        .unwrap()["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (answer, stale_in_hand_over) = std::thread::scope(|s| {
        let asking = s.spawn(|| {
            call(
                svc,
                "duplicates.inspect",
                json!({"root_id":lab.root,
                "budget":{"max_candidates":200,"max_bytes":"1024","max_ms":5000}}),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !asking.is_finished()
            && svc
                .inner
                .engine
                .job_status(&JobId::new(&auto).unwrap())
                .unwrap()
                .state
                == JobState::Running
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        let stale = svc.inner.freshness.stale(1);
        svc.inner.scan.gate.release();
        (asking.join().unwrap(), stale)
    });
    let (job, _) = ok("duplicates.inspect", answer);
    let duplicate = job["job"]["job_id"].as_str().unwrap();
    let auto = settle(svc, &auto);
    let duplicate = settle(svc, duplicate);
    assert_eq!(
        auto["state"], "cancelled",
        "the owner pre-empts automatic work"
    );
    assert_eq!(duplicate["state"], "completed", "{duplicate:?}");
    assert_eq!(
        duplicate["duplicates"]["groups"].as_array().unwrap().len(),
        1
    );
    assert!(stale_in_hand_over, "hashing cannot repair freshness");
    f3_wait_live(svc);
}

/// #568: no automatic start may slip between candidate selection and job submission.
#[cfg(windows)]
#[test]
fn f3_duplicates_claim_fences_candidate_selection() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    let held = (Command::DuplicatesInspect, "candidates_ready");
    svc.inner.hooks.closed.lock().unwrap().push(held);
    svc.inner.scan.gate.arm();
    let (answer, during) = std::thread::scope(|s| {
        let asking = s.spawn(|| {
            call(
                svc,
                "duplicates.inspect",
                json!({"root_id":lab.root,
                "budget":{"max_candidates":200,"max_bytes":"1024","max_ms":5000}}),
            )
        });
        svc.inner
            .hooks
            .until("duplicates candidates selected", |h| {
                h.arrived.lock().unwrap().contains(&held)
            });
        svc.inner
            .freshness
            .record(1, vec![loomward_windows::watch::Change::RootDirty]);
        svc.inner.freshness.ready_targeted_hint(1);
        crate::freshness::refresh(&svc.inner);
        let during = svc
            .inner
            .engine
            .job_list(&serde_json::from_value(json!({"limit":50})).unwrap())
            .unwrap();
        svc.inner.scan.gate.release();
        svc.inner.hooks.open(held.0, held.1);
        (asking.join().unwrap(), during)
    });
    let (job, _) = ok("duplicates.inspect", answer);
    settle(svc, job["job"]["job_id"].as_str().unwrap());
    assert!(
        during.jobs.iter().all(|j| !matches!(
            j.state,
            JobState::Queued | JobState::Running | JobState::CancelRequested
        )),
        "an automatic scan started during candidate selection: {during:?}"
    );
    f3_wait_live(svc);
}

/// #568: a timed-out duplicates request must release its claim without submitting hashing.
#[cfg(windows)]
#[test]
fn f3_duplicates_expired_request_releases_the_refresh_fence() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    // Exercise actual pool exhaustion even though default scans now leave headroom.
    svc.inner
        .engine
        .budgets_set(
            &serde_json::from_value(json!({"pool":"scan_enumerate","max_workers":1})).unwrap(),
        )
        .unwrap();
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.scan.gate.arm();
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    svc.inner.freshness.ready_targeted_hint(1);
    svc.inner.scan.gate.wait_reached();
    let mut req = request(
        "duplicates.inspect",
        json!({"root_id":lab.root,
        "budget":{"max_candidates":200,"max_bytes":"1024","max_ms":5000}}),
    );
    req.deadline_ms = Some(Int::new(300).unwrap());
    let refused = err(svc.call(req, &CallContext::http())).code;
    svc.inner.hooks.until("duplicates request settled", |_| {
        svc.inner.freshness.unclaimed(1)
    });
    svc.inner.scan.gate.release();
    assert!(matches!(
        refused,
        ErrorCode::Busy | ErrorCode::DeadlineExceeded
    ));
    let (jobs, _) = ok("jobs.list", call(svc, "jobs.list", json!({"limit":50})));
    assert!(jobs["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|j| j["kind"] != "duplicates"));
    f3_wait_live(svc);
}

/// #568: bounded pre-emption must still retain a submitted job's idempotent receipt.
#[cfg(windows)]
#[test]
fn a_duplicates_job_started_past_its_deadline_reports_the_started_job() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    let mut req = request(
        "duplicates.inspect",
        json!({"root_id":lab.root,
        "budget":{"max_candidates":200,"max_bytes":"1024","max_ms":5000}}),
    );
    let first = past_deadline_once(svc, &mut req, Command::DuplicatesInspect);
    assert_eq!(err(first).code, ErrorCode::DeadlineExceeded);
    let (job, _) = ok("duplicates.inspect", svc.call(req, &CallContext::http()));
    assert_eq!(
        settle(svc, job["job"]["job_id"].as_str().unwrap())["state"],
        "completed"
    );
}

/// #716: a completed forget keeps its receipt when the caller has already timed out.
#[test]
fn a_forget_started_past_its_deadline_reports_success_on_retry() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, _) = fixture(&svc, 3);
    let root = svc.inner.root_id(root);
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": root, "purge_catalog": false}),
        ),
    );
    let mut req = request("roots.forget", json!({"root_id": root}));
    let first = past_deadline_once(&svc, &mut req, Command::RootsForget);
    assert_eq!(err(first).code, ErrorCode::DeadlineExceeded);
    let retry = svc.call(req.clone(), &CallContext::http());
    let (forgotten, _) = ok("roots.forget", retry.clone());
    assert_eq!(forgotten["root_id"], json!(root));
    assert!(forgotten["forgotten_at"].is_string());
    assert_eq!(svc.call(req, &CallContext::http()), retry);
    assert_eq!(
        svc.inner.hooks.runs.lock().unwrap()[&Command::RootsForget],
        1
    );
    assert!(
        ok("roots.list", call(&svc, "roots.list", json!({}))).0["roots"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

/// #698: sends `req` once, holds its worker after the handler finished and its caller
/// before the wait, then runs the service clock (`Hooks::skew`) past the deadline. The caller
/// therefore answers at its deadline and the worker finishes past it, on any host load; the
/// generous real deadline leaves the handler's own work out of the race. The clock is reset
/// before returning, so a retry with the same request_id waits on the real clock.
fn past_deadline_once(
    svc: &Service,
    req: &mut RequestEnvelope,
    command: Command,
) -> ResponseEnvelope {
    let hooks = &svc.inner.hooks;
    let (after, waiting) = ((command, "after"), (command, "mutate_wait"));
    hooks.arrived.lock().unwrap().clear();
    hooks.closed.lock().unwrap().extend([after, waiting]);
    req.deadline_ms = Some(Int::new(60_000).unwrap());
    let first = std::thread::scope(|s| {
        let asking = s.spawn(|| svc.call(req.clone(), &CallContext::http()));
        hooks.until("job started, caller waiting", |h| {
            let arrived = h.arrived.lock().unwrap();
            arrived.contains(&after) && arrived.contains(&waiting)
        });
        *hooks.skew.lock().unwrap() = Duration::from_secs(61);
        hooks.open(waiting.0, waiting.1);
        let first = asking.join().unwrap();
        hooks.open(after.0, after.1);
        first
    });
    hooks.until("worker settled", |h| {
        h.arrived
            .lock()
            .unwrap()
            .contains(&(command, "worker_exit"))
    });
    *hooks.skew.lock().unwrap() = Duration::ZERO;
    first
}

/// #565 review: a scan that started just past its request deadline reports the real job, and a
/// retry with the same request_id gets that job, never `deadline_exceeded` for an effect that ran.
#[cfg(windows)]
#[test]
fn a_scan_started_past_its_deadline_reports_the_started_job() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    let mut req = request("scan.start", json!({"root_id": lab.root, "mode": "full"}));
    let first = past_deadline_once(svc, &mut req, Command::ScanStart);
    assert_eq!(err(first).code, ErrorCode::DeadlineExceeded);
    let (job, _) = ok("scan.start", svc.call(req, &CallContext::http()));
    let job = job["job"]["job_id"].as_str().unwrap().to_owned();
    assert_eq!(settle(svc, &job)["state"], "completed");
}

/// #565 review: a request that gives up at its deadline releases its claim, so an automatic
/// refresh can run again and the root returns to live.
#[cfg(windows)]
#[test]
fn an_expired_explicit_request_releases_the_automatic_refresh_fence() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.scan.gate.arm();
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    svc.inner.freshness.ready_targeted_hint(1);
    svc.inner.scan.gate.wait_reached();
    // The held refresh cannot observe its cancel, so the explicit request runs out of time.
    let mut req = request("scan.start", json!({"root_id": lab.root, "mode": "full"}));
    req.deadline_ms = Some(Int::new(300).unwrap());
    let refused = err(svc.call(req, &CallContext::http())).code;
    svc.inner
        .hooks
        .until("the request settled", |_| svc.inner.freshness.unclaimed(1));
    svc.inner.scan.gate.release();
    assert!(matches!(
        refused,
        ErrorCode::Busy | ErrorCode::DeadlineExceeded
    ));
    // The cancelled refresh leaves the root stale; only a new automatic pass makes it live.
    f3_wait_live(svc);
}

/// #569 item 3: an explicit scan.start pre-empts a running automatic refresh, then fails on its
/// retry. The explicit start reports the error, the root stays stale while the cancelled automatic
/// run is still held, and a later automatic refresh runs and makes the root live.
#[cfg(windows)]
#[test]
fn f3_failed_explicit_start_after_cancel_leaves_an_automatic_refresh_to_resume() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    let baseline = start(svc, &lab.root, "full");
    settle(svc, &baseline);
    f3_wait_live(svc);
    svc.inner.scan.gate.arm();
    svc.inner
        .freshness
        .record(1, vec![loomward_windows::watch::Change::RootDirty]);
    svc.inner.freshness.ready_targeted_hint(1);
    svc.inner.scan.gate.wait_reached();
    let (jobs, _) = ok("jobs.list", call(svc, "jobs.list", json!({"limit":50})));
    let auto = jobs["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["state"] == "running")
        .expect("the watcher refresh holds the root")["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    svc.inner
        .hooks
        .fail_explicit_retry
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (refused, stale_while_held, hits_while_held) = std::thread::scope(|s| {
        let asking = s.spawn(|| {
            call(
                svc,
                "scan.start",
                json!({"root_id": lab.root, "mode": "full"}),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !asking.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        // The automatic run is still held (cancel requested, not observed): nothing can go live.
        let stale = svc.inner.freshness.stale(1);
        let hits = f3_hits(svc);
        // Release before asserting so a regression cannot strand the held scan worker.
        svc.inner.scan.gate.release();
        (asking.join().unwrap(), stale, hits)
    });
    // (a) the explicit start returns the honest engine error, and no job id.
    let error = err(refused);
    assert_eq!(error.code, ErrorCode::InternalError);
    assert_eq!(error.message.as_str(), "synthetic retry failure");
    // (b) the root stays stale while the automatic run is still held.
    assert!(
        stale_while_held,
        "a failed explicit start must not make the root live"
    );
    assert!(hits_while_held["hits"]
        .as_array()
        .unwrap()
        .iter()
        .all(|h| h["freshness"]["state"] == "stale"));
    assert_eq!(settle(svc, &auto)["state"], "cancelled");
    // (c) the pump's next automatic refresh runs and the root returns to live, bounded by
    // f3_wait_live's 30 s deadline; a completed job other than the cancelled one must exist.
    f3_wait_live(svc);
    let (jobs, _) = ok("jobs.list", call(svc, "jobs.list", json!({"limit":50})));
    assert!(jobs["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|j| j["job_id"] != auto.as_str()
            && j["job_id"] != baseline.as_str()
            && j["state"] == "completed"));
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
    // #561: late watcher hints may start a refresh; read once freshness is idle.
    f3_wait_live(svc);
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
    // #561: late watcher hints may start a refresh; read once freshness is idle.
    f3_wait_live(svc);
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
    // Reproduce a late watcher refresh overlapping the read after the foreground scan.
    svc.inner.scan.gate.arm();
    svc.inner.freshness.record(
        1,
        vec![loomward_windows::watch::Change::Name {
            action: 1,
            name: "a/new.bin"
                .replace('/', std::path::MAIN_SEPARATOR_STR)
                .encode_utf16()
                .collect(),
        }],
    );
    svc.inner.freshness.ready_targeted_hint(1);
    svc.inner.scan.gate.wait_reached();
    let overlapping = call(
        svc,
        "tree.slice",
        json!({"anchor": {"kind": "root", "root_id": lab.root}, "depth": 3, "max_nodes": 256, "min_share": 0.0, "basis": "logical", "include_files": true}),
    );
    svc.inner.scan.gate.release();
    assert_eq!(err(overlapping).code, ErrorCode::PartialCoverage);
    f3_wait_live(svc);
    let again = lab_slice(svc, &lab.root);
    assert_eq!(child_named(&again, "b"), Some(kept));
    assert_eq!(slice_totals(&again), (5, 2, 165));
}

/// E0: detaching a granted volume retains the catalogue, but cannot start a scan or content read.
#[cfg(windows)]
#[test]
fn detachable_root_offline_retains_last_seen_sizes_and_reconnects_by_identity() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    let job = start(svc, &lab.root, "full");
    assert_eq!(settle(svc, &job)["state"], "completed");
    let (before, _) = ok("roots.list", call(svc, "roots.list", json!({})));
    assert_eq!(before["roots"][0]["totals"]["logical_bytes"], "165");
    let (volumes, _) = ok("volumes.list", call(svc, "volumes.list", json!({})));
    let classification = loomward_windows::volumes::classify(&lab.path).unwrap();
    assert_eq!(
        volumes["volumes"][0]["device"]["bus_type"],
        classification.bus_type
    );
    assert_eq!(
        volumes["volumes"][0]["device"]["basis"],
        "ioctl_storage_query_property"
    );
    let moved = lab.path.with_file_name("detached-lab");
    std::fs::rename(&lab.path, &moved).unwrap();
    let (roots, _) = ok("roots.list", call(svc, "roots.list", json!({})));
    assert_eq!(roots["roots"][0]["grant_state"], "active");
    assert_eq!(roots["roots"][0]["scan"]["state"], "stale");
    assert_eq!(roots["roots"][0]["totals"], before["roots"][0]["totals"]);
    assert_eq!(slice_totals(&lab_slice(svc, &lab.root)), (5, 3, 165));
    let volume_key = crate::db::volumes(&svc.inner.db(), None).unwrap()[0]
        .key
        .clone();
    svc.inner.hooks.space.lock().unwrap().push((
        volume_key,
        SpaceFake::Fixed {
            capacity: 1024,
            free: 512,
        },
    ));
    let (volumes, _) = ok("volumes.list", call(svc, "volumes.list", json!({})));
    assert_eq!(volumes["volumes"][0]["online"], false);
    assert!(volumes["volumes"][0]["capacity_bytes"].is_null());
    assert!(volumes["volumes"][0]["free_bytes"].is_null());
    svc.inner.hooks.space.lock().unwrap().clear();
    for (command, payload) in [
        ("scan.start", json!({"root_id": lab.root, "mode": "full"})),
        (
            "duplicates.inspect",
            json!({"root_id": lab.root, "budget": {"max_candidates": 20, "max_bytes": "1024", "max_ms": 1000}}),
        ),
    ] {
        let error = err(call(svc, command, payload));
        assert_eq!(error.code, ErrorCode::DeviceOffline, "{error:?}");
    }
    assert_eq!(slice_totals(&lab_slice(svc, &lab.root)), (5, 3, 165));
    std::fs::rename(&moved, &lab.path).unwrap();
    let (volumes, _) = ok("volumes.list", call(svc, "volumes.list", json!({})));
    assert_eq!(volumes["volumes"][0]["online"], true);
    let job = start(svc, &lab.root, "full");
    assert_eq!(settle(svc, &job)["state"], "completed");
    assert_eq!(slice_totals(&lab_slice(svc, &lab.root)), (5, 3, 165));
}

/// E0: a replacement with the same serial and folder ID but another volume GUID is never read.
#[cfg(windows)]
#[test]
fn detachable_root_replacement_guid_records_identity_changed_before_reading() {
    for command in ["scan.start", "duplicates.inspect"] {
        let Some(lab) = lab() else { return };
        let svc = &lab.svc;
        let job = start(svc, &lab.root, "full");
        assert_eq!(settle(svc, &job)["state"], "completed");
        // Simulate a different volume at the same mount point without USB hardware.
        svc.inner
            .db()
            .execute_batch(
                "UPDATE st.root_grant SET volume_guid='00000000000000000000000000000001';
             UPDATE st.lab_root SET volume_guid='00000000000000000000000000000001';",
            )
            .unwrap();
        let row = crate::db::roots(&svc.inner.db(), None)
            .unwrap()
            .pop()
            .unwrap();
        let refused = svc.inner.session_root_gate(&row).unwrap_err();
        assert_eq!(refused.message.as_str(), "root_identity_changed");
        let payload = if command == "scan.start" {
            json!({"root_id": lab.root, "mode": "full"})
        } else {
            json!({"root_id": lab.root, "budget": {"max_candidates": 20, "max_bytes": "1024", "max_ms": 1000}})
        };
        let e = err(call(svc, command, payload));
        assert_eq!(e.code, ErrorCode::PermissionDenied);
        let state: String = svc
            .inner
            .db()
            .query_row("SELECT state FROM st.root_grant", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "identity_changed");
        let count: i64 = svc
            .inner
            .db()
            .query_row("SELECT count(*) FROM main.file", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count, 5,
            "retained observations are never replaced with the new volume"
        );
    }
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

/// #669 item 4: a root replaced while Loomward was stopped is found by automatic watch setup at
/// the next start, and the durable grant ends `identity_changed` (docs/41 section 7 step 1), so
/// search and inspect stop disclosing its retained catalogue.
#[cfg(windows)]
#[test]
fn freshness_setup_records_a_root_replaced_while_stopped() {
    let Some(lab) = lab() else { return };
    let Lab {
        _tmp, path, svc, ..
    } = lab;
    drop(svc);
    std::fs::rename(&path, path.with_file_name("lab-moved")).unwrap();
    std::fs::create_dir(&path).unwrap();
    // No CLI grants: only the durable grant and automatic freshness setup see the root.
    let svc = open(&_tmp.path().join("state"), DatasetClass::Synthetic);
    let state: String = svc
        .inner
        .db()
        .query_row("SELECT state FROM st.root_grant", [], |r| r.get(0))
        .unwrap();
    assert_eq!(state, "identity_changed");
}

/// #669 item 5: re-granting a verified root at a new path is fresh consent for that path, never
/// a reuse of the grant that still names the old path; the old grant's folder was renamed, so it
/// ends `identity_changed` (docs/41 section 16, "Root renamed").
#[cfg(windows)]
#[test]
fn regranting_a_renamed_root_mints_a_grant_for_its_new_path() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    let old = svc.inner.root_row(&lab.root).unwrap();
    let moved = lab.path.with_file_name("lab-renamed");
    std::fs::rename(&lab.path, &moved).unwrap();
    let moved = moved.canonicalize().unwrap();
    let (root, fresh) = svc.inner.grant(&moved, "cli_flag", false).unwrap();
    assert!(fresh, "a moved root needs a new grant");
    assert_ne!(root, old);
    let rows: Vec<(String, String)> = svc
        .inner
        .db()
        .prepare("SELECT display_path,state FROM st.root_grant ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let text = moved.to_string_lossy();
    let new_path = text.strip_prefix(r"\\?\").unwrap_or(&text).to_string();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].1, "identity_changed");
    assert_eq!(rows[1], (new_path, "active".into()));
    // Granting the same path again reuses the new grant.
    assert_eq!(
        svc.inner.grant(&moved, "cli_flag", false).unwrap(),
        (root, false)
    );
}

/// Retro review (w12): an identity-unverified grant whose folder was renamed ends
/// `identity_changed` too, once its old path no longer answers; the weak ID alone retires nothing.
#[cfg(windows)]
#[test]
fn regranting_a_renamed_unverified_root_retires_the_grant_naming_the_old_path() {
    if loomward_windows::enumerate::skip_elevated_test() {
        return;
    }
    let tmp = temp_root();
    let folder = tmp.path().join("fat-folder");
    std::fs::create_dir(&folder).unwrap();
    let folder = folder.canonicalize().unwrap();
    let svc = open(&tmp.path().join("state"), DatasetClass::Personal);
    *svc.inner.hooks.filesystem.lock().unwrap() = Some("exFAT");
    let (old, _) = svc.inner.grant(&folder, "cli_flag", true).unwrap();
    let moved = tmp.path().join("fat-folder-renamed");
    std::fs::rename(&folder, &moved).unwrap();
    let moved = moved.canonicalize().unwrap();
    let (root, fresh) = svc.inner.grant(&moved, "cli_flag", true).unwrap();
    assert!(fresh && root != old);
    let states: Vec<(String, String)> = svc
        .inner
        .db()
        .prepare("SELECT identity_mode,state FROM st.root_grant ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        states,
        [
            ("unverified".into(), "identity_changed".into()),
            ("unverified".into(), "active".into())
        ],
        "one active grant per native folder"
    );
}

/// Renames an unverified grant's folder (optionally recreating another folder at the old path),
/// re-grants it at the new path with the probe error injected, and returns the two grants' states.
#[cfg(windows)]
fn regrant_renamed_unverified(
    probe_error: Option<loomward_windows::enumerate::SourceError>,
    replace_old: bool,
) -> Option<(String, String)> {
    if loomward_windows::enumerate::skip_elevated_test() {
        return None;
    }
    let tmp = temp_root();
    let folder = tmp.path().join("fat-folder");
    std::fs::create_dir(&folder).unwrap();
    let folder = folder.canonicalize().unwrap();
    let svc = open(&tmp.path().join("state"), DatasetClass::Personal);
    *svc.inner.hooks.filesystem.lock().unwrap() = Some("exFAT");
    svc.inner.grant(&folder, "cli_flag", true).unwrap();
    let moved = tmp.path().join("fat-folder-renamed");
    std::fs::rename(&folder, &moved).unwrap();
    if replace_old {
        std::fs::create_dir(&folder).unwrap();
    }
    let moved = moved.canonicalize().unwrap();
    *svc.inner.hooks.probe_error.lock().unwrap() = probe_error;
    svc.inner.grant(&moved, "cli_flag", true).unwrap();
    let states: Vec<String> = svc
        .inner
        .db()
        .prepare("SELECT state FROM st.root_grant ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    Some((states[0].clone(), states[1].clone()))
}

/// A file at the old path cannot be the previously granted folder.
#[cfg(windows)]
#[test]
fn a_definitely_non_directory_old_path_retires_the_unverified_grant() {
    use loomward_windows::enumerate::SourceError;
    for code in [267, 0xC000_0103_u32 as i32] {
        if let Some(states) = regrant_renamed_unverified(Some(SourceError::Io(code)), false) {
            assert_eq!(
                states,
                ("identity_changed".into(), "active".into()),
                "{code:#x}"
            );
        }
    }
}

/// A real file at the old path retires the grant through native attribute validation, not an
/// injected error; fresh consent for the renamed folder remains active.
#[cfg(windows)]
#[test]
fn a_real_file_at_the_old_path_retires_the_unverified_grant() {
    if loomward_windows::enumerate::skip_elevated_test() {
        return;
    }
    let tmp = temp_root();
    let folder = tmp.path().join("fat-folder");
    std::fs::create_dir(&folder).unwrap();
    let folder = folder.canonicalize().unwrap();
    let svc = open(&tmp.path().join("state"), DatasetClass::Personal);
    *svc.inner.hooks.filesystem.lock().unwrap() = Some("exFAT");
    let (old, _) = svc.inner.grant(&folder, "cli_flag", true).unwrap();
    let moved = tmp.path().join("fat-folder-renamed");
    std::fs::rename(&folder, &moved).unwrap();
    std::fs::write(&folder, b"synthetic replacement file").unwrap();
    let moved = moved.canonicalize().unwrap();
    let (new, fresh) = svc.inner.grant(&moved, "cli_flag", true).unwrap();
    assert!(fresh && new != old);
    let states: Vec<(String, String)> = svc
        .inner
        .db()
        .prepare("SELECT identity_mode,state FROM st.root_grant ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        states,
        [
            ("unverified".into(), "identity_changed".into()),
            ("unverified".into(), "active".into())
        ]
    );
}

/// Holds an old-path observation while the caller exercises concurrent owner state changes.
#[cfg(windows)]
fn with_stalled_old_path_probe(test: impl FnOnce(&Service, i64, &Path)) -> Option<String> {
    if loomward_windows::enumerate::skip_elevated_test() {
        return None;
    }
    let tmp = temp_root();
    let folder = tmp.path().join("fat-folder");
    std::fs::create_dir(&folder).unwrap();
    let folder = folder.canonicalize().unwrap();
    let state = tmp.path().join("state");
    let svc = open(&state, DatasetClass::Personal);
    *svc.inner.hooks.filesystem.lock().unwrap() = Some("exFAT");
    let (old, _) = svc.inner.grant(&folder, "cli_flag", true).unwrap();
    let grant: i64 = svc
        .inner
        .db()
        .query_row("SELECT grant_id FROM main.root WHERE id=?1", [old], |r| {
            r.get(0)
        })
        .unwrap();
    let moved = tmp.path().join("fat-folder-renamed");
    std::fs::rename(&folder, &moved).unwrap();
    let moved = moved.canonicalize().unwrap();
    *svc.inner.hooks.probe_error.lock().unwrap() =
        Some(loomward_windows::enumerate::SourceError::Io(3));
    svc.inner
        .hooks
        .closed
        .lock()
        .unwrap()
        .push((Command::RootsRequestGrant, "old_path_probe"));
    let peer = svc.inner.clone();
    let regrant = std::thread::spawn(move || peer.grant(&moved, "cli_flag", true));
    svc.inner.hooks.until("old path probe", |h| {
        h.arrived
            .lock()
            .unwrap()
            .contains(&(Command::RootsRequestGrant, "old_path_probe"))
    });
    // Always release the worker, including when the regression assertion fails.
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| test(&svc, grant, &state)));
    svc.inner
        .hooks
        .open(Command::RootsRequestGrant, "old_path_probe");
    let regranted = regrant.join().unwrap().unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    assert!(regranted.1 && regranted.0 != old);
    let new_state: String = svc
        .inner
        .db()
        .query_row(
            "SELECT g.state FROM st.root_grant g JOIN main.root r ON r.grant_id=g.id WHERE r.id=?1",
            [regranted.0],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(new_state, "active");
    let old_state = svc
        .inner
        .db()
        .query_row(
            "SELECT state FROM st.root_grant WHERE id=?1",
            [grant],
            |r| r.get(0),
        )
        .unwrap();
    Some(old_state)
}

#[cfg(windows)]
#[test]
fn a_stalled_old_path_probe_does_not_block_an_unrelated_mutation() {
    let state = with_stalled_old_path_probe(|svc, _, _| {
        // Revocation of an unrelated grant needs both service locks.
        let obs = RootObservation {
            volume_key: "synthetic-other-volume".into(),
            volume_guid: None,
            display_name: "synthetic-other-root".into(),
            display_path: "synthetic-other-root".into(),
            root_file_id: Some(vec![42; 16]),
            filesystem: Some("exFAT".into()),
            origin: "owner_granted".into(),
            granted_via: "cli_flag".into(),
            observed_at_ns: crate::now_ns(),
        };
        let WriteReply::Root { grant_id, .. } = svc
            .inner
            .catalog
            .writer()
            .call(WriteCommand::RegisterRoot(obs))
            .unwrap()
        else {
            panic!("root reply")
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let peer = svc.inner.clone();
        let mutation = std::thread::spawn(move || tx.send(peer.revoke(grant_id)).unwrap());
        let completed = rx.recv_timeout(Duration::from_secs(3));
        svc.inner
            .hooks
            .open(Command::RootsRequestGrant, "old_path_probe");
        mutation.join().unwrap();
        assert!(
            completed.is_ok(),
            "unrelated mutation waited for filesystem probing"
        );
        completed.unwrap().unwrap();
        let state: String = svc
            .inner
            .db()
            .query_row(
                "SELECT state FROM st.root_grant WHERE id=?1",
                [grant_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "revoked");
    });
    if let Some(state) = state {
        assert_eq!(state, "identity_changed");
    }
}

#[cfg(windows)]
#[test]
fn an_old_path_probe_does_not_retire_a_changed_candidate() {
    // An independent SQLite connection injects changes while the service has dropped its locks.
    for change in [
        "display_path=display_path || '-changed'",
        "root_file_id=X'01010101010101010101010101010101'",
        "volume_key='synthetic-changed-volume'",
        "volume_guid='synthetic-changed-guid'",
        "identity_mode='verified'",
        "state='revoked'",
    ] {
        let state = with_stalled_old_path_probe(|_, grant, state| {
            let conn = rusqlite::Connection::open(state.join("state.db")).unwrap();
            conn.execute(
                &format!("UPDATE root_grant SET {change} WHERE id=?1"),
                [grant],
            )
            .unwrap();
        });
        if let Some(state) = state {
            assert_eq!(
                state,
                if change == "state='revoked'" {
                    "revoked"
                } else {
                    "active"
                },
                "{change}"
            );
        }
    }
}

/// An old path whose probe fails inconclusively proves nothing about the folder:
/// access denied, a not-ready volume or a cancelled open must keep the healthy grant active.
#[cfg(windows)]
#[test]
fn an_inconclusive_old_path_probe_never_retires_an_unverified_grant() {
    use loomward_windows::enumerate::SourceError;
    for error in [
        SourceError::AccessDenied,
        SourceError::Io(21),
        SourceError::Io(995),
        SourceError::Refused,
    ] {
        if let Some(states) = regrant_renamed_unverified(Some(error), false) {
            assert_eq!(states, ("active".into(), "active".into()), "{error:?}");
        }
    }
}

/// An old path that now answers with another folder's identity is definitely not the same folder.
#[cfg(windows)]
#[test]
fn an_old_path_answering_another_identity_retires_the_unverified_grant() {
    if let Some(states) = regrant_renamed_unverified(None, true) {
        assert_eq!(states, ("identity_changed".into(), "active".into()));
    }
}

/// An old path that still opens the same folder (here another spelling of it) is no rename.
#[cfg(windows)]
#[test]
fn an_old_path_still_answering_the_same_identity_keeps_the_unverified_grant() {
    if loomward_windows::enumerate::skip_elevated_test() {
        return;
    }
    let tmp = temp_root();
    let folder = tmp.path().join("fat-folder");
    std::fs::create_dir(&folder).unwrap();
    let folder = folder.canonicalize().unwrap();
    let svc = open(&tmp.path().join("state"), DatasetClass::Personal);
    *svc.inner.hooks.filesystem.lock().unwrap() = Some("exFAT");
    svc.inner.grant(&folder, "cli_flag", true).unwrap();
    let other_spelling = std::path::PathBuf::from(folder.to_string_lossy().to_uppercase());
    let (_, fresh) = svc.inner.grant(&other_spelling, "cli_flag", true).unwrap();
    assert!(fresh, "another spelling is another path");
    let states: Vec<String> = svc
        .inner
        .db()
        .prepare("SELECT state FROM st.root_grant ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(states, ["active", "active"]);
}

/// #378: duplicate inspection that finds the root replaced records `identity_changed` and emits
/// `roots.changed` exactly as scan start does, so the UI stops offering an inspection that fails.
#[cfg(windows)]
#[test]
fn a_replaced_root_found_by_inspection_records_identity_changed() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    std::fs::rename(&lab.path, lab.path.with_file_name("lab-moved")).unwrap();
    std::fs::create_dir(&lab.path).unwrap();
    let mut events = svc.subscribe(None);
    let e = err(call(
        svc,
        "duplicates.inspect",
        json!({"root_id": lab.root, "budget": {"max_candidates": 200, "max_bytes": "1024", "max_ms": 1000}}),
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
    assert_eq!(event.data["roots"][0]["grant_state"], "identity_changed");
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
    let inspected = err(call(
        &synthetic,
        "duplicates.inspect",
        json!({"root_id": synthetic.inner.root_id(root), "budget": {"max_candidates": 200, "max_bytes": "1024", "max_ms": 1000}}),
    ));
    assert_eq!(inspected.code, ErrorCode::PermissionDenied);
    assert_eq!(
        reason(&inspected),
        Some("synthetic_session_requires_lab_root")
    );
}

fn session_gate_lab_guid_case(binding: &str) {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 1);
    let guid = "00000000000000000000000000000001";
    let conn = svc.inner.db();
    conn.execute(
        "UPDATE st.root_grant SET origin='lab_generated',volume_guid=?1 WHERE id=?2",
        rusqlite::params![guid, grant],
    )
    .unwrap();
    if binding != "missing" {
        crate::db::insert_lab_root(&conn, "fixture", &[1; 16], guid, "digest", "seed", "S")
            .unwrap();
        let registered_guid = match binding {
            "matching" => Some(guid),
            "mismatched" => Some("00000000000000000000000000000002"),
            "null" => None,
            _ => unreachable!(),
        };
        conn.execute("UPDATE st.lab_root SET volume_guid=?1", [registered_guid])
            .unwrap();
    }
    let row = crate::db::roots(&conn, Some(root)).unwrap().remove(0);
    assert_eq!(row.grant_state, "active");
    drop(conn);
    let result = svc.inner.session_root_gate(&row);
    if binding == "matching" {
        result.unwrap();
    } else {
        let e = result.expect_err("an active lab grant needs a matching registration GUID");
        assert_eq!(e.code, ErrorCode::PermissionDenied);
        assert_eq!(reason(&e), Some("synthetic_session_requires_lab_root"));
    }
    let stored_guid: String = svc
        .inner
        .db()
        .query_row(
            "SELECT volume_guid FROM st.root_grant WHERE id=?1",
            [grant],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored_guid, guid, "the gate must never adopt a GUID");
}

#[test]
fn session_gate_lab_guid_null_is_refused() {
    session_gate_lab_guid_case("null");
}

#[test]
fn session_gate_lab_guid_mismatched_is_refused() {
    session_gate_lab_guid_case("mismatched");
}

#[test]
fn session_gate_lab_guid_missing_is_refused() {
    session_gate_lab_guid_case("missing");
}

#[test]
fn session_gate_lab_guid_matching_is_accepted() {
    session_gate_lab_guid_case("matching");
}

/// A personal session scans only its own active owner grants: an unknown root, a lab root and
/// a revoked grant are refused before the engine sees them.
#[test]
fn personal_scan_start_refuses_unknown_lab_and_revoked_roots() {
    let tmp = temp_root();
    let personal = open(&tmp.path().join("personal"), DatasetClass::Personal);
    // Every refusal leaves no job behind.
    let scan = |root: &str| {
        let e = err(call(
            &personal,
            "scan.start",
            json!({"root_id": root, "mode": "full"}),
        ));
        // q-10: content inspection shares the scan's root gate, so it refuses the same roots.
        let inspected = err(call(
            &personal,
            "duplicates.inspect",
            json!({"root_id": root, "budget": {"max_candidates": 200, "max_bytes": "1024", "max_ms": 1000}}),
        ));
        assert_eq!(
            (inspected.code, reason(&inspected)),
            (e.code, reason(&e)),
            "{root}: {inspected:?}"
        );
        let (list, _) = ok(
            "jobs.list",
            call(&personal, "jobs.list", json!({"limit": 50})),
        );
        assert_eq!(list["jobs"], json!([]), "{root}: no job started");
        e
    };
    let e = scan("rt_x");
    assert_eq!(e.code, ErrorCode::NotFound, "{e:?}");
    // The catalogue never registers a lab root in a personal store; a row that says so anyway
    // (a planted or corrupted catalogue) is still refused by the scan gate.
    let lab_root = RootObservation {
        volume_guid: None,
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

/// Deep review LOW 3: a personal session advertises metadata scans exactly where they run
/// (Windows), like a synthetic one.
#[test]
fn personal_hello_reports_metadata_scan_on_windows() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Personal);
    let (hello, _) = ok("session.hello", call(&svc, "session.hello", json!({})));
    assert_eq!(hello["dataset_class"], "personal");
    assert_eq!(
        hello["capabilities"]["observation"]["metadata_scan"],
        cfg!(windows)
    );
}

/// An active owner grant for `path` made without today's fences: through the grant itself where
/// native identity opens (as an older build recorded it), else as a fixture row naming the path.
fn unfenced_owner_grant(svc: &Service, path: &Path) -> i64 {
    if let Ok((root, _)) = svc.inner.grant(path, "cli_flag", false) {
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
        let inspected = err(call(
            &svc,
            "duplicates.inspect",
            json!({"root_id": svc.inner.root_id(root), "budget": {"max_candidates": 200, "max_bytes": "1024", "max_ms": 1000}}),
        ));
        assert_eq!(inspected.code, ErrorCode::PermissionDenied, "{inspected:?}");
        assert_eq!(reason(&inspected), Some("protected_profile_or_store"));
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
        allow_identity_unverified: false,
        grant_roots: vec![path.clone()],
    });
    if loomward_windows::enumerate::skip_elevated_test() {
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
        volume_guid: None,
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
    // A first full scan commits into the writer's bulk build until EndRun publishes it (#148).
    let rows = |table: &str| -> Vec<String> {
        let conn = svc.inner.db();
        let mut s = conn
            .prepare(&format!("SELECT name FROM main.{table} ORDER BY name"))
            .unwrap();
        let names = s.query_map([], |r| r.get(0)).unwrap();
        names.map(Result::unwrap).collect()
    };
    let files = || {
        assert!(rows("file").is_empty(), "nothing is visible before EndRun");
        rows("bulk_file")
    };
    // The refresh published the obsolete stage upsert-only.
    assert_eq!(files(), vec!["stale.bin".to_string()]);
    sink.consume(&id, listing(new, &["kept.bin"])).unwrap();
    sink.consume(&id, done(new)).unwrap();
    // Listing steps commit asynchronously (#148); settling waits for them and finds none obsolete.
    assert_eq!(sink.settle(&id).unwrap(), Vec::<u64>::new());
    assert_eq!(files(), vec!["kept.bin".to_string()]);
    sink.finish_run(&id, 41, &RunScope::FullRoot, true).unwrap();
    assert_eq!(rows("file"), vec!["kept.bin".to_string()]);
    assert!(rows("bulk_file").is_empty());
    assert_eq!(
        sink.consume(&id, listing(new, &["after.bin"]))
            .unwrap_err()
            .to_body()
            .code,
        ErrorCode::StaleGeneration,
        "an ended run accepts nothing"
    );
}

/// #763: a send-time refusal never reaches the writer's chunk fence. Even if the caller keeps
/// sending and claims Complete, keep accepted entries and publish partial without absence.
#[test]
fn a_send_time_rejected_chunk_never_publishes_a_complete_listing() {
    use loomward_engine::scan::source::{IdBasis, ListOutcome, OpenedIdentity};
    for chunks in [
        &[false, true][..],
        &[true, false],
        &[true, false, true],
        &[false],
    ] {
        let tmp = temp_root();
        let svc = open(tmp.path(), DatasetClass::Synthetic);
        let (root, grant) = register(&svc, 7);
        let dir = crate::db::active_root_dir(&svc.inner.db(), root)
            .unwrap()
            .unwrap();
        let id = RootId::new(svc.inner.root_id(root)).unwrap();
        let sink = &svc.inner.scan;
        sink.bind(&id, grant, dir);
        let identity = OpenedIdentity {
            id: None,
            basis: IdBasis::None,
            volume_guid: None,
            volume_serial: None,
            attributes: 0x10,
            reparse_tag: None,
        };
        // Seed an existing row: a partial listing must not establish its absence.
        sink.begin_run(&id, 40, &RunScope::FullRoot).unwrap();
        let seed = sink.prepare_listing(&id, 40, None, &[], identity).unwrap();
        sink.consume(
            &id,
            ScanMessage::DirListing {
                run: 40,
                ticket: seed,
                entries: vec![entry("previous.bin", 1, false)],
            },
        )
        .unwrap();
        sink.consume(
            &id,
            ScanMessage::ListingDone {
                run: 40,
                ticket: seed,
                outcome: ListOutcome::Complete,
            },
        )
        .unwrap();
        sink.finish_run(&id, 40, &RunScope::FullRoot, true).unwrap();

        sink.begin_run(&id, 41, &RunScope::FullRoot).unwrap();
        let ticket = sink.prepare_listing(&id, 41, None, &[], identity).unwrap();
        let mut accepted = Vec::new();
        for (i, valid) in chunks.iter().enumerate() {
            let entries = if *valid {
                let name = format!("accepted-{i}.bin");
                accepted.push(name.clone());
                vec![entry(&name, 2, false)]
            } else {
                vec![
                    entry("rejected.bin", 3, false),
                    entry("duplicate.bin", 4, false),
                    entry("duplicate.bin", 5, false),
                ]
            };
            let result = sink.consume(
                &id,
                ScanMessage::DirListing {
                    run: 41,
                    ticket,
                    entries,
                },
            );
            if *valid {
                result.unwrap();
            } else {
                assert!(
                    matches!(
                        result,
                        Err(loomward_engine::EngineError::StaleGeneration { .. })
                    ),
                    "duplicate names are refused at send time"
                );
            }
        }
        sink.consume(
            &id,
            ScanMessage::ListingDone {
                run: 41,
                ticket,
                outcome: ListOutcome::Complete,
            },
        )
        .unwrap();
        sink.settle(&id).unwrap();
        let conn = svc.inner.db();
        let names: Vec<String> = conn
            .prepare("SELECT name FROM main.file ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            !names.iter().any(|n| n == "rejected.bin"),
            "rejected chunk entries never reached the writer"
        );
        for name in &accepted {
            assert!(names.contains(name), "accepted chunk missing: {names:?}");
        }
        let (state, coverage): (String, String) = conn
            .query_row(
                "SELECT listing_state,listing_outcome FROM main.dir WHERE id=?1",
                [dir],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (state.as_str(), coverage.as_str()),
            ("incomplete", "partial"),
            "chunks={chunks:?}, published entries={names:?}"
        );
        assert!(
            names.iter().any(|n| n == "previous.bin"),
            "partial coverage must preserve unseen entries"
        );
        drop(conn);
        sink.finish_run(&id, 41, &RunScope::FullRoot, false)
            .unwrap();
    }
}

/// #781: a drained budget stop records a partial sample, without globally reconciling absence.
#[test]
fn a_limited_run_records_partial_history_and_keeps_pending_absence() {
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
    let identity = OpenedIdentity {
        id: None,
        basis: IdBasis::None,
        volume_guid: None,
        volume_serial: None,
        attributes: 0x10,
        reparse_tag: None,
    };
    let publish = |run, ticket, entries| {
        sink.consume(
            &id,
            ScanMessage::DirListing {
                run,
                ticket,
                entries,
            },
        )
        .unwrap();
        sink.consume(
            &id,
            ScanMessage::ListingDone {
                run,
                ticket,
                outcome: ListOutcome::Complete,
            },
        )
        .unwrap();
    };
    sink.begin_run(&id, 40, &RunScope::FullRoot).unwrap();
    let seed = sink.prepare_listing(&id, 40, None, &[], identity).unwrap();
    publish(40, seed, vec![entry("unseen", 0, true)]);
    let name: Vec<u16> = "unseen".encode_utf16().collect();
    let child = sink
        .prepare_listing(&id, 40, Some(seed.dir), &name, identity)
        .unwrap();
    publish(40, child, vec![entry("kept.bin", 7, false)]);
    sink.finish_run(&id, 40, &RunScope::FullRoot, true).unwrap();

    sink.begin_run(&id, 41, &RunScope::FullRoot).unwrap();
    let ticket = sink.prepare_listing(&id, 41, None, &[], identity).unwrap();
    publish(41, ticket, vec![entry("observed.bin", 13, false)]);
    // Individual complete listings may mark an unseen directory absent_pending. Even if the
    // catalogue looks traversed, the engine's limit veto must prevent global retirement.
    sink.finish_traversal(&id, 41, &RunScope::FullRoot, false, false, true)
        .unwrap();
    let conn = svc.inner.db();
    let (run, state, finished, complete): (i64, String, Option<i64>, Option<bool>) = conn
        .query_row(
            "SELECT id,state,finished_at_ns,json_extract(counters_json,'$.complete') FROM main.scan_run ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!((state.as_str(), complete), ("completed", Some(false)));
    assert!(finished.is_some());
    let (generation, state): (Option<i64>, String) = conn
        .query_row(
            "SELECT generation,state FROM main.root WHERE id=?1",
            [root],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((generation, state.as_str()), (Some(run), "partial"));
    let kept: (String, i64) = conn
        .query_row(
            "SELECT listing_state,(SELECT count(*) FROM main.file WHERE dir_id=?1 AND name='kept.bin') FROM main.dir WHERE id=?1",
            [child.dir as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((kept.0.as_str(), kept.1), ("absent_pending", 1));
    drop(conn);
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    let scan = &roots["roots"][0]["scan"];
    assert_eq!(scan["coverage"], "partial");
    assert!(!scan["finished_at"].is_null(), "{scan:?}");
    assert_eq!(scan["generation"], run.to_string());
    let (alerts, _) = ok("alerts.list", call(&svc, "alerts.list", json!({})));
    let growth = alerts["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "fast_growth")
        .unwrap();
    assert_eq!(growth["coverage"], "partial", "{growth:?}");
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
    // #561: late watcher hints from the tree build may start a refresh; read once it is idle.
    f3_wait_live(svc);
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
        volume_guid: None,
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
        ("health.get", json!({})),
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
            .handle(&req, cmd, &CallContext::http(), Some(&active), None);
        assert!(control.is_ok(), "{command} control: {:?}", control.err());
        assert_eq!(
            svc.inner
                .handle(&req, cmd, &CallContext::http(), Some(&expired), None)
                .err()
                .expect(command)
                .code,
            ErrorCode::DeadlineExceeded,
            "{command}"
        );
    }
}

#[test]
fn health_counts_do_not_use_the_shared_mutation_connection() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    fixture(&svc, 3);
    let expected = crate::db::counts(&svc.inner.db()).unwrap();
    svc.inner
        .db()
        .authorizer(Some(|ctx: AuthContext<'_>| {
            if ctx.database_name == Some("main")
                && matches!(
                    ctx.action,
                    AuthAction::Read {
                        table_name: "file" | "dir",
                        ..
                    }
                )
            {
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }))
        .unwrap();
    let (health, _) = ok("health.get", call(&svc, "health.get", json!({})));
    assert_eq!(health["catalog"]["files"], expected.0);
    assert_eq!(health["catalog"]["dirs"], expected.1);
}

#[test]
fn followup_picker_refusal_reaches_the_user() {
    struct Pick(std::path::PathBuf);
    impl NativeDialogs for Pick {
        fn pick_folder(&self) -> Option<std::path::PathBuf> {
            Some(self.0.clone())
        }
        fn confirm_disclosure(
            &self,
            _: &DisclosureSummary,
        ) -> Result<bool, loomward_protocol::ErrorBody> {
            Ok(false)
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
        fn confirm_disclosure(
            &self,
            _: &DisclosureSummary,
        ) -> Result<bool, loomward_protocol::ErrorBody> {
            Ok(false)
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
    if loomward_windows::enumerate::skip_elevated_test() {
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

#[path = "crash_sweep_tests.rs"]
mod crash_sweep_tests;
#[path = "duplicates_tests.rs"]
mod duplicates_tests;
#[path = "forget_tests.rs"]
mod forget_tests;
#[path = "learning_tests.rs"]
mod learning_tests;

#[cfg(test)]
#[path = "collections_tests.rs"]
mod collections_tests;

#[path = "meaning_tests.rs"]
mod meaning_tests;

/// #219: unverified rows never back a placement proposal, even a simulated one.
#[test]
fn placement_refuses_identity_unverified_rows() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, _, volume) = placement_fixture(&svc);
    ok(
        "placement.simulate",
        call(
            &svc,
            "placement.simulate",
            placement_request(&volume, false),
        ),
    );
    svc.inner
        .db()
        .execute(
            "UPDATE main.dir SET flags=flags|?2 WHERE root_id=?1 AND parent_id IS NOT NULL",
            rusqlite::params![root, loomward_catalog::IDENTITY_UNVERIFIED],
        )
        .unwrap();
    for (command, payload) in [
        ("placement.simulate", placement_request(&volume, false)),
        (
            "placement.candidates",
            json!({"source_volume_id":volume,"basis":"largest_dirs","max_groups":50,"min_bytes":"0"}),
        ),
    ] {
        let refused = err(call(&svc, command, payload));
        assert_eq!(refused.code, ErrorCode::PermissionDenied, "{command}");
        assert_eq!(refused.message.as_str(), "identity_unverified", "{command}");
    }
}

/// The grant row's identity mode and its root row's identity flag for catalogue root `root`.
#[cfg(windows)]
fn grant_identity(svc: &Service, root: i64) -> (String, u32, Option<Vec<u8>>) {
    svc.inner
        .db()
        .query_row(
            "SELECT g.identity_mode,d.flags & ?2,d.file_id FROM main.root r JOIN st.root_grant g ON g.id=r.grant_id
             JOIN main.dir d ON d.root_id=r.id AND d.parent_id IS NULL WHERE r.id=?1",
            rusqlite::params![root, loomward_catalog::IDENTITY_UNVERIFIED],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
}

/// #219: a personal root on a FAT, exFAT or unknown filesystem is granted only with the owner's
/// explicit confirmation (`--allow-identity-unverified`), is marked so in the grant record, and
/// the engine's granted root then allows identity-unverified listing. NTFS stays verified.
#[cfg(windows)]
#[test]
fn identity_unverified_cli_grants_need_explicit_confirmation() {
    if loomward_windows::enumerate::skip_elevated_test() {
        return;
    }
    let tmp = temp_root();
    let svc = open(&tmp.path().join("state"), DatasetClass::Personal);
    let (fat, ntfs) = (
        tmp.path().join("fat-folder"),
        tmp.path().join("ntfs-folder"),
    );
    std::fs::create_dir_all(&fat).unwrap();
    std::fs::create_dir_all(&ntfs).unwrap();
    let (fat, ntfs) = (fat.canonicalize().unwrap(), ntfs.canonicalize().unwrap());
    *svc.inner.hooks.filesystem.lock().unwrap() = Some("exFAT");
    assert_eq!(
        svc.inner.grant(&fat, "cli_flag", false),
        Err(crate::Refused::IdentityUnverified)
    );
    assert_eq!(crate::db::active_grants(&svc.inner.db()).unwrap(), 0);
    let (root, fresh) = svc.inner.grant(&fat, "cli_flag", true).unwrap();
    assert!(fresh);
    let (mode, flag, root_id) = grant_identity(&svc, root);
    assert_eq!(
        (mode.as_str(), flag, root_id),
        ("unverified", loomward_catalog::IDENTITY_UNVERIFIED, None)
    );
    let grant: i64 = svc
        .inner
        .db()
        .query_row("SELECT grant_id FROM main.root WHERE id=?1", [root], |r| {
            r.get(0)
        })
        .unwrap();
    let granted = svc
        .inner
        .engine
        .granted_root(
            RootId::new("rt_unverified_grant").unwrap(),
            grant,
            &loomward_engine::scan::CatalogRootIdentity {
                row: root,
                instance: svc.inner.instance.clone(),
            },
        )
        .unwrap();
    assert!(granted.allows_identity_unverified());
    *svc.inner.hooks.filesystem.lock().unwrap() = None;
    let (root, _) = svc.inner.grant(&ntfs, "cli_flag", true).unwrap();
    assert_eq!(grant_identity(&svc, root).0, "verified");
}

#[cfg(windows)]
#[test]
fn regrant_backfills_a_legacy_null_filesystem_without_replacing_the_grant() {
    if loomward_windows::enumerate::skip_elevated_test() {
        eprintln!("skipped: elevated token; native identity reads refuse elevation (invariant 2)");
        return;
    }
    let tmp = temp_root();
    let folder = tmp.path().join("synthetic-folder");
    std::fs::create_dir(&folder).unwrap();
    let folder = folder.canonicalize().unwrap();
    let svc = open(&tmp.path().join("state"), DatasetClass::Personal);
    let (root, _) = svc.inner.grant(&folder, "cli_flag", false).unwrap();
    let before = crate::db::active_grants(&svc.inner.db()).unwrap();
    svc.inner
        .db()
        .execute_batch(
            "UPDATE main.volume SET filesystem=NULL; UPDATE st.root_grant SET filesystem=NULL,volume_guid=NULL",
        )
        .unwrap();
    assert_eq!(
        svc.inner.grant(&folder, "cli_flag", false).unwrap(),
        (root, false)
    );
    let filesystem: Option<String> = svc
        .inner
        .db()
        .query_row("SELECT filesystem FROM main.volume", [], |r| r.get(0))
        .unwrap();
    assert_eq!(filesystem.as_deref(), Some("NTFS"));
    assert_eq!(
        svc.inner
            .db()
            .query_row("SELECT filesystem FROM st.root_grant", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "NTFS"
    );
    assert_eq!(crate::db::active_grants(&svc.inner.db()).unwrap(), before);
}

#[cfg(windows)]
#[test]
fn stored_unverified_grant_needs_no_second_cli_confirmation_after_restart() {
    if loomward_windows::enumerate::skip_elevated_test() {
        eprintln!("skipped: elevated token; native identity reads refuse elevation (invariant 2)");
        return;
    }
    let tmp = temp_root();
    let folder = tmp.path().join("synthetic-folder");
    std::fs::create_dir(&folder).unwrap();
    let folder = folder.canonicalize().unwrap();
    let state = tmp.path().join("state");
    let svc = open(&state, DatasetClass::Personal);
    *svc.inner.hooks.filesystem.lock().unwrap() = Some("exFAT");
    let (root, _) = svc.inner.grant(&folder, "cli_flag", true).unwrap();
    svc.inner
        .db()
        .execute("UPDATE st.root_grant SET volume_guid=NULL", [])
        .unwrap();
    drop(svc);
    let svc = open(&state, DatasetClass::Personal);
    *svc.inner.hooks.filesystem.lock().unwrap() = Some("exFAT");
    svc.inner
        .db()
        .execute("UPDATE st.root_grant SET volume_guid=NULL", [])
        .unwrap();
    assert_eq!(
        svc.inner.grant(&folder, "cli_flag", false).unwrap(),
        (root, false)
    );
    assert_eq!(grant_identity(&svc, root).0, "unverified");
    let (key, id) = crate::lab::identity(&folder).unwrap();
    let guid: String = svc
        .inner
        .db()
        .query_row(
            "SELECT volume_guid FROM st.root_grant WHERE state='active'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        crate::db::active_grant(&svc.inner.db(), &key, &id, "Another synthetic root", &guid)
            .unwrap(),
        None,
        "a recyclable ID cannot transfer the saved permission to another path"
    );
    let other = tmp.path().join("synthetic-other");
    std::fs::create_dir(&other).unwrap();
    assert_eq!(
        svc.inner
            .grant(&other.canonicalize().unwrap(), "cli_flag", false),
        Err(crate::Refused::IdentityUnverified)
    );
    svc.inner
        .catalog
        .writer()
        .call(WriteCommand::RevokeGrant {
            grant_id: svc
                .inner
                .db()
                .query_row("SELECT grant_id FROM main.root WHERE id=?1", [root], |r| {
                    r.get(0)
                })
                .unwrap(),
            revoked_at_ns: crate::now_ns(),
        })
        .unwrap();
    assert_eq!(
        svc.inner.grant(&folder, "cli_flag", false),
        Err(crate::Refused::IdentityUnverified)
    );
}

#[cfg(windows)]
#[test]
fn revocation_between_regrant_lookup_and_write_cannot_reuse_confirmation() {
    if loomward_windows::enumerate::skip_elevated_test() {
        eprintln!("skipped: elevated token; native identity reads refuse elevation (invariant 2)");
        return;
    }
    let tmp = temp_root();
    let folder = tmp.path().join("synthetic-folder");
    std::fs::create_dir(&folder).unwrap();
    let folder = folder.canonicalize().unwrap();
    let svc = open(&tmp.path().join("state"), DatasetClass::Personal);
    *svc.inner.hooks.filesystem.lock().unwrap() = Some("exFAT");
    let (root, _) = svc.inner.grant(&folder, "cli_flag", true).unwrap();
    let grant = svc
        .inner
        .db()
        .query_row("SELECT grant_id FROM main.root WHERE id=?1", [root], |r| {
            r.get(0)
        })
        .unwrap();
    svc.inner
        .hooks
        .closed
        .lock()
        .unwrap()
        .push((Command::RootsRequestGrant, "grant_observed"));
    let peer = svc.inner.clone();
    let regrant = std::thread::spawn(move || peer.grant(&folder, "cli_flag", false));
    svc.inner.hooks.until("regrant observed", |hooks| {
        hooks
            .arrived
            .lock()
            .unwrap()
            .contains(&(Command::RootsRequestGrant, "grant_observed"))
    });
    // The catalogue connection can revoke independently of the service's mutation mutex.
    svc.inner
        .catalog
        .writer()
        .call(WriteCommand::RevokeGrant {
            grant_id: grant,
            revoked_at_ns: crate::now_ns(),
        })
        .unwrap();
    svc.inner
        .hooks
        .open(Command::RootsRequestGrant, "grant_observed");
    assert!(
        regrant.join().unwrap().is_err(),
        "a revoked grant cannot supply a saved confirmation"
    );
    assert_eq!(crate::db::active_grants(&svc.inner.db()).unwrap(), 0);
}

/// #219: the desktop picker asks the one-line native confirmation only for a volume that cannot
/// verify identity; declining grants nothing and reports `cancelled_by_user`.
#[cfg(windows)]
#[test]
fn picker_confirms_identity_unverified_volumes_explicitly() {
    struct Pick(std::path::PathBuf, bool, std::sync::atomic::AtomicUsize);
    impl NativeDialogs for Pick {
        fn pick_folder(&self) -> Option<std::path::PathBuf> {
            Some(self.0.clone())
        }
        fn confirm_disclosure(&self, _: &DisclosureSummary) -> Result<bool, ErrorBody> {
            Ok(false)
        }
        fn confirm_identity_unverified(&self, folder: &Path, filesystem: &str) -> bool {
            assert_eq!(folder, self.0.canonicalize().unwrap());
            assert_eq!(filesystem, "FAT32");
            self.2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.1
        }
    }
    if loomward_windows::enumerate::skip_elevated_test() {
        return;
    }
    let tmp = temp_root();
    let svc = open(&tmp.path().join("state"), DatasetClass::Personal);
    let pick = |name: &str, filesystem: Option<&'static str>, accept: bool| {
        let folder = tmp.path().join(name);
        std::fs::create_dir_all(&folder).unwrap();
        *svc.inner.hooks.filesystem.lock().unwrap() = filesystem;
        // The picker has a two-second cooldown between dialogs.
        *svc.inner.picker_last.lock().unwrap() = None;
        let dialogs = Arc::new(Pick(folder, accept, Default::default()));
        let (result, _) = ok(
            "roots.request_grant",
            svc.call(
                request("roots.request_grant", json!({"purpose":"metadata_scan"})),
                &CallContext::tauri(dialogs.clone()),
            ),
        );
        (
            result["outcome"].as_str().unwrap().to_owned(),
            dialogs.2.load(std::sync::atomic::Ordering::SeqCst),
        )
    };
    assert_eq!(
        pick("declined", Some("FAT32"), false),
        ("cancelled_by_user".into(), 1)
    );
    assert_eq!(crate::db::active_grants(&svc.inner.db()).unwrap(), 0);
    assert_eq!(pick("accepted", Some("FAT32"), true), ("granted".into(), 1));
    assert_eq!(pick("ntfs", None, false), ("granted".into(), 0));
    let modes: Vec<String> = svc
        .inner
        .db()
        .prepare("SELECT identity_mode FROM st.root_grant ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(modes, ["unverified", "verified"]);
}

/// #219: the root's own open decides the run's identity mode: every row an unverified run
/// publishes is flagged and stored without a file ID, while a verified run keeps its IDs.
#[test]
fn an_unverified_run_flags_every_published_row() {
    use loomward_engine::scan::source::{FileIdObs, IdBasis, ListOutcome, OpenedIdentity};
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 21);
    let dir = crate::db::active_root_dir(&svc.inner.db(), root)
        .unwrap()
        .unwrap();
    let id = RootId::new(svc.inner.root_id(root)).unwrap();
    let sink = &svc.inner.scan;
    sink.bind(&id, grant, dir);
    for (run, basis, name) in [
        (61, IdBasis::PostOpen, "synthetic-verified"),
        (62, IdBasis::Unverified, "synthetic-unverified"),
    ] {
        sink.begin_run(&id, run, &RunScope::FullRoot).unwrap();
        let identity = OpenedIdentity {
            id: None,
            basis,
            volume_guid: None,
            volume_serial: None,
            attributes: 0x10,
            reparse_tag: None,
        };
        let ticket = sink.prepare_listing(&id, run, None, &[], identity).unwrap();
        let entry = loomward_engine::scan::Entry {
            name: name.encode_utf16().collect(),
            file_id: Some(FileIdObs::Id128([run as u8; 16])),
            identity_eligible: true,
            attributes: 0,
            reparse_tag: None,
            logical: 1,
            allocated: Some(1),
            creation: None,
            last_write: None,
            change: None,
            last_access: None,
            excluded: false,
            traversal_error: None,
        };
        sink.consume(
            &id,
            ScanMessage::DirListing {
                run,
                ticket,
                entries: vec![entry],
            },
        )
        .unwrap();
        sink.consume(
            &id,
            ScanMessage::ListingDone {
                run,
                ticket,
                outcome: ListOutcome::Incomplete(
                    loomward_engine::scan::source::IncompleteReason::Cancelled,
                ),
            },
        )
        .unwrap();
        sink.finish_run(&id, run, &RunScope::FullRoot, false)
            .unwrap();
        let (flags, file_id): (u32, Option<Vec<u8>>) = svc
            .inner
            .db()
            .query_row(
                "SELECT flags,file_id FROM main.file WHERE name=?1",
                [name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let unverified = basis == IdBasis::Unverified;
        assert_eq!(
            flags & loomward_catalog::IDENTITY_UNVERIFIED != 0,
            unverified,
            "{name}"
        );
        assert_eq!(file_id.is_none(), unverified, "{name}");
    }
}

/// #219 (coordinator follow-up): grants record their volume's real filesystem, so a file in a
/// live-scanned NTFS root keeps a durable reference. Before, every grant recorded none and labels
/// and collection adds were refused as `durable_reference_unavailable`.
#[cfg(windows)]
fn label_and_collect_live(svc: &Service, root: &str, file: &str) {
    let (filesystem, mode): (Option<String>, String) = svc
        .inner
        .db()
        .query_row(
            "SELECT v.filesystem,g.identity_mode FROM main.root r JOIN main.volume v ON v.id=r.volume_id
             JOIN st.root_grant g ON g.id=r.grant_id",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (filesystem.as_deref(), mode.as_str()),
        (Some("NTFS"), "verified")
    );
    let job = settle(svc, &start(svc, root, "full"));
    assert_eq!(job["state"], "completed", "{job:?}");
    // #561: a background watcher publication would otherwise race the collection read below.
    f3_wait_live(svc);
    let node = child_named(&lab_slice(svc, root), file).unwrap();
    let (inspected, _) = ok(
        "node.inspect",
        call(svc, "node.inspect", json!({"node_id": node})),
    );
    assert_eq!(inspected["identity"]["durable_reference"], "available");
    let label = call(
        svc,
        "feedback.record",
        json!({"node_id": node, "client_event_id": "ce_live_ntfs", "label": "Documents", "retract": false}),
    );
    // q-11: personal learning is open, so a real granted NTFS folder is labelled like a lab root
    // (the eligibility Codex asked for on #368, now that grants record their filesystem, #372).
    ok("feedback.record", label);
    let (created, _) = ok(
        "collections.create",
        call(svc, "collections.create", json!({"name": "Live NTFS"})),
    );
    let mut add = request(
        "collections.update_members",
        json!({"collection_id": created["collection_id"], "add": [node], "remove": []}),
    );
    add.expected_state_rev = Some(Generation(svc.inner.revisions().unwrap().1 as u64));
    ok(
        "collections.update_members",
        svc.call(add, &CallContext::http()),
    );
    let members = live_collection_members(svc, &created["collection_id"]);
    let items = members["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{members:?}");
    assert_eq!(items[0]["status"], "resolved");
}

#[cfg(windows)]
fn live_collection_members(svc: &Service, collection: &Value) -> Payload {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let response = call(
            svc,
            "collections.members",
            json!({"collection_id": collection, "limit": 10, "cursor": null}),
        );
        match &response {
            ResponseEnvelope::Err(e) if e.error.code == ErrorCode::StaleGeneration => {
                checked("collections.members", &response);
                assert!(
                    Instant::now() < deadline,
                    "live members never stabilised: {:?}",
                    e.error
                );
                // A newer watcher publication invalidates this read, not the durable membership.
                f3_wait_live(svc);
            }
            _ => return ok("collections.members", response).0,
        }
    }
}

#[cfg(windows)]
#[test]
fn live_collection_read_recovers_from_a_background_publication() {
    let Some(lab) = lab() else { return };
    let svc = Arc::new(lab.svc);
    let hydrate = (Command::CollectionsMembers, "hydrate");
    let published = (Command::ScanStart, "targeted_catalogue_published");
    svc.inner
        .hooks
        .closed
        .lock()
        .unwrap()
        .extend([hydrate, published]);
    let reading = {
        let svc = svc.clone();
        let root = lab.root.clone();
        std::thread::spawn(move || label_and_collect_live(&svc, &root, "top.bin"))
    };
    svc.inner.hooks.until("live members hydration", |h| {
        h.arrived.lock().unwrap().contains(&hydrate)
    });
    let clean = svc.inner.hit_freshness(1);
    let before = svc.inner.revisions().unwrap();
    // A real change inside the grant, with the state/WAL files outside watcher reach.
    std::fs::write(lab.path.join("a").join("new.bin"), b"synthetic").unwrap();
    let stale = svc.inner.hit_freshness(1);
    svc.inner.freshness.ready_targeted_hint(1);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !svc.inner.hooks.arrived.lock().unwrap().contains(&published) && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    let reached = svc.inner.hooks.arrived.lock().unwrap().contains(&published);
    let after = svc.inner.revisions().unwrap();
    // Release both workers before asserting, including when the reproduction fails.
    svc.inner.hooks.open(published.0, published.1);
    svc.inner.hooks.open(hydrate.0, hydrate.1);
    let result = reading.join();
    assert_ne!(
        clean.state,
        SearchFreshnessState::Stale,
        "state writes and reads dirtied the grant"
    );
    assert_eq!(stale.state, SearchFreshnessState::Stale);
    assert!(reached, "native change never caused a targeted publication");
    assert!(
        after.0 > before.0,
        "background scan did not advance catalogue revision"
    );
    assert_eq!(
        after.1, before.1,
        "the race must be a catalogue publication, not a preference edit"
    );
    result.unwrap();
    assert!(svc.inner.hooks.runs.lock().unwrap()[&Command::CollectionsMembers] >= 2);
}

#[cfg(windows)]
#[test]
fn a_live_scanned_ntfs_grant_can_be_labelled_and_collected() {
    let Some(lab) = lab() else { return };
    label_and_collect_live(&lab.svc, &lab.root, "top.bin");
    let tmp = temp_root();
    let folder = tmp.path().join("owner-folder");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("synthetic-note.txt"), b"synthetic").unwrap();
    let svc = Service::open(Config {
        state_dir: tmp.path().join("state"),
        dataset: DatasetClass::Personal,
        allow_personal: true,
        allow_identity_unverified: false,
        grant_roots: vec![folder.canonicalize().unwrap()],
    })
    .unwrap();
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    let root = roots["roots"][0]["root_id"].as_str().unwrap().to_owned();
    label_and_collect_live(&svc, &root, "synthetic-note.txt");
}

/// #710: a library caller may spell a root with a trailing separator. The grant must store the
/// canonical spelling, or a later open with the canonical form cannot reuse the verified grant.
#[cfg(windows)]
#[test]
fn open_stores_the_canonical_spelling_of_a_non_canonical_root() {
    let tmp = temp_root();
    let folder = tmp.path().join("owner-folder");
    std::fs::create_dir_all(&folder).unwrap();
    let canonical = folder.canonicalize().unwrap();
    let spelled = std::path::PathBuf::from(format!("{}\\", folder.display()));
    let opened = Service::open(Config {
        state_dir: tmp.path().join("state"),
        dataset: DatasetClass::Personal,
        allow_personal: true,
        allow_identity_unverified: false,
        grant_roots: vec![spelled],
    });
    if loomward_windows::enumerate::skip_elevated_test() {
        // Native identity refuses elevated tokens (invariant 2), including hosted Windows CI.
        let error = opened.err().expect("an elevated grant is refused");
        assert!(error.contains("identity_unavailable"), "{error}");
        return;
    }
    let svc = opened.unwrap();
    // Compare text: `PathBuf` equality ignores a trailing separator, which is the very difference.
    let stored: Vec<String> = crate::db::active_root_paths(&svc.inner.db())
        .unwrap()
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let expected = canonical.to_string_lossy();
    let expected = expected.strip_prefix(r"\\?\").unwrap_or(&expected);
    assert_eq!(stored, vec![expected.to_owned()]);
}

#[cfg(test)]
#[path = "backups_tests.rs"]
mod backups_tests;

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod diagnostics_tests;

#[cfg(test)]
#[path = "copy_tests.rs"]
mod copy_tests;
#[path = "gap_tests.rs"]
mod gap_tests;

#[cfg(test)]
#[path = "alerts_tests.rs"]
mod alerts_tests;

#[cfg(test)]
#[path = "clutter_tests.rs"]
mod clutter_tests;

#[test]
fn active_grant_matches_guid_and_unverified_path_together() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let guid = "00000000000000000000000000000001";
    let WriteReply::Root { grant_id, .. } = svc
        .inner
        .catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: Some(guid.into()),
            volume_key: "fixture-volume".into(),
            root_file_id: Some(vec![1; 16]),
            display_name: "Synthetic volume".into(),
            display_path: "Synthetic root".into(),
            filesystem: Some("exFAT".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 1,
        }))
        .unwrap()
    else {
        panic!()
    };
    let conn = svc.inner.db();
    assert_eq!(
        crate::db::active_grant(&conn, "fixture-volume", &[1; 16], "Synthetic root", guid).unwrap(),
        Some((grant_id, "unverified".into()))
    );
    assert_eq!(
        crate::db::active_grant(
            &conn,
            "fixture-volume",
            &[1; 16],
            "Another synthetic root",
            guid
        )
        .unwrap(),
        None
    );
    assert_eq!(
        crate::db::active_grant(
            &conn,
            "fixture-volume",
            &[1; 16],
            "Synthetic root",
            "00000000000000000000000000000002"
        )
        .unwrap(),
        None
    );
}

/// #148: a listing's chunk and its publication reach the writer without waiting for each receipt,
/// so one transaction commits both; the next child lookup waits for that commit first.
#[cfg(windows)]
#[test]
fn a_listing_chunk_and_its_publication_share_one_transaction() {
    use loomward_engine::scan::source::{FileIdObs, IdBasis, ListOutcome, OpenedIdentity};
    use loomward_engine::scan::Entry;
    let Some(lab) = lab() else { return };
    let inner = &lab.svc.inner;
    inner.freshness.pause_pump();
    let row = inner.root_row(&lab.root).unwrap();
    let stored = crate::db::roots(&inner.db(), Some(row)).unwrap().remove(0);
    let id = RootId::new(&lab.root).unwrap();
    inner.scan.bind(&id, stored.grant, stored.dir.unwrap());
    let sink: &dyn ScanSink = &*inner.scan;
    let identity = |id| OpenedIdentity {
        id: Some(FileIdObs::Id64(id)),
        basis: IdBasis::Listed,
        volume_guid: None,
        volume_serial: Some(42),
        attributes: 0x10,
        reparse_tag: None,
    };
    let entry = |name: &str, id, attributes| Entry {
        name: name.encode_utf16().collect(),
        file_id: Some(FileIdObs::Id64(id)),
        identity_eligible: true,
        attributes,
        reparse_tag: None,
        logical: 10,
        allocated: Some(4096),
        creation: Some(1),
        last_write: Some(1),
        change: Some(1),
        last_access: Some(1),
        excluded: false,
        traversal_error: None,
    };
    sink.begin_run(&id, 7, &RunScope::FullRoot).unwrap();
    let ticket = sink
        .prepare_listing(&id, 7, None, &[], identity(1))
        .unwrap();
    let writer = inner.catalog.writer();
    let before = writer.timings().transactions;
    // The writer holds its first listing command; a waiting sink could not send the second.
    writer.publication_window().arm();
    let (sent, returned) = std::sync::mpsc::channel();
    let outcome = std::thread::scope(|scope| {
        scope.spawn(|| {
            let entries = vec![entry("a.bin", 2, 0x20), entry("sub", 3, 0x10)];
            sink.consume(
                &id,
                ScanMessage::DirListing {
                    run: 7,
                    ticket,
                    entries,
                },
            )
            .unwrap();
            let outcome = ListOutcome::Complete;
            sink.consume(
                &id,
                ScanMessage::ListingDone {
                    run: 7,
                    ticket,
                    outcome,
                },
            )
            .unwrap();
            sent.send(()).unwrap();
        });
        let pipelined = returned.recv_timeout(Duration::from_secs(10)).is_ok();
        // The child lookup starts while the writer still holds the publication: it must wait.
        let (looked_up, child) = std::sync::mpsc::channel();
        let id = &id;
        scope.spawn(move || {
            let name: Vec<u16> = "sub".encode_utf16().collect();
            let found = sink.prepare_listing(id, 7, Some(ticket.dir), &name, identity(3));
            looked_up.send(found).unwrap();
        });
        let early = child.recv_timeout(Duration::from_millis(200));
        writer.publication_window().release();
        let late = child.recv_timeout(Duration::from_secs(10));
        (pipelined, early, late)
    });
    let (pipelined, early, late) = outcome;
    assert!(pipelined, "consume waited for a receipt");
    assert!(
        matches!(early, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
        "the child lookup answered before its parent's publication committed"
    );
    late.expect("the child lookup finishes once the writer commits")
        .expect("the child lookup waits for its parent's publication");
    assert_eq!(writer.timings().transactions - before, 1);
    sink.finish_run(&id, 7, &RunScope::FullRoot, false).unwrap();
}

/// Run 42 on a fresh root whose listing the writer will refuse after `consume` returned: each
/// chunk is valid alone, but the publication names one entry twice.
fn stage_deferred_refusal(svc: &Service) -> (RootId, u64) {
    use loomward_engine::scan::source::{IdBasis, ListOutcome, OpenedIdentity};
    let (root, grant) = register(svc, 7);
    let dir = crate::db::active_root_dir(&svc.inner.db(), root)
        .unwrap()
        .unwrap();
    let id = RootId::new(svc.inner.root_id(root)).unwrap();
    let sink = &svc.inner.scan;
    sink.bind(&id, grant, dir);
    sink.begin_run(&id, 42, &RunScope::FullRoot).unwrap();
    let identity = OpenedIdentity {
        id: None,
        basis: IdBasis::None,
        volume_guid: None,
        volume_serial: None,
        attributes: 0x10,
        reparse_tag: None,
    };
    let ticket = sink.prepare_listing(&id, 42, None, &[], identity).unwrap();
    for n in 1..=2 {
        let entries = vec![entry("dup.bin", n, false)];
        sink.consume(
            &id,
            ScanMessage::DirListing {
                run: 42,
                ticket,
                entries,
            },
        )
        .unwrap();
    }
    let outcome = ListOutcome::Complete;
    sink.consume(
        &id,
        ScanMessage::ListingDone {
            run: 42,
            ticket,
            outcome,
        },
    )
    .unwrap();
    (id, ticket.dir)
}

/// #148: a chunk the writer refuses after `consume` returned is still relisted: settling names its
/// directory, and its listing published partial, never complete.
#[test]
fn a_deferred_refusal_settles_as_a_relist_and_never_completes() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (id, dir) = stage_deferred_refusal(&svc);
    let sink = &svc.inner.scan;
    assert_eq!(sink.settle(&id).unwrap(), vec![dir]);
    assert_eq!(sink.settle(&id).unwrap(), Vec::<u64>::new(), "settled once");
    let state: String = svc
        .inner
        .db()
        .query_row(
            "SELECT listing_state FROM main.bulk_dir WHERE id=?1 UNION ALL SELECT listing_state FROM main.dir WHERE id=?1 LIMIT 1",
            [dir as i64],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(state, "complete");
    sink.finish_run(&id, 42, &RunScope::FullRoot, false)
        .unwrap();
}

/// #148, invariant 4: an engine that claims completeness while a deferred refusal is still
/// unsettled cannot end the run completed; `finish_run` settles it itself and downgrades.
#[test]
fn an_unsettled_deferred_refusal_cannot_end_the_run_completed() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (id, _) = stage_deferred_refusal(&svc);
    svc.inner
        .scan
        .finish_run(&id, 42, &RunScope::FullRoot, true)
        .unwrap();
    let state: String = svc
        .inner
        .db()
        .query_row(
            "SELECT state FROM main.scan_run ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "cancelled", "the run must not end completed");
}

/// #781: a budget stop cannot turn an unresolved writer refusal into a completed sample.
#[test]
fn an_unsettled_deferred_refusal_cannot_finish_a_limited_sample() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (id, _) = stage_deferred_refusal(&svc);
    svc.inner
        .scan
        .finish_traversal(&id, 42, &RunScope::FullRoot, false, false, true)
        .unwrap();
    let (state, complete): (String, Option<bool>) = svc
        .inner
        .db()
        .query_row(
            "SELECT state,json_extract(counters_json,'$.complete') FROM main.scan_run ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((state.as_str(), complete), ("cancelled", None));
}
