//! #1092 `changes.new_since`: synthetic trees scanned through the real writer (so `born_run` is stamped
//! by real runs), then rescanned with additions. The baseline, `no_baseline`, unknown allocation, partial
//! reasons and refusals are pinned here; the row-level query is pinned in `loomward-catalog`'s `new_since`.
use super::*;

/// 2025-10-30T08:10:00Z, so a baseline date is checked as text.
const SCAN_ONE_NS: i64 = 1_761_811_800_000_000_000;

fn node(name: &'static str, files: &[(&'static str, u64)], dirs: Vec<Node>) -> Node {
    dir(name, files, dirs)
}

fn first_tree() -> Node {
    node(
        "",
        &[("a.txt", 100)],
        vec![node("sub", &[("b.txt", 200)], vec![])],
    )
}

/// One more completed full-root run over `tree`, through the writer, with the given counters.
fn rescan(svc: &Service, root: &str, tree: &Node) -> i64 {
    let row = svc.inner.root_row(root).unwrap();
    let (grant, dir_id): (i64, i64) = svc
        .inner
        .db()
        .query_row(
            "SELECT r.grant_id,d.id FROM main.root r JOIN main.dir d ON d.root_id=r.id AND d.parent_id IS NULL WHERE r.id=?1",
            [row],
            |r| Ok((r.get(0)?, r.get(1)?)),
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
    list(svc, run, dir_id, tree);
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 2,
    })
    .unwrap();
    run
}

fn second_tree() -> Node {
    Node {
        name: "",
        files: vec![("a.txt", 100, Some(100)), ("new.bin", 5000, Some(5000))],
        dirs: vec![
            node("sub", &[("b.txt", 200), ("c.txt", 300)], vec![]),
            node("newdir", &[("n.bin", 7000)], vec![]),
        ],
    }
}

fn since(svc: &Service, root: &str, baseline: Value, limit: u32) -> Value {
    json!(
        ok(
            "changes.new_since",
            call(
                svc,
                "changes.new_since",
                json!({"root_id": root, "baseline_run": baseline, "limit": limit})
            )
        )
        .0
    )
}

fn refused(svc: &Service, payload: Value, code: ErrorCode) {
    assert_eq!(
        err(call(svc, "changes.new_since", payload.clone())).code,
        code,
        "{payload}"
    );
}

fn names(r: &Value, list: &str) -> Vec<String> {
    r[list]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn rows_born_after_the_previous_scan_are_counted_and_ranked() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(&svc, 1, &first_tree());
    svc.inner
        .db()
        .execute(
            "UPDATE main.scan_run SET finished_at_ns=?1 WHERE id=1",
            [SCAN_ONE_NS],
        )
        .unwrap();
    rescan(&svc, &root, &second_tree());
    let before = svc.inner.revisions().unwrap();

    let r = since(&svc, &root, Value::Null, 10);
    assert_eq!(
        svc.inner.revisions().unwrap(),
        before,
        "a read must not write"
    );
    assert_eq!(r["outcome"], "ok");
    assert_eq!(
        (
            r["baseline"]["run"].as_str(),
            r["baseline"]["mode"].as_str()
        ),
        (Some("1"), Some("full"))
    );
    assert_eq!(
        r["baseline"]["finished_at"], "2025-10-30T08:10:00.000Z",
        "the date comes from the run"
    );
    assert_eq!(r["baseline"]["complete"], true);
    assert_eq!(r["latest"]["run"], "2");
    // n.bin, new.bin and c.txt are new; a.txt and b.txt kept their history across the rescan.
    assert_eq!(
        (r["new_files"].as_u64(), r["new_dirs"].as_u64()),
        (Some(3), Some(1))
    );
    assert_eq!(r["logical_bytes"], "12300");
    assert_eq!(r["allocated_bytes"], "12300");
    assert_eq!(r["allocation_unknown_files"], 0);
    assert_eq!(names(&r, "largest_files"), ["n.bin", "new.bin", "c.txt"]);
    assert_eq!(names(&r, "largest_folders"), ["newdir"]);
    assert_eq!(r["largest_folders"][0]["logical_bytes"], "7000");
    assert_eq!(r["partial"], false);
    assert_eq!(r["partial_reasons"], json!([]));
    let topics: Vec<_> = r["caveats"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["topic"].as_str().unwrap())
        .collect();
    assert!(
        topics.contains(&"moved_files_count_as_new")
            && topics.contains(&"recreated_files_count_as_new")
    );
    // The sealed node ids open like any other node (Explorer links).
    for list in ["largest_files", "largest_folders"] {
        let id = r[list][0]["node_id"].as_str().unwrap();
        ok(
            "node.inspect",
            call(&svc, "node.inspect", json!({"node_id": id})),
        );
    }

    // Naming the same baseline gives the same answer; the newest run as the baseline leaves nothing new,
    // and that is a count of zero, not an unknown.
    assert_eq!(since(&svc, &root, json!("1"), 10), r);
    let none = since(&svc, &root, json!("2"), 10);
    assert_eq!(
        (
            none["outcome"].as_str(),
            none["new_files"].as_u64(),
            none["new_dirs"].as_u64()
        ),
        (Some("ok"), Some(0), Some(0))
    );
    assert_eq!(none["logical_bytes"], "0");
    // The limit caps the lists, not the counts.
    let one = since(&svc, &root, Value::Null, 1);
    assert_eq!(names(&one, "largest_files"), ["n.bin"]);
    assert_eq!(one["new_files"], 3);
}

#[test]
fn a_file_moved_between_folders_counts_as_new() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(&svc, 1, &first_tree());
    // b.txt moves from sub/ to the root: a new row in a folder that had no row to pair it with.
    rescan(
        &svc,
        &root,
        &node(
            "",
            &[("a.txt", 100), ("b.txt", 200)],
            vec![node("sub", &[], vec![])],
        ),
    );
    let r = since(&svc, &root, Value::Null, 5);
    assert_eq!(names(&r, "largest_files"), ["b.txt"]);
    assert_eq!(r["new_files"], 1);
    assert!(r["caveats"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["topic"] == "moved_files_count_as_new"));
}

#[test]
fn no_earlier_completed_scan_is_no_baseline_never_zero() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(&svc, 1, &first_tree());
    let r = since(&svc, &root, Value::Null, 5);
    assert_eq!(r["outcome"], "no_baseline");
    assert!(r["baseline"].is_null());
    assert_eq!(r["latest"]["run"], "1");
    for field in [
        "new_files",
        "new_dirs",
        "logical_bytes",
        "allocated_bytes",
        "allocation_unknown_files",
    ] {
        assert!(r[field].is_null(), "{field} must be null, not zero");
    }
    assert_eq!(r["largest_files"], json!([]));
    assert_eq!(r["caveats"], json!([]));
    // Naming the only run as the baseline is a real question: nothing was born after it.
    let named = since(&svc, &root, json!("1"), 5);
    assert_eq!(
        (named["outcome"].as_str(), named["new_files"].as_u64()),
        (Some("ok"), Some(0))
    );
    // A granted root that was never scanned has no latest run either.
    let (fresh_row, _) = register(&svc, 2);
    let fresh = svc.inner.root_id(fresh_row);
    let r = since(&svc, &fresh, Value::Null, 5);
    assert_eq!(r["outcome"], "no_baseline");
    assert!(r["latest"].is_null() && r["new_files"].is_null());
}

#[test]
fn unknown_allocation_is_null_not_a_partial_sum() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(&svc, 1, &first_tree());
    let mut tree = first_tree();
    tree.files.push(("known.bin", 40, Some(4096)));
    tree.files.push(("unknown.bin", 60, None));
    rescan(&svc, &root, &tree);
    let r = since(&svc, &root, Value::Null, 5);
    assert_eq!(
        (r["new_files"].as_u64(), r["logical_bytes"].as_str()),
        (Some(2), Some("100"))
    );
    assert!(r["allocated_bytes"].is_null());
    assert_eq!(r["allocation_unknown_files"], 1);
    assert_eq!(names(&r, "largest_files"), ["unknown.bin", "known.bin"]);
    assert!(r["largest_files"][0]["allocated_bytes"].is_null());
    assert_eq!(r["largest_files"][1]["allocated_bytes"], "4096");
}

#[test]
fn an_incomplete_baseline_or_a_running_scan_makes_the_answer_partial() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(&svc, 1, &first_tree());
    rescan(&svc, &root, &second_tree());
    let clean = since(&svc, &root, Value::Null, 5);
    assert_eq!(clean["partial"], false);
    // A baseline recorded without its counters is not known to be a clean traversal.
    svc.inner
        .db()
        .execute("UPDATE main.scan_run SET counters_json='{}' WHERE id=1", [])
        .unwrap();
    let r = since(&svc, &root, Value::Null, 5);
    assert_eq!(
        (r["partial"].as_bool(), r["baseline"]["complete"].as_bool()),
        (Some(true), Some(false))
    );
    assert_eq!(r["partial_reasons"], json!(["baseline_incomplete"]));
    assert_eq!(r["new_files"], 3, "the figure is still given, as partial");
    // A failed baseline.
    svc.inner
        .db()
        .execute("UPDATE main.scan_run SET state='failed' WHERE id=1", [])
        .unwrap();
    let r = since(&svc, &root, json!("1"), 5);
    assert_eq!(r["partial_reasons"], json!(["baseline_not_completed"]));
    // A scan running now: its rows so far are in the figures.
    svc.inner
        .db()
        .execute("UPDATE main.scan_run SET state='completed' WHERE id=1", [])
        .unwrap();
    svc.inner
        .db()
        .execute(
            "UPDATE main.scan_run SET counters_json='{\"complete\":true}' WHERE id=1",
            [],
        )
        .unwrap();
    svc.inner
        .db()
        .execute(
            "INSERT INTO main.scan_run(root_id,mode,strategy,state,started_at_ns,counters_json,budget_json) VALUES(1,'full','fixture','running',3,'{}','{}')",
            [],
        )
        .unwrap();
    let r = since(&svc, &root, json!("1"), 5);
    assert_eq!(
        (r["partial"].as_bool(), r["partial_reasons"].clone()),
        (Some(true), json!(["scan_running"]))
    );
}

#[test]
fn bad_requests_unknown_roots_and_clients_are_refused() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(&svc, 1, &first_tree());
    let other = scanned(&svc, 2, &first_tree());
    rescan(&svc, &root, &second_tree());
    let ok_payload = json!({"root_id": root, "baseline_run": null, "limit": 5});
    for bad in [
        json!({"root_id": root, "baseline_run": null, "limit": 0}),
        json!({"root_id": root, "baseline_run": null, "limit": 51}),
        json!({"root_id": root, "limit": 5}),
        json!({"root_id": root, "baseline_run": 1, "limit": 5}),
        json!({"root_id": root, "baseline_run": "0", "limit": 5}),
    ] {
        refused(&svc, bad, ErrorCode::InvalidRequest);
    }
    // A run that does not exist, or that belongs to another root, is not a baseline.
    refused(
        &svc,
        json!({"root_id": root, "baseline_run": "99", "limit": 5}),
        ErrorCode::NotFound,
    );
    refused(
        &svc,
        json!({"root_id": root, "baseline_run": "2", "limit": 5, "extra": 1}),
        ErrorCode::InvalidRequest,
    );
    let other_run = svc
        .inner
        .db()
        .query_row("SELECT id FROM main.scan_run WHERE root_id=2", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
    refused(
        &svc,
        json!({"root_id": root, "baseline_run": other_run.to_string(), "limit": 5}),
        ErrorCode::NotFound,
    );
    refused(
        &svc,
        json!({"root_id": "rt_1_00000000_0000000000000000", "baseline_run": null, "limit": 5}),
        ErrorCode::NotFound,
    );
    // Only a single-root generation precondition is meaningful elsewhere; this command takes none.
    let mut req = request("changes.new_since", ok_payload.clone());
    req.expected_generation = Some(Generation(1));
    assert_eq!(
        err(svc.call(req, &CallContext::http())).code,
        ErrorCode::InvalidRequest
    );
    // Only the owner asks.
    for client in ["ask:local", "mcp:reader"] {
        let e = err(svc.call(
            request("changes.new_since", ok_payload.clone()),
            &CallContext::client(client),
        ));
        assert_eq!(e.code, ErrorCode::PermissionDenied, "{client}");
    }
    assert!(!crate::clients::open_to_clients(Command::ChangesNewSince));
    // A revoked root answers like an unknown one.
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": other, "purge_catalog": false}),
        ),
    );
    refused(
        &svc,
        json!({"root_id": other, "baseline_run": null, "limit": 5}),
        ErrorCode::NotFound,
    );
}
