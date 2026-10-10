use loomward_catalog::*;
use rusqlite::Connection;

fn observation() -> RootObservation {
    RootObservation {
        volume_key: "fixture-volume".into(),
        display_name: "Synthetic volume".into(),
        display_path: "Synthetic root".into(),
        root_file_id: Some(vec![1; 16]),
        filesystem: Some("NTFS".into()),
        origin: "fixture".into(),
        granted_via: "fixture".into(),
        observed_at_ns: 1,
    }
}

fn register_root(catalog: &Catalog) -> (i64, i64, i64) {
    let WriteReply::Root {
        grant_id,
        root_id,
        dir_id,
    } = catalog
        .writer()
        .call(WriteCommand::RegisterRoot(observation()))
        .unwrap()
    else {
        panic!("RegisterRoot did not return a root")
    };
    (grant_id, root_id, dir_id)
}

#[test]
fn unknown_dataset_class_is_refused_before_any_file_is_created() {
    let temp = tempfile::tempdir().unwrap();
    for dataset in ["work", "", "SYNTHETIC"] {
        match Catalog::open(temp.path(), dataset).err() {
            Some(Error::Invalid(message)) => {
                assert_eq!(message, "dataset class", "{dataset:?}")
            }
            other => panic!("dataset {dataset:?} must be refused, got {other:?}"),
        }
    }
    assert_eq!(
        std::fs::read_dir(temp.path()).unwrap().count(),
        0,
        "a refused open must not create state"
    );
    let catalog = Catalog::open(temp.path(), "synthetic").unwrap();
    assert!(temp.path().join("catalog.db").exists());
    assert!(temp.path().join("state.db").exists());
    let class: String = Connection::open(temp.path().join("catalog.db"))
        .unwrap()
        .query_row(
            "SELECT value FROM meta WHERE key='dataset_class'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(class, "synthetic");
    drop(catalog);
}

#[test]
fn unc_and_device_state_paths_are_refused_with_exact_message() {
    for path in [
        r"\\server\x",
        r"\\.\x",
        r"\\?\UNC\server\share",
        "//server/share",
    ] {
        match Catalog::open(std::path::Path::new(path), "synthetic").err() {
            Some(Error::Invalid(message)) => {
                assert_eq!(message, "network or device database path", "{path:?}")
            }
            other => panic!("path {path:?} must be refused, got {other:?}"),
        }
    }
}

#[test]
fn verbatim_device_namespace_state_paths_are_refused() {
    for path in [
        r"\\?\GLOBALROOT\Device\NoSuchDevice\state",
        r"\\?\Volume{00000000-0000-0000-0000-000000000000}\state",
        r"\??\C:\state",
        r"\\?\C:state",
        "//?/GLOBALROOT/Device/fixture/state",
    ] {
        match Catalog::open(std::path::Path::new(path), "synthetic").err() {
            Some(Error::Invalid(message)) => assert_eq!(message, "network or device database path"),
            other => panic!("path {path:?} must be refused as a device path, got {other:?}"),
        }
    }
}

#[test]
fn synthetic_catalog_reopens_with_stable_instance() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(temp.path(), "synthetic").unwrap();
    let instance: String = Connection::open(temp.path().join("catalog.db"))
        .unwrap()
        .query_row("SELECT value FROM meta WHERE key='instance_id'", [], |r| {
            r.get(0)
        })
        .unwrap();
    drop(catalog);
    let catalog = Catalog::open(temp.path(), "synthetic").unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT value FROM meta WHERE key='instance_id'", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        instance
    );
    assert_eq!(
        db.query_row(
            "SELECT value FROM meta WHERE key='dataset_class'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "synthetic"
    );
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    drop(catalog);
}

#[test]
fn active_grant_derives_a_never_scanned_root() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(temp.path(), "synthetic").unwrap();
    let (grant, root, dir) = register_root(&catalog);
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT state FROM root WHERE id=?1", [root], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "never_scanned"
    );
    assert_eq!(
        db.query_row("SELECT grant_id FROM root WHERE id=?1", [root], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        grant
    );
    assert_eq!(
        db.query_row(
            "SELECT display_name FROM volume WHERE volume_key='fixture-volume'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "Synthetic volume"
    );
    assert_eq!(
        db.query_row("SELECT name FROM dir WHERE id=?1", [dir], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "Synthetic volume"
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM dir WHERE root_id=?1 AND parent_id IS NULL",
            [root],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let state = Connection::open(temp.path().join("state.db")).unwrap();
    assert_eq!(
        state
            .query_row("SELECT state FROM root_grant WHERE id=?1", [grant], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        "active"
    );
    assert!(
        matches!(
            catalog.writer().call(WriteCommand::BeginRun {
                grant_id: i64::MAX,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 2,
            }),
            Err(Error::NotFound)
        ),
        "a run for an unknown grant must answer NotFound"
    );
    drop(catalog);
}

#[test]
fn revoked_grant_answers_not_found_and_stays_stale_across_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(temp.path(), "synthetic").unwrap();
    let (grant, root, _) = register_root(&catalog);
    assert!(
        matches!(
            catalog.writer().call(WriteCommand::RevokeGrant {
                grant_id: 9999,
                revoked_at_ns: 2,
            }),
            Err(Error::NotFound)
        ),
        "revoking an unknown grant must answer NotFound"
    );
    catalog
        .writer()
        .call(WriteCommand::RevokeGrant {
            grant_id: grant,
            revoked_at_ns: 3,
        })
        .unwrap();
    assert!(
        matches!(
            catalog.writer().call(WriteCommand::BeginRun {
                grant_id: grant,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 4,
            }),
            Err(Error::NotFound)
        ),
        "a revoked grant must not start a run"
    );
    let state = Connection::open(temp.path().join("state.db")).unwrap();
    assert_eq!(
        state
            .query_row("SELECT state FROM root_grant WHERE id=?1", [grant], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        "revoked"
    );
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT state FROM root WHERE id=?1", [root], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "stale"
    );
    drop(state);
    drop(db);
    drop(catalog);
    let catalog = Catalog::open(temp.path(), "synthetic").unwrap();
    let state = Connection::open(temp.path().join("state.db")).unwrap();
    assert_eq!(
        state
            .query_row("SELECT state FROM root_grant WHERE id=?1", [grant], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        "revoked"
    );
    assert_eq!(
        state
            .query_row(
                "SELECT count(*) FROM root_grant WHERE state='active'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT state FROM root WHERE id=?1", [root], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "stale"
    );
    assert!(
        matches!(
            catalog.writer().call(WriteCommand::BeginRun {
                grant_id: grant,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 5,
            }),
            Err(Error::NotFound)
        ),
        "a revoked grant must still answer NotFound after reopen"
    );
    drop(catalog);
}

#[test]
fn mismatched_witness_token_discards_pending_reference_and_keeps_binding() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(temp.path(), "synthetic").unwrap();
    let (grant, root, dir) = register_root(&catalog);
    let WriteReply::Run(run) = catalog
        .writer()
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "full".into(),
            strategy: "fixture".into(),
            started_at_ns: 2,
        })
        .unwrap()
    else {
        panic!()
    };
    let mut file = Observation::file("pinned", 10, Some(10));
    file.file_id = Some(vec![9; 16]);
    file.created_ft = Some(42);
    catalog
        .writer()
        .call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id: dir,
            files: vec![file],
            dirs: vec![],
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        }))
        .unwrap();
    let file_row: i64 = Connection::open(temp.path().join("catalog.db"))
        .unwrap()
        .query_row("SELECT id FROM file WHERE name='pinned'", [], |r| r.get(0))
        .unwrap();
    let WriteReply::Reference(reference) = catalog
        .writer()
        .call(WriteCommand::ObjectReference {
            node: NodeKey::File(file_row),
            observed_at_ns: 5,
        })
        .unwrap()
    else {
        panic!()
    };
    catalog
        .writer()
        .call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 6,
        })
        .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT state FROM root WHERE id=?1", [root], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "complete"
    );
    let instance: String = db
        .query_row("SELECT value FROM meta WHERE key='instance_id'", [], |r| {
            r.get(0)
        })
        .unwrap();
    drop(db);
    let state = Connection::open(temp.path().join("state.db")).unwrap();
    let binding: (String, i64, Option<i64>, Option<i64>, String) = state
        .query_row(
            "SELECT state,retired,row_id,born_run,catalog_instance FROM object_ref WHERE id=?1",
            [reference],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(binding.0, "resolved");
    assert_eq!(binding.1, 0);
    assert_eq!(binding.4, instance);
    assert_eq!(
        state
            .query_row("SELECT count(*) FROM pending_reference", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let state_rev: i64 = state
        .query_row("SELECT state_rev FROM revision WHERE id=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    drop(state);
    drop(catalog);
    Connection::open(temp.path().join("catalog.db"))
        .unwrap()
        .execute(
            "INSERT INTO meta(key,value) VALUES('reference_publication','witness-token-1')",
            [],
        )
        .unwrap();
    let state = Connection::open(temp.path().join("state.db")).unwrap();
    state
        .execute(
            "INSERT INTO pending_reference(object_ref_id,catalog_instance,publication_token) VALUES(?1,?2,'mismatched-token-9')",
            rusqlite::params![reference, instance],
        )
        .unwrap();
    assert_eq!(
        state
            .query_row("SELECT count(*) FROM pending_reference", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(state);
    let catalog = Catalog::open(temp.path(), "synthetic").unwrap();
    let state = Connection::open(temp.path().join("state.db")).unwrap();
    assert_eq!(
        state
            .query_row("SELECT count(*) FROM pending_reference", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0,
        "a token that misses the witness must be discarded on open"
    );
    assert_eq!(
        state
            .query_row(
                "SELECT state,retired,row_id,born_run,catalog_instance FROM object_ref WHERE id=?1",
                [reference],
                |r| Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, String>(4)?
                ))
            )
            .unwrap(),
        binding,
        "the original binding must survive a mismatched token"
    );
    assert_eq!(
        state
            .query_row("SELECT state_rev FROM revision WHERE id=1", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        state_rev,
        "discarding a mismatched intent must not bump the revision"
    );
    drop(catalog);
}
