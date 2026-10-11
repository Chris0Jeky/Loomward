use crate::tests::*;

fn learning_node(svc: &Service) -> String {
    fixture(svc, 3);
    let conn = svc.inner.db();
    conn.execute_batch(
        "UPDATE main.file SET file_id=cast(printf('%016d',id) AS BLOB),created_ft=1",
    )
    .unwrap();
    let id: i64 = conn
        .query_row("SELECT min(id) FROM main.file", [], |r| r.get(0))
        .unwrap();
    let born: i64 = conn
        .query_row("SELECT born_run FROM main.file WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    svc.inner.ids.seal(
        loomward_catalog::NodeKey::File(id),
        born,
        &svc.inner.instance,
    )
}

pub(super) fn feedback(node: &str, key: &str, label: Option<&str>, retract: bool) -> Value {
    json!({"node_id":node,"client_event_id":key,"label":label,"retract":retract})
}

pub(super) fn finish_refit(svc: &Service) -> Payload {
    let (job, _) = ok("learning.refit", call(svc, "learning.refit", json!({})));
    let id: JobId = serde_json::from_value(job["job"]["job_id"].clone()).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let job = svc.inner.engine.job_status(&id).unwrap();
        if matches!(
            job.state,
            JobState::Completed | JobState::Cancelled | JobState::Failed
        ) {
            assert_eq!(job.state, JobState::Completed);
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
    ok("learning.status", call(svc, "learning.status", json!({}))).0
}

#[test]
fn learning_refuses_nondurable_invalid_and_unresolved_labels() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let node = learning_node(&svc);
    svc.inner
        .db()
        .execute("UPDATE main.file SET file_id=NULL WHERE id=1", [])
        .unwrap();
    assert_eq!(
        reason(&err(call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_no_id", Some("Documents"), false)
        ))),
        Some("durable_reference_unavailable")
    );
    svc.inner
        .db()
        .execute(
            "UPDATE main.file SET file_id=cast('1111111111111111' AS BLOB) WHERE id=1",
            [],
        )
        .unwrap();
    for p in [
        feedback(&node, "ce_empty_label", None, false),
        feedback(&node, "ce_bad_retract", Some("Documents"), true),
        feedback(&node, "ce_no_prior", None, true),
        feedback(&node, "ce_wrong_label", Some("UnknownLabel"), false),
    ] {
        assert_eq!(
            err(call(&svc, "feedback.record", p)).code,
            ErrorCode::InvalidRequest
        );
    }
    assert_eq!(
        svc.inner
            .db()
            .query_row("SELECT count(*) FROM st.object_ref", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_resolved", Some("Documents"), false),
        ),
    );
    svc.inner.db().execute_batch("INSERT OR REPLACE INTO main.meta VALUES('reference_publication','learning_retirement');INSERT INTO st.pending_reference(object_ref_id,catalog_instance,publication_token) SELECT id,catalog_instance,'learning_retirement' FROM st.object_ref;UPDATE st.revision SET state_rev=state_rev+1").unwrap();
    let status = finish_refit(&svc);
    assert_eq!(status["training_count"], 0);
    assert_eq!(status["unresolved_human_events"], 1);
    svc.inner
        .db()
        .execute("DELETE FROM st.pending_reference", [])
        .unwrap();
    svc.inner.db().execute_batch("UPDATE st.object_ref SET state='unresolved',retired=1;UPDATE st.revision SET state_rev=state_rev+1").unwrap();
    let status = finish_refit(&svc);
    assert_eq!(status["training_count"], 0);
    assert_eq!(status["unresolved_human_events"], 1);
}

#[test]
fn learning_timed_out_feedback_precedes_refit_in_fifo() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let node = learning_node(&svc);
    *svc.inner.hooks.delay.lock().unwrap() =
        Some((Command::FeedbackRecord, Duration::from_millis(300)));
    let mut req = request(
        "feedback.record",
        feedback(&node, "ce_deadline", Some("Documents"), false),
    );
    req.deadline_ms = Some(Int::new(5).unwrap());
    let error = err(svc.call(req, &CallContext::http()));
    assert_eq!(error.code, ErrorCode::DeadlineExceeded);
    assert!(!error.retryable);
    let status = finish_refit(&svc);
    assert_eq!(status["training_count"], 1);
    assert_eq!(status["human_events"], 1);
}

#[test]
fn learning_service_can_cancel_refit_jobs() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    svc.inner.engine.register_job_runner(
        JobKind::Refit,
        Arc::new(|ctx, _| {
            let until = Instant::now() + Duration::from_secs(2);
            while !ctx.is_cancelled() {
                assert!(Instant::now() < until);
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(CoverageState::Cancelled)
        }),
    );
    let job = svc
        .inner
        .engine
        .job_submit(loomward_engine::jobs::JobSpec::new(JobKind::Refit, None))
        .unwrap();
    assert_eq!(
        err(call(&svc, "scan.cancel", json!({"job_id":job.job_id}))).code,
        ErrorCode::InvalidRequest
    );
    let (cancel, _) = ok(
        "jobs.cancel",
        call(&svc, "jobs.cancel", json!({"job_id":job.job_id})),
    );
    assert_eq!(cancel["job"]["state"], "cancel_requested");
    assert_eq!(cancel["job"]["kind"], "refit");
}

#[test]
fn learning_durable_replay_conflict_and_retraction() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let node = learning_node(&svc);
    let p = feedback(&node, "ce_label", Some("Documents"), false);
    let (first, _) = ok("feedback.record", call(&svc, "feedback.record", p.clone()));
    assert_eq!(first["data_class"], "human");
    let (replay, _) = ok("feedback.record", call(&svc, "feedback.record", p.clone()));
    assert_eq!(replay["event_id"], first["event_id"]);
    assert_eq!(replay["idempotent_replay"], true);
    assert_eq!(
        err(call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_label", Some("Archive"), false)
        ))
        .code,
        ErrorCode::InvalidRequest
    );
    let (withdrawn, _) = ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_retract", None, true),
        ),
    );
    assert_eq!(withdrawn["revision"], 2);
    let (again, _) = ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_retract_again", None, true),
        ),
    );
    assert_eq!(again["revision"], 3);
    assert_eq!(
        svc.inner
            .db()
            .query_row(
                "SELECT label FROM st.human_feedback WHERE retracted=1",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "Documents"
    );
    drop(svc);
    let reopened = open(tmp.path(), DatasetClass::Synthetic);
    let (replay, _) = ok("feedback.record", call(&reopened, "feedback.record", p));
    assert_eq!(replay["event_id"], first["event_id"]);
    assert_eq!(replay["learning"], first["learning"]);
    assert_eq!(replay["idempotent_replay"], true);
}

fn count(svc: &Service, sql: &str) -> i64 {
    svc.inner.db().query_row(sql, [], |r| r.get(0)).unwrap()
}

/// q-11: a personal session learns locally from the owner's labels on its active grants; the
/// teacher stays refused, and revocation redacts and withdraws while withdrawal stays possible.
#[test]
fn learning_personal_session_learns_locally_and_revocation_redacts() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Personal);
    let node = learning_node(&svc);
    let (status, _) = ok("learning.status", call(&svc, "learning.status", json!({})));
    assert_eq!(status["gate"], Value::Null);
    assert_eq!(status["dataset_class"], "personal");
    let (queue, _) = ok(
        "learning.queue",
        call(
            &svc,
            "learning.queue",
            json!({"root_id":null,"limit":10,"cursor":null}),
        ),
    );
    assert_eq!(queue["items"].as_array().unwrap().len(), 3);
    let (first, _) = ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_personal_1", Some("Documents"), false),
        ),
    );
    assert_eq!(first["data_class"], "human");
    // A teacher row in the same store is a separate class and never trains the student.
    svc.inner.db().execute_batch("PRAGMA foreign_keys=OFF;INSERT INTO st.teacher_label(event_id,request_id,object_ref_id,revision,label,abstain,reason,evidence_json,taxonomy_version,features_json,received_at_ns) SELECT 'tl_personal',999,object_ref_id,1,'Archive',0,'fixture','[]',1,features_json,0 FROM st.human_feedback LIMIT 1;PRAGMA foreign_keys=ON").unwrap();
    let status = finish_refit(&svc);
    assert_eq!(status["training_count"], 1);
    assert_eq!(status["teacher_labels"], 1);
    assert_eq!(status["human_support"]["Archive"], 0);
    assert_eq!(status["calibrated"], false);
    assert_eq!(status["autonomy_allowed"], false);
    let (meaning, _) = ok(
        "threads.meaning",
        call(&svc, "threads.meaning", json!({"node_ids":[node]})),
    );
    assert_eq!(meaning["model_id"], status["model_id"]);
    let suggestion = &meaning["items"][0]["meaning"]["suggestion"];
    assert_eq!(suggestion["ranked"][0]["label"], "Documents");
    assert_eq!(suggestion["calibrated"], false);
    assert_eq!(suggestion["teacher"], Value::Null);
    for (command, payload) in [
        (
            "teacher.preview",
            json!({"node_ids":[node],"recipient":"codex_cli_gpt_6_1_sol"}),
        ),
        (
            "grants.create_disclosure",
            json!({"preview_id":"tp_0001","payload_digest":format!("sha256:{}", "0".repeat(64))}),
        ),
        ("teacher.run", json!({"grant_id":"gr_0002"})),
        ("teacher.results", json!({"job_id":"jb_0010"})),
    ] {
        let e = err(call(&svc, command, payload));
        assert_eq!(e.code, ErrorCode::CapabilityUnavailable, "{command}");
        assert_eq!(reason(&e), Some("teacher_personal_gated"), "{command}");
    }
    assert!(count(&svc, "SELECT count(*) FROM st.human_feedback WHERE features_json LIKE '%name%' AND features_json NOT LIKE '%\"name\":\"\"%'") > 0);
    let root = svc.inner.root_id(1);
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": root, "purge_catalog": false}),
        ),
    );
    assert_eq!(
        count(&svc, "SELECT count(*) FROM st.human_feedback WHERE features_json!='{\"name\":\"\",\"extension\":\"\",\"context\":\"\",\"size_bytes\":0}'"),
        0,
        "revocation redacts every label's features"
    );
    assert_eq!(count(&svc, "SELECT count(*) FROM st.student_model"), 0);
    let (status, _) = ok("learning.status", call(&svc, "learning.status", json!({})));
    assert_eq!(status["model_id"], Value::Null);
    assert_eq!(
        finish_refit(&svc)["training_count"],
        0,
        "revocation withdraws"
    );
    assert_eq!(
        err(call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_after_revoke", Some("Finance"), false),
        ))
        .code,
        ErrorCode::NotFound
    );
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_withdraw_revoked", None, true),
        ),
    );
    assert_eq!(
        count(
            &svc,
            "SELECT count(*) FROM st.human_feedback WHERE retracted=1"
        ),
        1
    );
    let (queue, _) = ok(
        "learning.queue",
        call(
            &svc,
            "learning.queue",
            json!({"root_id":null,"limit":10,"cursor":null}),
        ),
    );
    assert!(queue["items"].as_array().unwrap().is_empty());
}

/// Each store records its dataset class: neither class's session opens, or trains on, the other's labels.
#[test]
fn learning_stores_never_cross_dataset_classes() {
    for (class, other) in [
        (DatasetClass::Synthetic, DatasetClass::Personal),
        (DatasetClass::Personal, DatasetClass::Synthetic),
    ] {
        let tmp = temp_root();
        let svc = open(tmp.path(), class);
        let node = learning_node(&svc);
        ok(
            "feedback.record",
            call(
                &svc,
                "feedback.record",
                feedback(&node, "ce_class", Some("Documents"), false),
            ),
        );
        drop(svc);
        assert!(Service::open(Config {
            state_dir: tmp.path().to_path_buf(),
            dataset: other,
            allow_personal: true,
            grant_roots: vec![],
        })
        .is_err());
        // Even an engine handed the wrong store refuses before reading a label.
        let engine = loomward_engine::Engine::open(loomward_engine::EngineConfig::new(
            tmp.path().into(),
            other,
        ))
        .unwrap();
        engine.install_learning(loomward_engine::learning::Store::open(tmp.path()).unwrap());
        assert_eq!(
            engine.learning_status().unwrap_err().code(),
            ErrorCode::PermissionDenied
        );
        assert_eq!(
            engine.learning_refit().unwrap_err().code(),
            ErrorCode::PermissionDenied
        );
    }
}

#[test]
fn learning_queue_binds_both_revisions_and_filters() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let node = learning_node(&svc);
    let p = json!({"root_id":null,"limit":1,"cursor":null});
    let (page, _) = ok("learning.queue", call(&svc, "learning.queue", p.clone()));
    assert!(page["model_id"].is_string());
    assert_eq!(page["model_id"], page["items"][0]["suggestion"]["model_id"]);
    assert_eq!(page["items"][0]["suggestion"]["calibrated"], false);
    let cursor = page["next_cursor"].clone();
    assert!(cursor.is_string());
    assert_eq!(
        err(call(
            &svc,
            "learning.queue",
            json!({"root_id":svc.inner.root_id(1),"limit":1,"cursor":cursor})
        ))
        .code,
        ErrorCode::InvalidRequest
    );
    let resume = json!({"root_id":null,"limit":1,"cursor":cursor});
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_cursor", Some("Documents"), false),
        ),
    );
    assert_eq!(
        err(call(&svc, "learning.queue", resume)).code,
        ErrorCode::StaleGeneration
    );
    let (page, _) = ok("learning.queue", call(&svc, "learning.queue", p));
    svc.inner
        .db()
        .execute("UPDATE main.revision SET catalog_rev=catalog_rev+1", [])
        .unwrap();
    assert_eq!(
        err(call(
            &svc,
            "learning.queue",
            json!({"root_id":null,"limit":1,"cursor":page["next_cursor"]})
        ))
        .code,
        ErrorCode::StaleGeneration
    );
}

#[test]
fn learning_refit_is_human_only_candidate_and_retraction_withdraws() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let node = learning_node(&svc);
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_fit_1", Some("Documents"), false),
        ),
    );
    // A separate weak label must never contribute to this lane's human-only refit.
    let WriteReply::Reference(teacher_ref) = svc
        .inner
        .catalog
        .writer()
        .call(WriteCommand::ObjectReference {
            node: loomward_catalog::NodeKey::File(2),
            observed_at_ns: 0,
        })
        .unwrap()
    else {
        panic!()
    };
    svc.inner.db().execute_batch(&format!("PRAGMA foreign_keys=OFF;INSERT INTO st.teacher_label(event_id,request_id,object_ref_id,revision,label,abstain,reason,evidence_json,taxonomy_version,features_json,received_at_ns) SELECT 'tl_fixture',999,{teacher_ref},1,'Archive',0,'fixture','[]',1,features_json,0 FROM st.human_feedback LIMIT 1;PRAGMA foreign_keys=ON")).unwrap();
    let teacher_node = svc.inner.ids.seal(
        loomward_catalog::NodeKey::File(2),
        svc.inner
            .db()
            .query_row("SELECT born_run FROM main.file WHERE id=2", [], |r| {
                r.get(0)
            })
            .unwrap(),
        &svc.inner.instance,
    );
    assert_eq!(
        err(call(
            &svc,
            "feedback.record",
            feedback(&teacher_node, "ce_teacher_only", None, true)
        ))
        .code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        svc.inner
            .db()
            .query_row("SELECT count(*) FROM st.human_feedback", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    for (expected, retract) in [(1, false), (0, true)] {
        if retract {
            ok(
                "feedback.record",
                call(
                    &svc,
                    "feedback.record",
                    feedback(&node, "ce_withdraw", None, true),
                ),
            );
        }
        let (job, _) = ok("learning.refit", call(&svc, "learning.refit", json!({})));
        let id: JobId = serde_json::from_value(job["job"]["job_id"].clone()).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let j = svc.inner.engine.job_status(&id).unwrap();
            if matches!(
                j.state,
                JobState::Completed | JobState::Failed | JobState::Cancelled
            ) {
                assert_eq!(j.state, JobState::Completed);
                assert!(j.seq.get() > 0 && j.rev.get() >= 2);
                break;
            }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
        let (status, _) = ok("learning.status", call(&svc, "learning.status", json!({})));
        assert_eq!(status["training_count"], expected);
        assert_eq!(status["teacher_labels"], 1);
        assert_eq!(status["autonomy_allowed"], false);
        assert_eq!(
            svc.inner
                .db()
                .query_row(
                    "SELECT count(*) FROM st.student_model WHERE status='active'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}

#[test]
fn learning_retracts_without_current_node_access_and_beyond_history_budget() {
    for mode in ["excluded", "unresolved", "revoked", "missing", "legacy"] {
        let tmp = temp_root();
        let svc = open(tmp.path(), DatasetClass::Synthetic);
        let node = learning_node(&svc);
        ok(
            "feedback.record",
            call(
                &svc,
                "feedback.record",
                feedback(&node, "ce_first", Some("Documents"), false),
            ),
        );
        assert_eq!(finish_refit(&svc)["training_count"], 1);
        match mode {
            "excluded" => {
                svc.inner
                    .db()
                    .execute("UPDATE main.file SET flags=1", [])
                    .unwrap();
            }
            "unresolved" => {
                svc.inner
                    .db()
                    .execute("UPDATE st.object_ref SET state='unresolved',retired=1", [])
                    .unwrap();
            }
            "revoked" => {
                svc.inner
                    .db()
                    .execute(
                        "UPDATE st.root_grant SET state='revoked',revoked_at_ns=1",
                        [],
                    )
                    .unwrap();
            }
            "missing" => {
                svc.inner
                    .db()
                    .execute("UPDATE st.object_ref SET row_id=NULL,born_run=NULL,state='unresolved',retired=1", [])
                    .unwrap();
            }
            "legacy" => {
                svc.inner
                    .db()
                    .execute("DELETE FROM st.learning_node", [])
                    .unwrap();
            }
            _ => unreachable!(),
        }
        ok(
            "feedback.record",
            call(
                &svc,
                "feedback.record",
                feedback(&node, "ce_withdraw", None, true),
            ),
        );
        let (status, _) = ok("learning.status", call(&svc, "learning.status", json!({})));
        assert_eq!(status["model_id"], Value::Null, "{mode}");
        assert_eq!(status["human_support"]["Documents"], 0, "{mode}");
        let (queue, _) = ok(
            "learning.queue",
            call(
                &svc,
                "learning.queue",
                json!({"root_id":null,"limit":1,"cursor":null}),
            ),
        );
        if let Some(item) = queue["items"].as_array().unwrap().first() {
            assert!(item["suggestion"]["reasons"]
                .as_array()
                .unwrap()
                .contains(&json!("no_training_examples")));
        }
        // Resolution loss alone must not be what suppresses the withdrawn example.
        if matches!(mode, "unresolved" | "missing") {
            svc.inner.db().execute_batch("UPDATE st.object_ref SET state='resolved',retired=0,row_id=1,born_run=(SELECT born_run FROM main.file WHERE id=1)").unwrap();
        }
        assert_eq!(finish_refit(&svc)["training_count"], 0, "{mode}");
        assert_eq!(svc.inner.db().query_row("SELECT count(*) FROM st.student_model WHERE status='candidate' AND training_count=0", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(
            svc.inner
                .db()
                .query_row(
                    "SELECT count(*) FROM st.human_feedback WHERE retracted=1",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }
}

#[test]
fn learning_latest_label_window_does_not_cap_history_or_withdrawal() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let node = learning_node(&svc);
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_first", Some("Documents"), false),
        ),
    );
    svc.inner.db().execute_batch(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<9999)
         INSERT INTO st.human_feedback(event_id,client_event_id,content_digest,object_ref_id,revision,label,retracted,taxonomy_version,features_json,recorded_at_ns)
         SELECT 'fe_bulk'||i,'ce_bulk'||i,h.content_digest,h.object_ref_id,h.revision+i,h.label,0,h.taxonomy_version,h.features_json,h.recorded_at_ns FROM n, st.human_feedback h WHERE h.client_event_id='ce_first'",
    ).unwrap();
    assert_eq!(finish_refit(&svc)["training_count"], 1);
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_over_budget", Some("Finance"), false),
        ),
    );
    let status = finish_refit(&svc);
    assert_eq!(status["human_events"], 10_001);
    assert_eq!(status["human_support"]["Finance"], 1);
    assert_eq!(status["human_support"]["Documents"], 0);
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_withdraw", None, true),
        ),
    );
    assert_eq!(finish_refit(&svc)["training_count"], 0);
}

#[test]
fn learning_queue_filters_before_its_candidate_budget() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    fixture(&svc, 10_002);
    svc.inner.db().execute_batch("UPDATE main.file SET file_id=NULL,created_ft=NULL;UPDATE main.file SET file_id=cast('1111111111111111' AS BLOB),created_ft=1 WHERE id=10002").unwrap();
    let (page, _) = ok(
        "learning.queue",
        call(
            &svc,
            "learning.queue",
            json!({"root_id":null,"limit":100,"cursor":null}),
        ),
    );
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["next_cursor"], Value::Null);
    // A genuinely eligible overflow must still be refused.
    svc.inner
        .db()
        .execute_batch("UPDATE main.file SET file_id=cast(printf('%016d',id) AS BLOB),created_ft=1")
        .unwrap();
    assert_eq!(
        err(call(
            &svc,
            "learning.queue",
            json!({"root_id":null,"limit":1,"cursor":null})
        ))
        .code,
        ErrorCode::ResourceBudget
    );
}

#[test]
fn learning_legacy_retraction_prefers_live_then_newest_reference() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let node = learning_node(&svc);
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_first", Some("Documents"), false),
        ),
    );
    svc.inner.db().execute_batch("DELETE FROM st.learning_node;
        INSERT INTO st.object_ref(volume_key,file_id,creation_ft,kind,state,incarnation,retired,catalog_instance,row_id,born_run) SELECT volume_key,file_id,creation_ft,kind,'unresolved',incarnation+1,1,catalog_instance,row_id,born_run FROM st.object_ref WHERE id=1;
        INSERT INTO st.human_feedback(event_id,client_event_id,content_digest,object_ref_id,revision,label,retracted,taxonomy_version,features_json,recorded_at_ns) SELECT 'fe_stale','ce_stale',content_digest,2,100,'Archive',0,1,features_json,recorded_at_ns FROM st.human_feedback WHERE object_ref_id=1").unwrap();
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_live_retract", None, true),
        ),
    );
    assert_eq!(svc.inner.db().query_row("SELECT object_ref_id FROM st.human_feedback WHERE client_event_id='ce_live_retract'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    svc.inner
        .db()
        .execute_batch(
            "DELETE FROM st.learning_node;UPDATE st.object_ref SET state='unresolved',retired=1",
        )
        .unwrap();
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_newest_retract", None, true),
        ),
    );
    assert_eq!(svc.inner.db().query_row("SELECT object_ref_id FROM st.human_feedback WHERE client_event_id='ce_newest_retract'", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
}

#[test]
fn learning_training_window_uses_newest_distinct_objects() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let node = learning_node(&svc);
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            feedback(&node, "ce_first", Some("Archive"), false),
        ),
    );
    svc.inner.db().execute_batch("WITH RECURSIVE n(i) AS (SELECT 2 UNION ALL SELECT i+1 FROM n WHERE i<10001)
        INSERT INTO st.object_ref(volume_key,file_id,creation_ft,kind,state,retired) SELECT 'fixture',cast(printf('%016d',i+100) AS BLOB),1,'file','resolved',0 FROM n;
        INSERT INTO st.human_feedback(event_id,client_event_id,content_digest,object_ref_id,revision,label,retracted,taxonomy_version,features_json,recorded_at_ns)
        SELECT 'fe_distinct'||o.id,'ce_distinct'||o.id,'fixture',o.id,10002-o.id,CASE WHEN o.id=10001 THEN 'Finance' ELSE 'Documents' END,0,1,'{\"name\":\"fixture.txt\",\"extension\":\"txt\",\"context\":\"fixture\",\"size_bytes\":1}',0 FROM st.object_ref o WHERE o.id>1 ORDER BY o.id").unwrap();
    let status = finish_refit(&svc);
    assert_eq!(status["training_count"], 10_000);
    assert_eq!(status["human_events"], 10_001);
    assert_eq!(status["human_support"]["Archive"], 0);
    assert_eq!(status["human_support"]["Finance"], 1);
    assert_eq!(status["human_support"]["Documents"], 9_999);
}
