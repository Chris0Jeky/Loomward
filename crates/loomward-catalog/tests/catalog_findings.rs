//! Regression probes for the 2026-10-17 catalogue bug-hunt claims.
use loomward_catalog::*;
use rusqlite::{params, Connection};

fn root(c: &Catalog, id: u8) -> (i64, i64) {
    let WriteReply::Root {
        grant_id, dir_id, ..
    } = c
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "synthetic-volume".into(),
            display_name: format!("Synthetic root {id}"),
            display_path: format!("Synthetic root {id}"),
            root_file_id: Some(vec![id; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 1,
        }))
        .unwrap()
    else {
        panic!()
    };
    (grant_id, dir_id)
}

fn run(c: &Catalog, grant: i64) -> i64 {
    let WriteReply::Run(id) = c
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
    id
}

fn finish(c: &Catalog, run: i64) {
    c.writer()
        .call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 3,
        })
        .unwrap();
}

fn entry(name: &str, id: u8, created: i64) -> Observation {
    let mut o = Observation::file(name, 10, Some(4096));
    o.file_id = Some(vec![id; 16]);
    o.created_ft = Some(created);
    o
}

fn listing(run: i64, dir: i64, files: Vec<Observation>, dirs: Vec<Observation>) -> WriteCommand {
    WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: dir,
        files,
        dirs,
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    })
}

fn reference(c: &Catalog, node: NodeKey) -> i64 {
    let WriteReply::Reference(id) = c
        .writer()
        .call(WriteCommand::ObjectReference {
            node,
            observed_at_ns: 4,
        })
        .unwrap()
    else {
        panic!()
    };
    id
}

#[test]
fn forgetting_reused_native_id_preserves_other_roots_retired_reference() {
    for directory in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let c = Catalog::open(temp.path(), "synthetic").unwrap();
        c.writer().set_bulk_first_scan(false);
        let (grant_b, dir_b) = root(&c, 2);
        let old = entry("retired-object", 10, 100);
        let scan = run(&c, grant_b);
        c.writer()
            .call(if directory {
                listing(scan, dir_b, vec![], vec![old])
            } else {
                listing(scan, dir_b, vec![old], vec![])
            })
            .unwrap();
        // A complete traversal publishes the child directory before retiring it in the next scan.
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        let table = if directory { "dir" } else { "file" };
        let old_id: i64 = db
            .query_row(
                &format!("SELECT id FROM {table} WHERE name='retired-object'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        if directory {
            c.writer()
                .call(listing(scan, old_id, vec![], vec![]))
                .unwrap();
        }
        finish(&c, scan);
        let reference = reference(
            &c,
            if directory {
                NodeKey::Dir(old_id)
            } else {
                NodeKey::File(old_id)
            },
        );
        let st = Connection::open(temp.path().join("state.db")).unwrap();
        st.execute_batch(
            "INSERT INTO collection(name,kind,created_at_ns) VALUES('keep','human',0)",
        )
        .unwrap();
        st.execute("INSERT INTO collection_member VALUES(1,?1,0)", [reference])
            .unwrap();
        let scan = run(&c, grant_b);
        c.writer()
            .call(listing(scan, dir_b, vec![], vec![]))
            .unwrap();
        finish(&c, scan);
        assert_eq!(
            st.query_row(
                "SELECT state FROM object_ref WHERE id=?1",
                [reference],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "unresolved"
        );
        assert_eq!(
            db.query_row(
                &format!("SELECT count(*) FROM {table} WHERE id=?1"),
                [old_id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );

        let (grant_a, dir_a) = root(&c, 1);
        let scan = run(&c, grant_a);
        let reused = entry("replacement-object", 10, 200);
        c.writer()
            .call(if directory {
                listing(scan, dir_a, vec![], vec![reused])
            } else {
                listing(scan, dir_a, vec![reused], vec![])
            })
            .unwrap();
        finish(&c, scan);
        c.writer()
            .call(WriteCommand::RevokeGrant {
                grant_id: grant_a,
                revoked_at_ns: 5,
            })
            .unwrap();
        c.writer()
            .call(WriteCommand::ForgetGrant {
                grant_id: grant_a,
                forgotten_at_ns: 6,
                expected_state_rev: None,
            })
            .unwrap();
        assert_eq!(st.query_row("SELECT count(*) FROM object_ref WHERE id=?1 AND state='unresolved' AND creation_ft=100", [reference], |r| r.get::<_, i64>(0)).unwrap(), 1, "directory={directory}");
        assert_eq!(
            st.query_row(
                "SELECT count(*) FROM collection_member WHERE object_ref_id=?1",
                [reference],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
}

#[test]
fn forgetting_native_id_reuse_does_not_treat_distinct_objects_as_hardlinks() {
    for (directory, other_directory, other_created) in [
        (false, false, 200),
        (true, true, 200),
        (false, true, 100),
        (true, false, 100),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let c = Catalog::open(temp.path(), "synthetic").unwrap();
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        let st = Connection::open(temp.path().join("state.db")).unwrap();
        let mut grants = Vec::new();
        let mut references = Vec::new();
        for (id, created) in [(1, 100), (2, other_created)] {
            let directory = if id == 1 { directory } else { other_directory };
            let (grant, dir) = root(&c, id);
            let scan = run(&c, grant);
            let o = entry("reused-id", 10, created);
            c.writer()
                .call(if directory {
                    listing(scan, dir, vec![], vec![o])
                } else {
                    listing(scan, dir, vec![o], vec![])
                })
                .unwrap();
            finish(&c, scan);
            let key: i64 = db
                .query_row(
                    if directory {
                        "SELECT id FROM dir WHERE parent_id=?1"
                    } else {
                        "SELECT id FROM file WHERE dir_id=?1"
                    },
                    [dir],
                    |r| r.get(0),
                )
                .unwrap();
            references.push(reference(
                &c,
                if directory {
                    NodeKey::Dir(key)
                } else {
                    NodeKey::File(key)
                },
            ));
            grants.push(grant);
        }
        c.writer()
            .call(WriteCommand::RevokeGrant {
                grant_id: grants[0],
                revoked_at_ns: 5,
            })
            .unwrap();
        c.writer()
            .call(WriteCommand::ForgetGrant {
                grant_id: grants[0],
                forgotten_at_ns: 6,
                expected_state_rev: None,
            })
            .unwrap();
        let kept: Vec<i64> = st
            .prepare("SELECT id FROM object_ref ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            kept,
            vec![references[1]],
            "directory={directory}: only the other incarnation's reference survives"
        );
    }
}

#[test]
fn mixed_staged_and_direct_listings_accept_distinct_names() {
    for bulk in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let c = Catalog::open(temp.path(), "synthetic").unwrap();
        c.writer().set_bulk_first_scan(bulk);
        let (grant, dir) = root(&c, 1);
        let scan = run(&c, grant);
        c.writer()
            .call(WriteCommand::StageChunk {
                run_id: scan,
                dir_id: dir,
                seq: 0,
                files: vec![entry("staged", 10, 100)],
                dirs: vec![],
            })
            .unwrap();
        c.writer()
            .call(listing(scan, dir, vec![entry("direct", 11, 100)], vec![]))
            .unwrap();
        finish(&c, scan);
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}

#[test]
fn mixed_staged_and_direct_listings_reject_duplicate_names_atomically() {
    for bulk in [false, true] {
        for staged_dir in [false, true] {
            for direct_dir in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let c = Catalog::open(temp.path(), "synthetic").unwrap();
                c.writer().set_bulk_first_scan(bulk);
                let (grant, dir) = root(&c, 1);
                let scan = run(&c, grant);
                let staged = entry("duplicate", 10, 100);
                c.writer()
                    .call(WriteCommand::StageChunk {
                        run_id: scan,
                        dir_id: dir,
                        seq: 0,
                        files: if staged_dir {
                            vec![]
                        } else {
                            vec![staged.clone()]
                        },
                        dirs: if staged_dir { vec![staged] } else { vec![] },
                    })
                    .unwrap();
                let direct = entry("duplicate", 11, 100);
                let result = c.writer().call(if direct_dir {
                    listing(scan, dir, vec![], vec![direct])
                } else {
                    listing(scan, dir, vec![direct], vec![])
                });
                assert!(
                    matches!(result, Err(Error::Invalid(_))),
                    "bulk={bulk}, staged_dir={staged_dir}, direct_dir={direct_dir}: {result:?}"
                );
                let db = Connection::open(temp.path().join("catalog.db")).unwrap();
                let count: i64 = db.query_row("SELECT (SELECT count(*) FROM file)+(SELECT count(*) FROM bulk_file)+(SELECT count(*) FROM dir WHERE parent_id IS NOT NULL)+(SELECT count(*) FROM bulk_dir WHERE parent_id IS NOT NULL)", [], |r| r.get(0)).unwrap();
                assert_eq!(count, 0, "a rejected listing must roll back both inputs");
            }
        }
    }
}

#[test]
fn allocated_slice_exposes_unknown_separately_from_layout_zero() {
    let temp = tempfile::tempdir().unwrap();
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    let (grant, dir) = root(&c, 1);
    let scan = run(&c, grant);
    c.writer()
        .call(listing(
            scan,
            dir,
            vec![
                Observation::file("unknown", 10, None),
                Observation::file("known-zero", 10, Some(0)),
            ],
            vec![],
        ))
        .unwrap();
    finish(&c, scan);
    let slice = c
        .reader()
        .unwrap()
        .slice(&SliceRequest {
            anchor: NodeKey::Dir(dir),
            depth: 1,
            max_nodes: 16,
            min_share: 0.0,
            basis: Basis::Allocated,
            include_files: true,
        })
        .unwrap();
    let unknown = slice.nodes.iter().find(|n| n.name == "unknown").unwrap();
    let zero = slice.nodes.iter().find(|n| n.name == "known-zero").unwrap();
    assert_eq!(unknown.size_bytes, "0");
    assert_eq!(unknown.allocated_bytes, None);
    assert_eq!(unknown.size_unknown_files, 1);
    assert_eq!(zero.size_bytes, "0");
    assert_eq!(zero.allocated_bytes.as_deref(), Some("0"));
    assert_eq!(zero.size_unknown_files, 0);
}

#[test]
fn search_tracks_file_upsert_and_directory_update_renames() {
    for bulk in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let c = Catalog::open(temp.path(), "synthetic").unwrap();
        c.writer().set_bulk_first_scan(bulk);
        let (grant, dir) = root(&c, 1);
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        let scan = run(&c, grant);
        c.writer()
            .call(listing(
                scan,
                dir,
                vec![entry("oldfiletoken", 10, 100)],
                vec![entry("olddirtoken", 11, 100)],
            ))
            .unwrap();
        finish(&c, scan);
        let file: i64 = db
            .query_row("SELECT id FROM file", [], |r| r.get(0))
            .unwrap();
        let child: i64 = db
            .query_row("SELECT id FROM dir WHERE parent_id=?1", [dir], |r| r.get(0))
            .unwrap();
        let mut reader = c.reader().unwrap();
        let mut search = |text: &str| {
            reader
                .search(&SearchRequest {
                    text: text.into(),
                    kind: "any".into(),
                    limit: 10,
                    work_budget: 10_000,
                    ..SearchRequest::default()
                })
                .unwrap()
                .hits
        };
        assert_eq!(search("oldfiletoken").len(), 1);
        assert_eq!(search("olddirtoken").len(), 1);
        let scan = run(&c, grant);
        c.writer()
            .call(listing(
                scan,
                dir,
                vec![entry("newfiletoken", 10, 100)],
                vec![entry("newdirtoken", 11, 100)],
            ))
            .unwrap();
        finish(&c, scan);
        assert_eq!(
            db.query_row("SELECT name FROM file WHERE id=?1", [file], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "newfiletoken"
        );
        assert_eq!(
            db.query_row("SELECT name FROM dir WHERE id=?1", [child], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "newdirtoken"
        );
        assert_eq!(search("newfiletoken").len(), 1);
        assert_eq!(search("newdirtoken").len(), 1);
        assert!(search("oldfiletoken").is_empty());
        assert!(search("olddirtoken").is_empty());
        // Check index terms directly too: a stale term must not merely be filtered by the row name.
        for (id, name) in [(file + (1 << 48), "oldfiletoken"), (child, "olddirtoken")] {
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM search_name WHERE rowid=?1 AND search_name MATCH ?2",
                    params![id, name],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
        }
    }
}

#[test]
fn forgetting_purges_a_migrated_v1_reference_matched_by_native_id_alone() {
    // A v1 state.db migrates its references with creation_ft=0, unresolved and no bound row
    // (db.rs): forget must still purge them and their memberships by native id, as before #499.
    let temp = tempfile::tempdir().unwrap();
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    c.writer().set_bulk_first_scan(false);
    let (grant, dir) = root(&c, 1);
    let scan = run(&c, grant);
    c.writer()
        .call(listing(
            scan,
            dir,
            vec![entry("legacy-object", 10, 100)],
            vec![],
        ))
        .unwrap();
    finish(&c, scan);
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let file: i64 = db
        .query_row("SELECT id FROM file WHERE name='legacy-object'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let reference = reference(&c, NodeKey::File(file));
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    st.execute(
        "UPDATE object_ref SET creation_ft=0,state='unresolved',retired=1,row_id=NULL,catalog_instance=NULL WHERE id=?1",
        [reference],
    )
    .unwrap();
    st.execute_batch("INSERT INTO collection(name,kind,created_at_ns) VALUES('legacy','human',0)")
        .unwrap();
    st.execute("INSERT INTO collection_member VALUES(1,?1,0)", [reference])
        .unwrap();
    c.writer()
        .call(WriteCommand::RevokeGrant {
            grant_id: grant,
            revoked_at_ns: 5,
        })
        .unwrap();
    c.writer()
        .call(WriteCommand::ForgetGrant {
            grant_id: grant,
            forgotten_at_ns: 6,
            expected_state_rev: None,
        })
        .unwrap();
    let left = |sql: &str| {
        st.query_row(sql, [reference], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    assert_eq!(
        left("SELECT count(*) FROM object_ref WHERE id=?1"),
        0,
        "the migrated reference survived the forget"
    );
    assert_eq!(
        left("SELECT count(*) FROM collection_member WHERE object_ref_id=?1"),
        0
    );
}
