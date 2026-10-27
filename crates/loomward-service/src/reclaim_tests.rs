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
