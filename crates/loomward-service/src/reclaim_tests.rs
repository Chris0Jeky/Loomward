//! Synthetic catalogue fixtures and, for `reclaim.observe` (#939), synthetic lab trees whose
//! names the metadata-only link-count pass opens. No cleanup anywhere.
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

fn volume_id(svc: &Service) -> Value {
    let listed = json!(ok("volumes.list", call(svc, "volumes.list", json!({}))).0);
    listed["volumes"][0]["volume_id"].clone()
}

fn reclaim(svc: &Service) -> Value {
    json!(
        ok(
            "reclaim.plan",
            call(
                svc,
                "reclaim.plan",
                json!({"volume_id": volume_id(svc), "limit": 50})
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
    assert_eq!(plan["volume"]["volume_id"], volume_id(&svc));
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
fn reclaim_unscanned_covered_root_is_partial_with_unknown_sizes_and_no_candidates() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let scanned = fixture(&svc);
    // Known volume space makes the unknown unaccounted total depend on root coverage.
    svc.inner
        .db()
        .execute(
            "UPDATE main.volume SET capabilities_json=?1",
            [json!({"space":{"capacity_bytes":"200000", "free_bytes":"100000"}}).to_string()],
        )
        .unwrap();
    let before = reclaim(&svc);
    assert_eq!(before["unaccounted_bytes"], "89990");
    let (unscanned, _) = register(&svc, 2);
    let unscanned = svc.inner.root_id(unscanned);
    let plan = reclaim(&svc);
    let roots = plan["covered_roots"].as_array().unwrap();
    assert_eq!(roots.len(), 2);
    let root = roots.iter().find(|r| r["root_id"] == unscanned).unwrap();
    assert_eq!(root["coverage"], "unscanned");
    assert!(root["allocated_bytes"].is_null());
    assert!(plan["unaccounted_bytes"].is_null());
    assert_eq!(plan["partial"], true);
    assert_eq!(plan["budget_hit"], false);
    assert_eq!(before["groups"], plan["groups"]);
    assert!(roots.iter().any(|r| r["root_id"] == scanned));
    assert!(plan["groups"][0]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["name"] == "target"));
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
    let volume = volume_id(&lab.svc);
    let read = || {
        json!(
            ok(
                "reclaim.plan",
                call(
                    &lab.svc,
                    "reclaim.plan",
                    json!({"volume_id":volume,"limit":50})
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
    let unknown = err(call(
        &svc,
        "reclaim.plan",
        json!({"volume_id": "vo_unknown", "limit": 5}),
    ));
    assert_eq!(unknown.code, ErrorCode::NotFound);
    assert_eq!(unknown.message.as_str(), "unknown volume");
    let foreign_tmp = temp_root();
    let foreign = open(foreign_tmp.path(), DatasetClass::Synthetic);
    fixture(&foreign);
    let foreign_id = volume_id(&foreign);
    let refused = err(call(
        &svc,
        "reclaim.plan",
        json!({"volume_id": foreign_id, "limit": 5}),
    ));
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(reason(&refused), Some("catalog_instance_changed"));
    assert_eq!(
        refused,
        svc.inner
            .volume_row(foreign_id.as_str().unwrap())
            .unwrap_err()
    );
    for limit in [0, 201] {
        assert_eq!(
            err(call(
                &svc,
                "reclaim.plan",
                json!({"volume_id": volume_id(&svc), "limit": limit})
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
                request(
                    "reclaim.plan",
                    json!({"volume_id": volume_id(&svc), "limit": 5})
                ),
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
        json!({"volume_id": volume_id(&svc), "limit": 5}),
    ));
    assert_eq!(refused.code, ErrorCode::InvalidRequest);
    assert_eq!(refused.message.as_str(), "no_active_grant");
}

#[test]
fn reclaim_limit_applies_per_group_after_grouping() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    // A virtual disk (informational) larger than a package cache (relocate): one machine-store
    // detector capped at the request's limit would keep only the disk.
    scanned(
        &svc,
        1,
        &dir(
            "",
            &[("ext4.vhdx", 9000)],
            vec![dir(
                ".cargo",
                &[],
                vec![dir(
                    "registry",
                    &[],
                    vec![dir("cache", &[("cache.bin", 2000)], vec![])],
                )],
            )],
        ),
    );
    let plan = json!(
        ok(
            "reclaim.plan",
            call(
                &svc,
                "reclaim.plan",
                json!({"volume_id": volume_id(&svc), "limit": 1})
            )
        )
        .0
    );
    let filled: Vec<_> = plan["groups"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|g| !g["candidates"].as_array().unwrap().is_empty())
        .map(|g| g["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(filled.contains(&"relocate".to_owned()), "{plan}");
    assert!(filled.contains(&"informational".to_owned()), "{plan}");
}

#[cfg(windows)]
fn observe_job(svc: &Service) -> String {
    let (job, _) = ok(
        "reclaim.observe",
        call(svc, "reclaim.observe", json!({"volume_id": volume_id(svc)})),
    );
    assert_eq!(job["job"]["kind"], "reclaim");
    assert!(job["job"]["root_id"].is_null());
    job["job"]["job_id"].as_str().unwrap().to_owned()
}

fn relief_rows(svc: &Service) -> i64 {
    svc.inner
        .db()
        .query_row("SELECT count(*) FROM st.reclaim_relief", [], |r| r.get(0))
        .unwrap()
}

/// Every `state.db` table except the observation's own, so a job proves it wrote nothing else.
#[cfg(windows)]
fn state_checksums(db: &rusqlite::Connection) -> Vec<(String, u64)> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let tables: Vec<String> = db
        .prepare("SELECT name FROM st.sqlite_master WHERE type='table' AND name<>'reclaim_relief' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    tables
        .into_iter()
        .map(|table| {
            let mut checksum = DefaultHasher::new();
            let mut stmt = db
                .prepare(&format!("SELECT * FROM st.\"{table}\""))
                .unwrap();
            let columns = stmt.column_count();
            let mut rows = stmt.query([]).unwrap();
            let mut all = Vec::new();
            while let Some(row) = rows.next().unwrap() {
                all.push(
                    (0..columns)
                        .map(|c| format!("{:?}", row.get_ref(c).unwrap()))
                        .collect::<Vec<_>>(),
                );
            }
            all.sort();
            all.hash(&mut checksum);
            (table, checksum.finish())
        })
        .collect()
}

/// A Cargo project in the lab: `target` is the candidate. Sizes are whole clusters, so the
/// listed and the handle allocation agree.
#[cfg(windows)]
fn cargo_lab(extra: impl FnOnce(&std::path::Path)) -> Option<Lab> {
    let lab = lab()?;
    std::fs::write(lab.path.join("Cargo.toml"), b"synthetic manifest").unwrap();
    let target = lab.path.join("target");
    std::fs::create_dir_all(target.join("debug")).unwrap();
    std::fs::write(target.join("one.bin"), vec![1u8; 8192]).unwrap();
    std::fs::write(target.join("two.bin"), vec![2u8; 12288]).unwrap();
    extra(&target);
    settle(&lab.svc, &start(&lab.svc, &lab.root, "full"));
    f3_wait_live(&lab.svc);
    // Later changes reach the catalogue only through an explicit rescan, and never mark it stale.
    lab.svc.inner.freshness.stop();
    Some(lab)
}

#[cfg(windows)]
fn target_candidate(svc: &Service) -> Value {
    reclaim(svc)["groups"][0]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "target")
        .unwrap()
        .clone()
}

#[cfg(windows)]
#[test]
fn reclaim_selection_closes_cross_candidate_links() {
    let Some(lab) = cargo_lab(|target| {
        let cache = target.parent().unwrap().join(".cargo/registry/cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::hard_link(target.join("one.bin"), cache.join("shared.bin")).unwrap();
    }) else {
        return;
    };
    settle(&lab.svc, &observe_job(&lab.svc));
    let plan = reclaim(&lab.svc);
    let candidates: Vec<_> = plan["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["candidates"].as_array().unwrap())
        .filter(|c| c["name"] == "target" || c["class"] == "package_cache")
        .collect();
    assert_eq!(candidates.len(), 2);
    assert!(candidates.iter().all(|c| c["reclaimable_bytes"].is_null()));
    let selection: Vec<_> = candidates.iter().map(|c| c["node_id"].clone()).collect();
    let catalogue = row_checksums(&lab.svc.inner.db());
    let catalog_path = loomward_catalog::sqlite_dir(&lab.svc.inner.state_dir).join("catalog.db");
    let catalogue_bytes = std::fs::read(&catalog_path).unwrap();
    let job = ok(
        "reclaim.observe",
        call(
            &lab.svc,
            "reclaim.observe",
            json!({"volume_id": volume_id(&lab.svc), "selection": selection}),
        ),
    )
    .0;
    let done = settle(&lab.svc, job["job"]["job_id"].as_str().unwrap());
    assert_eq!(done["coverage"], "complete", "{done:?}");
    assert_eq!(done["reclaim"]["names_checked"], 3);
    let result = ok(
        "reclaim.plan",
        call(
            &lab.svc,
            "reclaim.plan",
            json!({"volume_id": volume_id(&lab.svc), "limit": 50, "selection": selection}),
        ),
    )
    .0;
    assert_eq!(
        result["selection"]["reclaimable_bytes"], "20480",
        "{result:?}"
    );
    assert!(result["selection"]["reason"].is_null());
    assert!(result["selection"]["observed_at"].is_string());
    assert_eq!(row_checksums(&lab.svc.inner.db()), catalogue);
    assert_eq!(std::fs::read(catalog_path).unwrap(), catalogue_bytes);
}

#[cfg(windows)]
fn selection_ids(svc: &Service) -> Vec<Value> {
    reclaim(svc)["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["candidates"].as_array().unwrap())
        .filter(|c| !c["node_id"].is_null())
        .map(|c| c["node_id"].clone())
        .collect()
}

#[cfg(windows)]
fn selection_job(svc: &Service, selection: &[Value]) -> String {
    ok(
        "reclaim.observe",
        call(
            svc,
            "reclaim.observe",
            json!({"volume_id":volume_id(svc), "selection": selection}),
        ),
    )
    .0["job"]["job_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[cfg(windows)]
fn selected_result(svc: &Service, selection: &[Value]) -> Value {
    ok(
        "reclaim.plan",
        call(
            svc,
            "reclaim.plan",
            json!({"volume_id":volume_id(svc), "limit":50, "selection": selection}),
        ),
    )
    .0["selection"]
        .clone()
}

#[test]
fn reclaim_selection_rejects_duplicates_non_candidates_wrong_volumes_and_ancestry() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = fixture(&svc);
    let plan = reclaim(&svc);
    let candidate = plan["groups"][0]["candidates"][0]["node_id"].clone();
    let request = |selection: Value| {
        call(
            &svc,
            "reclaim.plan",
            json!({"volume_id": volume_id(&svc), "limit":50, "selection":selection}),
        )
    };
    assert_eq!(
        err(request(json!([candidate, candidate]))).code,
        ErrorCode::InvalidRequest
    );
    let root_dir = crate::db::active_root_dir(&svc.inner.db(), svc.inner.root_row(&root).unwrap())
        .unwrap()
        .unwrap();
    let mut not_candidate = json!({"node_id":format!("nd_dir_{root_dir}")});
    svc.inner.seal(&mut not_candidate).unwrap();
    assert_eq!(
        err(request(json!([not_candidate["node_id"]]))).code,
        ErrorCode::InvalidRequest
    );
    let nested = scanned(
        &svc,
        2,
        &dir(
            "",
            &[],
            vec![dir(
                "linked",
                &[(".git", 12), ("Cargo.toml", 8)],
                vec![dir("target", &[("output.bin", 20)], vec![])],
            )],
        ),
    );
    let nested_row = svc.inner.root_row(&nested).unwrap();
    let nested_plan = reclaim(&svc);
    let ids: Vec<_> = nested_plan["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["candidates"].as_array().unwrap())
        .filter(|c| {
            c["name"] == "linked" || (c["name"] == "target" && c["allocated_bytes"] == "20")
        })
        .map(|c| c["node_id"].clone())
        .collect();
    assert_eq!(ids.len(), 2, "{nested_plan}");
    assert!(err(request(json!(ids)))
        .message
        .as_str()
        .contains("descendant"));
    // A genuine second volume, with the selected node left on the first.
    svc.inner.db().execute("INSERT INTO main.volume(volume_key,display_name,filesystem,identity_json,capabilities_json,device_json,online,observed_at_ns) VALUES('other','Other','NTFS','{}','{}','{}',1,0)", []).unwrap();
    let other: i64 = svc
        .inner
        .db()
        .query_row(
            "SELECT id FROM main.volume WHERE volume_key='other'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    svc.inner
        .db()
        .execute(
            "UPDATE main.root SET volume_id=?1 WHERE id=?2",
            rusqlite::params![other, nested_row],
        )
        .unwrap();
    let wrong = err(call(
        &svc,
        "reclaim.plan",
        json!({"volume_id":svc.inner.volume_id(other), "limit":50, "selection":[candidate]}),
    ));
    assert_eq!(wrong.code, ErrorCode::InvalidRequest);
}

/// #1003: more than 200 candidates in one group, so the 200-wide membership view and the displayed
/// plan rank differently once stored relief promotes the smallest candidate.
#[test]
fn reclaim_selection_accepts_a_displayed_candidate_promoted_past_the_detector_cap_by_stored_relief()
{
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let projects = |first: u8, low: bool| {
        (0..120)
            .map(|i| {
                let name: &'static str = if low && i == 0 {
                    "lowrelief"
                } else {
                    Box::leak(format!("p{first}-{i}").into_boxed_str())
                };
                let size = if low && i == 0 { 1000 } else { 5000 + i };
                dir(
                    name,
                    &[("Cargo.toml", 10)],
                    vec![dir("target", &[("out.bin", size)], vec![])],
                )
            })
            .collect::<Vec<_>>()
    };
    scanned(&svc, 1, &dir("", &[], projects(1, false)));
    scanned(&svc, 2, &dir("", &[], projects(2, true)));
    let wide = |selection: Option<Value>| {
        let mut request = json!({"volume_id": volume_id(&svc), "limit": 200});
        if let Some(selection) = selection {
            request["selection"] = selection;
        }
        ok("reclaim.plan", call(&svc, "reclaim.plan", request)).0
    };
    let before = json!(wide(None));
    let cleanup = &before["groups"][0];
    assert_eq!(cleanup["kind"], "tool_owned_cleanup");
    assert_eq!(cleanup["candidates"].as_array().unwrap().len(), 200);
    assert_eq!(before["truncated"], true);
    let (low, grant): (i64, i64) = svc
        .inner
        .db()
        .query_row(
            "SELECT d.id,r.grant_id FROM main.dir d JOIN main.dir p ON p.id=d.parent_id JOIN main.root r ON r.id=d.root_id WHERE p.name='lowrelief' AND d.name='target'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let mut low_id = json!({"node_id": format!("nd_dir_{low}")});
    svc.inner.seal(&mut low_id).unwrap();
    let low_id = low_id["node_id"].clone();
    assert!(
        !cleanup["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["node_id"] == low_id),
        "the smallest candidate is outside the top 200 before relief"
    );
    svc.inner
        .db()
        .execute(
            "INSERT INTO st.reclaim_relief(grant_id,node_kind,node_id,volume_id,generation,incarnation,subtree_rev,catalog_instance,observed_at_ns,unique_allocated_bytes,reason)
             SELECT ?1,'dir',d.id,r.volume_id,r.generation,d.born_run,d.subtree_rev,?3,0,1000,NULL FROM main.dir d JOIN main.root r ON r.id=d.root_id WHERE d.id=?2",
            rusqlite::params![grant, low, svc.inner.instance],
        )
        .unwrap();
    let shown = reclaim(&svc);
    assert_eq!(shown["groups"][0]["candidates"][0]["node_id"], low_id);
    assert_eq!(
        shown["groups"][0]["candidates"][0]["reclaimable_bytes"],
        "1000"
    );
    let selected = json!(wide(Some(json!([low_id]))));
    assert_eq!(selected["selection"]["members"], json!([low_id]));
    assert_eq!(selected["selection"]["status"], "unobserved");
}

#[cfg(windows)]
#[test]
fn reclaim_selection_external_links_and_identity_mismatch_stay_unknown() {
    for mismatch in [false, true] {
        let Some(lab) = cargo_lab(|target| {
            if !mismatch {
                std::fs::hard_link(
                    target.join("one.bin"),
                    target.parent().unwrap().join("outside.bin"),
                )
                .unwrap();
            }
        }) else {
            return;
        };
        if mismatch {
            let file = lab.path.join("target/two.bin");
            std::fs::remove_file(&file).unwrap();
            std::fs::write(file, vec![3u8; 12288]).unwrap();
        }
        let selection = selection_ids(&lab.svc);
        settle(&lab.svc, &selection_job(&lab.svc, &selection));
        let result = selected_result(&lab.svc, &selection);
        assert_eq!(result["status"], "recorded", "{result}");
        assert!(result["reclaimable_bytes"].is_null());
        assert!(result["reason"].as_str().unwrap().contains("hard-link"));
    }
}

#[cfg(windows)]
#[test]
fn reclaim_selection_cancel_and_global_budgets_replace_previous_estimates_with_unknown() {
    let Some(lab) = cargo_lab(|target| {
        let cache = target.parent().unwrap().join(".cargo/registry/cache");
        std::fs::create_dir_all(&cache).unwrap();
        for n in 0..4 {
            std::fs::write(cache.join(format!("{n}.bin")), vec![4u8; 4096]).unwrap();
        }
    }) else {
        return;
    };
    let selection = selection_ids(&lab.svc);
    assert_eq!(selection.len(), 2);
    settle(&lab.svc, &selection_job(&lab.svc, &selection));
    assert!(selected_result(&lab.svc, &selection)["reclaimable_bytes"].is_string());
    let hooks = &lab.svc.inner.hooks;
    let gate = (Command::ReclaimObserve, "candidate");
    hooks.closed.lock().unwrap().push(gate);
    let id = selection_job(&lab.svc, &selection);
    hooks.until("selection reached candidate", |h| {
        h.arrived.lock().unwrap().contains(&gate)
    });
    ok(
        "jobs.cancel",
        call(&lab.svc, "jobs.cancel", json!({"job_id":id})),
    );
    hooks.open(gate.0, gate.1);
    assert_eq!(settle(&lab.svc, &id)["state"], "cancelled");
    let cancelled = selected_result(&lab.svc, &selection);
    assert!(cancelled["reclaimable_bytes"].is_null(), "{cancelled}");
    assert!(cancelled["reason"].as_str().unwrap().contains("cancelled"));
    *hooks.reclaim_budget.lock().unwrap() = Some(Duration::ZERO);
    assert_eq!(
        settle(&lab.svc, &selection_job(&lab.svc, &selection))["coverage"],
        "partial"
    );
    assert!(selected_result(&lab.svc, &selection)["reason"]
        .as_str()
        .unwrap()
        .contains("time budget"));
    *hooks.reclaim_budget.lock().unwrap() = None;
    hooks
        .reclaim_selection_limit
        .store(5, std::sync::atomic::Ordering::Release);
    hooks
        .reclaim_names_opened
        .store(0, std::sync::atomic::Ordering::Release);
    let done = settle(&lab.svc, &selection_job(&lab.svc, &selection));
    assert_eq!(done["coverage"], "partial");
    assert!(names_opened(&lab.svc) <= 5);
    let limited = selected_result(&lab.svc, &selection);
    assert!(limited["reclaimable_bytes"].is_null());
    assert!(
        limited["reason"].as_str().unwrap().contains("budget"),
        "{limited}"
    );
}

#[cfg(windows)]
#[test]
fn reclaim_selection_directory_budget_and_placeholder_are_unknown() {
    let Some(lab) = cargo_lab(|target| {
        for n in 0..6 {
            std::fs::create_dir(target.join(format!("empty-{n}"))).unwrap();
        }
    }) else {
        return;
    };
    let selection = selection_ids(&lab.svc);
    lab.svc
        .inner
        .hooks
        .reclaim_selection_limit
        .store(5, std::sync::atomic::Ordering::Release);
    let done = settle(&lab.svc, &selection_job(&lab.svc, &selection));
    assert_eq!(done["coverage"], "partial");
    assert_eq!(names_opened(&lab.svc), 0);
    assert!(selected_result(&lab.svc, &selection)["reason"]
        .as_str()
        .unwrap()
        .contains("directory budget"));
    lab.svc
        .inner
        .hooks
        .reclaim_selection_limit
        .store(0, std::sync::atomic::Ordering::Release);
    lab.svc
        .inner
        .db()
        .execute(
            "UPDATE main.file SET attrs=attrs | 4096 WHERE name='one.bin'",
            [],
        )
        .unwrap();
    settle(&lab.svc, &selection_job(&lab.svc, &selection));
    assert!(selected_result(&lab.svc, &selection)["reclaimable_bytes"].is_null());
}

#[cfg(windows)]
#[test]
fn reclaim_selection_shared_ancestors_cannot_hide_directory_budget_overflow() {
    let Some(lab) = cargo_lab(|target| {
        let root = target.parent().unwrap();
        let cache = root.join(".cargo/registry/cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("first.bin"), vec![1u8; 12288]).unwrap();
        for name in ["a", "b"] {
            let folder = root.join(".cargo/git/db").join(name);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("file.bin"), vec![2u8; 4096]).unwrap();
        }
    }) else {
        return;
    };
    let selection: Vec<_> = reclaim(&lab.svc)["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["candidates"].as_array().unwrap())
        .filter(|c| c["class"] == "package_cache")
        .map(|c| c["node_id"].clone())
        .collect();
    assert_eq!(selection.len(), 2);
    let hidden = lab.path.join(".cargo/git/db/b");
    let moved = lab.path.join("moved-b");
    std::fs::rename(&hidden, &moved).unwrap();
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&hidden)
        .arg(&moved)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    lab.svc
        .inner
        .hooks
        .reclaim_selection_limit
        .store(6, std::sync::atomic::Ordering::Release);
    let done = settle(&lab.svc, &selection_job(&lab.svc, &selection));
    let result = selected_result(&lab.svc, &selection);
    std::fs::remove_dir(&hidden).unwrap();
    assert_eq!(done["coverage"], "partial", "{done:?}");
    assert!(result["reclaimable_bytes"].is_null(), "{result}");
    assert!(
        result["reason"].as_str().unwrap().contains("budget"),
        "{result}"
    );
    assert!(
        names_opened(&lab.svc) <= 1,
        "no files below the truncated second candidate are opened"
    );
    std::fs::rename(&moved, &hidden).unwrap();
    lab.svc
        .inner
        .hooks
        .reclaim_selection_limit
        .store(7, std::sync::atomic::Ordering::Release);
    assert_eq!(
        settle(&lab.svc, &selection_job(&lab.svc, &selection))["coverage"],
        "complete"
    );
    assert_eq!(
        selected_result(&lab.svc, &selection)["reclaimable_bytes"],
        "20480"
    );
}

#[cfg(windows)]
#[test]
fn reclaim_selection_expires_on_each_binding_and_is_removed_by_revoke_or_forget() {
    for forget in [false, true] {
        let Some(lab) = cargo_lab(|_| {}) else { return };
        let selection = selection_ids(&lab.svc);
        settle(&lab.svc, &selection_job(&lab.svc, &selection));
        for (change, restore) in [
            ("UPDATE main.dir SET subtree_rev=subtree_rev+1 WHERE name='target'", "UPDATE main.dir SET subtree_rev=subtree_rev-1 WHERE name='target'"),
            ("UPDATE main.root SET generation=generation+1", "UPDATE main.root SET generation=generation-1"),
            ("UPDATE st.reclaim_selection SET catalog_instance='old'", "UPDATE st.reclaim_selection SET catalog_instance=(SELECT value FROM main.meta WHERE key='instance_id')"),
            ("UPDATE st.reclaim_selection SET members=json_set(members,'$[0].incarnation',-1)", "UPDATE st.reclaim_selection SET members=json_set(members,'$[0].incarnation',(SELECT born_run FROM main.dir WHERE name='target'))"),
        ] {
            lab.svc.inner.db().execute_batch(change).unwrap();
            let expired = selected_result(&lab.svc, &selection);
            assert_eq!(expired["status"], "expired", "{change}: {expired}");
            assert!(expired["reclaimable_bytes"].is_null());
            lab.svc.inner.db().execute_batch(restore).unwrap();
        }
        assert_eq!(
            selected_result(&lab.svc, &selection)["reclaimable_bytes"],
            "20480"
        );
        ok(
            "roots.revoke",
            call(
                &lab.svc,
                "roots.revoke",
                json!({"root_id":lab.root, "purge_catalog":false}),
            ),
        );
        if forget {
            ok(
                "roots.forget",
                call(&lab.svc, "roots.forget", json!({"root_id":lab.root})),
            );
        }
        let rows: i64 = lab
            .svc
            .inner
            .db()
            .query_row("SELECT count(*) FROM st.reclaim_selection", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0);
    }
}

#[cfg(windows)]
#[test]
fn reclaim_selection_revocation_during_observation_cannot_publish_or_open_names() {
    let Some(lab) = cargo_lab(|_| {}) else { return };
    let selection = selection_ids(&lab.svc);
    let hooks = &lab.svc.inner.hooks;
    let gate = (Command::ReclaimObserve, "chunk");
    hooks.closed.lock().unwrap().push(gate);
    let id = selection_job(&lab.svc, &selection);
    hooks.until("selection reached native names", |h| {
        h.arrived.lock().unwrap().contains(&gate)
    });
    ok(
        "roots.revoke",
        call(
            &lab.svc,
            "roots.revoke",
            json!({"root_id":lab.root, "purge_catalog":false}),
        ),
    );
    hooks.open(gate.0, gate.1);
    assert_eq!(settle(&lab.svc, &id)["coverage"], "partial");
    assert_eq!(names_opened(&lab.svc), 0);
    let rows: i64 = lab
        .svc
        .inner
        .db()
        .query_row("SELECT count(*) FROM st.reclaim_selection", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(rows, 0);
}

#[cfg(windows)]
#[test]
fn reclaim_observe_counts_hard_links_inside_a_candidate_once() {
    let Some(lab) = cargo_lab(|target| {
        std::fs::hard_link(
            target.join("one.bin"),
            target.join("debug").join("one-link.bin"),
        )
        .unwrap();
    }) else {
        return;
    };
    let before = target_candidate(&lab.svc);
    assert!(before["reclaimable_bytes"].is_null());
    assert_eq!(
        before["reclaimable_reason"],
        loomward_engine::reclaim::RELIEF_UNOBSERVED
    );
    let catalogue = row_checksums(&lab.svc.inner.db());
    let state = state_checksums(&lab.svc.inner.db());
    let job = settle(&lab.svc, &observe_job(&lab.svc));
    assert_eq!(job["state"], "completed", "{job:?}");
    assert_eq!(job["coverage"], "complete");
    assert_eq!(
        job["reclaim"]["candidates_done"],
        job["reclaim"]["candidates_total"]
    );
    assert_eq!(job["reclaim"]["names_checked"], 3);
    let after = target_candidate(&lab.svc);
    // Unique allocation: the linked object once, not once per name.
    assert_eq!(after["reclaimable_bytes"], "20480", "{after}");
    assert!(after["reclaimable_reason"].is_null());
    assert_eq!(row_checksums(&lab.svc.inner.db()), catalogue);
    assert_eq!(state_checksums(&lab.svc.inner.db()), state);
    // Revocation deletes the root's verdicts.
    ok(
        "roots.revoke",
        call(
            &lab.svc,
            "roots.revoke",
            json!({"root_id": lab.root, "purge_catalog": false}),
        ),
    );
    assert_eq!(relief_rows(&lab.svc), 0);
}

#[cfg(windows)]
#[test]
fn reclaim_observe_link_across_the_candidate_boundary_stays_unknown() {
    let Some(lab) = cargo_lab(|target| {
        std::fs::hard_link(
            target.join("one.bin"),
            target.parent().unwrap().join("outside.bin"),
        )
        .unwrap();
    }) else {
        return;
    };
    let job = settle(&lab.svc, &observe_job(&lab.svc));
    assert_eq!(job["state"], "completed", "{job:?}");
    let after = target_candidate(&lab.svc);
    assert!(after["reclaimable_bytes"].is_null(), "{after}");
    assert_eq!(
        after["reclaimable_reason"],
        loomward_engine::reclaim::RELIEF_UNOBSERVED
    );
    let stored: (Option<i64>, Option<String>) = lab
        .svc
        .inner
        .db()
        .query_row(
            "SELECT unique_allocated_bytes,reason FROM st.reclaim_relief WHERE node_kind='dir'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        stored,
        (
            None,
            Some(loomward_engine::reclaim::RELIEF_UNOBSERVED.into())
        )
    );
}

#[cfg(windows)]
#[test]
fn reclaim_observe_name_replaced_after_the_scan_stays_unknown() {
    let Some(lab) = cargo_lab(|_| {}) else { return };
    let two = lab.path.join("target").join("two.bin");
    std::fs::remove_file(&two).unwrap();
    // Same name and size, another object: its link count must not stand in for the listed one.
    std::fs::write(&two, vec![3u8; 12288]).unwrap();
    let job = settle(&lab.svc, &observe_job(&lab.svc));
    assert_eq!(job["state"], "completed", "{job:?}");
    let after = target_candidate(&lab.svc);
    assert!(after["reclaimable_bytes"].is_null(), "{after}");
    assert_eq!(relief_rows(&lab.svc), 1);
}

#[cfg(windows)]
#[test]
fn reclaim_observe_cancel_and_budget_stops_leave_relief_unknown() {
    let Some(lab) = cargo_lab(|_| {}) else { return };
    let hooks = lab.svc.inner.hooks.clone();
    let gate = (Command::ReclaimObserve, "candidate");
    hooks.closed.lock().unwrap().push(gate);
    let id = observe_job(&lab.svc);
    hooks.until("observation reached its first candidate", |h| {
        h.arrived.lock().unwrap().contains(&gate)
    });
    ok(
        "jobs.cancel",
        call(&lab.svc, "jobs.cancel", json!({"job_id": id})),
    );
    hooks.open(gate.0, gate.1);
    let job = settle(&lab.svc, &id);
    assert_eq!(job["state"], "cancelled", "{job:?}");
    assert_eq!(relief_rows(&lab.svc), 0);
    assert!(target_candidate(&lab.svc)["reclaimable_bytes"].is_null());

    *hooks.reclaim_budget.lock().unwrap() = Some(Duration::ZERO);
    let job = settle(&lab.svc, &observe_job(&lab.svc));
    assert_eq!(job["state"], "completed", "{job:?}");
    assert_eq!(job["coverage"], "partial");
    assert_eq!(job["reclaim"]["candidates_done"], 0);
    assert_eq!(relief_rows(&lab.svc), 0);
    assert!(target_candidate(&lab.svc)["reclaimable_bytes"].is_null());
}

#[cfg(windows)]
#[test]
fn reclaim_observe_rows_lapse_when_a_rescan_moves_the_generation() {
    let Some(lab) = cargo_lab(|_| {}) else { return };
    settle(&lab.svc, &observe_job(&lab.svc));
    assert_eq!(target_candidate(&lab.svc)["reclaimable_bytes"], "20480");
    settle(&lab.svc, &start(&lab.svc, &lab.root, "full"));
    let after = target_candidate(&lab.svc);
    assert!(after["allocated_bytes"].is_string(), "{after}");
    assert!(after["reclaimable_bytes"].is_null(), "{after}");
    assert_eq!(
        after["reclaimable_reason"],
        loomward_engine::reclaim::RELIEF_UNOBSERVED
    );
}

#[test]
fn reclaim_observe_is_owner_only_and_refuses_what_the_plan_refuses() {
    // Hosted Windows runners are elevated (#823); the job refuses an elevated token first.
    #[cfg(windows)]
    if loomward_windows::enumerate::skip_elevated_test() {
        return;
    }
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = fixture(&svc);
    let refused = err(call(
        &svc,
        "reclaim.observe",
        json!({"volume_id": "vo_unknown"}),
    ));
    assert_eq!(refused.code, ErrorCode::NotFound);
    svc.grant_client(
        "mcp:reader",
        &[RootId::new(root.clone()).unwrap()],
        &["size", "volumes"],
        None,
    )
    .unwrap();
    for client in ["ask:local", "mcp:reader"] {
        assert_eq!(
            err(svc.call(
                request("reclaim.observe", json!({"volume_id": volume_id(&svc)})),
                &CallContext::client(client)
            ))
            .code,
            ErrorCode::PermissionDenied
        );
    }
    svc.inner
        .hooks
        .reclaim_elevated
        .store(true, std::sync::atomic::Ordering::Release);
    let elevated = err(call(
        &svc,
        "reclaim.observe",
        json!({"volume_id": volume_id(&svc)}),
    ));
    assert_eq!(elevated.code, ErrorCode::PermissionDenied);
    assert_eq!(reason(&elevated), Some("elevated_token"));
    svc.inner
        .hooks
        .reclaim_elevated
        .store(false, std::sync::atomic::Ordering::Release);
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
        "reclaim.observe",
        json!({"volume_id": volume_id(&svc)}),
    ));
    assert_eq!(refused.code, ErrorCode::InvalidRequest);
    assert_eq!(refused.message.as_str(), "no_active_grant");
    assert_eq!(relief_rows(&svc), 0);
}

#[cfg(windows)]
fn names_opened(svc: &Service) -> usize {
    svc.inner
        .hooks
        .reclaim_names_opened
        .load(std::sync::atomic::Ordering::Acquire)
}

/// #970 review 1: rows written under a candidate without a generation bump (a targeted or running
/// scan) retire its verdict, as does a scan in flight on the root.
#[cfg(windows)]
#[test]
fn reclaim_observe_rows_lapse_when_the_subtree_changes_without_a_generation_bump() {
    let Some(lab) = cargo_lab(|_| {}) else { return };
    settle(&lab.svc, &observe_job(&lab.svc));
    assert_eq!(target_candidate(&lab.svc)["reclaimable_bytes"], "20480");
    let db = || lab.svc.inner.db();
    db().execute(
        "UPDATE main.dir SET subtree_rev=subtree_rev+1 WHERE name='target'",
        [],
    )
    .unwrap();
    let after = target_candidate(&lab.svc);
    assert!(after["allocated_bytes"].is_string(), "{after}");
    assert!(after["reclaimable_bytes"].is_null(), "{after}");
    db().execute(
        "UPDATE main.dir SET subtree_rev=subtree_rev-1 WHERE name='target'",
        [],
    )
    .unwrap();
    assert_eq!(target_candidate(&lab.svc)["reclaimable_bytes"], "20480");
    db().execute(
        "UPDATE main.root SET active_run=(SELECT max(id) FROM main.scan_run)",
        [],
    )
    .unwrap();
    assert!(target_candidate(&lab.svc)["reclaimable_bytes"].is_null());
}

/// #970 review 2: a directory replaced by a junction after the scan is never traversed, even when
/// it leads back to the same objects.
#[cfg(windows)]
#[test]
fn reclaim_observe_never_opens_names_below_a_junction() {
    let Some(lab) = cargo_lab(|target| {
        std::fs::write(target.join("debug").join("deep.bin"), vec![4u8; 4096]).unwrap();
    }) else {
        return;
    };
    let debug = lab.path.join("target").join("debug");
    let moved = lab.path.join("moved-debug");
    std::fs::rename(&debug, &moved).unwrap();
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&debug)
        .arg(&moved)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    let job = settle(&lab.svc, &observe_job(&lab.svc));
    assert_eq!(job["state"], "completed", "{job:?}");
    assert_eq!(names_opened(&lab.svc), 0);
    assert!(target_candidate(&lab.svc)["reclaimable_bytes"].is_null());
    std::fs::remove_dir(&debug).unwrap();
}

/// #970 review 3: a name Win32 would read as a device is opened as the file it is.
#[cfg(windows)]
#[test]
fn reclaim_observe_device_names_are_ordinary_names() {
    let verbatim = |p: &std::path::Path| {
        let p = p.to_str().unwrap();
        if p.starts_with(r"\\?\") {
            std::path::PathBuf::from(p)
        } else {
            std::path::PathBuf::from(format!(r"\\?\{p}"))
        }
    };
    let Some(lab) = cargo_lab(|target| {
        std::fs::write(verbatim(&target.join("com3")), vec![5u8; 4096]).unwrap();
    }) else {
        return;
    };
    let job = settle(&lab.svc, &observe_job(&lab.svc));
    assert_eq!(job["state"], "completed", "{job:?}");
    let after = target_candidate(&lab.svc);
    std::fs::remove_file(verbatim(&lab.path.join("target").join("com3"))).unwrap();
    assert_eq!(after["reclaimable_bytes"], "24576", "{after}");
}

#[cfg(windows)]
fn many_lab() -> Option<Lab> {
    cargo_lab(|target| {
        std::fs::create_dir(target.join("many")).unwrap();
        for i in 0..1200 {
            std::fs::write(target.join("many").join(format!("f{i}.bin")), [6u8; 10]).unwrap();
        }
    })
}

/// #970 review 4: revoking the root stops the pass at the next chunk; nothing is stored.
#[cfg(windows)]
#[test]
fn reclaim_observe_revocation_stops_opening_names() {
    let Some(lab) = many_lab() else { return };
    let hooks = lab.svc.inner.hooks.clone();
    let gate = (Command::ReclaimObserve, "chunk");
    hooks.closed.lock().unwrap().push(gate);
    let id = observe_job(&lab.svc);
    hooks.until("second chunk", |h| {
        h.arrived.lock().unwrap().contains(&gate)
    });
    assert_eq!(names_opened(&lab.svc), 1000);
    ok(
        "roots.revoke",
        call(
            &lab.svc,
            "roots.revoke",
            json!({"root_id": lab.root, "purge_catalog": false}),
        ),
    );
    hooks.open(gate.0, gate.1);
    let job = settle(&lab.svc, &id);
    assert_eq!(job["state"], "completed", "{job:?}");
    assert_eq!(job["coverage"], "partial");
    assert_eq!(names_opened(&lab.svc), 1000);
    assert_eq!(relief_rows(&lab.svc), 0);
}

/// #970 review 7: a budget that expires inside the last candidate is a partial job, not complete.
#[cfg(windows)]
#[test]
fn reclaim_observe_budget_inside_a_candidate_is_partial() {
    let Some(lab) = many_lab() else { return };
    let hooks = lab.svc.inner.hooks.clone();
    *hooks.reclaim_budget.lock().unwrap() = Some(Duration::from_secs(5));
    let gate = (Command::ReclaimObserve, "chunk");
    hooks.closed.lock().unwrap().push(gate);
    let id = observe_job(&lab.svc);
    hooks.until("second chunk", |h| {
        h.arrived.lock().unwrap().contains(&gate)
    });
    std::thread::sleep(Duration::from_secs(6));
    hooks.open(gate.0, gate.1);
    let job = settle(&lab.svc, &id);
    assert_eq!(job["state"], "completed", "{job:?}");
    assert_eq!(job["coverage"], "partial");
    assert_eq!(relief_rows(&lab.svc), 0);
}

/// #970 review 6: one candidate's failure leaves it unknown; the others are still observed.
#[cfg(windows)]
#[test]
fn reclaim_observe_one_candidate_failure_does_not_fail_the_job() {
    let Some(lab) = cargo_lab(|target| {
        let project = target.parent().unwrap();
        std::fs::write(project.join("package.json"), b"{}").unwrap();
        std::fs::create_dir(project.join("node_modules")).unwrap();
        std::fs::write(project.join("node_modules").join("x.bin"), vec![7u8; 4096]).unwrap();
    }) else {
        return;
    };
    lab.svc
        .inner
        .hooks
        .reclaim_fail_candidate
        .store(true, std::sync::atomic::Ordering::Release);
    let job = settle(&lab.svc, &observe_job(&lab.svc));
    assert_eq!(job["state"], "completed", "{job:?}");
    assert_eq!(job["coverage"], "partial");
    assert_eq!(relief_rows(&lab.svc), 1);
}

/// #970 review 5: a root that fails admission is skipped, and the read job writes no grant state.
#[cfg(windows)]
#[test]
fn reclaim_observe_skips_a_root_that_fails_admission() {
    // Hosted Windows runners are elevated (#823); the job refuses an elevated token first.
    #[cfg(windows)]
    if loomward_windows::enumerate::skip_elevated_test() {
        return;
    }
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = fixture(&svc);
    let grants = || {
        svc.inner
            .db()
            .query_row("SELECT group_concat(state) FROM st.root_grant", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap()
    };
    let before = grants();
    let job = settle(&svc, &observe_job(&svc));
    assert_eq!(job["state"], "completed", "{job:?}");
    assert_eq!(job["coverage"], "partial");
    assert_eq!(relief_rows(&svc), 0);
    assert_eq!(grants(), before);
    let _ = root;
}

/// #970 review 3: roots that are not already verbatim (the lab's canonical roots are) gain the
/// prefix, shares through `UNC`.
#[cfg(windows)]
#[test]
fn reclaim_observe_paths_are_verbatim() {
    use std::path::{Path, PathBuf};
    for (input, expected) in [
        (r"C:\lab\root", r"\\?\C:\lab\root"),
        (r"\\?\C:\lab\root", r"\\?\C:\lab\root"),
        (r"\\server\share\root", r"\\?\UNC\server\share\root"),
    ] {
        assert_eq!(
            crate::reclaim::verbatim(Path::new(input)),
            PathBuf::from(expected)
        );
    }
}

#[test]
fn reclaim_inspect_budget_is_explicit_and_partial() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    fixture(&svc);
    let complete = reclaim(&svc);
    assert_eq!(complete["budget_hit"], false);
    assert_eq!(complete["partial"], false);
    svc.inner
        .hooks
        .reclaim_inspect_budget
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let plan = reclaim(&svc);
    assert_eq!(plan["budget_hit"], true);
    assert_eq!(plan["partial"], true);
    assert_eq!(plan["covered_roots"][0]["coverage"], "unknown");
    assert!(plan["covered_roots"][0]["allocated_bytes"].is_null());
    assert_eq!(
        reclaim(&svc),
        complete,
        "inspection refusal is one-shot and read-only"
    );
}

fn set_space(svc: &Service, free: u64) {
    svc.inner
        .db()
        .execute(
            "UPDATE main.volume SET capabilities_json=?1",
            [
                json!({"space":{"capacity_bytes":"200000", "free_bytes":free.to_string()}})
                    .to_string(),
            ],
        )
        .unwrap();
}

fn sample(svc: &Service, role: &str, extra: Value) -> ResponseEnvelope {
    let mut payload = json!({"volume_id": volume_id(svc), "role": role});
    if let (Some(p), Some(e)) = (payload.as_object_mut(), extra.as_object()) {
        p.extend(e.clone());
    }
    call(svc, "reclaim.sample", payload)
}

fn relief(svc: &Service, role: &str, extra: Value) -> Value {
    json!(ok("reclaim.sample", sample(svc, role, extra)).0)
}

#[test]
fn relief_check_reports_the_observed_change_beside_the_estimate() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    fixture(&svc);
    assert!(
        reclaim(&svc).get("relief").is_none(),
        "nothing recorded yet"
    );
    set_space(&svc, 100_000);
    let before = relief(&svc, "before", json!({"estimate_bytes": "20480"}));
    assert_eq!(before["status"], "not_checked");
    assert!(before["after"].is_null() && before["change_bytes"].is_null());
    assert_eq!(before["before"]["free_bytes"], "100000");
    assert_eq!(before["before"]["estimate_bytes"], "20480");
    assert!(
        before["before"]["recycle_bin_bytes"].is_null(),
        "a bin that was not read is unknown, not zero"
    );
    assert_eq!(
        reclaim(&svc)["relief"],
        before,
        "the plan shows the stored check"
    );
    set_space(&svc, 130_000);
    let after = relief(&svc, "after", json!({}));
    assert_eq!(after["status"], "compared");
    assert_eq!(after["direction"], "more_free");
    assert_eq!(after["change_bytes"], "30000");
    assert_eq!(after["before"]["estimate_bytes"], "20480");
    assert!(after["after"]["estimate_bytes"].is_null());
    assert_eq!(reclaim(&svc)["relief"], after);
    // Free space that fell is reported as such, not as a negative or zero relief.
    set_space(&svc, 90_000);
    let worse = relief(&svc, "after", json!({}));
    assert_eq!(worse["direction"], "less_free");
    assert_eq!(worse["change_bytes"], "10000");
}

#[test]
fn relief_check_needs_a_before_sample_and_keeps_the_estimate_with_it() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    fixture(&svc);
    set_space(&svc, 100_000);
    let refused = err(sample(&svc, "after", json!({})));
    assert_eq!(refused.code, ErrorCode::InvalidRequest);
    assert!(reclaim(&svc).get("relief").is_none());
    relief(&svc, "before", json!({}));
    let refused = err(sample(&svc, "after", json!({"estimate_bytes": "1"})));
    assert_eq!(refused.code, ErrorCode::InvalidRequest);
    relief(&svc, "after", json!({}));
    // A new before sample starts a new check: the old after sample is gone.
    let restarted = relief(&svc, "before", json!({}));
    assert_eq!(restarted["status"], "not_checked");
    assert!(restarted["after"].is_null());
}

#[test]
fn relief_check_does_not_compare_a_different_volume_identity_or_unreadable_space() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    fixture(&svc);
    set_space(&svc, 100_000);
    relief(&svc, "before", json!({}));
    svc.inner
        .db()
        .execute(
            "UPDATE st.root_grant SET volume_guid='00000000000000000000000000000001'",
            [],
        )
        .unwrap();
    set_space(&svc, 150_000);
    let other = relief(&svc, "after", json!({}));
    assert_eq!(other["status"], "different_volume");
    assert!(other["direction"].is_null() && other["change_bytes"].is_null());
    assert_eq!(
        other["after"]["free_bytes"], "150000",
        "the sample is kept, only the comparison is refused"
    );
    // Free space the service could not read is unknown on that side, not zero.
    svc.inner
        .db()
        .execute("UPDATE st.root_grant SET volume_guid=NULL", [])
        .unwrap();
    relief(&svc, "before", json!({}));
    svc.inner
        .db()
        .execute("UPDATE main.volume SET capabilities_json='{}'", [])
        .unwrap();
    let unreadable = relief(&svc, "after", json!({}));
    assert_eq!(unreadable["status"], "unreadable");
    assert!(unreadable["after"]["free_bytes"].is_null());
    assert!(unreadable["change_bytes"].is_null());
}

#[test]
fn relief_check_writes_only_its_own_sample_rows() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    fixture(&svc);
    set_space(&svc, 100_000);
    let catalogue = row_checksums(&svc.inner.db());
    relief(&svc, "before", json!({}));
    relief(&svc, "after", json!({}));
    let rows: i64 = svc
        .inner
        .db()
        .query_row("SELECT count(*) FROM st.reclaim_sample", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 2, "at most one before and one after per volume");
    assert_eq!(row_checksums(&svc.inner.db()), catalogue);
}
