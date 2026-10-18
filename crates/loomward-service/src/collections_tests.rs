use super::*;
use loomward_catalog::NodeKey;

fn setup(class: DatasetClass) -> (tempfile::TempDir, Service, Vec<String>) {
    let dir = temp_root();
    let svc = open(dir.path(), class);
    if class == DatasetClass::Synthetic {
        fixture(&svc, 3);
    } else {
        let loomward_catalog::WriteReply::Root {
            grant_id, dir_id, ..
        } = svc
            .inner
            .catalog
            .writer()
            .call(loomward_catalog::WriteCommand::RegisterRoot(
                loomward_catalog::RootObservation {
                    volume_key: "fixture".into(),
                    display_name: "Synthetic".into(),
                    display_path: "Synthetic root".into(),
                    root_file_id: Some(vec![1; 16]),
                    filesystem: Some("NTFS".into()),
                    origin: "owner_granted".into(),
                    granted_via: "cli_flag".into(),
                    observed_at_ns: 0,
                },
            ))
            .unwrap()
        else {
            panic!()
        };
        relist(&svc, grant_id, dir_id, 3);
    }
    // Catalogue-only synthetic native identities: no host opens or content reads.
    svc.inner
        .db()
        .execute_batch(
            "UPDATE main.file SET file_id=CAST(printf('%016d',id) AS BLOB),created_ft=123;",
        )
        .unwrap();
    let ids = {
        let conn = svc.inner.db();
        let mut stmt = conn
            .prepare("SELECT id,born_run FROM main.file ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| {
            Ok(svc
                .inner
                .ids
                .seal(NodeKey::File(r.get(0)?), r.get(1)?, &svc.inner.instance))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    };
    (dir, svc, ids)
}
fn create(svc: &Service, name: &str) -> String {
    ok(
        "collections.create",
        call(svc, "collections.create", json!({"name":name})),
    )
    .0["collection_id"]
        .as_str()
        .unwrap()
        .into()
}
fn members(svc: &Service, id: &str, limit: i64, cursor: Value) -> ResponseEnvelope {
    call(
        svc,
        "collections.members",
        json!({"collection_id":id,"limit":limit,"cursor":cursor}),
    )
}
fn update(
    svc: &Service,
    id: &str,
    add: &[String],
    remove: &[String],
    expected: i64,
) -> ResponseEnvelope {
    let mut r = request(
        "collections.update_members",
        json!({"collection_id":id,"add":add,"remove":remove}),
    );
    r.expected_state_rev = Some(Generation(expected as u64));
    svc.call(r, &CallContext::http())
}

#[test]
fn node_inspect_keeps_typed_collection_memberships() {
    let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
    let collection = create(&svc, "Projects");
    ok(
        "collections.update_members",
        update(&svc, &collection, &ids[..1], &[], 2),
    );
    let (detail, _) = ok(
        "node.inspect",
        call(&svc, "node.inspect", json!({"node_id":ids[0]})),
    );
    assert_eq!(
        detail["memberships"],
        json!([{"collection_id":collection,"name":"Projects"}])
    );
    let root = RootId::new(svc.inner.root_id(1)).unwrap();
    svc.grant_client("mcp:membership", &[root], &[], None)
        .unwrap();
    let response = svc.call(
        request("node.inspect", json!({"node_id":ids[0]})),
        &CallContext::client("mcp:membership"),
    );
    let ResponseEnvelope::Ok(response) = response else {
        panic!("{response:?}")
    };
    let detail: ClientNodeDetail = serde_json::from_value(Value::Object(response.result)).unwrap();
    assert_eq!(detail.row.node_id.as_ref(), ids[0]);
}
#[test]
fn collections_create_replay_duplicate_and_restart_both_classes() {
    for class in [DatasetClass::Synthetic, DatasetClass::Personal] {
        let (dir, svc, ids) = setup(class);
        let r = request("collections.create", json!({"name":"Projects"}));
        let first = svc.call(r.clone(), &CallContext::http());
        let (c, meta) = ok("collections.create", first.clone());
        assert_eq!(meta.dataset_class, class);
        assert_eq!(meta.state_rev.unwrap().get(), 2);
        assert_eq!(svc.call(r, &CallContext::http()), first);
        let e = err(call(&svc, "collections.create", json!({"name":"Projects"})));
        assert_eq!(e.code, ErrorCode::InvalidRequest);
        assert_eq!(e.detail.unwrap()["existing_id"], c["collection_id"]);
        assert_eq!(svc.inner.revisions().unwrap().1, 2);
        let (c, _) = ok(
            "collections.update_members",
            update(&svc, c["collection_id"].as_str().unwrap(), &ids, &[], 2),
        );
        drop(svc);
        let svc = open(dir.path(), class);
        let (list, _) = ok(
            "collections.list",
            call(&svc, "collections.list", json!({})),
        );
        assert_eq!(list["collections"], json!([c]));
    }
}
#[test]
fn collections_atomic_precondition_and_identity_refusals() {
    let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
    let id = create(&svc, "Projects");
    assert_eq!(
        err(call(
            &svc,
            "collections.update_members",
            json!({"collection_id":id,"add":ids,"remove":[]})
        ))
        .code,
        ErrorCode::InvalidRequest
    );
    let before = svc.inner.revisions().unwrap();
    let e = err(update(&svc, &id, &ids, &[], 0));
    assert_eq!(e.code, ErrorCode::StaleGeneration);
    assert_eq!(reason(&e), Some("state_rev_changed"));
    assert_eq!(svc.inner.revisions().unwrap(), before);
    assert_eq!(
        svc.inner
            .db()
            .query_row("SELECT count(*) FROM st.object_ref", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        err(update(&svc, &id, &ids[..1], &ids[..1], 2)).code,
        ErrorCode::InvalidRequest
    );
    let bad = vec![ids[0].clone(), "nd_forged".into()];
    assert_eq!(
        err(update(&svc, &id, &bad, &[], 2)).code,
        ErrorCode::NotFound
    );
    assert_eq!(svc.inner.revisions().unwrap(), before);
    svc.inner
        .db()
        .execute("UPDATE main.file SET created_ft=NULL WHERE id=2", [])
        .unwrap();
    assert_eq!(
        err(update(&svc, &id, &ids, &[], 2)).code,
        ErrorCode::CapabilityUnavailable
    );
    assert_eq!(
        svc.inner
            .db()
            .query_row("SELECT count(*) FROM st.object_ref", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let mut mutation = request(
        "collections.update_members",
        json!({"collection_id":id,"add":ids[..1],"remove":[]}),
    );
    mutation.expected_state_rev = Some(Generation(2));
    let added = svc.call(mutation.clone(), &CallContext::http());
    ok("collections.update_members", added.clone());
    let rev = svc.inner.revisions().unwrap().1;
    ok(
        "collections.update_members",
        update(&svc, &id, &ids[..1], &[], rev),
    );
    assert_eq!(svc.inner.revisions().unwrap().1, rev);
    ok(
        "collections.update_members",
        update(&svc, &id, &[], &ids[..1], rev),
    );
    assert_eq!(svc.call(mutation, &CallContext::http()), added);
    assert_eq!(
        ok("collections.members", members(&svc, &id, 200, Value::Null)).0["total"],
        0
    );
}
#[test]
fn collections_members_keep_missing_revoked_and_bind_cursors() {
    let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
    let id = create(&svc, "Projects");
    ok(
        "collections.update_members",
        update(&svc, &id, &ids, &[], 2),
    );
    let (p, _) = ok("collections.members", members(&svc, &id, 1, Value::Null));
    let cursor = p["next_cursor"].clone();
    assert_eq!(p["total"], 3);
    assert_eq!(p["items"][0]["status"], "resolved");
    let (next, _) = ok("collections.members", members(&svc, &id, 2, cursor.clone()));
    assert_eq!(next["items"].as_array().unwrap().len(), 2);
    assert_ne!(next["items"][0]["member_id"], p["items"][0]["member_id"]);
    let other = create(&svc, "Other");
    assert_eq!(
        err(members(&svc, &other, 1, cursor.clone())).code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        reason(&err(members(&svc, &id, 1, cursor))),
        Some("state_rev_changed")
    );
    let (p, _) = ok("collections.members", members(&svc, &id, 1, Value::Null));
    svc.inner
        .db()
        .execute("UPDATE main.revision SET catalog_rev=catalog_rev+1", [])
        .unwrap();
    assert_eq!(
        reason(&err(members(&svc, &id, 1, p["next_cursor"].clone()))),
        Some("catalog_rev_changed")
    );
    // No name/path reattachment when a catalogue object is gone.
    svc.inner
        .db()
        .execute("DELETE FROM main.file WHERE id=1", [])
        .unwrap();
    let (p, _) = ok("collections.members", members(&svc, &id, 200, Value::Null));
    assert_eq!(p["total"], 3);
    assert_eq!(p["items"][0]["member_id"], "om_1");
    assert_eq!(p["items"][0]["status"], "unresolved");
    assert!(p["items"][0]["entry"].is_null());
    let (list, _) = ok(
        "collections.list",
        call(&svc, "collections.list", json!({})),
    );
    assert!(list["collections"][0]["logical_bytes"].is_null());
    svc.inner
        .db()
        .execute(
            "UPDATE st.root_grant SET state='revoked',revoked_at_ns=1",
            [],
        )
        .unwrap();
    let (p, _) = ok("collections.members", members(&svc, &id, 200, Value::Null));
    assert_eq!(p["items"][1]["status"], "root_revoked");
    assert!(p["items"][1]["entry"].is_null());
}
#[test]
fn collections_migration_preserves_unresolved_members() {
    let dir = temp_root();
    v1_store(dir.path());
    let conn = rusqlite::Connection::open(dir.path().join("state.db")).unwrap();
    conn.execute_batch("INSERT INTO object_ref VALUES(1,'fixture',zeroblob(16),'file'); INSERT INTO collection VALUES(1,'Legacy','human',0); INSERT INTO collection_member VALUES(1,1,0);").unwrap();
    drop(conn);
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (p, _) = ok(
        "collections.members",
        members(&svc, "co_1", 200, Value::Null),
    );
    assert_eq!(p["total"], 1);
    assert_eq!(p["items"][0]["status"], "unresolved");
    assert!(p["items"][0]["entry"].is_null());
    assert!(dir.path().join("state.before-v1.db").exists());
}
#[test]
fn collections_deadline_replay_and_fifo() {
    let (_dir, svc, _) = setup(DatasetClass::Synthetic);
    let svc = Arc::new(svc);
    *svc.inner.hooks.delay.lock().unwrap() =
        Some((Command::CollectionsCreate, Duration::from_millis(100)));
    let mut r = request("collections.create", json!({"name":"First"}));
    r.deadline_ms = Some(loomward_protocol::Int::new(10).unwrap());
    assert_eq!(
        err(svc.call(r.clone(), &CallContext::http())).code,
        ErrorCode::DeadlineExceeded
    );
    create(&svc, "Second");
    ok("collections.create", svc.call(r, &CallContext::http()));
    let (list, _) = ok(
        "collections.list",
        call(&svc, "collections.list", json!({})),
    );
    assert_eq!(list["collections"][0]["name"], "First");
    assert_eq!(list["collections"][1]["name"], "Second");
    assert_eq!(
        svc.inner.hooks.runs.lock().unwrap()[&Command::CollectionsCreate],
        2
    );
}
#[test]
fn collections_change_no_fixture_bytes_or_mtimes() {
    let (dir, svc, ids) = setup(DatasetClass::Synthetic);
    let root = dir.path().join("fixture-root");
    std::fs::create_dir(&root).unwrap();
    let path = root.join("report.txt");
    std::fs::write(&path, b"Synthetic document").unwrap();
    let snapshot = || {
        (
            std::fs::read(&path).unwrap(),
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            std::fs::metadata(&root).unwrap().modified().unwrap(),
        )
    };
    svc.inner
        .db()
        .execute(
            "UPDATE st.root_grant SET display_path=?1",
            [root.to_string_lossy().as_ref()],
        )
        .unwrap();
    let before = snapshot();
    let id = create(&svc, "Projects");
    ok(
        "collections.update_members",
        update(&svc, &id, &ids, &[], 2),
    );
    ok("collections.members", members(&svc, &id, 200, Value::Null));
    assert_eq!(snapshot(), before);
    assert_eq!(
        svc.inner
            .db()
            .query_row(
                "SELECT count(*) FROM st.root_grant WHERE state='active'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn collections_reference_survives_rescan_but_not_replacement() {
    let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
    let id = create(&svc, "Projects");
    ok(
        "collections.update_members",
        update(&svc, &id, &ids[..1], &[], 2),
    );
    let (first, _) = ok("collections.members", members(&svc, &id, 200, Value::Null));
    let writer = svc.inner.catalog.writer();
    for (name, created) in [("renamed.txt", 123), ("renamed.txt", 124)] {
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
        let mut observed = file(name, 1000);
        observed.file_id = Some(format!("{:016}", 1).into_bytes());
        observed.created_ft = Some(created);
        writer
            .call(WriteCommand::DirListing(DirListing {
                run_id: run,
                dir_id: 1,
                files: vec![observed],
                dirs: vec![],
                state: "complete".into(),
                skipped: 0,
                errors: 0,
            }))
            .unwrap();
        writer
            .call(WriteCommand::EndRun {
                run_id: run,
                state: "completed".into(),
                finished_at_ns: 1,
            })
            .unwrap();
        let (p, _) = ok("collections.members", members(&svc, &id, 200, Value::Null));
        assert_eq!(p["total"], 1);
        assert_eq!(p["items"][0]["member_id"], first["items"][0]["member_id"]);
        if created == 123 {
            assert_eq!(p["items"][0]["status"], "resolved");
            assert_eq!(p["items"][0]["entry"]["name"], "renamed.txt");
        } else {
            assert_eq!(p["items"][0]["status"], "unresolved");
            assert!(p["items"][0]["entry"].is_null());
        }
    }
}

#[test]
fn collections_protection_refuses_revoked_root() {
    collection_refusal(
        "UPDATE st.root_grant SET state='revoked',revoked_at_ns=1",
        ErrorCode::NotFound,
    );
}
#[test]
fn collections_protection_refuses_identity_changed() {
    collection_refusal(
        "UPDATE st.root_grant SET state='identity_changed'",
        ErrorCode::NotFound,
    );
}
#[test]
fn collections_protection_refuses_absent_subtree() {
    collection_refusal(
        "UPDATE main.dir SET listing_state='absent_pending'",
        ErrorCode::NotFound,
    );
}
#[test]
fn collections_protection_refuses_unsupported_volume() {
    collection_refusal(
        "UPDATE main.volume SET filesystem='exFAT'",
        ErrorCode::CapabilityUnavailable,
    );
}
#[test]
fn collections_protection_refuses_label_view() {
    collection_refusal(
        "UPDATE st.collection SET kind='label_view'",
        ErrorCode::CapabilityUnavailable,
    );
}
fn collection_refusal(sql: &str, code: ErrorCode) {
    let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
    let id = create(&svc, "Projects");
    svc.inner.db().execute_batch(sql).unwrap();
    let before = svc.inner.revisions().unwrap();
    assert_eq!(err(update(&svc, &id, &ids, &[], before.1)).code, code);
    assert_eq!(svc.inner.revisions().unwrap(), before);
    for table in ["object_ref", "collection_member"] {
        assert_eq!(
            svc.inner
                .db()
                .query_row(&format!("SELECT count(*) FROM st.{table}"), [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    // A successful control pins the refusal to this guard, rather than a broken mutation.
    svc.inner.db().execute_batch("UPDATE st.root_grant SET state='active',revoked_at_ns=NULL; UPDATE main.dir SET listing_state='complete'; UPDATE main.volume SET filesystem='NTFS'; UPDATE st.collection SET kind='human'").unwrap();
    assert_eq!(
        ok(
            "collections.update_members",
            update(&svc, &id, &ids, &[], before.1)
        )
        .0["member_count"],
        3
    );
}

#[test]
fn collections_protection_caps_creates_at_256() {
    let (_dir, svc, _) = setup(DatasetClass::Synthetic);
    svc.inner.db().execute_batch("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<255) INSERT INTO st.collection(name,kind,created_at_ns) SELECT 'Collection '||i,'human',0 FROM n").unwrap();
    create(&svc, "Last");
    let before = svc.inner.revisions().unwrap();
    assert_eq!(
        err(call(&svc, "collections.create", json!({"name":"Too many"}))).code,
        ErrorCode::ResourceBudget
    );
    assert_eq!(svc.inner.revisions().unwrap(), before);
    assert_eq!(
        ok(
            "collections.list",
            call(&svc, "collections.list", json!({}))
        )
        .0["collections"]
            .as_array()
            .unwrap()
            .len(),
        256
    );
}

#[test]
fn collections_remove_unresolved_by_member_id() {
    for damage in [
        "DELETE FROM main.file WHERE id=1",
        "UPDATE main.file SET created_ft=124 WHERE id=1",
        "UPDATE st.root_grant SET state='revoked',revoked_at_ns=1",
    ] {
        let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
        let id = create(&svc, "Projects");
        ok(
            "collections.update_members",
            update(&svc, &id, &ids[..1], &[], 2),
        );
        let other = create(&svc, "Other");
        ok(
            "collections.update_members",
            update(&svc, &other, &ids[..1], &[], 4),
        );
        svc.inner.db().execute_batch(damage).unwrap();
        let (page, meta) = ok("collections.members", members(&svc, &id, 200, Value::Null));
        assert!(page["items"][0]["entry"].is_null());
        let mut req = request(
            "collections.update_members",
            json!({"collection_id":id,"add":[],"remove":[],"remove_members":[page["items"][0]["member_id"]]}),
        );
        assert_eq!(
            err(svc.call(req.clone(), &CallContext::http())).code,
            ErrorCode::InvalidRequest
        );
        req.request_id = RequestId::new("remove_stale").unwrap();
        req.expected_state_rev = Some(Generation(0));
        assert_eq!(
            err(svc.call(req.clone(), &CallContext::http())).code,
            ErrorCode::StaleGeneration
        );
        req.request_id = RequestId::new("remove_current").unwrap();
        req.expected_state_rev = meta.state_rev;
        let (_, result_meta) = ok(
            "collections.update_members",
            svc.call(req, &CallContext::http()),
        );
        assert_eq!(
            result_meta.state_rev.unwrap().get(),
            meta.state_rev.unwrap().get() + 1
        );
        assert_eq!(
            ok("collections.members", members(&svc, &id, 200, Value::Null)).0["total"],
            0
        );
        assert_eq!(
            ok(
                "collections.members",
                members(&svc, &other, 200, Value::Null)
            )
            .0["total"],
            1
        );
    }
}

#[test]
fn collections_non_revoked_inactive_grant_is_unavailable() {
    let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
    let id = create(&svc, "Projects");
    ok(
        "collections.update_members",
        update(&svc, &id, &ids[..1], &[], 2),
    );
    svc.inner
        .db()
        .execute("UPDATE st.root_grant SET state='identity_changed'", [])
        .unwrap();
    let (page, _) = ok("collections.members", members(&svc, &id, 200, Value::Null));
    assert_eq!(page["items"][0]["status"], "root_unavailable");
    assert!(page["items"][0]["entry"].is_null());
}

#[test]
fn collections_names_reject_del_and_c1_controls() {
    let (_dir, svc, _) = setup(DatasetClass::Synthetic);
    for control in ['\u{7f}', '\u{85}', '\u{9f}'] {
        assert_eq!(
            err(call(
                &svc,
                "collections.create",
                json!({"name":format!("Project{control}")})
            ))
            .code,
            ErrorCode::InvalidRequest
        );
    }
    create(&svc, "Projects 日本語");
}

#[test]
fn collections_protection_rejects_publication_race() {
    for sql in [
        "UPDATE main.revision SET catalog_rev=catalog_rev+1",
        "UPDATE st.revision SET state_rev=state_rev+1",
    ] {
        let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
        let id = create(&svc, "Projects");
        ok(
            "collections.update_members",
            update(&svc, &id, &ids, &[], 2),
        );
        let svc = Arc::new(svc);
        svc.inner
            .hooks
            .closed
            .lock()
            .unwrap()
            .push((Command::CollectionsMembers, "hydrated"));
        let worker = svc.clone();
        let collection_id = id.clone();
        let read = std::thread::spawn(move || members(&worker, &collection_id, 200, Value::Null));
        svc.inner.hooks.until("hydrated", |h| {
            h.arrived
                .lock()
                .unwrap()
                .contains(&(Command::CollectionsMembers, "hydrated"))
        });
        svc.inner.db().execute(sql, []).unwrap();
        svc.inner
            .hooks
            .open(Command::CollectionsMembers, "hydrated");
        let e = err(read.join().unwrap());
        assert_eq!(e.code, ErrorCode::StaleGeneration);
        assert_eq!(reason(&e), Some("revision_changed"));
        assert_eq!(
            ok("collections.members", members(&svc, &id, 200, Value::Null)).0["total"],
            3
        );
    }
}

#[test]
fn collections_hydration_stops_at_page_deadline_and_releases_slot() {
    let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
    let id = create(&svc, "Projects");
    ok(
        "collections.update_members",
        update(&svc, &id, &ids, &[], 2),
    );
    let svc = Arc::new(svc);
    svc.inner
        .hooks
        .closed
        .lock()
        .unwrap()
        .push((Command::CollectionsMembers, "hydrate"));
    let worker = svc.clone();
    let read = std::thread::spawn(move || {
        let mut req = request(
            "collections.members",
            json!({"collection_id":id,"limit":200,"cursor":null}),
        );
        // Far longer than any loaded host needs to reach the gate; the deadline is passed below
        // by moving the page clock, not by waiting for it.
        req.deadline_ms = Some(Int::new(30_000).unwrap());
        worker.call(req, &CallContext::http())
    });
    svc.inner.hooks.until("hydrate", |h| {
        h.arrived
            .lock()
            .unwrap()
            .contains(&(Command::CollectionsMembers, "hydrate"))
    });
    assert_eq!(
        *svc.inner.reads.lock().unwrap(),
        1,
        "the page holds its slot"
    );
    *svc.inner.hooks.skew.lock().unwrap() = Duration::from_secs(31);
    svc.inner.hooks.open(Command::CollectionsMembers, "hydrate");
    assert_eq!(err(read.join().unwrap()).code, ErrorCode::DeadlineExceeded);
    svc.inner.hooks.until("read slot released", |_| {
        *svc.inner.reads.lock().unwrap() == 0
    });
    assert!(
        !svc.inner
            .hooks
            .arrived
            .lock()
            .unwrap()
            .contains(&(Command::CollectionsMembers, "hydrated")),
        "an expired page must not finish hydrating"
    );
    assert_eq!(
        ok(
            "collections.list",
            call(&svc, "collections.list", json!({}))
        )
        .0["collections"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn collections_hydration_interrupt_reaches_running_inspection() {
    let (_dir, svc, _) = setup(DatasetClass::Synthetic);
    svc.inner.db().execute_batch("UPDATE main.dir SET file_id=CAST(printf('%016d',id) AS BLOB),created_ft=123 WHERE id=1; WITH RECURSIVE n(i) AS (VALUES(10) UNION ALL SELECT i+1 FROM n WHERE i<20009) INSERT INTO main.file(id,dir_id,name,file_id,logical,allocated,created_ft,attrs,born_run,seen_run) SELECT i,1,'entry'||i,CAST(printf('%016d',i) AS BLOB),1,1,123,0,1,1 FROM n").unwrap();
    let born = svc
        .inner
        .db()
        .query_row("SELECT born_run FROM main.dir WHERE id=1", [], |r| r.get(0))
        .unwrap();
    let node = svc
        .inner
        .ids
        .seal(NodeKey::Dir(1), born, &svc.inner.instance);
    let id = create(&svc, "Projects");
    ok(
        "collections.update_members",
        update(&svc, &id, &[node], &[], 2),
    );
    // This control proves the bounded synthetic directory is inspectable without cancellation.
    assert_eq!(
        ok("collections.members", members(&svc, &id, 200, Value::Null)).0["items"][0]["status"],
        "resolved"
    );
    let svc = Arc::new(svc);
    let work = Arc::new(crate::ReadWork {
        deadline: Instant::now() + Duration::from_secs(5),
        interrupt: Mutex::new(None),
    });
    svc.inner.hooks.arrived.lock().unwrap().clear();
    svc.inner
        .hooks
        .closed
        .lock()
        .unwrap()
        .push((Command::CollectionsMembers, "hydrate"));
    let worker = svc.clone();
    let cancellation = work.clone();
    let read = std::thread::spawn(move || {
        worker.inner.collections_members(
            &request(
                "collections.members",
                json!({"collection_id":id,"limit":200,"cursor":null}),
            ),
            Some(&cancellation),
        )
    });
    svc.inner.hooks.until("hydrate", |h| {
        h.arrived
            .lock()
            .unwrap()
            .contains(&(Command::CollectionsMembers, "hydrate"))
    });
    svc.inner.hooks.open(Command::CollectionsMembers, "hydrate");
    let stop = Instant::now() + Duration::from_secs(1);
    while !read.is_finished() && Instant::now() < stop {
        work.interrupt.lock().unwrap().as_ref().unwrap().interrupt();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        read.join().unwrap().is_err(),
        "cancellation must reach catalogue inspection, not its idle state connection"
    );
}

#[test]
fn collections_remove_members_validation_and_noop() {
    let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
    let id = create(&svc, "Projects");
    ok(
        "collections.update_members",
        update(&svc, &id, &ids[..1], &[], 2),
    );
    for (add, remove_members, expected) in [
        (json!([]), json!([]), None),
        (json!([]), json!(["om_99"]), None),
        (json!([]), json!(["om_01"]), Some(ErrorCode::InvalidRequest)),
        (
            json!([]),
            json!(["om_1", "om_1"]),
            Some(ErrorCode::InvalidRequest),
        ),
        (json!([]), Value::Null, Some(ErrorCode::InvalidRequest)),
        (
            json!(ids[..1]),
            json!(["om_1"]),
            Some(ErrorCode::InvalidRequest),
        ),
    ] {
        let before = svc.inner.revisions().unwrap();
        let mut req = request(
            "collections.update_members",
            json!({"collection_id":id,"add":add,"remove":[],"remove_members":remove_members}),
        );
        req.expected_state_rev = Some(Generation(before.1 as u64));
        let response = svc.call(req, &CallContext::http());
        if let Some(code) = expected {
            assert_eq!(err(response).code, code);
        } else {
            assert_eq!(
                ok("collections.update_members", response).0["member_count"],
                1
            );
        }
        assert_eq!(svc.inner.revisions().unwrap(), before);
    }
}

/// #356: an add that lands between the writer's precious staging and its catalogue publication
/// creates a reference the staging never saw. It must still follow the publication: the hard
/// link's identity to its surviving row, the dropped file to retirement.
#[test]
fn collections_add_inside_the_publication_window_follows_the_publication() {
    let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
    let id = create(&svc, "Projects");
    let svc = Arc::new(svc);
    let scan = rescan_paused_in_publication_window(&svc);
    let rev = svc.inner.revisions().unwrap().1;
    ok(
        "collections.update_members",
        update(&svc, &id, &[ids[0].clone(), ids[2].clone()], &[], rev),
    );
    svc.inner.catalog.writer().publication_window().release();
    scan.join().unwrap();
    let bindings = {
        let conn = svc.inner.db();
        let mut s = conn
            .prepare("SELECT id,state,retired,row_id FROM st.object_ref ORDER BY id")
            .unwrap();
        s.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    };
    assert_eq!(
        bindings,
        vec![
            (1, "resolved".into(), false, 2),
            (2, "unresolved".into(), true, 3)
        ]
    );
    let (page, _) = ok("collections.members", members(&svc, &id, 200, Value::Null));
    assert_eq!(page["total"], 2);
    assert_eq!(page["items"][0]["status"], "resolved");
    assert_eq!(page["items"][0]["entry"]["name"], "report-001.txt");
    assert_eq!(page["items"][1]["status"], "unresolved");
}

#[test]
fn collections_list_resolves_references_once_for_all_collections() {
    let (_dir, svc, _) = setup(DatasetClass::Synthetic);
    // 256 collections over 30,000 invented references: evaluating the reference CTE per collection took
    // longer than the 2 s read deadline (measured 2026-10-11, debug build); once it takes ~0.1 s.
    svc.inner.db().execute_batch("UPDATE main.dir SET file_id=CAST(printf('%016d',id) AS BLOB),created_ft=123 WHERE id=1; WITH RECURSIVE n(i) AS (VALUES(10) UNION ALL SELECT i+1 FROM n WHERE i<30009) INSERT INTO main.file(id,dir_id,name,file_id,logical,allocated,created_ft,attrs,born_run,seen_run) SELECT i,1,'entry'||i,CAST(printf('%016d',i) AS BLOB),1,1,123,0,1,1 FROM n;
      WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<256) INSERT INTO st.collection(name,kind,created_at_ns) SELECT 'C'||i,'human',0 FROM n;
      INSERT INTO st.object_ref(volume_key,file_id,creation_ft,kind,state,last_resolved_ns,incarnation,catalog_instance,row_id,born_run) SELECT 'fixture',f.file_id,f.created_ft,'file','resolved',0,1,(SELECT value FROM main.meta WHERE key='instance_id'),f.id,f.born_run FROM main.file f WHERE f.id>=10;
      INSERT INTO st.collection_member SELECT ((o.id-1)%256)+1,o.id,0 FROM st.object_ref o;").unwrap();
    let (list, _) = ok(
        "collections.list",
        call(&svc, "collections.list", json!({})),
    );
    let all = list["collections"].as_array().unwrap();
    assert_eq!(all.len(), 256);
    assert_eq!(
        all.iter()
            .map(|c| c["member_count"].as_i64().unwrap())
            .sum::<i64>(),
        30000
    );
    for (i, c) in all.iter().enumerate() {
        assert_eq!(c["collection_id"], format!("co_{}", i + 1));
        assert_eq!(
            c["logical_bytes"],
            c["member_count"].as_i64().unwrap().to_string()
        );
    }
}
