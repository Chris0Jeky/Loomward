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
                    volume_guid: None,
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
fn collections_selected_batch_is_atomic_scoped_and_durable() {
    // Wire root IDs are sealed to a Service session. Compare every durable grant column across
    // reopen, and keep the full wire comparison within the original session below.
    let durable_grants = |svc: &Service| {
        let conn = svc.inner.db();
        let mut stmt = conn
            .prepare("SELECT * FROM st.root_grant ORDER BY id")
            .unwrap();
        let columns = stmt.column_count();
        let rows = stmt
            .query_map([], |row| {
                (0..columns)
                    .map(|column| row.get::<_, rusqlite::types::Value>(column))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap();
        rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
    };
    let (dir, svc, ids) = setup(DatasetClass::Synthetic);
    let path = dir.path().join("untouched-fixture.txt");
    std::fs::write(&path, b"Synthetic collection curation fixture").unwrap();
    let snapshot = || {
        (
            std::fs::read(&path).unwrap(),
            std::fs::metadata(&path).unwrap().modified().unwrap(),
        )
    };
    let file_before = snapshot();
    let a = create(&svc, "Curate");
    ok(
        "collections.update_members",
        update(&svc, &a, &ids, &[], svc.inner.revisions().unwrap().1),
    );
    let b = create(&svc, "Keep");
    ok(
        "collections.update_members",
        update(&svc, &b, &ids, &[], svc.inner.revisions().unwrap().1),
    );
    svc.inner
        .db()
        .execute_batch(
            "DELETE FROM main.file WHERE id=1; UPDATE main.revision SET catalog_rev=catalog_rev+1;",
        )
        .unwrap();
    let (before, meta) = ok("collections.members", members(&svc, &a, 200, Value::Null));
    assert_eq!(before["items"][0]["status"], "unresolved");
    assert_eq!(before["items"][1]["status"], "resolved");
    let other_before = ok("collections.members", members(&svc, &b, 200, Value::Null)).0;
    let grants = ok("grants.list", call(&svc, "grants.list", json!({}))).0;
    let durable_grants_before = durable_grants(&svc);
    let mut req = request(
        "collections.update_members",
        json!({"collection_id":a,"add":[],"remove":[],"remove_members":[before["items"][0]["member_id"],before["items"][1]["member_id"]]}),
    );
    req.expected_state_rev = Some(Generation(meta.state_rev.unwrap().get() - 1));
    let revisions = svc.inner.revisions().unwrap();
    assert_eq!(
        err(svc.call(req.clone(), &CallContext::http())).code,
        ErrorCode::StaleGeneration
    );
    assert_eq!(svc.inner.revisions().unwrap(), revisions);
    assert_eq!(
        ok("collections.members", members(&svc, &a, 200, Value::Null)).0,
        before
    );
    req.request_id = RequestId::new("curation_current").unwrap();
    req.expected_state_rev = meta.state_rev;
    let (updated, after_meta) = ok(
        "collections.update_members",
        svc.call(req, &CallContext::http()),
    );
    assert_eq!(updated["member_count"], 1);
    assert_eq!(after_meta.state_rev.unwrap().get(), revisions.1 as u64 + 1);
    assert_eq!(
        ok("collections.members", members(&svc, &b, 200, Value::Null)).0,
        other_before
    );
    assert_eq!(
        ok("grants.list", call(&svc, "grants.list", json!({}))).0,
        grants
    );
    assert_eq!(durable_grants(&svc), durable_grants_before);
    drop(svc);
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let remaining = ok("collections.members", members(&svc, &a, 200, Value::Null)).0;
    assert_eq!(remaining["total"], 1);
    assert_eq!(
        remaining["items"][0]["member_id"],
        before["items"][2]["member_id"]
    );
    let retained = ok("collections.members", members(&svc, &b, 200, Value::Null)).0;
    assert_eq!(retained["total"], 3);
    for i in 0..3 {
        assert_eq!(
            retained["items"][i]["member_id"],
            before["items"][i]["member_id"]
        );
    }
    assert_eq!(durable_grants(&svc), durable_grants_before);
    assert_eq!(snapshot(), file_before);
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
        repair_on_read: true,
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

// Decode dynamically so the tests-only checkpoint compiles on the pre-feature engine:
// absence of the operation is a behavioral assertion failure, not a compiler failure.
fn search_members(
    svc: &Service,
    id: &str,
    text: &str,
    limit: i64,
    cursor: Value,
) -> ResponseEnvelope {
    let parsed = serde_json::from_value::<RequestEnvelope>(json!({
        "protocol":"loomward/3", "request_id":format!("cs{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)),
        "deadline_ms":60_000,
        "command":"collections.search", "payload":{
            "collection_id":id,"text":text,"extension":null,"min_bytes":null,
            "max_bytes":null,"modified_after":null,"modified_before":null,
            "kind":"any","limit":limit,"cursor":cursor
        }
    }));
    assert!(
        parsed.is_ok(),
        "collections.search must be a supported owner-local read: {parsed:?}"
    );
    svc.call(parsed.unwrap(), &CallContext::http())
}

#[test]
fn collection_search_advertises_without_federated_search() {
    let (_dir, svc, _) = setup(DatasetClass::Synthetic);
    let (hello, _) = ok("session.hello", call(&svc, "session.hello", json!({})));
    assert_eq!(hello["features"]["federated_search"], false);
    assert_eq!(hello["features"]["collection_search"], true);
}

#[test]
fn collection_search_scopes_before_page_limit_and_binds_filters() {
    let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
    let a = create(&svc, "Search A");
    let b = create(&svc, "Search B");
    let rev = svc.inner.revisions().unwrap().1;
    ok(
        "collections.update_members",
        update(&svc, &a, &ids[1..], &[], rev),
    );
    let rev = svc.inner.revisions().unwrap().1;
    ok(
        "collections.update_members",
        update(&svc, &b, &ids[..1], &[], rev),
    );
    let (first, _) = ok(
        "collections.search",
        search_members(&svc, &a, "", 1, Value::Null),
    );
    assert_eq!(first["hits"].as_array().unwrap().len(), 1);
    assert_eq!(first["hits"][0]["row"]["node_id"], ids[1]);
    assert_eq!(first["total_members"], 2);
    let cursor = first["next_cursor"].clone();
    assert!(!cursor.is_null());
    let (second, _) = ok(
        "collections.search",
        search_members(&svc, &a, "", 1, cursor.clone()),
    );
    assert_eq!(second["hits"][0]["row"]["node_id"], ids[2]);
    assert!(second["next_cursor"].is_null());
    assert_eq!(
        err(search_members(&svc, &b, "", 1, cursor.clone())).code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        err(search_members(&svc, &a, "other", 1, cursor.clone())).code,
        ErrorCode::InvalidRequest
    );
    let rev = svc.inner.revisions().unwrap().1;
    ok(
        "collections.update_members",
        update(&svc, &a, &ids[..1], &[], rev),
    );
    assert_eq!(
        err(search_members(&svc, &a, "", 1, cursor)).code,
        ErrorCode::StaleGeneration
    );
    assert_eq!(
        err(search_members(&svc, "co_999999", "", 1, Value::Null)).code,
        ErrorCode::NotFound
    );
}

#[test]
fn collection_search_literal_predicates_and_direct_directory_only() {
    let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
    let id = create(&svc, "Literal");
    ok(
        "collections.update_members",
        update(&svc, &id, &ids, &[], 2),
    );
    svc.inner.db().execute_batch("UPDATE main.file SET name='Tiny_%[literal].TXT',modified_ft=134116992000000000 WHERE id=3; UPDATE main.revision SET catalog_rev=catalog_rev+1;").unwrap();
    let (page, _) = ok(
        "collections.search",
        search_members(&svc, &id, "_%[LITERAL]", 1, Value::Null),
    );
    assert_eq!(page["hits"][0]["row"]["node_id"], ids[2]);
    assert_eq!(page["examined_members"], 3);
    assert!(page["next_cursor"].is_null());
    let query = |extra: Value| {
        let mut payload = json!({"collection_id":id,"text":"", "extension":null,"min_bytes":null,"max_bytes":null,"modified_after":null,"modified_before":null,"kind":"any","limit":100,"cursor":null});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        call(&svc, "collections.search", payload)
    };
    for extra in [
        json!({"min_bytes":"1002"}),
        json!({"modified_after":"2026-01-01T00:00:00Z"}),
    ] {
        let (page, _) = ok("collections.search", query(extra));
        assert_eq!(page["hits"].as_array().unwrap().len(), 1);
        assert_eq!(page["hits"][0]["row"]["node_id"], ids[2]);
    }
    assert!(
        ok("collections.search", query(json!({"kind":"dir"}))).0["hits"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        ok("collections.search", query(json!({"extension":"pdf"}))).0["hits"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        err(query(json!({"min_bytes":"1002","max_bytes":"1"}))).code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(err(query(json!({"modified_after":"2026-01-02T00:00:00Z","modified_before":"2026-01-01T00:00:00Z"}))).code, ErrorCode::InvalidRequest);
    assert_eq!(
        err(query(json!({"root_id":null}))).code,
        ErrorCode::InvalidRequest
    );
    let folder = create(&svc, "Folder only");
    svc.inner.db().execute_batch("UPDATE main.dir SET file_id=CAST('folder-native-id' AS BLOB),created_ft=123 WHERE parent_id IS NOT NULL; UPDATE main.file SET dir_id=(SELECT id FROM main.dir WHERE parent_id IS NOT NULL LIMIT 1) WHERE id=1; UPDATE main.revision SET catalog_rev=catalog_rev+1;").unwrap();
    let folder_node = {
        let conn = svc.inner.db();
        let (row, born) = conn
            .query_row(
                "SELECT id,born_run FROM main.dir WHERE parent_id IS NOT NULL",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
            .unwrap();
        svc.inner
            .ids
            .seal(NodeKey::Dir(row), born, &svc.inner.instance)
    };
    let rev = svc.inner.revisions().unwrap().1;
    ok(
        "collections.update_members",
        update(&svc, &folder, std::slice::from_ref(&folder_node), &[], rev),
    );
    let (page, _) = ok(
        "collections.search",
        search_members(&svc, &folder, "", 100, Value::Null),
    );
    assert_eq!(page["hits"].as_array().unwrap().len(), 1);
    assert_eq!(page["hits"][0]["row"]["node_id"], folder_node);
    assert!(ok(
        "collections.search",
        search_members(&svc, &folder, "report", 100, Value::Null)
    )
    .0["hits"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn collection_search_unavailable_restart_catalogue_and_client_fences() {
    let (dir, svc, ids) = setup(DatasetClass::Synthetic);
    let id = create(&svc, "Durable search");
    ok(
        "collections.update_members",
        update(&svc, &id, &ids, &[], 2),
    );
    let (first, _) = ok(
        "collections.search",
        search_members(&svc, &id, "", 1, Value::Null),
    );
    svc.inner
        .db()
        .execute("UPDATE main.revision SET catalog_rev=catalog_rev+1", [])
        .unwrap();
    assert_eq!(
        reason(&err(search_members(
            &svc,
            &id,
            "",
            1,
            first["next_cursor"].clone()
        ))),
        Some("catalog_rev_changed")
    );
    let (first, _) = ok(
        "collections.search",
        search_members(&svc, &id, "", 1, Value::Null),
    );
    drop(svc);
    let svc = open(dir.path(), DatasetClass::Synthetic);
    assert_eq!(
        err(search_members(
            &svc,
            &id,
            "",
            1,
            first["next_cursor"].clone()
        ))
        .code,
        ErrorCode::StaleGeneration
    );
    assert_eq!(
        ok(
            "collections.search",
            search_members(&svc, &id, "", 100, Value::Null)
        )
        .0["hits"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let req = request(
        "collections.search",
        json!({"collection_id":id,"text":"","extension":null,"min_bytes":null,"max_bytes":null,"modified_after":null,"modified_before":null,"kind":"any","limit":1,"cursor":null}),
    );
    assert_eq!(
        err(svc.call(req, &CallContext::client("mcp:collection-test"))).code,
        ErrorCode::PermissionDenied
    );
    svc.inner
        .db()
        .execute_batch(
            "DELETE FROM main.file WHERE id=1; UPDATE main.revision SET catalog_rev=catalog_rev+1;",
        )
        .unwrap();
    let (page, _) = ok(
        "collections.search",
        search_members(&svc, &id, "", 100, Value::Null),
    );
    assert_eq!(page["total_members"], 3);
    assert_eq!(page["unavailable_members"], 1);
    assert_eq!(page["partial"], true);
    assert_eq!(page["hits"].as_array().unwrap().len(), 2);
    svc.inner.db().execute_batch("UPDATE st.root_grant SET state='revoked',revoked_at_ns=1; UPDATE st.revision SET state_rev=state_rev+1;").unwrap();
    let (page, _) = ok(
        "collections.search",
        search_members(&svc, &id, "", 100, Value::Null),
    );
    assert_eq!(page["unavailable_members"], 3);
    assert!(page["hits"].as_array().unwrap().is_empty());
    assert!(!serde_json::to_string(&page).unwrap().contains("report"));
    ok(
        "roots.forget",
        call(
            &svc,
            "roots.forget",
            json!({"root_id":svc.inner.root_id(1)}),
        ),
    );
    let (forgotten, _) = ok(
        "collections.search",
        search_members(&svc, &id, "", 100, Value::Null),
    );
    // The already-missing reference cannot be attributed to a current root; it remains an
    // unavailable durable member. Forgetting must never turn it into a global search.
    assert_eq!(forgotten["total_members"], 1);
    assert_eq!(forgotten["unavailable_members"], 1);
    assert!(forgotten["hits"].as_array().unwrap().is_empty());
}

#[test]
fn collection_search_bounded_empty_pages_make_progress_without_broadening() {
    let (_dir, svc, _) = setup(DatasetClass::Synthetic);
    let id = create(&svc, "Unavailable search");
    svc.inner.db().execute_batch("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<2001) INSERT INTO st.object_ref(volume_key,file_id,creation_ft,kind,state,incarnation,catalog_instance,row_id,born_run) SELECT 'fixture',CAST(printf('%016d',i) AS BLOB),123,'file','resolved',1,'old-instance',i,1 FROM n; INSERT INTO st.collection_member SELECT 1,id,0 FROM st.object_ref; UPDATE st.revision SET state_rev=state_rev+1;").unwrap();
    let (first, _) = ok(
        "collections.search",
        search_members(&svc, &id, "report", 100, Value::Null),
    );
    assert_eq!(first["total_members"], 2001);
    assert_eq!(first["examined_members"], 2000);
    assert_eq!(first["unavailable_members"], 2000);
    assert_eq!(first["budget_hit"], true);
    assert!(first["hits"].as_array().unwrap().is_empty());
    assert!(!first["next_cursor"].is_null());
    let (last, _) = ok(
        "collections.search",
        search_members(&svc, &id, "report", 100, first["next_cursor"].clone()),
    );
    assert_eq!(last["examined_members"], 1);
    assert_eq!(last["unavailable_members"], 1);
    assert_eq!(last["budget_hit"], false);
    assert!(last["next_cursor"].is_null());
}

#[test]
fn collection_search_rejects_publication_during_hydration_and_deadline_stops_work() {
    for expire in [false, true] {
        let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
        let id = create(&svc, "Concurrent search");
        ok(
            "collections.update_members",
            update(&svc, &id, &ids, &[], 2),
        );
        let svc = Arc::new(svc);
        let phase = if expire { "hydrate" } else { "hydrated" };
        svc.inner
            .hooks
            .closed
            .lock()
            .unwrap()
            .push((Command::CollectionsSearch, phase));
        let worker = svc.clone();
        let read = std::thread::spawn(move || search_members(&worker, &id, "", 100, Value::Null));
        svc.inner.hooks.until(phase, |h| {
            h.arrived
                .lock()
                .unwrap()
                .contains(&(Command::CollectionsSearch, phase))
        });
        if expire {
            *svc.inner.hooks.skew.lock().unwrap() = Duration::from_secs(61);
        } else {
            svc.inner
                .db()
                .execute("UPDATE main.revision SET catalog_rev=catalog_rev+1", [])
                .unwrap();
        }
        svc.inner.hooks.open(Command::CollectionsSearch, phase);
        let error = err(read.join().unwrap());
        assert_eq!(
            error.code,
            if expire {
                ErrorCode::DeadlineExceeded
            } else {
                ErrorCode::StaleGeneration
            }
        );
        svc.inner
            .hooks
            .until("collection search slot released", |_| {
                *svc.inner.reads.lock().unwrap() == 0
            });
    }
}

#[test]
fn collection_search_nonmatching_stale_members_keep_partial_coverage() {
    let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
    let id = create(&svc, "Stale nonmatches");
    ok(
        "collections.update_members",
        update(&svc, &id, &ids[..1], &[], 2),
    );
    // Synthetic metadata has complete rows but no live watcher. Pin both facts so the
    // regression cannot pass merely because row coverage or unavailable counts set partial.
    assert_eq!(
        svc.inner.hit_freshness(1).state,
        SearchFreshnessState::Stale
    );
    let (baseline, _) = ok(
        "collections.search",
        search_members(&svc, &id, "", 100, Value::Null),
    );
    assert_eq!(baseline["hits"][0]["row"]["coverage"], "complete");
    assert_eq!(baseline["hits"][0]["freshness"]["state"], "stale");
    for predicate in [
        json!({"text":"no-literal-match"}),
        json!({"kind":"dir"}),
        json!({"extension":"pdf"}),
        json!({"min_bytes":"999999"}),
        json!({"modified_after":"2099-01-01T00:00:00Z"}),
    ] {
        let mut payload = json!({"collection_id":id,"text":"","extension":null,"min_bytes":null,"max_bytes":null,"modified_after":null,"modified_before":null,"kind":"any","limit":100,"cursor":null});
        payload
            .as_object_mut()
            .unwrap()
            .extend(predicate.as_object().unwrap().clone());
        let (page, _) = ok(
            "collections.search",
            call(&svc, "collections.search", payload),
        );
        assert!(page["hits"].as_array().unwrap().is_empty(), "{predicate}");
        assert_eq!(page["examined_members"], 1);
        assert_eq!(page["unavailable_members"], 0);
        assert_eq!(page["budget_hit"], false);
        assert!(page["next_cursor"].is_null());
        assert_eq!(page["partial"], true, "stale nonmatch: {predicate}");
    }
}

#[test]
fn collection_search_retained_descendant_metadata_is_hidden_below_unavailable_ancestors() {
    let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
    let id = create(&svc, "Ancestor boundary");
    svc.inner
        .db()
        .execute(
            "UPDATE main.file SET dir_id=(SELECT id FROM main.dir WHERE parent_id IS NOT NULL LIMIT 1) WHERE id=1",
            [],
        )
        .unwrap();
    ok(
        "collections.update_members",
        update(&svc, &id, &ids[..1], &[], 2),
    );
    let (baseline, _) = ok(
        "collections.search",
        search_members(&svc, &id, "report", 100, Value::Null),
    );
    assert_eq!(baseline["hits"].as_array().unwrap().len(), 1);
    assert_eq!(baseline["hits"][0]["row"]["coverage"], "complete");
    // The immediate parent and file retain complete metadata. Only their grandparent changes;
    // checking the hydrated row's own coverage cannot enforce this boundary.
    for (outcome, flags, attrs) in [
        ("denied", 0, 0),
        ("excluded", 0, 0),
        ("unscanned", 0, 0),
        ("complete", 1 << 1, 0),
        ("complete", 1 << 3, 0),
        ("complete", 1 << 4, 0),
        ("complete", 1 << 5, 0),
        ("complete", 1 << 6, 0),
        ("complete", 0, 1024),
    ] {
        svc.inner
            .db()
            .execute(
                "UPDATE main.dir SET listing_state='incomplete',listing_outcome=?1,flags=?2,attrs=?3 WHERE parent_id IS NULL",
                rusqlite::params![outcome, flags, attrs],
            )
            .unwrap();
        svc.inner
            .db()
            .execute("UPDATE main.revision SET catalog_rev=catalog_rev+1", [])
            .unwrap();
        let (page, _) = ok(
            "collections.search",
            search_members(&svc, &id, "report", 100, Value::Null),
        );
        assert!(
            page["hits"].as_array().unwrap().is_empty(),
            "{outcome}/{flags}/{attrs}"
        );
        assert_eq!(page["total_members"], 1);
        assert_eq!(page["examined_members"], 1);
        assert_eq!(page["unavailable_members"], 1);
        assert_eq!(page["partial"], true);
        assert!(page["next_cursor"].is_null());
        let wire = serde_json::to_string(&page).unwrap();
        assert!(!wire.contains("report"));
        assert!(!wire.contains("Synthetic"));
    }
}

#[test]
fn collection_search_does_not_repair_dirty_members_or_unrelated_roots() {
    let (_dir, svc, ids) = setup(DatasetClass::Synthetic);
    let id = create(&svc, "Metadata only");
    svc.inner
        .db()
        .execute_batch("UPDATE main.dir SET file_id=CAST('folder-native-id' AS BLOB),created_ft=123 WHERE parent_id IS NOT NULL;")
        .unwrap();
    let directory = {
        let conn = svc.inner.db();
        let (row, born) = conn
            .query_row(
                "SELECT id,born_run FROM main.dir WHERE parent_id IS NOT NULL",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
            .unwrap();
        svc.inner
            .ids
            .seal(NodeKey::Dir(row), born, &svc.inner.instance)
    };
    ok(
        "collections.update_members",
        update(&svc, &id, &[ids[0].clone(), directory.clone()], &[], 2),
    );
    let WriteReply::Root {
        grant_id, dir_id, ..
    } = svc
        .inner
        .catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "unrelated-fixture".into(),
            display_name: "Unrelated synthetic root".into(),
            display_path: "Unrelated synthetic root".into(),
            root_file_id: Some(vec![2; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))
        .unwrap()
    else {
        panic!()
    };
    relist(&svc, grant_id, dir_id, 1);
    svc.inner
        .db()
        .execute_batch("UPDATE main.dir SET dirty_rev=agg_valid_rev+100; UPDATE main.revision SET catalog_rev=catalog_rev+1;")
        .unwrap();
    let aggregates = || {
        let conn = svc.inner.db();
        let mut statement = conn
            .prepare(
                "SELECT id,dirty_rev,agg_valid_rev,sub_logical,sub_files FROM main.dir ORDER BY id",
            )
            .unwrap();
        statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    let before = aggregates();
    let revisions = svc.inner.revisions().unwrap();
    let (page, _) = ok(
        "collections.search",
        search_members(&svc, &id, "", 100, Value::Null),
    );
    assert_eq!(page["hits"].as_array().unwrap().len(), 2);
    let folder = page["hits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hit| hit["row"]["node_id"] == directory)
        .unwrap();
    assert_eq!(folder["row"]["coverage"], "stale");
    assert_eq!(page["partial"], true);
    assert_eq!(aggregates(), before, "search must not repair any aggregate");
    assert_eq!(
        svc.inner.revisions().unwrap(),
        revisions,
        "search must not publish repairs"
    );
}
