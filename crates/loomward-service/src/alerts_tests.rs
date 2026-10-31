//! LW-121: synthetic growth and low-space fixtures raise exactly the expected alerts, and coverage
//! gaps suppress or qualify them rather than reading as zero.
use super::*;
use loomward_catalog::{DirListing, WriteCommand, WriteReply};

const GIB: u64 = 1 << 30;
const DAY_NS: i64 = 86_400 * 1_000_000_000;

/// One full run over the root's directory: a single file of `bytes`, finishing `days_ago` days ago.
/// `skipped > 0` makes the run's coverage partial while still completing.
fn scan(svc: &Service, grant: i64, root: i64, bytes: u64, days_ago: i64, skipped: u64) {
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
        panic!("run receipt")
    };
    w.call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: dir,
        files: vec![file("data.bin", bytes)],
        dirs: vec![],
        state: "complete".into(),
        skipped,
        errors: 0,
    }))
    .unwrap();
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: crate::now_ns() - days_ago * DAY_NS,
    })
    .unwrap();
}

fn set_space(svc: &Service, capacity: u64, free: u64) {
    svc.inner
        .db()
        .execute(
            "UPDATE main.volume SET capabilities_json=?1",
            [json!({"space":{"capacity_bytes":capacity.to_string(),"free_bytes":free.to_string(),"read_only":false}}).to_string()],
        )
        .unwrap();
}

fn report(svc: &Service) -> Value {
    let (r, _) = ok("alerts.list", call(svc, "alerts.list", json!({})));
    json!(r)
}

fn statuses(report: &Value) -> Vec<(String, String)> {
    report["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["kind"].as_str().unwrap().to_owned(),
                i["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn pair(kind: &str, status: &str) -> (String, String) {
    (kind.to_owned(), status.to_owned())
}

#[test]
fn before_any_scan_or_space_observation_nothing_is_claimed() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 1);
    // A scan with no recorded history inside the window: one old sample only.
    scan(&svc, grant, root, 100 * GIB, 400, 0);
    let r = report(&svc);
    assert_eq!(r["settings"], r["defaults"]);
    assert_eq!(r["settings"]["notifications_enabled"], false);
    assert_eq!(r["active_count"], 0);
    // No space observation: capacity and free stay null (unknown), never zero free.
    assert_eq!(
        statuses(&r),
        [
            pair("low_space", "unknown"),
            pair("fast_growth", "insufficient_history")
        ]
    );
    assert!(r["items"][0]["measured_bytes"].is_null());
    assert!(r["items"][0]["capacity_bytes"].is_null());
    assert!(r["items"][1]["measured_bytes"].is_null());
}

#[test]
fn fixtures_raise_exactly_the_expected_alerts() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (a, grant_a) = register(&svc, 1);
    let (b, grant_b) = register(&svc, 2);
    let (c, grant_c) = register(&svc, 3);
    // 5 GiB free of 100 GiB: below the larger of 10% and 10 GiB.
    set_space(&svc, 100 * GIB, 5 * GIB);
    // A grows 7 GiB over 5 days: alert. B reaches a huge size only in a partial scan: qualified.
    // C grows 1 GiB: ok.
    scan(&svc, grant_a, a, GIB, 5, 0);
    scan(&svc, grant_a, a, 8 * GIB, 1, 0);
    scan(&svc, grant_b, b, GIB, 5, 0);
    scan(&svc, grant_b, b, 90 * GIB, 1, 1);
    scan(&svc, grant_c, c, GIB, 5, 0);
    scan(&svc, grant_c, c, 2 * GIB, 1, 0);
    let r = report(&svc);
    assert_eq!(
        statuses(&r),
        [
            pair("low_space", "alert"),
            pair("fast_growth", "alert"),
            pair("fast_growth", "incomplete_coverage"),
            pair("fast_growth", "ok"),
        ]
    );
    assert_eq!(r["active_count"], 2);
    let items = r["items"].as_array().unwrap();
    assert_eq!(items[0]["measured_bytes"], (5 * GIB).to_string());
    assert_eq!(items[0]["threshold_bytes"], (10 * GIB).to_string());
    assert_eq!(items[0]["coverage"], "complete");
    assert_eq!(items[1]["measured_bytes"], (7 * GIB).to_string());
    assert_eq!(items[1]["root_id"], json!(svc.inner.root_id(a)));
    // Partial coverage claims no growth and says so.
    assert!(items[2]["measured_bytes"].is_null());
    assert_eq!(items[2]["coverage"], "partial");
    assert_eq!(items[3]["measured_bytes"], GIB.to_string());
}

#[test]
fn configure_persists_thresholds_and_changes_the_outcome() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (a, grant) = register(&svc, 1);
    set_space(&svc, 100 * GIB, 30 * GIB);
    scan(&svc, grant, a, GIB, 5, 0);
    scan(&svc, grant, a, 8 * GIB, 1, 0);
    assert_eq!(
        statuses(&report(&svc)),
        [pair("low_space", "ok"), pair("fast_growth", "alert")]
    );
    let strict = json!({
        "notifications_enabled": true,
        "low_space_free_percent": 40,
        "low_space_free_bytes": "0",
        "growth_window_days": 30,
        "growth_bytes": (20 * GIB).to_string()
    });
    let (configured, meta) = ok(
        "alerts.configure",
        call(&svc, "alerts.configure", strict.clone()),
    );
    assert_eq!(json!(configured)["settings"], strict);
    assert!(meta.state_rev.is_some());
    // 30 of 100 GiB free is now below 40%, and 7 GiB of growth is under 20 GiB.
    let r = report(&svc);
    assert_eq!(r["settings"], strict);
    assert_eq!(
        statuses(&r),
        [pair("low_space", "alert"), pair("fast_growth", "ok")]
    );
    assert_eq!(r["defaults"]["low_space_free_percent"], 10);
    assert!(r["notifications_note"]
        .as_str()
        .unwrap()
        .contains("enabled"));
    // The record survives a reopen of the same state directory.
    drop(svc);
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    assert_eq!(report(&svc)["settings"], strict);
}

#[test]
fn configure_refuses_out_of_range_thresholds_and_writes_nothing() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let mut bad = json!({
        "notifications_enabled": false,
        "low_space_free_percent": 0,
        "low_space_free_bytes": "10737418240",
        "growth_window_days": 7,
        "growth_bytes": "5368709120"
    });
    assert_eq!(
        err(call(&svc, "alerts.configure", bad.clone())).code,
        ErrorCode::InvalidRequest
    );
    bad["low_space_free_percent"] = json!(10);
    bad["growth_window_days"] = json!(91);
    assert_eq!(
        err(call(&svc, "alerts.configure", bad)).code,
        ErrorCode::InvalidRequest
    );
    let r = report(&svc);
    assert_eq!(r["settings"], r["defaults"]);
}

#[test]
fn a_revoked_root_stops_being_evaluated() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (a, grant) = register(&svc, 1);
    set_space(&svc, 100 * GIB, 5 * GIB);
    scan(&svc, grant, a, GIB, 1, 0);
    assert_eq!(report(&svc)["items"].as_array().unwrap().len(), 2);
    svc.inner.revoke(grant).unwrap();
    assert!(report(&svc)["items"].as_array().unwrap().is_empty());
}

/// #519: a low-space alert evaluates for a personal volume that holds an active owner grant, from
/// the space `volumes.list` observes through that grant. The threshold is set above any real free
/// space, so the observed temp volume must trip it.
#[cfg(windows)]
#[test]
fn low_space_alert_evaluates_for_an_active_personal_grant_volume() {
    let tmp = temp_root();
    let Some((svc, _folder)) = personal_grant_session(&tmp) else {
        return;
    };
    let mut settings = report(&svc)["settings"].clone();
    settings["low_space_free_bytes"] = json!((1u64 << 50).to_string());
    ok("alerts.configure", call(&svc, "alerts.configure", settings));
    let r = report(&svc);
    let low = r["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "low_space")
        .unwrap();
    assert_eq!(low["status"], "alert", "{:?}", low);
    assert!(low["measured_bytes"].is_string(), "{:?}", low);
}

fn history(svc: &Service, root: i64) -> Value {
    json!(
        ok(
            "alerts.history",
            call(
                svc,
                "alerts.history",
                json!({"root_id":svc.inner.root_id(root)})
            )
        )
        .0
    )
}

fn recorded_row(svc: &Service, root: i64, finished: Option<i64>, state: &str, counters: &str) {
    svc.inner.db().execute(
        "INSERT INTO main.scan_run(root_id,mode,strategy,state,started_at_ns,finished_at_ns,counters_json,budget_json)
         VALUES(?1,'full','history_fixture',?2,0,?3,?4,'{}')",
        rusqlite::params![root, state, finished, counters],
    ).unwrap();
}

#[test]
fn history_preserves_exact_zero_unknown_partial_and_filters_unfinished_runs() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, _) = register(&svc, 1);
    let when = now_ns() - DAY_NS;
    for counters in [
        r#"{"logical_bytes":9007199254740993,"complete":true}"#,
        r#"{"logical_bytes":0,"complete":true}"#,
        r#"{"logical_bytes":23,"complete":false}"#,
        r#"{"logical_bytes":42}"#,
        r#"{"complete":true}"#,
        r#"{"logical_bytes":-1,"complete":true}"#,
        r#"{"logical_bytes":"unknown","complete":true}"#,
        "malformed",
        r#"{"complete":false}"#,
    ] {
        recorded_row(&svc, root, Some(when), "completed", counters);
    }
    for state in ["running", "cancelled", "failed"] {
        recorded_row(
            &svc,
            root,
            Some(when),
            state,
            r#"{"logical_bytes":99,"complete":true}"#,
        );
    }
    recorded_row(&svc, root, None, "completed", "{}");
    let result = history(&svc, root);
    let samples = result["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 9);
    assert_eq!(samples[0]["logical_bytes"], "9007199254740993");
    assert_eq!(samples[1]["logical_bytes"], "0");
    assert_eq!(samples[2]["coverage"], "partial");
    assert_eq!(samples[3]["logical_bytes"], "42");
    assert_eq!(samples[3]["coverage"], "unknown");
    for sample in &samples[4..8] {
        assert!(sample["logical_bytes"].is_null());
        assert_eq!(sample["coverage"], "unknown");
    }
    assert!(samples[8]["logical_bytes"].is_null());
    assert_eq!(samples[8]["coverage"], "partial");
    assert_eq!(result["truncated"], false);
    // The same existing evaluator sees unknown completeness as partial and skips absent sizes.
    assert_eq!(result["check"], report(&svc)["items"][1]);
    assert_eq!(result["check"]["status"], "incomplete_coverage");
}

#[test]
fn history_retains_the_newest_64_with_stable_ties_and_unchanged_window_math() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 1);
    scan(&svc, grant, root, 90 * GIB, 40, 0);
    scan(&svc, grant, root, GIB, 5, 0);
    scan(&svc, grant, root, 8 * GIB, 1, 0);
    let result = history(&svc, root);
    assert_eq!(
        result["samples"].as_array().unwrap().len(),
        3,
        "history includes retained older records"
    );
    assert_eq!(
        result["check"]["measured_bytes"],
        (7 * GIB).to_string(),
        "old records do not enter the growth window"
    );
    assert_eq!(result["check"], report(&svc)["items"][1]);
    let when = now_ns();
    for size in 1..=65 {
        recorded_row(
            &svc,
            root,
            Some(when),
            "completed",
            &json!({"logical_bytes":size,"complete":true}).to_string(),
        );
    }
    let capped = history(&svc, root);
    let samples = capped["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 64);
    assert_eq!(capped["truncated"], true);
    assert_eq!(
        samples
            .iter()
            .map(|s| s["logical_bytes"].as_str().unwrap().parse::<u64>().unwrap())
            .collect::<Vec<_>>(),
        (2..=65).collect::<Vec<_>>()
    );
    assert_eq!(capped["check"]["measured_bytes"], "63");
    assert_eq!(capped["check"], report(&svc)["items"][1]);
}

#[test]
fn history_reopens_with_fresh_ids_and_leaves_preferences_grants_revisions_and_files_unchanged() {
    let tmp = temp_root();
    let original = tmp.path().join("owner-fixture.txt");
    std::fs::write(&original, b"synthetic owner bytes").unwrap();
    let before_file = (
        std::fs::read(&original).unwrap(),
        std::fs::metadata(&original).unwrap().modified().unwrap(),
    );
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 1);
    scan(&svc, grant, root, GIB, 5, 0);
    scan(&svc, grant, root, 8 * GIB, 1, 0);
    let settings = json!({"notifications_enabled":false,"low_space_free_percent":10,"low_space_free_bytes":"0","growth_window_days":14,"growth_bytes":"99999999999"});
    ok(
        "alerts.configure",
        call(&svc, "alerts.configure", settings.clone()),
    );
    let before_revs = svc.inner.revisions().unwrap();
    let before_grants = json!(ok("grants.list", call(&svc, "grants.list", json!({}))).0);
    let saved = history(&svc, root);
    assert_eq!(saved["growth_window_days"], 14);
    assert_eq!(svc.inner.revisions().unwrap(), before_revs);
    assert_eq!(
        json!(ok("grants.list", call(&svc, "grants.list", json!({}))).0),
        before_grants
    );
    assert_eq!(report(&svc)["settings"], settings);
    drop(svc);
    let reopened = open(tmp.path(), DatasetClass::Synthetic);
    let roots = json!(ok("roots.list", call(&reopened, "roots.list", json!({}))).0);
    let fresh_id = roots["roots"][0]["root_id"].clone();
    let before_revs = reopened.inner.revisions().unwrap();
    let fresh = json!(
        ok(
            "alerts.history",
            call(&reopened, "alerts.history", json!({"root_id":fresh_id}))
        )
        .0
    );
    assert_eq!(fresh["samples"], saved["samples"]);
    assert_eq!(fresh["growth_window_days"], saved["growth_window_days"]);
    assert_eq!(
        fresh["check"]["measured_bytes"],
        saved["check"]["measured_bytes"]
    );
    assert_eq!(fresh["check"]["status"], saved["check"]["status"]);
    assert_eq!(report(&reopened)["settings"], settings);
    assert_eq!(reopened.inner.revisions().unwrap(), before_revs);
    assert_eq!(
        (
            std::fs::read(&original).unwrap(),
            std::fs::metadata(&original).unwrap().modified().unwrap()
        ),
        before_file
    );
}

#[test]
fn history_refuses_foreign_revoked_forgotten_and_client_access_and_fences_pending_replies() {
    for forget in [false, true] {
        for adapter in [Adapter::Http, Adapter::Tauri] {
            let tmp = temp_root();
            let svc = open(tmp.path(), DatasetClass::Synthetic);
            let (root, grant) = register(&svc, 1);
            scan(&svc, grant, root, GIB, 1, 0);
            let id = svc.inner.root_id(root);
            let payload = json!({"root_id":id});
            let foreign_dir = temp_root();
            let foreign = open(foreign_dir.path(), DatasetClass::Synthetic);
            let (foreign_root, _) = register(&foreign, 1);
            assert!(matches!(
                call(
                    &svc,
                    "alerts.history",
                    json!({"root_id":foreign.inner.root_id(foreign_root)})
                ),
                ResponseEnvelope::Err(_)
            ));
            svc.grant_client(
                "mcp:alert-history",
                &[RootId::new(id.clone()).unwrap()],
                &["size"],
                None,
            )
            .unwrap();
            assert_eq!(
                err(svc.call(
                    request("alerts.history", payload.clone()),
                    &CallContext::client("mcp:alert-history")
                ))
                .code,
                ErrorCode::PermissionDenied
            );
            let mut context = CallContext::http();
            context.adapter = adapter;
            let pending = svc.call_pending(request("alerts.history", payload.clone()), &context);
            svc.inner.revoke(grant).unwrap();
            if forget {
                ok(
                    "roots.forget",
                    call(&svc, "roots.forget", json!({"root_id":id})),
                );
            }
            assert_eq!(reason(&err(pending.admit())), Some("forgotten_result"));
            let refusal = err(call(&svc, "alerts.history", payload));
            assert_eq!(refusal.code, ErrorCode::NotFound);
            let body = serde_json::to_string(&refusal).unwrap();
            assert!(!body.contains("Synthetic root 1") && !body.contains("logical_bytes"));
        }
    }
}

#[test]
fn history_deadline_discards_a_held_read_and_releases_its_worker() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, _) = register(&svc, 1);
    let before = svc.inner.revisions().unwrap();
    svc.inner
        .hooks
        .closed
        .lock()
        .unwrap()
        .push((Command::AlertsHistory, "history_snapshot"));
    *svc.inner.hooks.read_ahead.lock().unwrap() = Duration::from_secs(61);
    let response = call(
        &svc,
        "alerts.history",
        json!({"root_id":svc.inner.root_id(root)}),
    );
    assert_eq!(err(response).code, ErrorCode::DeadlineExceeded);
    svc.inner.hooks.until("history snapshot held", |h| {
        h.arrived
            .lock()
            .unwrap()
            .contains(&(Command::AlertsHistory, "history_snapshot"))
    });
    *svc.inner.hooks.skew.lock().unwrap() = Duration::from_secs(61);
    svc.inner
        .hooks
        .open(Command::AlertsHistory, "history_snapshot");
    svc.inner.hooks.until("history worker released", |_| {
        *svc.inner.reads.lock().unwrap() == 0
    });
    assert_eq!(svc.inner.revisions().unwrap(), before);
}

#[test]
fn history_refuses_a_snapshot_superseded_by_revocation() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 1);
    scan(&svc, grant, root, GIB, 1, 0);
    svc.inner
        .hooks
        .closed
        .lock()
        .unwrap()
        .push((Command::AlertsHistory, "history_snapshot"));
    let worker = svc.clone();
    let read = std::thread::spawn(move || {
        let mut req = request(
            "alerts.history",
            json!({"root_id":worker.inner.root_id(root)}),
        );
        req.deadline_ms = Some(Int::new(60_000).unwrap());
        worker.call(req, &CallContext::http())
    });
    svc.inner.hooks.until("snapshot held before revoke", |h| {
        h.arrived
            .lock()
            .unwrap()
            .contains(&(Command::AlertsHistory, "history_snapshot"))
    });
    svc.inner.revoke(grant).unwrap();
    svc.inner
        .hooks
        .open(Command::AlertsHistory, "history_snapshot");
    assert_eq!(err(read.join().unwrap()).code, ErrorCode::StaleGeneration);
}
