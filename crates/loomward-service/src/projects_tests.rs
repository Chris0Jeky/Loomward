use super::*;

fn refused(response: ResponseEnvelope, code: ErrorCode) {
    assert_eq!(err(response).code, code);
}

fn discover(svc: &Service, root: &str, limit: usize, cursor: Value) -> ResponseEnvelope {
    call(
        svc,
        "projects.find",
        json!({"root_id":root,"limit":limit,"cursor":cursor}),
    )
}

#[test]
fn guarded_project_footprint_reads_never_repair_dirty_catalogue() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(&svc, 1, &projects());
    let (page, _) = ok("projects.find", discover(&svc, &root, 50, Value::Null));
    let node = page["candidates"][0]["node_id"].clone();
    let generation = page["generation"].as_str().unwrap().parse::<u64>().unwrap();
    let read = |command, payload, expected: Option<u64>| {
        let mut req = request(command, payload);
        req.expected_generation = expected.map(Generation);
        svc.call(req, &CallContext::http())
    };
    let (detail, _) = ok(
        "node.inspect",
        read("node.inspect", json!({"node_id":node}), Some(generation)),
    );
    assert_eq!(detail["row"]["coverage"], "complete");
    svc.inner
        .db()
        .execute("UPDATE main.dir SET dirty_rev=agg_valid_rev+1", [])
        .unwrap();
    let before = svc.inner.revisions().unwrap();
    for (command, payload) in [
        ("node.inspect", json!({"node_id":node})),
        (
            "stats.breakdown",
            json!({"node_id":node,"by":"ext_family","basis":"logical","limit":8}),
        ),
        (
            "tree.children",
            json!({"node_id":node,"sort":"size_desc","basis":"logical","limit":12,"cursor":null}),
        ),
    ] {
        refused(
            read(command, payload.clone(), Some(generation + 1)),
            ErrorCode::StaleGeneration,
        );
        let (result, _) = ok(command, read(command, payload, Some(generation)));
        assert_eq!(
            svc.inner.revisions().unwrap(),
            before,
            "{command} repaired the catalogue"
        );
        assert!(svc
            .inner
            .db()
            .query_row(
                "SELECT count(*)>0 FROM main.dir WHERE dirty_rev>agg_valid_rev",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
        if command == "node.inspect" {
            assert_eq!(result["row"]["coverage"], "stale");
        }
        if command == "stats.breakdown" {
            assert_eq!(result["complete"], false);
        }
    }
    // Existing unguarded owner reads retain their repair behavior.
    ok(
        "node.inspect",
        read("node.inspect", json!({"node_id":node}), None),
    );
    assert!(svc.inner.revisions().unwrap().0 > before.0);
}

#[test]
fn guarded_project_footprint_refuses_same_generation_revision_race() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(&svc, 1, &projects());
    let (page, _) = ok("projects.find", discover(&svc, &root, 50, Value::Null));
    let node = page["candidates"][0]["node_id"].clone();
    let generation = page["generation"].as_str().unwrap().parse::<u64>().unwrap();
    for (command, payload) in [
        ("node.inspect", json!({"node_id":node})),
        (
            "stats.breakdown",
            json!({"node_id":node,"by":"ext_family","basis":"logical","limit":8}),
        ),
        (
            "tree.children",
            json!({"node_id":node,"sort":"size_desc","basis":"logical","limit":12,"cursor":null}),
        ),
    ] {
        let mut req = request(command, payload);
        req.expected_generation = Some(Generation(generation));
        ok(command, svc.call(req.clone(), &CallContext::http()));
        let path = loomward_catalog::sqlite_dir(&svc.inner.state_dir).join("catalog.db");
        *svc.inner.hooks.before_generation_read.lock().unwrap() = Some(Box::new(move || {
            rusqlite::Connection::open(path)
                .unwrap()
                .execute("UPDATE revision SET catalog_rev=catalog_rev+1", [])
                .unwrap();
        }));
        refused(
            svc.call(req, &CallContext::http()),
            ErrorCode::StaleGeneration,
        );
    }
}

#[test]
fn discovery_pages_bind_root_catalogue_and_state_and_never_write() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(&svc, 1, &projects());
    let other = scanned(&svc, 2, &dir("", &[], vec![]));
    let before = svc.inner.revisions().unwrap();
    let (page, _) = ok("projects.find", discover(&svc, &root, 1, Value::Null));
    assert_eq!(page["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(page["candidates"][0]["name"], "app");
    assert!(!page["next_cursor"].is_null());
    assert_eq!(svc.inner.revisions().unwrap(), before);
    let (next, _) = ok(
        "projects.find",
        discover(&svc, &root, 50, page["next_cursor"].clone()),
    );
    assert!(next["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "plain"));
    refused(
        discover(&svc, &other, 1, page["next_cursor"].clone()),
        ErrorCode::InvalidRequest,
    );
    svc.inner
        .db()
        .execute("UPDATE main.revision SET catalog_rev=catalog_rev+1", [])
        .unwrap();
    refused(
        discover(&svc, &root, 1, page["next_cursor"].clone()),
        ErrorCode::StaleGeneration,
    );
    svc.inner
        .db()
        .execute(
            "UPDATE main.dir SET dirty_rev=agg_valid_rev+1 WHERE name='plain'",
            [],
        )
        .unwrap();
    let before = svc.inner.revisions().unwrap();
    let (dirty, _) = ok("projects.find", discover(&svc, &root, 50, Value::Null));
    assert_eq!(dirty["partial"], true);
    assert_eq!(svc.inner.revisions().unwrap(), before);
    assert!(svc
        .inner
        .db()
        .query_row(
            "SELECT dirty_rev>agg_valid_rev FROM main.dir WHERE name='plain'",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    // The granted fixture root is invented; no source directory exists to open or enumerate.
    let req = request(
        "projects.find",
        json!({"root_id":root,"limit":50,"cursor":null}),
    );
    refused(
        svc.call(req, &CallContext::client("mcp:projects")),
        ErrorCode::PermissionDenied,
    );
}

#[test]
fn project_add_rechecks_generation_state_revision_and_current_identity() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(
        &svc,
        1,
        &dir(
            "",
            &[],
            vec![dir(
                "project",
                &[("Cargo.toml", 1), ("package.json", 1)],
                vec![dir("nested", &[("pyproject.toml", 1)], vec![])],
            )],
        ),
    );
    svc.inner
        .db()
        .execute(
            "UPDATE main.dir SET file_id=CAST(printf('%016d',id) AS BLOB),created_ft=123",
            [],
        )
        .unwrap();
    let (page, _) = ok("projects.find", discover(&svc, &root, 50, Value::Null));
    assert_eq!(page["candidates"].as_array().unwrap().len(), 2);
    assert_eq!(
        page["candidates"][0]["markers"].as_array().unwrap().len(),
        2
    );
    let id = &page["candidates"][0]["node_id"];
    let gen = page["generation"].as_str().unwrap().parse::<u64>().unwrap();
    let (collection, _) = ok(
        "collections.create",
        call(&svc, "collections.create", json!({"name":"Projects"})),
    );
    let payload = json!({"collection_id":collection["collection_id"],"add":[id],"remove":[]});
    let add = |generation, revision| {
        let mut r = request("collections.update_members", payload.clone());
        r.expected_generation = Some(Generation(generation));
        r.expected_state_rev = Some(Generation(revision));
        svc.call(r, &CallContext::http())
    };
    let revision = svc.inner.revisions().unwrap().1 as u64;
    refused(add(gen + 1, revision), ErrorCode::StaleGeneration);
    refused(add(gen, revision + 1), ErrorCode::StaleGeneration);
    assert_eq!(svc.inner.revisions().unwrap().1 as u64, revision);
    let (added, _) = ok("collections.update_members", add(gen, revision));
    assert_eq!(added["member_count"], 1);
    let revision = svc.inner.revisions().unwrap().1 as u64;
    svc.inner
        .db()
        .execute(
            "UPDATE main.dir SET listing_outcome='stale' WHERE name='project'",
            [],
        )
        .unwrap();
    refused(add(gen, revision), ErrorCode::StaleGeneration);
    svc.inner
        .db()
        .execute(
            "UPDATE main.dir SET listing_outcome=NULL,born_run=born_run+1 WHERE name='project'",
            [],
        )
        .unwrap();
    refused(add(gen, revision), ErrorCode::NotFound);
    assert_eq!(svc.inner.revisions().unwrap().1 as u64, revision);
    let root_row = svc.inner.root_row(&root).unwrap();
    svc.inner
        .catalog
        .writer()
        .call(WriteCommand::RevokeGrant {
            grant_id: root_row,
            revoked_at_ns: 2,
        })
        .unwrap();
    refused(discover(&svc, &root, 50, Value::Null), ErrorCode::NotFound);
}
