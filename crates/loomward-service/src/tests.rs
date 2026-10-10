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
/// commands finished their delay in, and a one-shot forced mutation-thread spawn failure.
#[derive(Default)]
pub(crate) struct Hooks {
    pub delay: Mutex<Option<(Command, Duration)>>,
    pub runs: Mutex<HashMap<Command, usize>>,
    pub order: Mutex<Vec<Command>>,
    pub fail_spawn: std::sync::atomic::AtomicBool,
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

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

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
    let tmp = tempfile::tempdir().unwrap();
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
    assert_eq!(hello["capabilities"]["observation"]["metadata_scan"], false);
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
    let svc = Service::open_with(
        Config {
            state_dir: tmp.path().into(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            grant_roots: vec![],
        },
        Duration::from_millis(1500),
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let other_tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    if cfg!(not(windows)) {
        // No native file identity off Windows: every grant root fails closed before provenance.
        assert!(refused.contains("identity_unavailable"), "{refused}");
        return;
    }
    assert!(
        refused.contains("synthetic_session_requires_lab_root"),
        "{refused}"
    );
    // A registered lab root is accepted, and listed as lab_generated.
    let lab = tmp.path().join("lab-s1");
    crate::lab::register(&state, &lab, "seed-1", "S", "sha256:00").unwrap();
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
    assert!(crate::lab::register(&state, &owner, "s", "S", "sha256:00").is_err());
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
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
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
            origin: "fixture".into(),
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 1);
    *svc.inner.hooks.delay.lock().unwrap() =
        Some((Command::VolumesDeclareTier, Duration::from_millis(1500)));
    for _ in 0..crate::MAX_PENDING {
        let e = err(declare_with_deadline(&svc, "vo_flood", 1));
        assert_eq!(e.code, ErrorCode::DeadlineExceeded);
    }
    let e = err(declare_with_deadline(&svc, "vo_flood", 1));
    assert_eq!(
        (e.code, e.retryable),
        (ErrorCode::Busy, true),
        "65th refused"
    );
    let t = Instant::now();
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
    assert!(
        t.elapsed() < Duration::from_millis(1000),
        "revocation did not wait for the blocked queue head"
    );
    *svc.inner.hooks.delay.lock().unwrap() = None;
}

/// H2: a mutation whose thread could not be spawned never takes a ticket, so a later mutation
/// still waits for an earlier one, and the queue keeps moving.
#[test]
fn a_failed_mutation_spawn_keeps_queue_order_and_liveness() {
    let tmp = tempfile::tempdir().unwrap();
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

/// A malformed stored node-ID key refuses to open the service instead of becoming zero bytes.
#[test]
fn a_malformed_node_id_key_refuses_to_open() {
    let tmp = tempfile::tempdir().unwrap();
    drop(open(tmp.path(), DatasetClass::Synthetic));
    for bad in ["zz".repeat(32), "00".repeat(31), "+0".repeat(32)] {
        rusqlite::Connection::open(tmp.path().join("state.db"))
            .unwrap()
            .execute("UPDATE meta SET value=?1 WHERE key='node_id_key'", [&bad])
            .unwrap();
        let opened = Service::open(Config {
            state_dir: tmp.path().to_path_buf(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            grant_roots: vec![],
        });
        assert!(
            opened.err().is_some_and(|e| e.contains("malformed")),
            "{bad}"
        );
    }
}
