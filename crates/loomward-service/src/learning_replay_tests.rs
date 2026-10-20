use super::*;

fn current_node(svc: &Service, row: i64) -> String {
    let born = svc
        .inner
        .db()
        .query_row("SELECT born_run FROM main.file WHERE id=?1", [row], |r| {
            r.get(0)
        })
        .unwrap();
    svc.inner.ids.seal(
        loomward_catalog::NodeKey::File(row),
        born,
        &svc.inner.instance,
    )
}

fn durable_counts(svc: &Service) -> [i64; 6] {
    [
        count(svc, "SELECT state_rev FROM st.revision"),
        count(svc, "SELECT catalog_rev FROM main.revision"),
        count(svc, "SELECT count(*) FROM st.human_feedback"),
        count(svc, "SELECT count(*) FROM st.learning_receipt"),
        count(svc, "SELECT count(*) FROM st.object_ref"),
        count(svc, "SELECT count(*) FROM st.learning_node"),
    ]
}

fn replay_after_restart(retract: bool) {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let old_node = learning_node(&svc);
    let label = feedback(&old_node, "ce_rebound_label", Some("Documents"), false);
    let (label_result, _) = ok(
        "feedback.record",
        call(&svc, "feedback.record", label.clone()),
    );
    let (request, original) = if retract {
        let request = feedback(&old_node, "ce_rebound_retract", None, true);
        let (result, _) = ok(
            "feedback.record",
            call(&svc, "feedback.record", request.clone()),
        );
        (request, result)
    } else {
        (label, label_result)
    };
    // Model the commit/reply crash window: only the durable receipt survives the service.
    drop(svc);
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let new_node = current_node(&svc, 1);
    assert_ne!(
        new_node, old_node,
        "the fixture must really rotate the node ID"
    );
    let before = durable_counts(&svc);
    let mut events = svc.subscribe(None);
    let mut remapped = request.clone();
    remapped["node_id"] = json!(new_node);
    let (replay, _) = ok("feedback.record", call(&svc, "feedback.record", remapped));
    let mut expected = original;
    expected["idempotent_replay"] = json!(true);
    assert_eq!(
        replay, expected,
        "the complete original receipt must survive"
    );
    let (exact, _) = ok("feedback.record", call(&svc, "feedback.record", request));
    assert_eq!(
        exact, expected,
        "the old opaque ID still supports exact replay"
    );
    assert_eq!(
        durable_counts(&svc),
        before,
        "replay must not write any state"
    );
    while let RecvOutcome::Event(event) = events.recv_timeout(Duration::ZERO) {
        assert_ne!(event.event, EventName::LearningUpdated);
    }
}

#[test]
fn learning_rebound_label_returns_the_receipt_after_restart() {
    replay_after_restart(false);
}

#[test]
fn learning_rebound_retraction_returns_the_receipt_after_restart() {
    replay_after_restart(true);
}

#[test]
fn learning_rebound_key_refuses_other_objects_and_changed_content_without_writes() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let node = learning_node(&svc);
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_rebound_conflict", Some("Documents"), false),
        ),
    );
    drop(svc);
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let same = current_node(&svc, 1);
    let other = current_node(&svc, 2);
    let before = durable_counts(&svc);
    let mut events = svc.subscribe(None);
    for request in [
        feedback(&other, "ce_rebound_conflict", Some("Documents"), false),
        feedback(&same, "ce_rebound_conflict", Some("Archive"), false),
        feedback(&same, "ce_rebound_conflict", None, true),
    ] {
        assert_eq!(
            err(call(&svc, "feedback.record", request)).code,
            ErrorCode::InvalidRequest
        );
        assert_eq!(durable_counts(&svc), before);
    }
    while let RecvOutcome::Event(event) = events.recv_timeout(Duration::ZERO) {
        assert_ne!(event.event, EventName::LearningUpdated);
    }
}

#[test]
fn learning_rebound_key_requires_a_current_resolved_unrevoked_binding() {
    for refusal in ["unresolved", "retired", "pending", "creation", "revoked"] {
        let dir = temp_root();
        let svc = open(dir.path(), DatasetClass::Synthetic);
        let old = learning_node(&svc);
        let request = feedback(&old, "ce_rebound_refusal", Some("Documents"), false);
        let (original, _) = ok(
            "feedback.record",
            call(&svc, "feedback.record", request.clone()),
        );
        drop(svc);
        let svc = open(dir.path(), DatasetClass::Synthetic);
        let new_node = current_node(&svc, 1);
        match refusal {
            "unresolved" => {
                svc.inner
                    .db()
                    .execute_batch("UPDATE st.object_ref SET state='unresolved',row_id=NULL")
                    .unwrap();
            }
            "retired" => {
                svc.inner
                    .db()
                    .execute_batch("UPDATE st.object_ref SET retired=1")
                    .unwrap();
            }
            "pending" => {
                svc.inner.db().execute_batch("INSERT OR REPLACE INTO main.meta VALUES('reference_publication','replay_retirement'); INSERT INTO st.pending_reference(object_ref_id,catalog_instance,publication_token) SELECT id,catalog_instance,'replay_retirement' FROM st.object_ref").unwrap();
            }
            "creation" => {
                svc.inner
                    .db()
                    .execute_batch("UPDATE main.file SET created_ft=created_ft+1 WHERE id=1")
                    .unwrap();
            }
            "revoked" => {
                ok(
                    "roots.revoke",
                    call(
                        &svc,
                        "roots.revoke",
                        json!({"root_id":svc.inner.root_id(1),"purge_catalog":false}),
                    ),
                );
            }
            _ => unreachable!(),
        }
        let before = durable_counts(&svc);
        let mut remapped = request.clone();
        remapped["node_id"] = json!(new_node);
        let error = err(call(&svc, "feedback.record", remapped));
        assert!(
            matches!(error.code, ErrorCode::InvalidRequest | ErrorCode::NotFound),
            "{refusal}: {error:?}"
        );
        let (exact, _) = ok("feedback.record", call(&svc, "feedback.record", request));
        let mut expected = original;
        expected["idempotent_replay"] = json!(true);
        assert_eq!(exact, expected, "{refusal}: exact replay is still durable");
        assert_eq!(durable_counts(&svc), before, "{refusal}");
    }
}

#[test]
fn learning_rebound_key_uses_the_published_reference_binding() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let old = learning_node(&svc);
    let request = feedback(&old, "ce_rebound_published", Some("Documents"), false);
    let (original, _) = ok(
        "feedback.record",
        call(&svc, "feedback.record", request.clone()),
    );
    drop(svc);
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let node = current_node(&svc, 1);
    // A committed catalogue publication may still await folding into state.db.
    svc.inner.db().execute_batch("INSERT OR REPLACE INTO main.meta VALUES('reference_publication','replay_rebind'); INSERT INTO st.pending_reference(object_ref_id,catalog_instance,publication_token,row_id,born_run) SELECT id,catalog_instance,'replay_rebind',row_id,born_run FROM st.object_ref; UPDATE st.object_ref SET row_id=99999,born_run=99999").unwrap();
    let before = durable_counts(&svc);
    let mut remapped = request;
    remapped["node_id"] = json!(node);
    let (replay, _) = ok("feedback.record", call(&svc, "feedback.record", remapped));
    let mut expected = original;
    expected["idempotent_replay"] = json!(true);
    assert_eq!(replay, expected);
    assert_eq!(durable_counts(&svc), before);
}

#[test]
fn learning_rebound_legacy_without_snapshot_provenance_keeps_exact_replay_only() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let old = learning_node(&svc);
    let request = feedback(&old, "ce_rebound_legacy", Some("Documents"), false);
    let (original, _) = ok(
        "feedback.record",
        call(&svc, "feedback.record", request.clone()),
    );
    svc.inner
        .db()
        .execute_batch("DELETE FROM st.learning_feature_grant; DELETE FROM st.learning_node")
        .unwrap();
    drop(svc);
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let before = durable_counts(&svc);
    let mut remapped = request.clone();
    remapped["node_id"] = json!(current_node(&svc, 1));
    assert_eq!(
        err(call(&svc, "feedback.record", remapped)).code,
        ErrorCode::InvalidRequest
    );
    let (exact, _) = ok("feedback.record", call(&svc, "feedback.record", request));
    let mut expected = original;
    expected["idempotent_replay"] = json!(true);
    assert_eq!(exact, expected);
    assert_eq!(durable_counts(&svc), before);
}

#[test]
fn learning_rebound_missing_receipt_never_creates_another_reference_or_event() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let old = learning_node(&svc);
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&old, "ce_rebound_no_receipt", Some("Documents"), false),
        ),
    );
    svc.inner
        .db()
        .execute_batch("DELETE FROM st.learning_receipt")
        .unwrap();
    drop(svc);
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let before = durable_counts(&svc);
    let other = current_node(&svc, 2);
    err(call(
        &svc,
        "feedback.record",
        feedback(&other, "ce_rebound_no_receipt", Some("Documents"), false),
    ));
    assert_eq!(durable_counts(&svc), before);
}

fn replay_with_committed_dirty_root(same_target: bool) {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let old = learning_node(&svc);
    let request = feedback(&old, "ce_rebound_dirty", Some("Documents"), false);
    let (original, _) = ok(
        "feedback.record",
        call(&svc, "feedback.record", request.clone()),
    );
    drop(svc);
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let node = current_node(&svc, if same_target { 1 } else { 2 });
    // Ordinary Reader methods repair this committed root before inspecting any node. Replaying
    // a receipt must not take that path, even when the eventual target comparison will fail.
    svc.inner
        .db()
        .execute_batch(
            "UPDATE main.dir SET dirty_rev=agg_valid_rev+1 WHERE parent_id IS NULL",
        )
        .unwrap();
    let needs_repair = "SELECT count(*) FROM main.dir d JOIN main.root r ON r.id=d.root_id
        WHERE d.parent_id IS NULL AND d.agg_valid_rev<d.dirty_rev
          AND r.active_run IS NULL AND r.state!='repairing'";
    assert_eq!(count(&svc, needs_repair), 1);
    let before = durable_counts(&svc);
    let mut events = svc.subscribe(None);
    let mut remapped = request;
    remapped["node_id"] = json!(node);
    let reply = call(&svc, "feedback.record", remapped);
    if same_target {
        let (replay, _) = ok("feedback.record", reply);
        let mut expected = original;
        expected["idempotent_replay"] = json!(true);
        assert_eq!(replay, expected);
    } else {
        assert_eq!(err(reply).code, ErrorCode::InvalidRequest);
    }
    assert_eq!(durable_counts(&svc), before);
    assert_eq!(count(&svc, needs_repair), 1, "replay must not repair the root");
    while let RecvOutcome::Event(event) = events.recv_timeout(Duration::ZERO) {
        assert!(!matches!(
            event.event,
            EventName::LearningUpdated | EventName::TreeInvalidated | EventName::RootsChanged
        ));
    }
}

#[test]
fn learning_rebound_dirty_root_returns_the_receipt_without_repair() {
    replay_with_committed_dirty_root(true);
}

#[test]
fn learning_rebound_dirty_root_refuses_another_target_without_repair() {
    replay_with_committed_dirty_root(false);
}

#[test]
fn learning_rebound_key_preserves_visibility_and_exclusion_guards() {
    for guard in [
        "absent_parent",
        "absent_ancestor",
        "excluded_ancestor",
        "excluded_target",
        "incarnation",
        "hardlink",
    ] {
        let dir = temp_root();
        let svc = open(dir.path(), DatasetClass::Synthetic);
        let old = learning_node(&svc);
        let request = feedback(&old, "ce_rebound_guard", Some("Documents"), false);
        let (original, _) = ok(
            "feedback.record",
            call(&svc, "feedback.record", request.clone()),
        );
        drop(svc);
        let svc = open(dir.path(), DatasetClass::Synthetic);
        let node = current_node(&svc, 1);
        let setup = match guard {
            "absent_parent" => {
                "UPDATE main.dir SET listing_state='absent_pending' WHERE id=1"
            }
            "absent_ancestor" => {
                "UPDATE main.file SET dir_id=(SELECT id FROM main.dir WHERE parent_id=1 LIMIT 1) WHERE id=1;
                 UPDATE main.dir SET listing_state='absent_pending' WHERE id=1"
            }
            "excluded_ancestor" => {
                "UPDATE main.file SET dir_id=(SELECT id FROM main.dir WHERE parent_id=1 LIMIT 1) WHERE id=1;
                 UPDATE main.dir SET listing_state='excluded' WHERE id=1"
            }
            "excluded_target" => "UPDATE main.file SET flags=1 WHERE id=1",
            "incarnation" => "UPDATE main.file SET born_run=born_run+1 WHERE id=1",
            "hardlink" => "UPDATE main.file SET flags=4 WHERE id=1",
            _ => unreachable!(),
        };
        svc.inner.db().execute_batch(setup).unwrap();
        let before = durable_counts(&svc);
        let mut remapped = request.clone();
        remapped["node_id"] = json!(node);
        let reply = call(&svc, "feedback.record", remapped);
        let mut expected = original;
        expected["idempotent_replay"] = json!(true);
        if guard == "hardlink" {
            let (replay, _) = ok("feedback.record", reply);
            assert_eq!(replay, expected);
        } else {
            let error = err(reply);
            assert!(
                matches!(error.code, ErrorCode::InvalidRequest | ErrorCode::NotFound),
                "{guard}: {error:?}"
            );
        }
        let (exact, _) = ok("feedback.record", call(&svc, "feedback.record", request));
        assert_eq!(exact, expected, "{guard}: exact replay is still durable");
        assert_eq!(durable_counts(&svc), before, "{guard}");
    }
}
