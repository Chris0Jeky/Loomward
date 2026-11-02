use super::*;

fn queue(svc: &Service, mode: &str, limit: usize, cursor: Value) -> ResponseEnvelope {
    call(
        svc,
        "learning.queue",
        json!({"root_id":null,"limit":limit,"cursor":cursor,"mode":mode}),
    )
}
fn choose(svc: &Service, node: &str, deferred: bool, key: &str, revision: i64) -> ResponseEnvelope {
    let mut req = request(
        "learning.set_deferred",
        json!({"node_id":node,"deferred":deferred,"client_event_id":key}),
    );
    req.expected_state_rev = Some(Generation(revision as u64));
    svc.call(req, &CallContext::http())
}
fn set(svc: &Service, node: &str, deferred: bool, key: &str) -> Payload {
    let rev = svc.inner.revisions().unwrap().1;
    ok(
        "learning.set_deferred",
        choose(svc, node, deferred, key, rev),
    )
    .0
}
fn table(svc: &Service, name: &str) -> Vec<Vec<rusqlite::types::Value>> {
    let db = svc.inner.db();
    let mut statement = db
        .prepare(&format!("SELECT * FROM st.{name} ORDER BY 1"))
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns).map(|column| row.get(column)).collect()
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn review_deferral_survives_restart_replays_and_preserves_learning() {
    let tmp = temp_root();
    let original = tmp.path().join("original.bin");
    std::fs::write(&original, b"unchanged preference fixture").unwrap();
    let before_file = (
        std::fs::read(&original).unwrap(),
        std::fs::metadata(&original).unwrap().modified().unwrap(),
    );
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let node = learning_node(&svc);
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_defer_label", Some("Documents"), false),
        ),
    );
    let learning = finish_refit(&svc);
    let tables = [
        "human_feedback",
        "student_model",
        "teacher_label",
        "root_grant",
    ];
    let before = tables.map(|name| table(&svc, name));
    let revision = svc.inner.revisions().unwrap().1;
    let original_receipt = ok(
        "learning.set_deferred",
        choose(&svc, &node, true, "ce_defer_restart", revision),
    )
    .0;
    assert_eq!(original_receipt["idempotent_replay"], false);
    let review = ok("learning.queue", queue(&svc, "review", 100, Value::Null)).0;
    assert_eq!(review["items"].as_array().unwrap().len(), 2);
    assert!(review["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["row"]["node_id"] != node));
    let deferred = ok("learning.queue", queue(&svc, "deferred", 100, Value::Null)).0;
    assert_eq!(deferred["items"].as_array().unwrap().len(), 1);
    assert_eq!(deferred["items"][0]["row"]["node_id"], node);
    assert_eq!(tables.map(|name| table(&svc, name)), before);
    drop(svc);
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let deferred = ok("learning.queue", queue(&svc, "deferred", 100, Value::Null)).0;
    let new_node = deferred["items"][0]["row"]["node_id"].as_str().unwrap();
    assert_ne!(node, new_node, "restart must rotate the session ID");
    let before_replay = svc.inner.revisions().unwrap();
    let mut expected = original_receipt;
    expected["idempotent_replay"] = json!(true);
    for id in [node.as_str(), new_node] {
        assert_eq!(
            ok(
                "learning.set_deferred",
                choose(&svc, id, true, "ce_defer_restart", revision)
            )
            .0,
            expected
        );
    }
    assert_eq!(svc.inner.revisions().unwrap(), before_replay);
    assert_eq!(
        count(&svc, "SELECT count(*) FROM st.learning_deferred_receipt"),
        1
    );
    assert_eq!(
        err(choose(&svc, new_node, false, "ce_defer_restart", revision)).code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        err(choose(
            &svc,
            new_node,
            true,
            "ce_defer_restart",
            revision + 1
        ))
        .code,
        ErrorCode::InvalidRequest
    );
    set(&svc, new_node, false, "ce_defer_restore");
    assert!(
        ok("learning.queue", queue(&svc, "deferred", 100, Value::Null)).0["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        ok("learning.queue", queue(&svc, "review", 100, Value::Null)).0["items"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(tables.map(|name| table(&svc, name)), before);
    assert_eq!(
        ok("learning.status", call(&svc, "learning.status", json!({}))).0,
        learning
    );
    assert_eq!(
        (
            std::fs::read(&original).unwrap(),
            std::fs::metadata(&original).unwrap().modified().unwrap()
        ),
        before_file
    );
    // Existing label correction/withdrawal/refit remain independent of the preference.
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(new_node, "ce_after_defer", None, true),
        ),
    );
    assert_eq!(finish_refit(&svc)["training_count"], 0);
}

#[test]
fn review_deferral_stale_and_ineligible_requests_leave_no_partial_state() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let node = learning_node(&svc);
    let rev = svc.inner.revisions().unwrap();
    let request_payload =
        json!({"node_id":node,"deferred":true,"client_event_id":"ce_defer_invalid"});
    assert_eq!(
        err(call(&svc, "learning.set_deferred", request_payload.clone())).code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        reason(&err(choose(&svc, &node, true, "ce_defer_stale", rev.1 + 1))),
        Some("state_rev_changed")
    );
    svc.inner
        .db()
        .execute("UPDATE main.dir SET listing_state='excluded'", [])
        .unwrap();
    assert_eq!(
        err(choose(&svc, &node, true, "ce_defer_hidden", rev.1)).code,
        ErrorCode::PermissionDenied
    );
    for name in [
        "object_ref",
        "learning_deferred",
        "learning_deferred_receipt",
    ] {
        assert!(table(&svc, name).is_empty(), "partial write in {name}");
    }
    assert_eq!(svc.inner.revisions().unwrap(), rev);
    let mut req = request("learning.set_deferred", request_payload);
    req.expected_state_rev = Some(Generation(rev.1 as u64));
    assert_eq!(
        err(svc.call(req, &CallContext::client("mcp:review"))).code,
        ErrorCode::PermissionDenied
    );
    assert_eq!(
        err(svc.call(
            request(
                "learning.queue",
                json!({"root_id":null,"limit":1,"cursor":null,"mode":"deferred"})
            ),
            &CallContext::client("mcp:review")
        ))
        .code,
        ErrorCode::PermissionDenied
    );
    assert_eq!(
        Command::LearningSetDeferred.adapters(),
        &[Adapter::Tauri, Adapter::Http]
    );
}

#[test]
fn review_deferral_pages_bind_mode_root_revisions_and_session() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    learning_node(&svc);
    let first = ok("learning.queue", queue(&svc, "review", 1, Value::Null)).0;
    let cursor = first["next_cursor"].clone();
    assert_eq!(
        err(queue(&svc, "deferred", 1, cursor.clone())).code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        err(call(
            &svc,
            "learning.queue",
            json!({"root_id":svc.inner.root_id(1),"limit":1,"cursor":cursor})
        ))
        .code,
        ErrorCode::InvalidRequest
    );
    let second = ok("learning.queue", queue(&svc, "review", 1, cursor.clone())).0;
    assert_ne!(
        first["items"][0]["row"]["node_id"],
        second["items"][0]["row"]["node_id"]
    );
    set(
        &svc,
        first["items"][0]["row"]["node_id"].as_str().unwrap(),
        true,
        "ce_defer_page_1",
    );
    assert_eq!(
        err(queue(&svc, "review", 1, cursor)).code,
        ErrorCode::StaleGeneration
    );
    let review = ok("learning.queue", queue(&svc, "review", 100, Value::Null)).0;
    for (i, item) in review["items"].as_array().unwrap().iter().enumerate() {
        set(
            &svc,
            item["row"]["node_id"].as_str().unwrap(),
            true,
            &format!("ce_defer_all_{i}"),
        );
    }
    assert!(
        ok("learning.queue", queue(&svc, "review", 100, Value::Null)).0["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let first = ok("learning.queue", queue(&svc, "deferred", 1, Value::Null)).0;
    let second = ok(
        "learning.queue",
        queue(&svc, "deferred", 1, first["next_cursor"].clone()),
    )
    .0;
    assert_ne!(first["items"][0], second["items"][0]);
    let cursor = second["next_cursor"].clone();
    svc.inner
        .db()
        .execute("UPDATE main.revision SET catalog_rev=catalog_rev+1", [])
        .unwrap();
    assert_eq!(
        err(queue(&svc, "deferred", 1, cursor)).code,
        ErrorCode::StaleGeneration
    );
    let cursor =
        ok("learning.queue", queue(&svc, "deferred", 1, Value::Null)).0["next_cursor"].clone();
    drop(svc);
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    assert_eq!(
        err(queue(&svc, "deferred", 1, cursor)).code,
        ErrorCode::StaleGeneration
    );
}

#[test]
fn review_deferral_never_transfers_to_unproven_or_reused_references() {
    for change in [
        "UPDATE st.object_ref SET state='unresolved',retired=1",
        "UPDATE st.object_ref SET catalog_instance='different'",
        "UPDATE main.file SET born_run=born_run+1",
        "UPDATE main.file SET created_ft=created_ft+1",
        "UPDATE main.file SET file_id=cast('different-native' AS BLOB)",
        "UPDATE main.dir SET listing_state='excluded'",
        "INSERT OR REPLACE INTO main.meta VALUES('reference_publication','defer_retirement'); INSERT INTO st.pending_reference(object_ref_id,catalog_instance,publication_token) SELECT id,catalog_instance,'defer_retirement' FROM st.object_ref",
    ] {
        let tmp = temp_root();
        let svc = open(tmp.path(), DatasetClass::Synthetic);
        let node = learning_node(&svc);
        set(&svc, &node, true, "ce_defer_identity");
        svc.inner.db().execute_batch(change).unwrap();
        let page = ok("learning.queue", queue(&svc, "deferred", 100, Value::Null)).0;
        assert!(page["items"].as_array().unwrap().is_empty(), "{change}");
    }
    // The effective binding, not an obsolete row pointer, retains a proven preference.
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let node = learning_node(&svc);
    set(&svc, &node, true, "ce_defer_pending");
    svc.inner.db().execute_batch("INSERT OR REPLACE INTO main.meta VALUES('reference_publication','defer_rebind'); INSERT INTO st.pending_reference(object_ref_id,catalog_instance,publication_token,row_id,born_run) SELECT id,catalog_instance,'defer_rebind',row_id,born_run FROM st.object_ref; UPDATE st.object_ref SET row_id=99999,born_run=99999").unwrap();
    assert_eq!(
        ok("learning.queue", queue(&svc, "deferred", 100, Value::Null)).0["items"][0]["row"]
            ["node_id"],
        node
    );
}

#[test]
fn review_deferral_revocation_and_forget_fence_pending_names_and_purge_preferences() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let node = learning_node(&svc);
    set(&svc, &node, true, "ce_defer_forget");
    let pending: Vec<_> = ["review", "deferred"]
        .map(|mode| {
            svc.call_pending(
                request(
                    "learning.queue",
                    json!({"root_id":null,"limit":20,"cursor":null,"mode":mode}),
                ),
                &CallContext::http(),
            )
        })
        .into();
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id":svc.inner.root_id(1),"purge_catalog":false}),
        ),
    );
    for response in pending {
        assert_eq!(reason(&err(response.admit())), Some("forgotten_result"));
    }
    assert!(table(&svc, "learning_deferred").is_empty());
    assert!(
        ok("learning.queue", queue(&svc, "deferred", 20, Value::Null)).0["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    ok(
        "roots.forget",
        call(
            &svc,
            "roots.forget",
            json!({"root_id":svc.inner.root_id(1)}),
        ),
    );
    for name in [
        "object_ref",
        "learning_deferred",
        "learning_deferred_receipt",
    ] {
        assert!(table(&svc, name).is_empty(), "forget retained {name}");
    }
    drop(svc);
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    assert!(
        ok("learning.queue", queue(&svc, "deferred", 20, Value::Null)).0["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn review_deferral_filters_before_the_candidate_budget() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    fixture(&svc, 10_002);
    svc.inner.db().execute_batch("UPDATE main.file SET file_id=cast(printf('%016d',id) AS BLOB),created_ft=1;
        INSERT INTO st.object_ref(volume_key,file_id,creation_ft,kind,state,last_resolved_ns,incarnation,catalog_instance,row_id,born_run)
        SELECT v.volume_key,f.file_id,f.created_ft,'file','resolved',0,1,(SELECT value FROM main.meta WHERE key='instance_id'),f.id,f.born_run
        FROM main.file f JOIN main.dir d ON d.id=f.dir_id JOIN main.root rt ON rt.id=d.root_id JOIN main.volume v ON v.id=rt.volume_id WHERE f.id<10002;
        INSERT INTO st.learning_deferred SELECT o.id,rt.grant_id FROM st.object_ref o JOIN main.file f ON f.id=o.row_id JOIN main.dir d ON d.id=f.dir_id JOIN main.root rt ON rt.id=d.root_id").unwrap();
    assert_eq!(
        ok("learning.queue", queue(&svc, "review", 100, Value::Null)).0["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        err(queue(&svc, "deferred", 100, Value::Null)).code,
        ErrorCode::ResourceBudget
    );
}
