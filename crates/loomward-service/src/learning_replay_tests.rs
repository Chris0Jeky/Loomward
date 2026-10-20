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
