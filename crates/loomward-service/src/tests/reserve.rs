use super::*;

const GIB: u64 = 1 << 30;

fn reserve_fixture(svc: &Service) -> (String, String, Value) {
    let (root, _, source) = placement_fixture(svc);
    {
        let conn = svc.inner.db();
        conn.execute(
            "DELETE FROM main.file WHERE id<>(SELECT min(id) FROM main.file)",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE main.file SET logical=?1,allocated=?1",
            [(30 * GIB) as i64],
        )
        .unwrap();
        conn.execute("UPDATE main.dir SET sub_logical=?1,sub_allocated=?1,sub_files=1 WHERE root_id=?2 AND parent_id IS NULL", rusqlite::params![(30 * GIB) as i64, root]).unwrap();
        conn.execute("UPDATE main.volume SET capabilities_json=?1", [json!({"space":{"capacity_bytes":(200*GIB).to_string(),"free_bytes":(10*GIB).to_string(),"read_only":false}}).to_string()]).unwrap();
    }
    let WriteReply::Root { root_id, .. } = svc
        .inner
        .catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "reserve-target".into(),
            display_name: "Reserve target".into(),
            display_path: "Synthetic destination".into(),
            root_file_id: Some(vec![2; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))
        .unwrap()
    else {
        panic!("target registration")
    };
    let target_row: i64 = svc
        .inner
        .db()
        .query_row(
            "SELECT volume_id FROM main.root WHERE id=?1",
            [root_id],
            |r| r.get(0),
        )
        .unwrap();
    svc.inner.db().execute("UPDATE main.volume SET capabilities_json=?1,bytes_per_cluster=4096 WHERE id=?2", rusqlite::params![json!({"space":{"capacity_bytes":(200*GIB).to_string(),"free_bytes":(100*GIB).to_string(),"read_only":false}}).to_string(), target_row]).unwrap();
    let target = svc.inner.volume_id(target_row);
    ok(
        "volumes.declare_tier",
        call(
            svc,
            "volumes.declare_tier",
            json!({"volume_id":target,"tier":1}),
        ),
    );
    let (candidates, _) = ok(
        "placement.candidates",
        call(
            svc,
            "placement.candidates",
            json!({"source_volume_id":source,"basis":"largest_dirs","max_groups":50,"min_bytes":"1"}),
        ),
    );
    assert_eq!(candidates["groups"].as_array().unwrap().len(), 1);
    let simulation = json!({"source_volume_id":source,"target_free_bytes":(40*GIB).to_string(),"max_transfer_bytes":(60*GIB).to_string(),"heat_policy":"assumed_only","relief_policy":"entry_allocation_whatif","candidate_basis":"largest_dirs","max_groups":50,"overrides":[{"group_id":candidates["groups"][0]["group_id"],"heat":0.1,"pinned":false,"active":false,"protected":false,"days_since_move":365}],"node_budget":1000,"save":false});
    (source, target, simulation)
}

fn reserve_request(svc: &Service, volume: &str, bytes: Option<u64>) -> RequestEnvelope {
    let (_, meta) = ok("tiers.model", call(svc, "tiers.model", json!({})));
    let mut req = request(
        "volumes.declare_reserve",
        json!({"volume_id":volume,"reserve_bytes":bytes.map(|n|n.to_string())}),
    );
    req.expected_state_rev = meta.state_rev;
    req
}

fn declare_reserve(svc: &Service, volume: &str, bytes: Option<u64>) -> ResponseEnvelope {
    svc.call(reserve_request(svc, volume, bytes), &CallContext::http())
}

fn target_model(svc: &Service) -> (Value, ResponseMeta) {
    let (model, meta) = ok("tiers.model", call(svc, "tiers.model", json!({})));
    (
        model["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["display_name"] == "Reserve target")
            .unwrap()
            .clone(),
        meta,
    )
}

#[test]
fn destination_reserve_bounds_100_gib_free_without_changing_candidate_eligibility() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (_, target, simulation) = reserve_fixture(&svc);
    let (initial, _) = ok(
        "placement.simulate",
        call(&svc, "placement.simulate", simulation.clone()),
    );
    assert_eq!(
        initial["satisfied"], true,
        "fixture must admit the same 30 GiB candidate without a reserve: {initial:?}"
    );
    for (reserve, satisfied, count) in [(80, false, 0), (60, true, 1)] {
        ok(
            "volumes.declare_reserve",
            svc.call(
                reserve_request(&svc, &target, Some(reserve * GIB)),
                &CallContext::http(),
            ),
        );
        let (plan, _) = ok(
            "placement.simulate",
            call(&svc, "placement.simulate", simulation.clone()),
        );
        assert_eq!(plan["satisfied"], satisfied, "{plan:?}");
        assert_eq!(
            plan["proposals"].as_array().unwrap().len(),
            count,
            "{plan:?}"
        );
        assert!(
            plan["pre_rejected"].as_array().unwrap().is_empty(),
            "reserve must not alter eligibility: {plan:?}"
        );
        assert_eq!(
            plan["shortfall_bytes"],
            if satisfied {
                "0".to_owned()
            } else {
                (30 * GIB).to_string()
            }
        );
        assert_eq!(plan["filesystem_changed"], false);
    }
}

#[test]
fn destination_reserve_survives_restart_and_old_retries_cannot_overwrite_revision() {
    let tmp = temp_root();
    let state = tmp.path().join("state");
    let original = tmp.path().join("original.bin");
    std::fs::write(&original, b"original synthetic bytes").unwrap();
    let svc = open(&state, DatasetClass::Synthetic);
    let (_, target, mut simulation) = reserve_fixture(&svc);
    let (mut grants, _) = ok("grants.list", call(&svc, "grants.list", json!({})));
    let before = target_model(&svc);
    assert_eq!(before.0["declared_reserve_bytes"], Value::Null);
    assert_eq!(before.0["reserve_bytes"], "0");
    let request80 = reserve_request(&svc, &target, Some(80 * GIB));
    let first = svc.call(request80.clone(), &CallContext::http());
    let (_, written) = ok("volumes.declare_reserve", first.clone());
    assert_eq!(
        written.state_rev.unwrap().get(),
        before.1.state_rev.unwrap().get() + 1
    );
    assert_eq!(svc.call(request80.clone(), &CallContext::http()), first);
    simulation["save"] = json!(true);
    let (old, _) = ok(
        "placement.simulate",
        call(&svc, "placement.simulate", simulation.clone()),
    );
    let stale = reserve_request(&svc, &target, Some(90 * GIB));
    ok(
        "volumes.declare_reserve",
        declare_reserve(&svc, &target, Some(60 * GIB)),
    );
    assert_eq!(
        reason(&err(svc.call(stale, &CallContext::http()))),
        Some("state_rev_changed")
    );
    assert_eq!(svc.call(request80.clone(), &CallContext::http()), first);
    let mut conflict = request80;
    conflict.payload.insert("reserve_bytes".into(), json!("1"));
    assert_eq!(
        reason(&err(svc.call(conflict, &CallContext::http()))),
        Some("idempotency_conflict")
    );
    assert_eq!(
        target_model(&svc).0["declared_reserve_bytes"],
        (60 * GIB).to_string()
    );
    let (historical, _) = ok(
        "proposals.get",
        call(
            &svc,
            "proposals.get",
            json!({"proposal_id":old["proposal_id"]}),
        ),
    );
    assert_eq!(historical["summary"]["status"], "stale");
    assert_eq!(historical["plan"], json!(old));
    let (new, _) = ok(
        "placement.simulate",
        call(&svc, "placement.simulate", simulation.clone()),
    );
    assert_eq!(new["satisfied"], true);
    assert_ne!(new["proposal_id"], old["proposal_id"]);
    ok(
        "volumes.declare_reserve",
        declare_reserve(&svc, &target, Some(80 * GIB)),
    );
    let (again, _) = ok(
        "placement.simulate",
        call(&svc, "placement.simulate", simulation),
    );
    assert_ne!(
        again["proposal_id"], old["proposal_id"],
        "returning to the same value must retain the new reserve revision"
    );
    ok(
        "volumes.declare_reserve",
        declare_reserve(&svc, &target, Some(60 * GIB)),
    );
    let rev = target_model(&svc).1.state_rev;
    assert_eq!(
        ok("grants.list", call(&svc, "grants.list", json!({}))).0,
        grants
    );
    drop(svc);
    let svc = open(&state, DatasetClass::Synthetic);
    let (model, meta) = target_model(&svc);
    assert_eq!(meta.state_rev, rev);
    assert_eq!(model["declared_reserve_bytes"], (60 * GIB).to_string());
    assert_eq!(model["reserve_bytes"], (60 * GIB).to_string());
    assert_eq!(
        std::fs::read(original).unwrap(),
        b"original synthetic bytes"
    );
    let (mut after_grants, _) = ok("grants.list", call(&svc, "grants.list", json!({})));
    // Session-sealed root handles rotate on restart; grant IDs and permission facts do not.
    for list in [&mut grants, &mut after_grants] {
        for row in list.get_mut("grants").unwrap().as_array_mut().unwrap() {
            row.as_object_mut().unwrap().remove("root_id");
        }
    }
    assert_eq!(
        grants, after_grants,
        "reserve preferences do not grant or revoke access"
    );
}

#[test]
fn destination_reserve_preserves_exact_bytes_clear_and_unknown_physics() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (_, target, _) = reserve_fixture(&svc);
    for bytes in [Some(u64::MAX), Some(0), None] {
        let (result, _) = ok(
            "volumes.declare_reserve",
            declare_reserve(&svc, &target, bytes),
        );
        assert_eq!(result["reserve_bytes"], json!(bytes.map(|n| n.to_string())));
        let (model, _) = target_model(&svc);
        assert_eq!(model["declared_reserve_bytes"], result["reserve_bytes"]);
        assert_eq!(model["reserve_bytes"], bytes.unwrap_or(0).to_string());
    }
    svc.inner
        .db()
        .execute(
            "UPDATE main.volume SET capabilities_json='{}' WHERE volume_key='reserve-target'",
            [],
        )
        .unwrap();
    ok(
        "volumes.declare_reserve",
        declare_reserve(&svc, &target, Some(80 * GIB)),
    );
    let (model, _) = target_model(&svc);
    assert_eq!(model["reserve_bytes"], (80 * GIB).to_string());
    assert!(model["capacity_bytes"].is_null() && model["free_bytes"].is_null());
    assert_eq!(model["source_eligible"], false);
}

#[test]
fn destination_reserve_requires_owner_revision_and_strict_byte_payload() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (_, target, _) = reserve_fixture(&svc);
    let before = target_model(&svc).1.state_rev;
    let mut missing = reserve_request(&svc, &target, Some(80 * GIB));
    missing.expected_state_rev = None;
    assert_eq!(
        err(svc.call(missing, &CallContext::http())).code,
        ErrorCode::InvalidRequest
    );
    for invalid in [
        json!(1),
        json!("-1"),
        json!("01"),
        json!("1.5"),
        json!("18446744073709551616"),
    ] {
        let mut req = reserve_request(&svc, &target, None);
        req.payload.insert("reserve_bytes".into(), invalid);
        assert_eq!(
            err(svc.call(req, &CallContext::http())).code,
            ErrorCode::InvalidRequest
        );
    }
    let mut missing_bytes = reserve_request(&svc, &target, None);
    missing_bytes.payload.remove("reserve_bytes");
    assert_eq!(
        err(svc.call(missing_bytes, &CallContext::http())).code,
        ErrorCode::InvalidRequest
    );
    let denied = svc.call(
        reserve_request(&svc, &target, Some(1)),
        &CallContext::client("mcp:reserve-fixture"),
    );
    assert_eq!(err(denied).code, ErrorCode::PermissionDenied);
    assert_eq!(target_model(&svc).1.state_rev, before);
}

#[test]
fn destination_reserve_does_not_follow_a_replacement_volume_identity() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (_, target, _) = reserve_fixture(&svc);
    ok(
        "volumes.declare_reserve",
        declare_reserve(&svc, &target, Some(80 * GIB)),
    );
    // Same display label and catalogue row, different native identity: preference lookup must not follow it.
    svc.inner.db().execute("UPDATE main.volume SET volume_key='replacement-native-identity' WHERE volume_key='reserve-target'",[]).unwrap();
    let (replacement, _) = target_model(&svc);
    assert_eq!(replacement["declared_reserve_bytes"], Value::Null);
    assert_eq!(replacement["reserve_bytes"], "0");
}

#[test]
fn destination_reserve_revision_refuses_a_same_generation_save_race() {
    let tmp = temp_root();
    let svc = Arc::new(open(tmp.path(), DatasetClass::Synthetic));
    let (_, target, mut simulation) = reserve_fixture(&svc);
    ok(
        "volumes.declare_reserve",
        declare_reserve(&svc, &target, Some(60 * GIB)),
    );
    let before = target_model(&svc).1;
    svc.inner
        .hooks
        .closed
        .lock()
        .unwrap()
        .push((Command::PlacementSimulate, "placement_read"));
    simulation["save"] = json!(true);
    let worker = svc.clone();
    let pending = std::thread::spawn(move || call(&worker, "placement.simulate", simulation));
    svc.inner.hooks.until("reserve simulation snapshot", |h| {
        h.arrived
            .lock()
            .unwrap()
            .contains(&(Command::PlacementSimulate, "placement_read"))
    });
    // A separate durable writer changes only owner state after the simulation acquired its snapshot.
    let writer = rusqlite::Connection::open(tmp.path().join("state.db")).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE; INSERT INTO reserve_declaration(volume_key,reserve_bytes,declared_at_ns) VALUES('reserve-target','85899345920',1); UPDATE revision SET state_rev=state_rev+1; COMMIT;").unwrap();
    svc.inner
        .hooks
        .open(Command::PlacementSimulate, "placement_read");
    assert_eq!(
        err(pending.join().unwrap()).code,
        ErrorCode::StaleGeneration
    );
    let after = target_model(&svc).1;
    assert_eq!(after.catalog_rev, before.catalog_rev);
    assert_ne!(after.state_rev, before.state_rev);
    let (plans, _) = ok("proposals.list", call(&svc, "proposals.list", json!({})));
    assert!(plans["proposals"].as_array().unwrap().is_empty());
}
