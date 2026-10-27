//! Synthetic catalogue fixtures only; no live cleanup or link-count observations.
use super::*;

fn fixture(svc: &Service) -> String {
    scanned(
        svc,
        1,
        &dir(
            "",
            &[("Cargo.toml", 10)],
            vec![
                dir("target", &[("output.bin", 8000)], vec![]),
                dir(
                    ".cargo",
                    &[],
                    vec![dir(
                        "registry",
                        &[],
                        vec![dir("cache", &[("cache.bin", 2000)], vec![])],
                    )],
                ),
            ],
        ),
    )
}

fn reclaim(svc: &Service) -> Value {
    json!(
        ok(
            "reclaim.plan",
            call(
                svc,
                "reclaim.plan",
                json!({"volume_key": "fixture", "limit": 50})
            )
        )
        .0
    )
}

#[test]
fn reclaim_composes_four_groups_with_unknown_relief_and_sealed_references() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = fixture(&svc);
    let plan = reclaim(&svc);
    assert_eq!(plan["volume"]["volume_key"], "fixture");
    assert!(plan["volume"]["reserve_bytes"].is_null());
    assert_eq!(plan["covered_roots"][0]["root_id"], root);
    assert_eq!(plan["covered_roots"][0]["allocated_bytes"], "10010");
    assert_eq!(
        plan["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "tool_owned_cleanup",
            "relocate",
            "archive_candidates",
            "informational"
        ]
    );
    let cleanup = plan["groups"][0]["candidates"].as_array().unwrap();
    let cargo = cleanup.iter().find(|c| c["name"] == "target").unwrap();
    assert_eq!(cargo["logical_bytes"], "8000");
    assert_eq!(cargo["allocated_bytes"], "8000");
    assert!(cargo["reclaimable_bytes"].is_null());
    assert_eq!(
        cargo["reclaimable_reason"],
        loomward_engine::reclaim::RELIEF_UNOBSERVED
    );
    assert!(plan["groups"][1]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["class"] == "package_cache" && c["provider"] == "cargo"));
    for c in cleanup {
        let id = c["node_id"].as_str().unwrap();
        assert!(id.starts_with("nd_") && !id.starts_with("nd_dir_"));
        ok(
            "node.inspect",
            call(&svc, "node.inspect", json!({"node_id": id})),
        );
    }
    assert_eq!(reclaim(&svc), plan, "same catalogue, same plan");
}

#[test]
fn reclaim_counts_only_outermost_active_roots_and_keeps_adjacent_roots() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let outer = fixture(&svc);
    let nested = scanned(&svc, 2, &dir("", &[("nested.bin", 30)], vec![]));
    let sibling = scanned(&svc, 3, &dir("", &[("sibling.bin", 50)], vec![]));
    for (root, path) in [
        (&outer, "Synthetic lab/outer/"),
        (&nested, "synthetic LAB\\outer\\nested"),
        (&sibling, "Synthetic lab\\outer-sibling"),
    ] {
        svc.inner.db().execute("UPDATE st.root_grant SET display_path=?2 WHERE id=(SELECT grant_id FROM main.root WHERE id=?1)", rusqlite::params![svc.inner.root_row(root).unwrap(), path]).unwrap();
    }
    svc.inner
        .db()
        .execute(
            "UPDATE main.volume SET capabilities_json=?1",
            [json!({"space":{"capacity_bytes":"200000", "free_bytes":"100000"}}).to_string()],
        )
        .unwrap();
    let plan = reclaim(&svc);
    let roots = plan["covered_roots"].as_array().unwrap();
    assert_eq!(
        roots
            .iter()
            .map(|r| r["root_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [outer.as_str(), sibling.as_str()]
    );
    assert_eq!(plan["unaccounted_bytes"], "89940");
    assert_eq!(plan["volume"]["capacity_bytes"], "200000");
    assert_eq!(plan["volume"]["free_bytes"], "100000");
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id":outer,"purge_catalog":false}),
        ),
    );
    let plan = reclaim(&svc);
    let roots = plan["covered_roots"].as_array().unwrap();
    assert_eq!(
        roots
            .iter()
            .map(|r| r["root_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [nested.as_str(), sibling.as_str()]
    );
    assert_eq!(plan["unaccounted_bytes"], "99920");
}

#[test]
fn reclaim_exhausted_detector_budget_is_explicit_and_partial() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = fixture(&svc);
    let row = svc.inner.root_row(&root).unwrap();
    let conn = svc.inner.db();
    let root_dir = crate::db::active_root_dir(&conn, row).unwrap().unwrap();
    conn.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<250000)
        INSERT INTO main.file(dir_id,name,logical,attrs,flags,born_run,seen_run)
        SELECT ?1,'synthetic-'||i||'.dat',1,32,0,1,1 FROM n",
        [root_dir],
    )
    .unwrap();
    drop(conn);
    let plan = reclaim(&svc);
    assert_eq!(plan["budget_hit"], true);
    assert_eq!(plan["partial"], true);
}

#[cfg(windows)]
#[test]
fn reclaim_watcher_staleness_hides_all_candidate_sizes() {
    let Some(lab) = lab() else { return };
    std::fs::write(lab.path.join("Cargo.toml"), b"synthetic manifest").unwrap();
    std::fs::create_dir(lab.path.join("target")).unwrap();
    std::fs::write(
        lab.path.join("target/output.bin"),
        b"synthetic build output",
    )
    .unwrap();
    settle(&lab.svc, &start(&lab.svc, &lab.root, "full"));
    f3_wait_live(&lab.svc);
    lab.svc.inner.freshness.pause_pump();
    let volume = lab
        .svc
        .inner
        .db()
        .query_row("SELECT volume_key FROM main.volume LIMIT 1", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap();
    let read = || {
        json!(
            ok(
                "reclaim.plan",
                call(
                    &lab.svc,
                    "reclaim.plan",
                    json!({"volume_key":volume,"limit":50})
                )
            )
            .0
        )
    };
    let before = read();
    assert!(before["groups"][0]["candidates"][0]["allocated_bytes"].is_string());
    let row = lab.svc.inner.root_row(&lab.root).unwrap();
    lab.svc
        .inner
        .freshness
        .record(row, vec![loomward_windows::watch::Change::RootDirty]);
    assert!(lab.svc.inner.freshness.stale(row));
    let stale = read();
    assert_eq!(stale["partial"], true);
    assert!(stale["covered_roots"][0]["allocated_bytes"].is_null());
    let candidate = &stale["groups"][0]["candidates"][0];
    assert_eq!(candidate["coverage"], "stale");
    for field in ["logical_bytes", "allocated_bytes", "reclaimable_bytes"] {
        assert!(candidate[field].is_null(), "{field}: {candidate}");
    }
}

#[test]
fn reclaim_rejects_unknown_volumes_bad_limits_and_clients_even_with_a_grant() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = fixture(&svc);
    assert_eq!(
        err(call(
            &svc,
            "reclaim.plan",
            json!({"volume_key": "unknown", "limit": 5})
        ))
        .code,
        ErrorCode::NotFound
    );
    for limit in [0, 201] {
        assert_eq!(
            err(call(
                &svc,
                "reclaim.plan",
                json!({"volume_key": "fixture", "limit": limit})
            ))
            .code,
            ErrorCode::InvalidRequest
        );
    }
    svc.grant_client(
        "mcp:reader",
        &[RootId::new(root).unwrap()],
        &["size", "volumes"],
        None,
    )
    .unwrap();
    for client in ["ask:local", "mcp:reader"] {
        assert_eq!(
            err(svc.call(
                request("reclaim.plan", json!({"volume_key": "fixture", "limit": 5})),
                &CallContext::client(client)
            ))
            .code,
            ErrorCode::PermissionDenied
        );
    }
}

fn row_checksums(db: &rusqlite::Connection) -> Vec<(usize, u64)> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    ["dir", "file"]
        .into_iter()
        .map(|table| {
            let mut checksum = DefaultHasher::new();
            let mut stmt = db
                .prepare(&format!("SELECT * FROM {table} ORDER BY id"))
                .unwrap();
            let columns = stmt.column_count();
            let mut rows = stmt.query([]).unwrap();
            let mut count = 0;
            while let Some(row) = rows.next().unwrap() {
                count += 1;
                for col in 0..columns {
                    format!("{:?}", row.get_ref(col).unwrap()).hash(&mut checksum);
                }
            }
            (count, checksum.finish())
        })
        .collect()
}

#[test]
fn reclaim_dirty_root_is_partial_without_repairing_catalogue_or_state() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = fixture(&svc);
    let row = svc.inner.root_row(&root).unwrap();
    svc.inner
        .db()
        .execute(
            "UPDATE dir SET dirty_rev=agg_valid_rev+1 WHERE root_id=?1 AND parent_id IS NULL",
            [row],
        )
        .unwrap();
    let before = row_checksums(&svc.inner.db());
    let state = std::fs::read(tmp.path().join("state.db")).unwrap();
    let plan = reclaim(&svc);
    assert_eq!(plan["partial"], true);
    assert_eq!(plan["covered_roots"][0]["coverage"], "stale");
    assert!(plan["covered_roots"][0]["allocated_bytes"].is_null());
    assert!(plan["unaccounted_bytes"].is_null());
    for c in plan["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["candidates"].as_array().unwrap())
    {
        assert_eq!(c["coverage"], "stale");
        for field in ["logical_bytes", "allocated_bytes", "reclaimable_bytes"] {
            assert!(c[field].is_null(), "{field}: {c}");
        }
        assert!(c["reclaimable_reason"].is_string());
    }
    assert_eq!(row_checksums(&svc.inner.db()), before);
    assert_eq!(std::fs::read(tmp.path().join("state.db")).unwrap(), state);
}

#[test]
fn reclaim_refuses_a_volume_with_no_active_grant_instead_of_an_empty_plan() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = fixture(&svc);
    assert!(reclaim(&svc)["covered_roots"]
        .as_array()
        .is_some_and(|r| !r.is_empty()));
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": root, "purge_catalog": false}),
        ),
    );
    let refused = err(call(
        &svc,
        "reclaim.plan",
        json!({"volume_key": "fixture", "limit": 5}),
    ));
    assert_eq!(refused.code, ErrorCode::InvalidRequest);
    assert_eq!(refused.message.as_str(), "no_active_grant");
}
