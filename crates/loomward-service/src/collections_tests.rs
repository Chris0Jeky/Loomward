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
