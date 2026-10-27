//! LW-119 `clutter.developer`: synthetic project trees only. Candidates carry their anchor, kind and
//! recorded sizes; unknown stays unknown; a detector that runs out of budget says so; only the owner asks.
use super::*;
use loomward_catalog::{DirListing, Observation, WriteCommand, WriteReply};

#[path = "reclaim_tests.rs"]
mod reclaim_tests;

struct Node {
    name: &'static str,
    files: Vec<(&'static str, u64, Option<u64>)>,
    dirs: Vec<Node>,
}

fn dir(name: &'static str, files: &[(&'static str, u64)], dirs: Vec<Node>) -> Node {
    Node {
        name,
        files: files.iter().map(|(n, b)| (*n, *b, Some(*b))).collect(),
        dirs,
    }
}

fn list(svc: &Service, run: i64, dir_id: i64, node: &Node) {
    let w = svc.inner.catalog.writer();
    let mut dirs = vec![];
    for d in &node.dirs {
        let mut o = Observation::file(d.name, 0, Some(0));
        o.family = "none".into();
        dirs.push(o);
    }
    w.call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id,
        files: node
            .files
            .iter()
            .map(|(n, b, a)| Observation::file(*n, *b, *a))
            .collect(),
        dirs,
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }))
    .unwrap();
    for d in &node.dirs {
        // A first full scan builds in bulk until EndRun, so the child is looked up in both tables.
        let id: i64 = svc
            .inner
            .db()
            .query_row(
                "SELECT id FROM main.dir WHERE parent_id=?1 AND name=?2 UNION ALL SELECT id FROM main.bulk_dir WHERE parent_id=?1 AND name=?2",
                rusqlite::params![dir_id, d.name],
                |r| r.get(0),
            )
            .unwrap();
        list(svc, run, id, d);
    }
}

/// A synthetic root (id `file_id`) scanned once from `tree`, whose own files and dirs are the root's.
fn scanned(svc: &Service, file_id: u8, tree: &Node) -> String {
    let (root, grant) = register(svc, file_id);
    let dir_id = svc
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
    list(svc, run, dir_id, tree);
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 1,
    })
    .unwrap();
    svc.inner.root_id(root)
}

fn clutter(svc: &Service, root: &str, limit: u32) -> Value {
    json!(
        ok(
            "clutter.developer",
            call(
                svc,
                "clutter.developer",
                json!({"root_id": root, "limit": limit})
            )
        )
        .0
    )
}

fn projects() -> Node {
    dir(
        "",
        &[],
        vec![
            dir(
                "app",
                &[("Cargo.toml", 10)],
                vec![
                    dir("src", &[("main.rs", 100)], vec![]),
                    dir(
                        "target",
                        &[("a.bin", 5000)],
                        vec![dir("deep", &[("b.bin", 3000)], vec![])],
                    ),
                ],
            ),
            dir(
                "web",
                &[("package.json", 10)],
                vec![
                    dir("node_modules", &[("x.js", 1000)], vec![]),
                    dir("src", &[("index.js", 5)], vec![]),
                ],
            ),
            // Output-looking names with no manifest, and a manifest with a dataset beside it.
            dir(
                "loose",
                &[],
                vec![dir("target", &[("huge.bin", 99_000)], vec![])],
            ),
            dir(
                "plain",
                &[("Cargo.toml", 10)],
                vec![
                    dir("datasets", &[("rows.csv", 99_000)], vec![]),
                    dir("model-files", &[("w.bin", 99_000)], vec![]),
                ],
            ),
        ],
    )
}

#[test]
fn candidates_carry_anchor_kind_and_recorded_sizes_ranked_largest_first() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(&svc, 1, &projects());
    let r = clutter(&svc, &root, 50);
    assert_eq!(r["root_id"], root);
    assert_eq!(r["rules_version"], 1);
    assert_eq!(r["budget_hit"], false);
    assert_eq!(r["partial"], false);
    assert!(r["budget_note"].is_null());
    assert_eq!(r["truncated"], false);
    let rows = r["candidates"].as_array().unwrap();
    // Untracked source, datasets, model files and a manifest-less `target` are never listed.
    let names: Vec<_> = rows.iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["target", "node_modules"]);
    let (cargo, npm) = (&rows[0], &rows[1]);
    assert_eq!(cargo["anchor_name"], "Cargo.toml");
    assert_eq!(cargo["provider"], "cargo");
    assert_eq!(cargo["kind"], "build_output");
    assert_eq!(cargo["logical_bytes"], "8000");
    assert_eq!(cargo["allocated_bytes"], "8000");
    assert_eq!(cargo["coverage"], "complete");
    assert_eq!(cargo["regeneration_cost"], "unknown");
    assert_eq!(npm["anchor_name"], "package.json");
    assert_eq!(npm["kind"], "dependency_install");
    assert_eq!(npm["logical_bytes"], "1000");
    // Sealed node IDs, never the catalogue's internal spelling.
    for c in rows {
        for key in ["node_id", "anchor_node_id"] {
            let id = c[key].as_str().unwrap();
            assert!(
                id.starts_with("nd_") && !id.starts_with("nd_dir_") && !id.starts_with("nd_file_"),
                "{id}"
            );
        }
    }
    // What no provider supplies is named, not valued.
    assert_eq!(
        r["not_assessed"],
        json!([
            {
                "topic": "regeneration_cost",
                "note": "Rebuild cost and time are not measured yet (LW-087)."
            },
            {
                "topic": "model_host_lifecycle",
                "note": "Model files and model-host state are not assessed yet (LW-086)."
            },
            {
                "topic": "stale_worktrees",
                "note": "Stale worktrees need lease and dirty-state evidence, which is not collected yet (LW-076)."
            }
        ])
    );
    // The sealed anchor opens as the recorded manifest.
    let (anchor, _) = ok(
        "node.inspect",
        call(
            &svc,
            "node.inspect",
            json!({"node_id": cargo["anchor_node_id"]}),
        ),
    );
    assert_eq!(anchor["row"]["name"], "Cargo.toml");
}

#[test]
fn a_limit_truncates_and_an_unknown_allocation_stays_unknown() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let mut tree = projects();
    // An output whose allocation was not observed: logical is known, allocated is not zero but null.
    tree.dirs.push(Node {
        name: "svc",
        files: vec![("pyproject.toml", 5, Some(5))],
        dirs: vec![Node {
            name: ".venv",
            files: vec![("lib.py", 700, None)],
            dirs: vec![],
        }],
    });
    let root = scanned(&svc, 1, &tree);
    let all = clutter(&svc, &root, 50);
    assert_eq!(all["partial"], true);
    let venv = all["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == ".venv")
        .unwrap();
    assert_eq!(venv["kind"], "environment");
    assert_eq!(venv["logical_bytes"], "700");
    assert!(venv["allocated_bytes"].is_null());
    let one = clutter(&svc, &root, 1);
    assert_eq!(one["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(one["truncated"], true);
    assert_eq!(one["candidates"][0]["name"], "target");
}

#[cfg(windows)]
#[test]
fn stale_watcher_hides_sizes_and_retains_recorded_size_order_even_when_limited() {
    use loomward_windows::watch::Change;

    let Some(lab) = lab() else { return };
    std::fs::write(lab.path.join("Cargo.toml"), b"synthetic manifest").unwrap();
    std::fs::write(lab.path.join("package.json"), b"{}").unwrap();
    // Larger under the name that sorts later, so size order differs from name/id order.
    for (name, bytes) in [("target", 8000), ("node_modules", 1000)] {
        std::fs::create_dir(lab.path.join(name)).unwrap();
        std::fs::write(lab.path.join(name).join("synthetic.bin"), vec![0u8; bytes]).unwrap();
    }
    settle(&lab.svc, &start(&lab.svc, &lab.root, "full"));
    f3_wait_live(&lab.svc);
    lab.svc.inner.freshness.pause_pump();
    let before = clutter(&lab.svc, &lab.root, 50);
    assert_eq!(before["partial"], false);
    assert_eq!(before["candidates"][0]["name"], "target");
    assert_eq!(before["candidates"][0]["logical_bytes"], "8000");
    assert_eq!(before["candidates"][1]["logical_bytes"], "1000");
    let row = lab.svc.inner.root_row(&lab.root).unwrap();
    lab.svc.inner.freshness.record(row, vec![Change::RootDirty]);
    assert!(lab.svc.inner.freshness.stale(row));

    let stale = clutter(&lab.svc, &lab.root, 50);
    assert_eq!(stale["partial"], true);
    assert_eq!(stale["budget_hit"], false);
    let candidates = stale["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    for (i, c) in candidates.iter().enumerate() {
        assert_eq!(c["node_id"], before["candidates"][i]["node_id"]);
        assert_eq!(c["coverage"], "stale");
        assert!(c["logical_bytes"].is_null());
        assert!(c["allocated_bytes"].is_null());
    }
    let limited = clutter(&lab.svc, &lab.root, 1);
    assert_eq!(limited["partial"], true);
    assert_eq!(limited["truncated"], true);
    assert_eq!(limited["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(limited["candidates"][0], candidates[0]);
}

#[test]
fn cross_provider_candidates_never_supply_a_grand_total() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(&svc, 1, &projects());
    let report = clutter(&svc, &root, 50);
    let keys: Vec<_> = report
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "budget_hit",
            "budget_note",
            "candidates",
            "generation",
            "not_assessed",
            "partial",
            "root_id",
            "rules_version",
            "truncated",
        ]
    );
    let rows = report["candidates"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0]["provider"], rows[1]["provider"]);
    for row in rows {
        let sizes: Vec<_> = row
            .as_object()
            .unwrap()
            .keys()
            .filter(|key| key.contains("bytes") || key.contains("total"))
            .map(String::as_str)
            .collect();
        assert_eq!(sizes, ["allocated_bytes", "logical_bytes"]);
    }
    // A combined size cannot silently become part of the wire contract either.
    serde_json::from_value::<DeveloperClutterReport>(report.clone()).unwrap();
    for key in [
        "total_bytes",
        "total_logical_bytes",
        "total_allocated_bytes",
        "grand_total",
    ] {
        let mut with_total = report.clone();
        with_total[key] = json!("9000");
        assert!(serde_json::from_value::<DeveloperClutterReport>(with_total).is_err());
    }
}

#[test]
fn an_unscanned_root_lists_nothing_and_says_it_is_partial() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, _) = register(&svc, 1);
    let r = clutter(&svc, &svc.inner.root_id(root), 10);
    assert_eq!(r["candidates"].as_array().unwrap().len(), 0);
    assert_eq!(r["partial"], true);
    assert_eq!(r["budget_hit"], false);
}

/// A directory of 250,000 files that also holds an output name costs more than the detector's step
/// budget (about 12 steps per file against 2,000,000): the answer is `budget_hit`, not an empty list
/// that reads as "nothing found".
#[test]
fn a_detector_that_runs_out_of_budget_says_so_instead_of_returning_an_empty_list() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(
        &svc,
        1,
        &dir(
            "",
            &[("Cargo.toml", 1)],
            vec![dir("target", &[("a.bin", 1)], vec![])],
        ),
    );
    let root_dir: i64 = svc
        .inner
        .db()
        .query_row("SELECT id FROM main.dir WHERE parent_id IS NULL", [], |r| {
            r.get(0)
        })
        .unwrap();
    svc.inner
        .db()
        .execute(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<250000)
             INSERT INTO main.file(dir_id,name,logical,attrs,flags,born_run,seen_run) SELECT ?1,'f'||i||'.dat',1,32,0,1,1 FROM n",
            [root_dir],
        )
        .unwrap();
    let r = clutter(&svc, &root, 50);
    assert_eq!(r["budget_hit"], true);
    assert_eq!(r["candidates"].as_array().unwrap().len(), 0);
    assert_eq!(r["partial"], true);
    assert!(r["budget_note"].as_str().unwrap().contains("says nothing"));
}

#[test]
fn only_the_owner_asks_and_an_unknown_or_revoked_root_is_not_found() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(&svc, 1, &projects());
    for client in ["ask:local", "mcp:reader"] {
        let e = err(svc.call(
            request("clutter.developer", json!({"root_id": root, "limit": 5})),
            &CallContext::client(client),
        ));
        assert_eq!(e.code, ErrorCode::PermissionDenied, "{client}");
    }
    let e = err(call(
        &svc,
        "clutter.developer",
        json!({"root_id": "rt_AAAAAAAAAAAAAAAA", "limit": 5}),
    ));
    assert_eq!(e.code, ErrorCode::NotFound);
    for bad in [0, 201] {
        let e = err(call(
            &svc,
            "clutter.developer",
            json!({"root_id": root, "limit": bad}),
        ));
        assert_eq!(e.code, ErrorCode::InvalidRequest, "limit {bad}");
    }
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": root, "purge_catalog": false}),
        ),
    );
    let e = err(call(
        &svc,
        "clutter.developer",
        json!({"root_id": root, "limit": 5}),
    ));
    assert_eq!(e.code, ErrorCode::NotFound);
}
