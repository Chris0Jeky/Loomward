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
