use super::learning_tests::{feedback, finish_refit};
use crate::tests::*;

#[test]
fn meaning_is_bounded_read_only_advice_with_explicit_unknowns() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, _) = fixture(&svc, 6);
    svc.inner
        .db()
        .execute_batch("UPDATE main.file SET name=(CASE WHEN id<=2 THEN 'finance payroll ledger budget invoice balance audit ' ELSE 'document report notes study reading summary literature essay ' END) || id || '.txt',file_id=cast(printf('%016d',id) AS BLOB),created_ft=1")
        .unwrap();
    let slice = root_slice(&svc, root);
    let nodes: Vec<Value> = slice["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "file")
        .map(|n| n["node_id"].clone())
        .collect();
    let payload = json!({"node_ids": nodes});
    let before = svc.inner.revisions().unwrap();
    let empty = ok(
        "threads.meaning",
        call(&svc, "threads.meaning", payload.clone()),
    )
    .0;
    assert_eq!(empty["model_id"], Value::Null);
    assert!(empty["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|n| n["meaning"]["state"] == "no_model"));
    assert_eq!(svc.inner.revisions().unwrap(), before);
    for (i, node) in nodes.iter().enumerate() {
        ok(
            "feedback.record",
            call(
                &svc,
                "feedback.record",
                feedback(
                    node.as_str().unwrap(),
                    &format!("ce_meaning_{i}"),
                    Some(if i < 4 { "Documents" } else { "Finance" }),
                    false,
                ),
            ),
        );
    }
    let status = finish_refit(&svc);
    let before = svc.inner.revisions().unwrap();
    let fitted = ok(
        "threads.meaning",
        call(&svc, "threads.meaning", payload.clone()),
    )
    .0;
    assert_eq!(fitted["model_id"], status["model_id"]);
    let m = &fitted["items"][0]["meaning"];
    assert_eq!(m["source"], "student");
    assert_eq!(m["state"], "suggested", "{m:?}");
    assert!(!m["suggestion"]["ranked"].as_array().unwrap().is_empty());
    assert_eq!(m["suggestion"]["calibrated"], false);
    assert_eq!(m["suggestion"]["autonomy_allowed"], false);
    assert_eq!(svc.inner.revisions().unwrap(), before);
    svc.inner.db().execute("UPDATE main.file SET name='zqx vwj klp mnb hjg rew qas iop uyt fds lkj zxc vbn qwe rty asd fgh jkl.txt'", []).unwrap();
    let uncertain = ok("threads.meaning", call(&svc, "threads.meaning", payload)).0;
    let m = &uncertain["items"][0]["meaning"];
    assert_eq!(m["state"], "abstained");
    assert_eq!(m["label"], Value::Null);
    assert_eq!(m["share"], Value::Null);
    assert_eq!(m["suggestion"]["abstain"], true);
    assert!(m["suggestion"]["reasons"]
        .as_array()
        .unwrap()
        .contains(&json!("unfamiliar_features")));
    let anchor = slice["anchor_node_id"].clone();
    let folder = ok(
        "threads.meaning",
        call(&svc, "threads.meaning", json!({"node_ids":[anchor]})),
    )
    .0;
    assert_eq!(folder["items"][0]["meaning"]["state"], "not_eligible");
    assert_eq!(
        err(call(
            &svc,
            "threads.meaning",
            json!({"node_ids":["nd_unknown"]})
        ))
        .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        err(call(
            &svc,
            "threads.meaning",
            json!({"node_ids":[nodes[0], nodes[0]]})
        ))
        .code,
        ErrorCode::InvalidRequest
    );
    let mut req = request("threads.meaning", json!({"node_ids":[nodes[0]]}));
    req.deadline_ms = Some(Int::new(1).unwrap());
    *svc.inner.hooks.delay.lock().unwrap() =
        Some((Command::ThreadsMeaning, Duration::from_millis(20)));
    assert_eq!(
        err(svc.call(req, &CallContext::http())).code,
        ErrorCode::DeadlineExceeded
    );
    let personal_dir = temp_root();
    let personal = open(personal_dir.path(), DatasetClass::Personal);
    let refused = err(call(
        &personal,
        "threads.meaning",
        json!({"node_ids":["nd_unknown"]}),
    ));
    assert_eq!(refused.detail.unwrap()["reason"], "learning_personal_gated");
}

#[test]
fn meaning_work_budget_is_an_honest_prefix() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, _) = fixture(&svc, 1100);
    let slice = ok("tree.slice", call(&svc, "tree.slice", json!({"anchor":{"kind":"root","root_id":svc.inner.root_id(root)},"depth":2,"max_nodes":1200,"min_share":0,"basis":"logical","include_files":true}))).0;
    let ids: Vec<Value> = slice["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["node_id"].clone())
        .collect();
    assert!(ids.len() > 1000);
    let result = ok(
        "threads.meaning",
        call(&svc, "threads.meaning", json!({"node_ids":ids})),
    )
    .0;
    assert_eq!(result["budget_hit"], true);
    assert_eq!(result["items"].as_array().unwrap().len(), 1000);
}

#[test]
fn meaning_features_read_each_file_once() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    fixture(&svc, 3);
    svc.inner
        .db()
        .execute_batch("UPDATE main.file SET file_id=cast(printf('%016d',id) AS BLOB),created_ft=1")
        .unwrap();
    let mut reader = svc.inner.catalog.reader().unwrap();
    let features =
        crate::meaning::eligible_features(&mut reader, loomward_catalog::NodeKey::File(1))
            .unwrap()
            .expect("a file with a durable identity is eligible");
    assert_eq!(features["extension"], "txt");
    assert_eq!(
        reader.inspect_count, 1,
        "eligibility and features share one detail read"
    );
    let mut reader = svc.inner.catalog.reader().unwrap();
    assert!(
        crate::meaning::eligible_features(&mut reader, loomward_catalog::NodeKey::Dir(1))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        reader.inspect_count, 0,
        "a folder is ineligible without a read"
    );
    svc.inner
        .db()
        .execute_batch("UPDATE main.file SET file_id=NULL")
        .unwrap();
    let mut reader = svc.inner.catalog.reader().unwrap();
    assert!(
        crate::meaning::eligible_features(&mut reader, loomward_catalog::NodeKey::File(1))
            .unwrap()
            .is_none()
    );
    assert_eq!(reader.inspect_count, 1);
}
