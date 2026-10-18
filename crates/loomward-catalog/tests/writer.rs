use loomward_catalog::*;
use rusqlite::Connection;

fn setup() -> (tempfile::TempDir, Catalog, i64, i64, i64) {
    let temp = tempfile::tempdir().unwrap();
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    let WriteReply::Root {
        grant_id, dir_id, ..
    } = c
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "fixture-volume".into(),
            display_name: "Synthetic volume".into(),
            display_path: "Synthetic root".into(),
            root_file_id: Some([1; 16].to_vec()),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 1,
        }))
        .unwrap()
    else {
        panic!()
    };
    let WriteReply::Run(run) = c
        .writer()
        .call(WriteCommand::BeginRun {
            grant_id,
            mode: "full".into(),
            strategy: "fixture".into(),
            started_at_ns: 2,
        })
        .unwrap()
    else {
        panic!()
    };
    (temp, c, grant_id, dir_id, run)
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
fn identified(name: &str, id: u8, size: u64) -> Observation {
    let mut o = Observation::file(name, size, Some(size));
    o.file_id = Some([id; 16].to_vec());
    o
}

#[test]
fn rejected_chunk_fences_queued_completion_without_deleting_old_entries() {
    for with_reference in [false, true] {
        let (temp, c, grant, root, run) = setup();
        let w = c.writer();
        let mut old = identified("old", 2, 10);
        old.created_ft = Some(1);
        w.call(listing(
            run,
            root,
            vec![old],
            vec![identified("child", 3, 0)],
        ))
        .unwrap();
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        let file: i64 = db
            .query_row("SELECT id FROM file", [], |r| r.get(0))
            .unwrap();
        let child: i64 = db
            .query_row("SELECT id FROM dir WHERE name='child'", [], |r| r.get(0))
            .unwrap();
        if with_reference {
            w.call(WriteCommand::ObjectReference {
                node: NodeKey::File(file),
                observed_at_ns: 2,
            })
            .unwrap();
        }
        w.call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 3,
        })
        .unwrap();
        let WriteReply::Run(run) = w
            .call(WriteCommand::BeginRun {
                grant_id: grant,
                mode: "refresh".into(),
                strategy: "fixture".into(),
                started_at_ns: 4,
            })
            .unwrap()
        else {
            panic!()
        };
        let good = w
            .send(WriteCommand::StageChunk {
                run_id: run,
                dir_id: root,
                seq: 0,
                files: vec![identified("accepted", 4, 20)],
                dirs: vec![],
            })
            .unwrap();
        let bad = w
            .send(WriteCommand::StageChunk {
                run_id: run,
                dir_id: root,
                seq: 2,
                files: vec![identified("rejected", 5, 30)],
                dirs: vec![],
            })
            .unwrap();
        let done = w
            .send(WriteCommand::ListingDone {
                run_id: run,
                dir_id: root,
                outcome: ListingOutcome::Complete,
                skipped: 0,
                errors: 0,
            })
            .unwrap();
        // A different directory remains publishable even after this listing fails.
        let other = w
            .send(WriteCommand::ListingDone {
                run_id: run,
                dir_id: child,
                outcome: ListingOutcome::Complete,
                skipped: 0,
                errors: 0,
            })
            .unwrap();
        good.wait().unwrap();
        assert!(bad.wait().is_err());
        done.wait().unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM file WHERE id=?1", [file], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        other.wait().unwrap();
        assert_eq!(
            db.query_row("SELECT listing_outcome FROM dir WHERE id=?1", [root], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "partial"
        );
        assert!(matches!(
            w.call(WriteCommand::StageChunk {
                run_id: run,
                dir_id: root,
                seq: 1,
                files: vec![],
                dirs: vec![]
            }),
            Err(Error::Invalid(_))
        ));
        w.call(WriteCommand::ListingDone {
            run_id: run,
            dir_id: root,
            outcome: ListingOutcome::Incomplete("denied".into()),
            skipped: 0,
            errors: 1,
        })
        .unwrap();
        assert_eq!(
            db.query_row("SELECT listing_outcome FROM dir WHERE id=?1", [root], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "denied"
        );
        w.call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 5,
        })
        .unwrap();
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM dir WHERE id=?1 AND listing_state='complete'",
                [child],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_ne!(
            db.query_row("SELECT listing_state FROM dir WHERE id=?1", [root], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            "complete"
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM file WHERE id=?1", [file], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM file WHERE name='rejected'", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        if with_reference {
            let state = Connection::open(temp.path().join("state.db")).unwrap();
            assert_eq!(
                state
                    .query_row("SELECT state FROM object_ref", [], |r| r
                        .get::<_, String>(0))
                    .unwrap(),
                "resolved"
            );
        }
        assert_eq!(w.queued_bytes(), 0);
    }
}

fn check_lab_root_guid(registered_guid: Option<&str>, observed_guid: Option<&str>, accepted: bool) {
    let temp = tempfile::tempdir().unwrap();
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    let state = Connection::open(temp.path().join("state.db")).unwrap();
    state.execute(
            "INSERT INTO lab_root(id,volume_key,root_file_id,manifest_digest,seed,tier,registered_at_ns,volume_guid) VALUES(1,'fixture',?1,'synthetic-digest','seed','small',0,?2)",
            rusqlite::params![vec![1u8; 16], registered_guid],
        ).unwrap();
    let result = c.writer().call(WriteCommand::RegisterRoot(RootObservation {
        volume_guid: observed_guid.map(str::to_owned),
        volume_key: "fixture".into(),
        display_name: "Fixture".into(),
        display_path: "Fixture root".into(),
        root_file_id: Some(vec![1; 16]),
        filesystem: Some("NTFS".into()),
        origin: "lab_generated".into(),
        granted_via: "fixture".into(),
        observed_at_ns: 0,
    }));
    if accepted {
        assert!(matches!(result, Ok(WriteReply::Root { .. })), "{result:?}");
    } else {
        assert!(
            matches!(result, Err(Error::Invalid("unregistered lab root"))),
            "registered={registered_guid:?}, observed={observed_guid:?}: {result:?}"
        );
    }
    assert_eq!(
        state
            .query_row("SELECT count(*) FROM root_grant", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        i64::from(accepted)
    );
}

#[test]
fn lab_root_mismatched_guid_is_refused() {
    check_lab_root_guid(Some("lab-guid"), Some("other-guid"), false);
}

#[test]
fn lab_root_legacy_null_guid_is_refused() {
    check_lab_root_guid(None, Some("lab-guid"), false);
}

#[test]
fn lab_root_missing_observed_guid_is_refused() {
    check_lab_root_guid(Some("lab-guid"), None, false);
    check_lab_root_guid(None, None, false);
}

#[test]
fn lab_root_matching_guid_is_accepted() {
    check_lab_root_guid(Some("lab-guid"), Some("lab-guid"), true);
}

#[test]
fn root_origins_must_match_the_dataset_before_any_grant_is_written() {
    for dataset in ["synthetic", "personal"] {
        for origin in ["fixture", "lab_generated", "owner_granted"] {
            let temp = tempfile::tempdir().unwrap();
            let c = Catalog::open(temp.path(), dataset).unwrap();
            let state = Connection::open(temp.path().join("state.db")).unwrap();
            if origin == "lab_generated" {
                state.execute("INSERT INTO lab_root(id,volume_key,root_file_id,manifest_digest,seed,tier,registered_at_ns,volume_guid) VALUES(1,'fixture',?1,'synthetic-digest','seed','small',0,'lab-guid')", [vec![1u8;16]]).unwrap();
            }
            let accepted = (dataset == "personal") == (origin == "owner_granted");
            let result = c.writer().call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: Some("lab-guid".into()),
                volume_key: "fixture".into(),
                display_name: "Fixture".into(),
                display_path: "Fixture root".into(),
                root_file_id: Some(vec![1; 16]),
                filesystem: Some("NTFS".into()),
                origin: origin.into(),
                granted_via: if origin == "owner_granted" {
                    "cli_flag"
                } else {
                    "fixture"
                }
                .into(),
                observed_at_ns: 0,
            }));
            assert_eq!(result.is_ok(), accepted, "{dataset}/{origin}: {result:?}");
            assert_eq!(
                state
                    .query_row("SELECT count(*) FROM root_grant", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                i64::from(accepted)
            );
        }
    }
}

#[test]
fn invalid_incomplete_reason_cannot_publish_or_delete() {
    let (temp, c, _, root, run) = setup();
    c.writer()
        .call(listing(run, root, vec![identified("old", 2, 10)], vec![]))
        .unwrap();
    for reason in ["", "complete", "invented"] {
        assert!(matches!(
            c.writer().call(WriteCommand::ListingDone {
                run_id: run,
                dir_id: root,
                outcome: ListingOutcome::Incomplete(reason.into()),
                skipped: 0,
                errors: 0
            }),
            Err(Error::Invalid(_))
        ));
    }
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT name FROM file", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "old"
    );
    assert_eq!(
        db.query_row("SELECT listing_state FROM dir WHERE id=?1", [root], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "complete"
    );
}

#[test]
fn refresh_preserves_identity_rename_swaps_and_cascades_removed_subtrees() {
    let (temp, c, grant, root, run) = setup();
    let w = c.writer();
    w.call(listing(
        run,
        root,
        vec![identified("a", 2, 5), identified("b", 3, 7)],
        vec![identified("dir", 4, 0)],
    ))
    .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let file_a: i64 = db
        .query_row("SELECT id FROM file WHERE name='a'", [], |r| r.get(0))
        .unwrap();
    let child: i64 = db
        .query_row("SELECT id FROM dir WHERE name='dir'", [], |r| r.get(0))
        .unwrap();
    w.call(listing(
        run,
        child,
        vec![identified("nested", 5, 13)],
        vec![],
    ))
    .unwrap();
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 3,
    })
    .unwrap();
    let WriteReply::Run(refresh) = w
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "refresh".into(),
            strategy: "fixture".into(),
            started_at_ns: 4,
        })
        .unwrap()
    else {
        panic!()
    };
    w.call(listing(
        refresh,
        root,
        vec![identified("b", 2, 5), identified("a", 3, 7)],
        vec![identified("renamed", 4, 0)],
    ))
    .unwrap();
    assert_eq!(
        db.query_row("SELECT id FROM file WHERE name='b'", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        file_a
    );
    assert_eq!(
        db.query_row("SELECT id FROM dir WHERE name='renamed'", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        child
    );
    let revision: i64 = db
        .query_row("SELECT listing_rev FROM dir WHERE id=?1", [root], |r| {
            r.get(0)
        })
        .unwrap();
    w.call(listing(
        refresh,
        root,
        vec![identified("b", 2, 5), identified("a", 3, 7)],
        vec![identified("renamed", 4, 0)],
    ))
    .unwrap();
    assert_eq!(
        db.query_row("SELECT listing_rev FROM dir WHERE id=?1", [root], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        revision
    );
    w.call(listing(
        refresh,
        root,
        vec![identified("replacement", 6, 9)],
        vec![],
    ))
    .unwrap();
    assert_eq!(
        db.query_row("SELECT listing_state FROM dir WHERE id=?1", [child], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "absent_pending"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM file WHERE name='nested'", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        1
    );
    w.call(WriteCommand::EndRun {
        run_id: refresh,
        state: "completed".into(),
        finished_at_ns: 5,
    })
    .unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM dir WHERE id=?1", [child], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(
        db.query_row("SELECT id FROM file", [], |r| r.get::<_, i64>(0))
            .unwrap()
            > file_a
    );
}

#[test]
fn unknown_allocation_hardlinks_and_failed_run_rollup_are_distinct() {
    let (temp, c, _, root, run) = setup();
    c.writer()
        .call(listing(
            run,
            root,
            vec![
                identified("one", 2, 10),
                identified("link", 2, 10),
                Observation::file("sparse", 100, Some(0)),
                Observation::file("unknown", 20, None),
            ],
            vec![identified("denied", 3, 0)],
        ))
        .unwrap();
    c.writer()
        .call(WriteCommand::EndRun {
            run_id: run,
            state: "cancelled".into(),
            finished_at_ns: 3,
        })
        .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let sums: (i64, i64, i64, i64) = db
        .query_row(
            "SELECT sub_logical,sub_allocated,sub_alloc_unknown,sub_complete FROM dir WHERE id=?1",
            [root],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(sums, (140, 20, 1, 0));
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM file WHERE flags & ?1 != 0",
            [HARDLINK_SUSPECTED],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
}

#[test]
fn restart_isolates_an_overdeep_root_and_keeps_healthy_reads_available() {
    let (temp, c, _, bad_dir, _) = setup();
    let WriteReply::Root {
        grant_id,
        dir_id: good_dir,
        ..
    } = c
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "healthy-volume".into(),
            display_name: "Healthy fixture".into(),
            display_path: "Healthy root".into(),
            root_file_id: Some(vec![2; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 1,
        }))
        .unwrap()
    else {
        panic!()
    };
    let WriteReply::Run(run) = c
        .writer()
        .call(WriteCommand::BeginRun {
            grant_id,
            mode: "full".into(),
            strategy: "fixture".into(),
            started_at_ns: 2,
        })
        .unwrap()
    else {
        panic!()
    };
    c.writer()
        .call(listing(
            run,
            good_dir,
            vec![identified("healthy", 3, 7)],
            vec![],
        ))
        .unwrap();
    c.writer()
        .call(WriteCommand::StageChunk {
            run_id: run,
            dir_id: good_dir,
            seq: 0,
            files: vec![identified("unpublished", 4, 9)],
            dirs: vec![],
        })
        .unwrap();
    drop(c);
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    // An overdeep chain left by the legacy reparent bug, including stale valid aggregates.
    db.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<600)
         INSERT INTO dir(id,root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run)
         SELECT i+1000,(SELECT root_id FROM dir WHERE id=?1),CASE i WHEN 1 THEN ?1 ELSE i+999 END,
                'deep'||i,'path_observation','none',i,0,'complete',1,1 FROM n",
        [bad_dir],
    ).unwrap();
    db.execute("UPDATE dir SET sub_complete=1 WHERE id=?1", [bad_dir])
        .unwrap();
    for _ in 0..2 {
        let c =
            Catalog::open(temp.path(), "synthetic").expect("one bad root must not brick the store");
        let mut reader = c.reader().unwrap();
        assert_eq!(
            reader
                .inspect(NodeKey::Dir(good_dir))
                .unwrap()
                .row
                .logical_bytes
                .as_deref(),
            Some("7")
        );
        assert!(matches!(
            reader.inspect(NodeKey::Dir(bad_dir)),
            Err(Error::RepairRequired)
        ));
        for (dir, repair_pending) in [(good_dir, false), (bad_dir, true)] {
            let result = reader.slice(&SliceRequest {
                anchor: NodeKey::Dir(dir),
                depth: 1,
                max_nodes: 16,
                min_share: 0.0,
                basis: Basis::Logical,
                include_files: true,
            });
            if repair_pending {
                assert!(matches!(result, Err(Error::RepairRequired)));
            } else {
                result.unwrap();
            }
        }
        assert_eq!(
            db.query_row(
                "SELECT state FROM root WHERE id=(SELECT root_id FROM dir WHERE id=?1)",
                [bad_dir],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "repairing"
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM scan_run WHERE state='running'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM root WHERE active_run IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM stage_entry", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        c.writer().call(WriteCommand::Barrier).unwrap();
    }
    // Once the tree can be repaired, the existing explicit repair clears the pending state.
    db.execute("UPDATE dir SET parent_id=?1 WHERE id>1000", [bad_dir])
        .unwrap();
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    c.writer()
        .call(WriteCommand::Repair { root_id: 1 })
        .unwrap();
    assert_eq!(
        db.query_row("SELECT state FROM root WHERE id=1", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "partial"
    );
    assert!(c.reader().unwrap().inspect(NodeKey::Dir(bad_dir)).is_ok());
}

#[test]
fn invalid_listing_rolls_back_and_restart_marks_only_interrupted_roots_stale() {
    let (temp, c, _, root, run) = setup();
    c.writer()
        .call(listing(
            run,
            root,
            vec![identified("committed", 2, 5)],
            vec![],
        ))
        .unwrap();
    let bad = Observation::file("oversize", u64::MAX, Some(1));
    assert!(c
        .writer()
        .call(listing(
            run,
            root,
            vec![identified("uncommitted", 3, 7), bad],
            vec![]
        ))
        .is_err());
    drop(c);
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT name FROM file", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "committed"
    );
    assert_eq!(
        db.query_row("SELECT state FROM scan_run WHERE id=?1", [run], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "failed"
    );
    assert_eq!(
        db.query_row("SELECT listing_state FROM dir WHERE id=?1", [root], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "complete"
    );
    assert_eq!(
        db.query_row(
            "SELECT sub_logical,sub_files,sub_complete FROM dir WHERE id=?1",
            [root],
            |r| Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?
            ))
        )
        .unwrap(),
        (5, 1, 0)
    );
    drop(c);
}

#[test]
fn abrupt_exit_mid_transaction_keeps_last_committed_listing() {
    let (temp, c, _, root, run) = setup();
    c.writer()
        .call(listing(
            run,
            root,
            vec![identified("committed", 2, 5)],
            vec![],
        ))
        .unwrap();
    drop(c);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_child", "--nocapture"])
        .env(
            "LOOMWARD_CATALOG_CRASH_FIXTURE",
            temp.path().join("catalog.db"),
        )
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(17));
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT name,logical FROM file", [], |r| Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?
        )))
        .unwrap(),
        ("committed".into(), 5)
    );
    drop(c);
}

#[test]
fn crash_child() {
    let Some(path) = std::env::var_os("LOOMWARD_CATALOG_CRASH_FIXTURE") else {
        return;
    };
    let db = Connection::open(path).unwrap();
    db.execute_batch("BEGIN IMMEDIATE;UPDATE file SET name='uncommitted',logical=999")
        .unwrap();
    // Simulate loss of the process before commit; no destructor can roll this back.
    std::process::exit(17);
}

#[test]
fn one_bad_listing_does_not_erase_other_queued_commits() {
    let (temp, c, _, root, run) = setup();
    let first = c
        .writer()
        .send(listing(run, root, vec![identified("good", 2, 7)], vec![]))
        .unwrap();
    let bad = c
        .writer()
        .send(listing(
            run,
            i64::MAX,
            vec![identified("bad", 3, 13)],
            vec![],
        ))
        .unwrap();
    first.wait().unwrap();
    assert!(bad.wait().is_err());
    c.writer().call(WriteCommand::Barrier).unwrap();
    assert_eq!(c.writer().queued_bytes(), 0);
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT name FROM file", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "good"
    );
}

#[test]
fn directory_identity_can_move_between_parents_without_order_dependent_unique_failure() {
    let (temp, c, grant, root, run) = setup();
    let w = c.writer();
    w.call(listing(
        run,
        root,
        vec![],
        vec![identified("left", 2, 0), identified("right", 3, 0)],
    ))
    .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let left: i64 = db
        .query_row("SELECT id FROM dir WHERE name='left'", [], |r| r.get(0))
        .unwrap();
    let right: i64 = db
        .query_row("SELECT id FROM dir WHERE name='right'", [], |r| r.get(0))
        .unwrap();
    w.call(listing(run, left, vec![], vec![identified("child", 4, 0)]))
        .unwrap();
    let child: i64 = db
        .query_row("SELECT id FROM dir WHERE name='child'", [], |r| r.get(0))
        .unwrap();
    w.call(listing(
        run,
        child,
        vec![identified("kept", 5, 10)],
        vec![identified("deep", 6, 0)],
    ))
    .unwrap();
    let deep: i64 = db
        .query_row("SELECT id FROM dir WHERE name='deep'", [], |r| r.get(0))
        .unwrap();
    w.call(listing(run, deep, vec![], vec![])).unwrap();
    w.call(listing(run, right, vec![], vec![identified("inner", 7, 0)]))
        .unwrap();
    let inner: i64 = db
        .query_row("SELECT id FROM dir WHERE name='inner'", [], |r| r.get(0))
        .unwrap();
    w.call(listing(run, inner, vec![], vec![])).unwrap();
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 3,
    })
    .unwrap();
    let WriteReply::Run(refresh) = w
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "refresh".into(),
            strategy: "fixture".into(),
            started_at_ns: 4,
        })
        .unwrap()
    else {
        panic!()
    };
    w.call(listing(
        refresh,
        inner,
        vec![],
        vec![identified("moved", 4, 0)],
    ))
    .unwrap();
    w.call(listing(refresh, left, vec![], vec![])).unwrap();
    assert_eq!(
        db.query_row("SELECT id,parent_id FROM dir WHERE name='moved'", [], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })
        .unwrap(),
        (child, inner)
    );
    assert_eq!(
        db.query_row("SELECT depth FROM dir WHERE id=?1", [deep], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM file WHERE name='kept'", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(w
        .call(listing(
            refresh,
            child,
            vec![],
            vec![identified("cycle", 3, 0)]
        ))
        .is_err());
    assert_eq!(
        db.query_row("SELECT name FROM dir WHERE id=?1", [right], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "right"
    );
}
#[test]
fn incomplete_listing_never_establishes_absence() {
    let (temp, c, _, root, run) = setup();
    c.writer()
        .call(listing(
            run,
            root,
            vec![identified("kept", 2, 10)],
            vec![identified("child", 3, 0)],
        ))
        .unwrap();
    c.writer()
        .call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id: root,
            files: vec![],
            dirs: vec![],
            state: "partial".into(),
            skipped: 0,
            errors: 1,
        }))
        .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM dir WHERE parent_id IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn source_first_move_retains_descendants_until_destination_is_listed() {
    let (temp, c, _, root, run) = setup();
    let w = c.writer();
    w.call(listing(
        run,
        root,
        vec![],
        vec![identified("A", 2, 0), identified("B", 3, 0)],
    ))
    .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let a: i64 = db
        .query_row("SELECT id FROM dir WHERE name='A'", [], |r| r.get(0))
        .unwrap();
    let b: i64 = db
        .query_row("SELECT id FROM dir WHERE name='B'", [], |r| r.get(0))
        .unwrap();
    w.call(listing(run, a, vec![], vec![identified("x", 4, 0)]))
        .unwrap();
    let x: i64 = db
        .query_row("SELECT id FROM dir WHERE name='x'", [], |r| r.get(0))
        .unwrap();
    w.call(listing(
        run,
        x,
        vec![identified("descendant", 5, 150)],
        vec![],
    ))
    .unwrap();
    w.call(listing(run, a, vec![], vec![])).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM file WHERE name='descendant'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    w.call(listing(run, b, vec![], vec![identified("x", 4, 0)]))
        .unwrap();
    assert_eq!(
        db.query_row("SELECT id FROM dir WHERE name='x'", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        x
    );
}
#[test]
fn obsolete_same_run_final_cannot_publish_current_revision() {
    let (temp, c, _, root, run) = setup();
    let w = c.writer();
    let WriteReply::Listing { revision: old } = w
        .call(listing(run, root, vec![identified("item", 2, 150)], vec![]))
        .unwrap()
    else {
        panic!()
    };
    let WriteReply::Listing { revision: new } = w
        .call(listing(run, root, vec![identified("item", 2, 200)], vec![]))
        .unwrap()
    else {
        panic!()
    };
    assert!(new > old);
    let totals = Totals {
        files: 1,
        logical: 150,
        allocated: 150,
        complete: true,
        ..Totals::default()
    };
    assert!(matches!(
        w.call(WriteCommand::DirFinal {
            run_id: run,
            dir_id: root,
            input_revision: old,
            totals
        }),
        Err(Error::StaleGeneration)
    ));
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT own_logical,agg_valid_rev<dirty_rev FROM dir WHERE id=?1",
            [root],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, bool>(1)?))
        )
        .unwrap(),
        (200, true)
    );
    w.call(WriteCommand::DirFinal {
        run_id: run,
        dir_id: root,
        input_revision: new,
        totals: Totals {
            files: 1,
            logical: 200,
            allocated: 200,
            complete: true,
            ..Totals::default()
        },
    })
    .unwrap();
    assert_eq!(
        db.query_row(
            "SELECT sub_logical,agg_valid_rev FROM dir WHERE id=?1",
            [root],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        )
        .unwrap(),
        (200, new)
    );
}

#[test]
fn targeted_run_keeps_an_unrelated_pending_tombstone() {
    let (temp, c, grant, root, run) = setup();
    let w = c.writer();
    w.call(listing(
        run,
        root,
        vec![],
        vec![identified("A", 2, 0), identified("B", 3, 0)],
    ))
    .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let a: i64 = db
        .query_row("SELECT id FROM dir WHERE name='A'", [], |r| r.get(0))
        .unwrap();
    let b: i64 = db
        .query_row("SELECT id FROM dir WHERE name='B'", [], |r| r.get(0))
        .unwrap();
    w.call(listing(run, a, vec![], vec![identified("x", 4, 0)]))
        .unwrap();
    let x: i64 = db
        .query_row("SELECT id FROM dir WHERE name='x'", [], |r| r.get(0))
        .unwrap();
    w.call(listing(
        run,
        x,
        vec![identified("descendant", 5, 150)],
        vec![],
    ))
    .unwrap();
    w.call(listing(run, a, vec![], vec![])).unwrap();
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "cancelled".into(),
        finished_at_ns: 3,
    })
    .unwrap();
    let WriteReply::Run(targeted) = w
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "targeted".into(),
            strategy: "fixture".into(),
            started_at_ns: 4,
        })
        .unwrap()
    else {
        panic!()
    };
    w.call(listing(targeted, b, vec![], vec![])).unwrap();
    w.call(WriteCommand::EndRun {
        run_id: targeted,
        state: "completed".into(),
        finished_at_ns: 5,
    })
    .unwrap();
    assert_eq!(
        db.query_row("SELECT listing_state FROM dir WHERE id=?1", [x], |r| r
            .get::<_, String>(
            0
        ))
        .unwrap(),
        "absent_pending"
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM file WHERE name='descendant'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let mut reader = c.reader().unwrap();
    assert!(matches!(
        reader.inspect(NodeKey::Dir(x)),
        Err(Error::NotFound)
    ));
    let WriteReply::Run(full) = w
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "refresh".into(),
            strategy: "fixture".into(),
            started_at_ns: 6,
        })
        .unwrap()
    else {
        panic!()
    };
    w.call(listing(
        full,
        root,
        vec![],
        vec![identified("A", 2, 0), identified("B", 3, 0)],
    ))
    .unwrap();
    w.call(listing(full, a, vec![], vec![])).unwrap();
    w.call(listing(full, b, vec![], vec![])).unwrap();
    w.call(WriteCommand::EndRun {
        run_id: full,
        state: "completed".into(),
        finished_at_ns: 7,
    })
    .unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM dir WHERE id=?1", [x], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn b_first_reparent_invalidates_both_chains_before_cancelled_repair() {
    let (temp, c, grant, root, run) = setup();
    let w = c.writer();
    w.call(listing(
        run,
        root,
        vec![],
        vec![identified("A", 2, 0), identified("B", 3, 0)],
    ))
    .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let a: i64 = db
        .query_row("SELECT id FROM dir WHERE name='A'", [], |r| r.get(0))
        .unwrap();
    let b: i64 = db
        .query_row("SELECT id FROM dir WHERE name='B'", [], |r| r.get(0))
        .unwrap();
    w.call(listing(run, a, vec![], vec![identified("x", 4, 0)]))
        .unwrap();
    let x: i64 = db
        .query_row("SELECT id FROM dir WHERE name='x'", [], |r| r.get(0))
        .unwrap();
    w.call(listing(run, x, vec![identified("file", 5, 150)], vec![]))
        .unwrap();
    w.call(listing(run, b, vec![], vec![])).unwrap();
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 3,
    })
    .unwrap();
    let WriteReply::Run(refresh) = w
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "refresh".into(),
            strategy: "fixture".into(),
            started_at_ns: 4,
        })
        .unwrap()
    else {
        panic!()
    };
    w.call(listing(refresh, b, vec![], vec![identified("x", 4, 0)]))
        .unwrap();
    for id in [a, b, root] {
        assert!(db
            .query_row(
                "SELECT agg_valid_rev<dirty_rev FROM dir WHERE id=?1",
                [id],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
    }
    w.call(WriteCommand::EndRun {
        run_id: refresh,
        state: "cancelled".into(),
        finished_at_ns: 5,
    })
    .unwrap();
    assert_eq!(
        db.query_row("SELECT sub_logical FROM dir WHERE id=?1", [a], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT sub_logical FROM dir WHERE id=?1", [b], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        150
    );
    assert_eq!(
        db.query_row("SELECT sub_logical FROM dir WHERE id=?1", [root], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        150
    );
}

#[test]
fn refs_64_bit_fallback_cannot_reparent_or_bind_durable_references() {
    let (temp, c, _, root, run) = setup();
    let w = c.writer();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    db.execute("UPDATE volume SET filesystem='ReFS'", [])
        .unwrap();
    let mut a = identified("A", 2, 0);
    a.file_id = Some(vec![2; 8]);
    a.created_ft = Some(42);
    let mut b = a.clone();
    b.name = "B".into();
    w.call(listing(run, root, vec![], vec![a.clone(), b.clone()]))
        .unwrap();
    let a_id: i64 = db
        .query_row("SELECT id FROM dir WHERE name='A'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM dir WHERE parent_id=?1 AND file_id IS NULL AND id_quality='path_observation'",[root],|r|r.get::<_,i64>(0)).unwrap(),2);
    assert!(w
        .call(WriteCommand::ObjectReference {
            node: NodeKey::Dir(a_id),
            observed_at_ns: 3
        })
        .is_err());
    a.file_id = Some(vec![2; 16]);
    b.file_id = Some(vec![3; 16]);
    w.call(listing(run, root, vec![], vec![a, b])).unwrap();
    let a_new: i64 = db
        .query_row(
            "SELECT id FROM dir WHERE name='A' AND file_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(a_id, a_new);
    assert!(matches!(
        w.call(WriteCommand::ObjectReference {
            node: NodeKey::Dir(a_new),
            observed_at_ns: 4
        })
        .unwrap(),
        WriteReply::Reference(_)
    ));
}

#[test]
fn reused_creation_time_keeps_retired_incarnation_until_explicit_reconciliation() {
    let (temp, c, _, root, run) = setup();
    let w = c.writer();
    let mut file = identified("item", 2, 150);
    file.created_ft = Some(42);
    w.call(listing(run, root, vec![file.clone()], vec![]))
        .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let old: i64 = db
        .query_row("SELECT id FROM file", [], |r| r.get(0))
        .unwrap();
    let WriteReply::Reference(reference) = w
        .call(WriteCommand::ObjectReference {
            node: NodeKey::File(old),
            observed_at_ns: 3,
        })
        .unwrap()
    else {
        panic!()
    };
    let mut reader = c.reader().unwrap();
    let (instance, born) = reader.incarnation(NodeKey::File(old)).unwrap();
    w.call(listing(run, root, vec![], vec![])).unwrap();
    w.call(listing(run, root, vec![file], vec![])).unwrap();
    let new: i64 = db
        .query_row("SELECT id FROM file", [], |r| r.get(0))
        .unwrap();
    assert!(new > old);
    assert!(matches!(
        reader.check_incarnation(NodeKey::File(old), &instance, born),
        Err(Error::NotFound)
    ));
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    assert_eq!(
        st.query_row(
            "SELECT state,retired FROM object_ref WHERE id=?1",
            [reference],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
        )
        .unwrap(),
        ("unresolved".into(), true)
    );
    let WriteReply::Reference(new_reference) = w
        .call(WriteCommand::ObjectReference {
            node: NodeKey::File(new),
            observed_at_ns: 4,
        })
        .unwrap()
    else {
        panic!()
    };
    assert_ne!(reference, new_reference);
    assert_eq!(
        st.query_row(
            "SELECT count(*) FROM object_ref WHERE file_id=?1",
            [vec![2; 16]],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    // Remove the newly created binding before explicitly reconciling the old owner-held reference.
    st.execute(
        "UPDATE object_ref SET state='unresolved',retired=1 WHERE id=?1",
        [new_reference],
    )
    .unwrap();
    w.call(WriteCommand::ReconcileReference {
        reference_id: reference,
        node: NodeKey::File(new),
        observed_at_ns: 5,
    })
    .unwrap();
    assert_eq!(
        st.query_row(
            "SELECT row_id FROM object_ref WHERE id=?1 AND state='resolved'",
            [reference],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        new
    );
}

#[test]
fn staged_chunks_are_invisible_until_atomic_publication_and_cancellation_is_bounded() {
    let (temp, c, _, root, run) = setup();
    let w = c.writer();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    for seq in 0..3 {
        w.call(WriteCommand::StageChunk {
            run_id: run,
            dir_id: root,
            seq,
            files: vec![identified(&format!("item-{seq}"), seq as u8 + 2, 10)],
            dirs: vec![],
        })
        .unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    w.call(WriteCommand::ListingDone {
        run_id: run,
        dir_id: root,
        outcome: ListingOutcome::Complete,
        skipped: 0,
        errors: 0,
    })
    .unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    let token = Cancellation::default();
    let held = w.reserve_bytes(QUEUE_BYTES / 2, &token).unwrap();
    std::thread::scope(|scope| {
        let handle = scope.spawn(|| w.send_cancellable(WriteCommand::Barrier, &token));
        std::thread::sleep(std::time::Duration::from_millis(20));
        let start = std::time::Instant::now();
        token.cancel();
        assert!(matches!(handle.join().unwrap(), Err(Error::Cancelled)));
        assert!(start.elapsed() < std::time::Duration::from_millis(250));
    });
    drop(held);
    assert_eq!(w.queued_bytes(), 0);
}

#[test]
fn durable_revocation_fences_queued_observations_and_catalogue_rebuild() {
    let (temp, c, grant, root, run) = setup();
    let w = c.writer();
    w.call(listing(run, root, vec![identified("item", 2, 10)], vec![]))
        .unwrap();
    w.call(WriteCommand::RevokeGrant {
        grant_id: grant,
        revoked_at_ns: 3,
    })
    .unwrap();
    assert!(w
        .call(listing(run, root, vec![identified("late", 3, 20)], vec![]))
        .is_err());
    let mut reader = c.reader().unwrap();
    assert!(matches!(
        reader.inspect(NodeKey::Dir(root)),
        Err(Error::NotFound)
    ));
    drop(reader);
    drop(c);
    std::fs::write(
        temp.path().join("catalog.db"),
        b"synthetic corrupt catalogue",
    )
    .unwrap();
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM root", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    assert_eq!(
        st.query_row("SELECT state FROM root_grant WHERE id=?1", [grant], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "revoked"
    );
    drop(c);
}
#[test]
fn creation_time_edits_retire_directory_and_file_bindings_without_auto_reattachment() {
    let (temp, c, grant, root, run) = setup();
    let w = c.writer();
    let mut dir = identified("dir", 2, 0);
    dir.created_ft = Some(42);
    let mut file = identified("file", 3, 10);
    file.created_ft = Some(42);
    w.call(listing(run, root, vec![file.clone()], vec![dir.clone()]))
        .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let child: i64 = db
        .query_row("SELECT id FROM dir WHERE name='dir'", [], |r| r.get(0))
        .unwrap();
    let f: i64 = db
        .query_row("SELECT id FROM file", [], |r| r.get(0))
        .unwrap();
    let mut old_child = identified("old-child", 4, 99);
    old_child.created_ft = Some(42);
    w.call(listing(run, child, vec![old_child], vec![]))
        .unwrap();
    let nested: i64 = db
        .query_row("SELECT id FROM file WHERE name='old-child'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mut reader = c.reader().unwrap();
    let (instance, born) = reader.incarnation(NodeKey::Dir(child)).unwrap();
    let (_, file_born) = reader.incarnation(NodeKey::File(f)).unwrap();
    let WriteReply::Reference(dr) = w
        .call(WriteCommand::ObjectReference {
            node: NodeKey::Dir(child),
            observed_at_ns: 3,
        })
        .unwrap()
    else {
        panic!()
    };
    let WriteReply::Reference(fr) = w
        .call(WriteCommand::ObjectReference {
            node: NodeKey::File(f),
            observed_at_ns: 3,
        })
        .unwrap()
    else {
        panic!()
    };
    let WriteReply::Reference(nr) = w
        .call(WriteCommand::ObjectReference {
            node: NodeKey::File(nested),
            observed_at_ns: 3,
        })
        .unwrap()
    else {
        panic!()
    };
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 4,
    })
    .unwrap();
    let WriteReply::Run(run) = w
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "refresh".into(),
            strategy: "fixture".into(),
            started_at_ns: 5,
        })
        .unwrap()
    else {
        panic!()
    };
    dir.created_ft = Some(100);
    file.created_ft = Some(100);
    let WriteCommand::DirListing(mut replacement_listing) =
        listing(run, root, vec![file], vec![dir])
    else {
        panic!()
    };
    replacement_listing.state = "partial".into();
    w.call(WriteCommand::DirListing(replacement_listing))
        .unwrap();
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    for reference in [dr, fr, nr] {
        assert_eq!(
            st.query_row(
                "SELECT state,retired FROM object_ref WHERE id=?1",
                [reference],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
            )
            .unwrap(),
            ("unresolved".into(), true)
        );
    }
    let replacement: (i64, i64, String) = db
        .query_row(
            "SELECT id,born_run,listing_state FROM dir WHERE name='dir' AND file_id IS NOT NULL",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert!(replacement.0 > child);
    assert_eq!(replacement.1, run);
    assert_ne!(replacement.1, born);
    assert_eq!(replacement.2, "unlisted");
    let new_file: (i64, i64) = db
        .query_row("SELECT id,born_run FROM file WHERE name='file'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert!(new_file.0 > f);
    assert_eq!(new_file.1, run);
    assert_ne!(new_file.1, file_born);
    assert!(matches!(
        reader.check_incarnation(NodeKey::Dir(child), &instance, born),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        reader.incarnation(NodeKey::File(f)),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        reader.incarnation(NodeKey::File(nested)),
        Err(Error::NotFound)
    ));
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM file WHERE dir_id=?1",
            [replacement.0],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    w.call(listing(
        run,
        replacement.0,
        vec![identified("new-child", 5, 7)],
        vec![],
    ))
    .unwrap();
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 4,
    })
    .unwrap();
    assert_eq!(
        reader
            .inspect(NodeKey::Dir(root))
            .unwrap()
            .subtree
            .unwrap()
            .logical_bytes,
        "17"
    );
}

#[test]
fn surviving_hardlink_keeps_the_durable_reference_and_membership() {
    for staged in [false, true] {
        let (temp, c, _, root, run) = setup();
        let w = c.writer();
        let mut first = identified("first", 2, 10);
        first.created_ft = Some(42);
        let mut second = first.clone();
        second.name = "second".into();
        w.call(listing(run, root, vec![first, second.clone()], vec![]))
            .unwrap();
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        let old: i64 = db
            .query_row("SELECT id FROM file WHERE name='first'", [], |r| r.get(0))
            .unwrap();
        let WriteReply::Reference(reference) = w
            .call(WriteCommand::ObjectReference {
                node: NodeKey::File(old),
                observed_at_ns: 3,
            })
            .unwrap()
        else {
            panic!()
        };
        let st = Connection::open(temp.path().join("state.db")).unwrap();
        st.execute(
            "INSERT INTO collection(name,kind,created_at_ns) VALUES('kept','human',3)",
            [],
        )
        .unwrap();
        st.execute("INSERT INTO collection_member VALUES(1,?1,3)", [reference])
            .unwrap();
        if staged {
            w.call(WriteCommand::StageChunk {
                run_id: run,
                dir_id: root,
                seq: 0,
                files: vec![second],
                dirs: vec![],
            })
            .unwrap();
            w.call(WriteCommand::ListingDone {
                run_id: run,
                dir_id: root,
                outcome: ListingOutcome::Complete,
                skipped: 0,
                errors: 0,
            })
            .unwrap();
        } else {
            w.call(listing(run, root, vec![second], vec![])).unwrap();
        }
        let survivor: (i64, i64) = db
            .query_row("SELECT id,born_run FROM file", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(
            st.query_row(
                "SELECT state,retired,row_id,born_run FROM object_ref WHERE id=?1",
                [reference],
                |r| Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, bool>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?
                ))
            )
            .unwrap(),
            ("resolved".into(), false, survivor.0, survivor.1)
        );
        assert_eq!(
            c.reader()
                .unwrap()
                .inspect(NodeKey::File(survivor.0))
                .unwrap()
                .memberships
                .len(),
            1
        );
        w.call(listing(run, root, vec![], vec![])).unwrap();
        assert_eq!(
            st.query_row(
                "SELECT state,retired FROM object_ref WHERE id=?1",
                [reference],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
            )
            .unwrap(),
            ("unresolved".into(), true)
        );
    }
}

#[test]
fn duplicate_staged_names_roll_back_the_whole_publication() {
    let (temp, c, _, root, run) = setup();
    let w = c.writer();
    w.call(listing(run, root, vec![identified("kept", 2, 10)], vec![]))
        .unwrap();
    for seq in 0..2 {
        w.call(WriteCommand::StageChunk {
            run_id: run,
            dir_id: root,
            seq,
            files: vec![identified("duplicate", seq as u8 + 3, 20)],
            dirs: vec![],
        })
        .unwrap();
    }
    assert!(w
        .call(WriteCommand::ListingDone {
            run_id: run,
            dir_id: root,
            outcome: ListingOutcome::Complete,
            skipped: 0,
            errors: 0
        })
        .is_err());
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT name FROM file", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "kept"
    );
}

#[test]
fn duplicate_staged_names_preserve_durable_references_and_old_listing() {
    rejected_staged_listing_preserves_references(false);
}

#[test]
fn conflicting_staged_families_preserve_durable_references_and_old_listing() {
    rejected_staged_listing_preserves_references(true);
}

fn rejected_staged_listing_preserves_references(conflicting_family: bool) {
    let (temp, c, grant, root, run) = setup();
    let w = c.writer();
    let mut old_file = identified("omitted", 2, 10);
    old_file.created_ft = Some(42);
    let mut old_dir = identified("replaced", 3, 0);
    old_dir.created_ft = Some(42);
    w.call(listing(run, root, vec![old_file], vec![old_dir.clone()]))
        .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let file: i64 = db
        .query_row("SELECT id FROM file", [], |r| r.get(0))
        .unwrap();
    let dir: i64 = db
        .query_row("SELECT id FROM dir WHERE parent_id=?1", [root], |r| {
            r.get(0)
        })
        .unwrap();
    for node in [NodeKey::File(file), NodeKey::Dir(dir)] {
        w.call(WriteCommand::ObjectReference {
            node,
            observed_at_ns: 3,
        })
        .unwrap();
    }
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    let bindings = || {
        st.prepare("SELECT id,state,retired,row_id,born_run FROM object_ref ORDER BY id")
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    let before = bindings();
    assert_eq!(before.len(), 2);
    let state_rev: i64 = st
        .query_row("SELECT state_rev FROM revision", [], |r| r.get(0))
        .unwrap();
    old_dir.created_ft = Some(100);
    for seq in 0..2 {
        let mut o = identified(
            if conflicting_family && seq == 1 {
                "other"
            } else {
                "duplicate"
            },
            seq as u8 + 4,
            20,
        );
        if conflicting_family {
            o.extension = Some("fixture".into());
            o.family = if seq == 0 { "document" } else { "image" }.into();
        }
        w.call(WriteCommand::StageChunk {
            run_id: run,
            dir_id: root,
            seq,
            files: vec![o],
            dirs: if seq == 0 {
                vec![old_dir.clone()]
            } else {
                vec![]
            },
        })
        .unwrap();
    }
    let revision: i64 = db
        .query_row("SELECT catalog_rev FROM revision", [], |r| r.get(0))
        .unwrap();
    assert!(matches!(
        w.call(WriteCommand::ListingDone {
            run_id: run,
            dir_id: root,
            outcome: ListingOutcome::Complete,
            skipped: 0,
            errors: 0,
        }),
        Err(Error::Invalid(_))
    ));
    assert_eq!(bindings(), before);
    assert_eq!(
        st.query_row("SELECT state_rev FROM revision", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        state_rev
    );
    assert_eq!(
        db.query_row("SELECT catalog_rev FROM revision", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        revision
    );
    assert_eq!(
        db.query_row("SELECT id,name,logical,created_ft FROM file", [], |r| Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?
        )))
        .unwrap(),
        (file, "omitted".into(), 10, 42)
    );
    assert_eq!(
        db.query_row(
            "SELECT id,created_ft,listing_state FROM dir WHERE parent_id=?1",
            [root],
            |r| Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?
            ))
        )
        .unwrap(),
        (dir, 42, "unlisted".into())
    );
    // A valid replacement still publishes and retires the omitted/replaced bindings.
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "cancelled".into(),
        finished_at_ns: 4,
    })
    .unwrap();
    let WriteReply::Run(refresh) = w
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "refresh".into(),
            strategy: "fixture".into(),
            started_at_ns: 5,
        })
        .unwrap()
    else {
        panic!()
    };
    w.call(listing(refresh, root, vec![], vec![old_dir]))
        .unwrap();
    assert!(bindings().iter().all(|r| r.1 == "unresolved" && r.2));
    assert_eq!(
        db.query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn revoking_one_root_clears_the_other_roots_cross_root_link_flag() {
    let (temp, c, _, root, run) = setup();
    let w = c.writer();
    let WriteReply::Root {
        dir_id: other,
        grant_id: other_grant,
        ..
    } = w
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "fixture-volume".into(),
            display_name: "Synthetic volume".into(),
            display_path: "Second synthetic root".into(),
            root_file_id: Some(vec![9; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 3,
        }))
        .unwrap()
    else {
        panic!()
    };
    let WriteReply::Run(other_run) = w
        .call(WriteCommand::BeginRun {
            grant_id: other_grant,
            mode: "full".into(),
            strategy: "fixture".into(),
            started_at_ns: 4,
        })
        .unwrap()
    else {
        panic!()
    };
    w.call(listing(run, root, vec![identified("first", 2, 10)], vec![]))
        .unwrap();
    w.call(listing(
        other_run,
        other,
        vec![identified("second", 2, 10)],
        vec![],
    ))
    .unwrap();
    for run_id in [run, other_run] {
        w.call(WriteCommand::EndRun {
            run_id,
            state: "completed".into(),
            finished_at_ns: 5,
        })
        .unwrap();
    }
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let first_flag = || -> i64 {
        db.query_row(
            "SELECT flags & ?1 FROM file WHERE name='first'",
            [HARDLINK_SUSPECTED],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_ne!(
        first_flag(),
        0,
        "a cross-root link is flagged while both roots are visible"
    );
    w.call(WriteCommand::RevokeGrant {
        grant_id: other_grant,
        revoked_at_ns: 6,
    })
    .unwrap();
    assert_eq!(first_flag(), 0, "the revoked root's copy no longer counts");
    assert_eq!(
        db.query_row("SELECT count(*) FROM multilink", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn multilink_flags_cover_both_roots_in_either_finalisation_order_and_clear_after_removal() {
    for reverse in [false, true] {
        let (temp, c, _, root, run) = setup();
        let w = c.writer();
        let WriteReply::Root {
            dir_id: other,
            grant_id: other_grant,
            ..
        } = w
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "fixture-volume".into(),
                display_name: "Synthetic volume".into(),
                display_path: "Second synthetic root".into(),
                root_file_id: Some(vec![9; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 3,
            }))
            .unwrap()
        else {
            panic!()
        };
        let WriteReply::Run(other_run) = w
            .call(WriteCommand::BeginRun {
                grant_id: other_grant,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 4,
            })
            .unwrap()
        else {
            panic!()
        };
        w.call(listing(run, root, vec![identified("first", 2, 10)], vec![]))
            .unwrap();
        w.call(listing(
            other_run,
            other,
            vec![identified("second", 2, 10)],
            vec![],
        ))
        .unwrap();
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        let runs = if reverse {
            [other_run, run]
        } else {
            [run, other_run]
        };
        for run_id in runs {
            w.call(WriteCommand::EndRun {
                run_id,
                state: "completed".into(),
                finished_at_ns: 5,
            })
            .unwrap();
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM file WHERE flags & ?1 != 0",
                    [HARDLINK_SUSPECTED],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                2
            );
            assert_eq!(
                db.query_row("SELECT names FROM multilink", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                2
            );
        }
        let WriteReply::Run(refresh) = w
            .call(WriteCommand::BeginRun {
                grant_id: other_grant,
                mode: "refresh".into(),
                strategy: "fixture".into(),
                started_at_ns: 6,
            })
            .unwrap()
        else {
            panic!()
        };
        w.call(listing(refresh, other, vec![], vec![])).unwrap();
        w.call(WriteCommand::EndRun {
            run_id: refresh,
            state: "completed".into(),
            finished_at_ns: 7,
        })
        .unwrap();
        assert_eq!(
            db.query_row(
                "SELECT name,flags & ?1 FROM file",
                [HARDLINK_SUSPECTED],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            )
            .unwrap(),
            ("first".into(), 0)
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM multilink", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn grant_survives_a_crash_between_precious_and_derived_commits() {
    let temp = tempfile::tempdir().unwrap();
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_root BEFORE INSERT ON root BEGIN SELECT RAISE(ABORT,'synthetic fault');END;").unwrap();
    assert!(c
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "fixture".into(),
            display_name: "Fixture".into(),
            display_path: "Fixture root".into(),
            root_file_id: Some(vec![1; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0
        }))
        .is_err());
    assert_eq!(
        db.query_row("SELECT count(*) FROM root", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    db.execute_batch("DROP TRIGGER fail_root").unwrap();
    drop(db);
    drop(c);
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM root", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(c);
}

#[test]
fn queued_staged_publications_isolate_failure_and_commit_other_directories() {
    let (temp, c, _, root, run) = setup();
    let w = c.writer();
    w.call(listing(
        run,
        root,
        vec![identified("old", 2, 10)],
        vec![identified("child", 3, 0)],
    ))
    .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let child: i64 = db
        .query_row("SELECT id FROM dir WHERE name='child'", [], |r| r.get(0))
        .unwrap();
    let mut receipts = Vec::new();
    for seq in 0..2 {
        receipts.push(
            w.send(WriteCommand::StageChunk {
                run_id: run,
                dir_id: root,
                seq,
                files: vec![identified("duplicate", seq as u8 + 4, 20)],
                dirs: vec![],
            })
            .unwrap(),
        );
    }
    let bad = w
        .send(WriteCommand::ListingDone {
            run_id: run,
            dir_id: root,
            outcome: ListingOutcome::Complete,
            skipped: 0,
            errors: 0,
        })
        .unwrap();
    receipts.push(
        w.send(WriteCommand::StageChunk {
            run_id: run,
            dir_id: child,
            seq: 0,
            files: vec![identified("good", 6, 30)],
            dirs: vec![],
        })
        .unwrap(),
    );
    let good = w
        .send(WriteCommand::ListingDone {
            run_id: run,
            dir_id: child,
            outcome: ListingOutcome::Complete,
            skipped: 0,
            errors: 0,
        })
        .unwrap();
    for receipt in receipts {
        receipt.wait().unwrap();
    }
    assert!(bad.wait().is_err());
    good.wait().unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM file WHERE name IN ('old','good')",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM file WHERE name='duplicate'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(w.queued_bytes(), 0);
}

fn begin(w: &Writer, grant: i64, at: i64) -> i64 {
    let WriteReply::Run(run) = w
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "refresh".into(),
            strategy: "fixture".into(),
            started_at_ns: at,
        })
        .unwrap()
    else {
        panic!()
    };
    run
}
fn end(w: &Writer, run: i64, state: &str) {
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: state.into(),
        finished_at_ns: run,
    })
    .unwrap();
}
fn directory(name: &str, id: u8) -> Observation {
    let mut o = identified(name, id, 0);
    o.attrs = 0x10;
    o.created_ft = Some(1);
    o
}

/// #206 MEDIUM-1: excluded entries (a junction, a symbolic link) stay visible, are never counted,
/// and a visible excluded directory does not stop a full traversal from completing.
#[test]
fn excluded_entries_stay_visible_uncounted_and_complete_a_full_traversal() {
    let (temp, c, grant, root, run) = setup();
    let w = c.writer();
    let mut junction = directory("junction", 5);
    junction.attrs |= 0x400;
    junction.flags = 1 << 4;
    let mut link = identified("link", 4, 50);
    link.attrs = 0x400;
    link.flags = 1 << 4;
    let WriteCommand::DirListing(mut l) = listing(
        run,
        root,
        vec![identified("kept", 2, 10), link],
        vec![junction.clone()],
    ) else {
        panic!()
    };
    l.skipped = 2;
    w.call(WriteCommand::DirListing(l)).unwrap();
    end(w, run, "completed");
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let (id, state, seen): (i64, String, i64) = db
        .query_row(
            "SELECT id,listing_state,seen_run FROM dir WHERE name='junction'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((state.as_str(), seen), ("excluded", run));
    // Visible but never counted: the root counts `kept` only, and is not complete.
    assert_eq!(
        db.query_row(
            "SELECT own_files,own_logical,sub_files,sub_dirs,sub_logical,sub_complete FROM dir WHERE id=?1",
            [root],
            |r| Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, bool>(5)?
            ))
        )
        .unwrap(),
        (1, 10, 1, 0, 10, false)
    );
    assert_eq!(
        db.query_row("SELECT flags FROM file WHERE name='link'", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1 << 4
    );
    // The excluded directory needs no listing: the full traversal set the generation.
    assert_eq!(
        db.query_row("SELECT generation FROM root", [], |r| r
            .get::<_, Option<i64>>(0))
            .unwrap(),
        Some(run)
    );
    // The next run keeps the same excluded row.
    let run = begin(w, grant, 7);
    w.call(listing(
        run,
        root,
        vec![identified("kept", 2, 10)],
        vec![junction],
    ))
    .unwrap();
    assert_eq!(
        db.query_row(
            "SELECT id FROM dir WHERE name='junction' AND listing_state='excluded'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        id
    );
}

/// #206 H1: a directory that is excluded for one scan only (an attribute change: offline,
/// recall or reparse, same identity) keeps its row; its descendants are absent like any unseen
/// child and keep their identities. When it returns, its references and memberships are intact.
#[test]
fn a_directory_excluded_for_one_scan_keeps_its_references_and_memberships() {
    let (temp, c, grant, root, run) = setup();
    let w = c.writer();
    let proj = directory("proj", 5);
    let sub = directory("sub", 6);
    w.call(listing(run, root, vec![], vec![proj.clone()]))
        .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let id = |name: &str| -> i64 {
        db.query_row("SELECT id FROM dir WHERE name=?1", [name], |r| r.get(0))
            .unwrap()
    };
    let proj_id = id("proj");
    w.call(listing(
        run,
        proj_id,
        vec![identified("top", 8, 30)],
        vec![sub.clone()],
    ))
    .unwrap();
    let mut doc = identified("doc", 7, 100);
    doc.created_ft = Some(1);
    w.call(listing(run, id("sub"), vec![doc.clone()], vec![]))
        .unwrap();
    let doc_row: (i64, i64) = db
        .query_row("SELECT id,born_run FROM file WHERE name='doc'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    let WriteReply::Reference(reference) = w
        .call(WriteCommand::ObjectReference {
            node: NodeKey::File(doc_row.0),
            observed_at_ns: 3,
        })
        .unwrap()
    else {
        panic!()
    };
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    st.execute(
        "INSERT INTO collection(name,kind,created_at_ns) VALUES('labelled','human',3)",
        [],
    )
    .unwrap();
    st.execute("INSERT INTO collection_member VALUES(1,?1,3)", [reference])
        .unwrap();
    end(w, run, "completed");
    let binding = || -> (String, bool, i64, i64) {
        st.query_row(
            "SELECT state,retired,row_id,born_run FROM object_ref WHERE id=?1",
            [reference],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
    };
    let bound = ("resolved".to_string(), false, doc_row.0, doc_row.1);
    assert_eq!(binding(), bound);
    // One scan sees `proj` as an online-only placeholder, in an incomplete listing.
    let run = begin(w, grant, 5);
    let mut placeholder = proj.clone();
    placeholder.attrs |= 0x1000 | 0x400000;
    placeholder.flags = 1 << 3;
    let WriteCommand::DirListing(mut l) = listing(run, root, vec![], vec![placeholder]) else {
        panic!()
    };
    l.state = "partial".into();
    l.skipped = 1;
    w.call(WriteCommand::DirListing(l)).unwrap();
    let (state, sub_state): (String, String) = db
        .query_row(
            "SELECT p.listing_state,s.listing_state FROM dir p JOIN dir s ON s.parent_id=p.id WHERE p.id=?1",
            [proj_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (state.as_str(), sub_state.as_str()),
        ("excluded", "absent_pending"),
        "the same row is excluded in place; its subtree is hidden, not replaced"
    );
    assert_eq!(binding(), bound, "nothing retires before a full traversal");
    end(w, run, "cancelled");
    let logical = || -> i64 {
        db.query_row("SELECT sub_logical FROM dir WHERE id=?1", [root], |r| {
            r.get(0)
        })
        .unwrap()
    };
    assert_eq!(
        logical(),
        0,
        "an excluded directory's old totals are not counted"
    );
    // The next scan sees it as a normal directory again: everything re-attaches.
    let run = begin(w, grant, 7);
    w.call(listing(run, root, vec![], vec![proj])).unwrap();
    assert_eq!(id("proj"), proj_id);
    w.call(listing(
        run,
        proj_id,
        vec![identified("top", 8, 30)],
        vec![sub],
    ))
    .unwrap();
    w.call(listing(run, id("sub"), vec![doc], vec![])).unwrap();
    end(w, run, "completed");
    assert_eq!(logical(), 130);
    assert_eq!(binding(), bound);
    assert_eq!(
        db.query_row("SELECT id,born_run FROM file WHERE name='doc'", [], |r| Ok(
            (r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)
        ))
        .unwrap(),
        doc_row
    );
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

/// #240 HIGH: a completed full traversal while a directory is excluded must not retire or
/// delete anything under it: its contents were never observed, so their absence is unknown. A
/// genuinely absent subtree still retires at the same traversal.
#[test]
fn a_completed_traversal_never_retires_the_subtree_of_an_excluded_directory() {
    let (temp, c, grant, root, run) = setup();
    let w = c.writer();
    let proj = directory("proj", 5);
    let gone = directory("gone", 9);
    w.call(listing(run, root, vec![], vec![proj.clone(), gone]))
        .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let id = |name: &str| -> i64 {
        db.query_row("SELECT id FROM dir WHERE name=?1", [name], |r| r.get(0))
            .unwrap()
    };
    let proj_id = id("proj");
    let sub = directory("sub", 6);
    w.call(listing(run, proj_id, vec![], vec![sub.clone()]))
        .unwrap();
    let deep = directory("deep", 11);
    w.call(listing(run, id("sub"), vec![], vec![deep.clone()]))
        .unwrap();
    let mut doc = identified("doc", 7, 100);
    doc.created_ft = Some(1);
    w.call(listing(run, id("deep"), vec![doc.clone()], vec![]))
        .unwrap();
    let mut old = identified("old", 10, 5);
    old.created_ft = Some(1);
    w.call(listing(run, id("gone"), vec![old], vec![])).unwrap();
    let file = |name: &str| -> (i64, i64) {
        db.query_row("SELECT id,born_run FROM file WHERE name=?1", [name], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap()
    };
    let (doc_row, old_row) = (file("doc"), file("old"));
    let reference = |row: i64| -> i64 {
        let WriteReply::Reference(reference) = w
            .call(WriteCommand::ObjectReference {
                node: NodeKey::File(row),
                observed_at_ns: 3,
            })
            .unwrap()
        else {
            panic!()
        };
        reference
    };
    let (kept, retired) = (reference(doc_row.0), reference(old_row.0));
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    st.execute(
        "INSERT INTO collection(name,kind,created_at_ns) VALUES('labelled','human',3)",
        [],
    )
    .unwrap();
    st.execute("INSERT INTO collection_member VALUES(1,?1,3)", [kept])
        .unwrap();
    end(w, run, "completed");
    let binding = |reference: i64| -> (String, bool, i64, i64) {
        st.query_row(
            "SELECT state,retired,row_id,born_run FROM object_ref WHERE id=?1",
            [reference],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
    };
    let bound = ("resolved".to_string(), false, doc_row.0, doc_row.1);
    assert_eq!(binding(kept), bound);
    // `proj` is a placeholder for one scan; `gone` is really gone. The root listing is complete
    // and the run completes a full traversal.
    let run = begin(w, grant, 5);
    let mut placeholder = proj.clone();
    placeholder.attrs |= 0x1000 | 0x400000;
    placeholder.flags = 1 << 3;
    let WriteCommand::DirListing(mut l) = listing(run, root, vec![], vec![placeholder]) else {
        panic!()
    };
    l.skipped = 1;
    w.call(WriteCommand::DirListing(l)).unwrap();
    end(w, run, "completed");
    assert_eq!(
        db.query_row("SELECT generation FROM root", [], |r| r
            .get::<_, Option<i64>>(0))
            .unwrap(),
        Some(run),
        "the traversal was full"
    );
    assert_eq!(binding(kept), bound, "unknown is never absent");
    assert_eq!(
        st.query_row(
            "SELECT state,retired FROM object_ref WHERE id=?1",
            [retired],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
        )
        .unwrap(),
        ("unresolved".into(), true),
        "a genuinely absent subtree still retires"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM dir WHERE name='gone'", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    // It returns as a normal directory: the same rows, binding and membership.
    let run = begin(w, grant, 7);
    w.call(listing(run, root, vec![], vec![proj])).unwrap();
    w.call(listing(run, proj_id, vec![], vec![sub])).unwrap();
    w.call(listing(run, id("sub"), vec![], vec![deep])).unwrap();
    w.call(listing(run, id("deep"), vec![doc], vec![])).unwrap();
    end(w, run, "completed");
    assert_eq!(file("doc"), doc_row);
    assert_eq!(binding(kept), bound);
    assert_eq!(
        st.query_row(
            "SELECT count(*) FROM collection_member WHERE object_ref_id=?1",
            [kept],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn multilink_ignores_files_below_excluded_or_absent_ancestors() {
    for excluded in [true, false] {
        let (temp, c, grant, root, run) = setup();
        let w = c.writer();
        let proj = directory("proj", 5);
        w.call(listing(run, root, vec![], vec![proj.clone()]))
            .unwrap();
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        let id = |name: &str| -> i64 {
            db.query_row("SELECT id FROM dir WHERE name=?1", [name], |r| r.get(0))
                .unwrap()
        };
        w.call(listing(
            run,
            id("proj"),
            vec![identified("hidden-top", 8, 100)],
            vec![directory("sub", 6)],
        ))
        .unwrap();
        w.call(listing(run, id("sub"), vec![], vec![directory("deep", 7)]))
            .unwrap();
        w.call(listing(
            run,
            id("deep"),
            vec![identified("hidden", 8, 100)],
            vec![],
        ))
        .unwrap();
        end(w, run, "completed");

        let run = begin(w, grant, 5);
        let dirs = if excluded {
            let mut placeholder = proj.clone();
            placeholder.flags = 1 << 3;
            vec![placeholder]
        } else {
            vec![]
        };
        w.call(listing(
            run,
            root,
            vec![identified("visible", 8, 100)],
            dirs,
        ))
        .unwrap();
        // Cancellation retains the genuinely absent subtree; exclusion retains it even after
        // a full traversal. Neither retained copy belongs in the visible link count.
        end(w, run, if excluded { "completed" } else { "cancelled" });
        assert_eq!(
            db.query_row("SELECT count(*) FROM file WHERE name='hidden'", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            1
        );
        if excluded {
            assert_eq!(
                db.query_row("SELECT generation FROM root", [], |r| r
                    .get::<_, Option<i64>>(0))
                    .unwrap(),
                Some(run),
                "the traversal was full"
            );
        }
        let mut reader = c.reader().unwrap();
        let sums = reader.inspect(NodeKey::Dir(root)).unwrap().subtree.unwrap();
        let flag: i64 = db
            .query_row(
                "SELECT flags & ?1 FROM file WHERE name='visible'",
                [HARDLINK_SUSPECTED],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!((flag, sums.multi_link_entries), (0, 0));
        assert_eq!(
            (sums.unique_objects, sums.unique_allocated_bytes),
            (Some(1), Some("100".into()))
        );

        // Positive control: the second name becomes visible again and must count as a link.
        let run = begin(w, grant, 7);
        w.call(listing(
            run,
            root,
            vec![identified("visible", 8, 100)],
            vec![proj],
        ))
        .unwrap();
        w.call(listing(
            run,
            id("proj"),
            vec![identified("hidden-top", 8, 100)],
            vec![directory("sub", 6)],
        ))
        .unwrap();
        w.call(listing(run, id("sub"), vec![], vec![directory("deep", 7)]))
            .unwrap();
        w.call(listing(
            run,
            id("deep"),
            vec![identified("hidden", 8, 100)],
            vec![],
        ))
        .unwrap();
        end(w, run, "completed");
        assert_eq!(
            reader
                .inspect(NodeKey::Dir(root))
                .unwrap()
                .subtree
                .unwrap()
                .multi_link_entries,
            3
        );
    }
}

#[test]
fn multilink_flags_only_visible_copies_when_hidden_copies_share_the_id() {
    let (temp, c, grant, root, run) = setup();
    let w = c.writer();
    let proj = directory("proj", 5);
    w.call(listing(run, root, vec![], vec![proj.clone()]))
        .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let proj_id: i64 = db
        .query_row("SELECT id FROM dir WHERE name='proj'", [], |r| r.get(0))
        .unwrap();
    w.call(listing(
        run,
        proj_id,
        vec![identified("hidden", 8, 100)],
        vec![],
    ))
    .unwrap();
    end(w, run, "completed");

    let run = begin(w, grant, 5);
    let mut placeholder = proj.clone();
    placeholder.flags = 1 << 3;
    w.call(listing(
        run,
        root,
        vec![
            identified("visible", 8, 100),
            identified("visible2", 8, 100),
        ],
        vec![placeholder],
    ))
    .unwrap();
    end(w, run, "completed");
    let flag = |name: &str| -> i64 {
        db.query_row(
            "SELECT flags & ?1 FROM file WHERE name=?2",
            rusqlite::params![HARDLINK_SUSPECTED, name],
            |r| r.get(0),
        )
        .unwrap()
    };
    // The two visible names are a genuine link; the hidden copy is neither counted nor flagged.
    assert_ne!(flag("visible"), 0);
    assert_ne!(flag("visible2"), 0);
    assert_eq!(flag("hidden"), 0);
}

#[test]
fn absent_parent_retires_its_excluded_child_and_descendants() {
    let (temp, c, grant, root, run) = setup();
    let w = c.writer();
    w.call(listing(run, root, vec![], vec![directory("parent", 5)]))
        .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let id = |name: &str| -> i64 {
        db.query_row("SELECT id FROM dir WHERE name=?1", [name], |r| r.get(0))
            .unwrap()
    };
    let child = directory("child", 6);
    w.call(listing(run, id("parent"), vec![], vec![child.clone()]))
        .unwrap();
    let mut doc = identified("doc", 7, 100);
    doc.created_ft = Some(1);
    w.call(listing(run, id("child"), vec![doc], vec![]))
        .unwrap();
    let file: i64 = db
        .query_row("SELECT id FROM file", [], |r| r.get(0))
        .unwrap();
    let WriteReply::Reference(reference) = w
        .call(WriteCommand::ObjectReference {
            node: NodeKey::File(file),
            observed_at_ns: 3,
        })
        .unwrap()
    else {
        panic!()
    };
    end(w, run, "completed");

    let run = begin(w, grant, 5);
    w.call(listing(run, root, vec![], vec![directory("parent", 5)]))
        .unwrap();
    let mut placeholder = child;
    placeholder.flags = 1 << 3;
    w.call(listing(run, id("parent"), vec![], vec![placeholder]))
        .unwrap();
    end(w, run, "completed");
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    let binding = || -> (String, bool) {
        st.query_row(
            "SELECT state,retired FROM object_ref WHERE id=?1",
            [reference],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    };
    assert_eq!(binding(), ("resolved".into(), false));

    let run = begin(w, grant, 7);
    w.call(listing(run, root, vec![], vec![])).unwrap();
    assert_eq!(
        binding(),
        ("resolved".into(), false),
        "retirement waits for EndRun"
    );
    end(w, run, "completed");
    assert_eq!(binding(), ("unresolved".into(), true));
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM dir WHERE parent_id IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

/// #240 MEDIUM: breakdowns and unique totals anchored at an excluded directory never count the
/// stale files still listed inside it.
#[test]
fn breakdown_and_unique_totals_at_an_excluded_directory_count_nothing() {
    let (temp, c, grant, root, run) = setup();
    let w = c.writer();
    let proj = directory("proj", 5);
    w.call(listing(run, root, vec![], vec![proj.clone()]))
        .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let proj_id: i64 = db
        .query_row("SELECT id FROM dir WHERE name='proj'", [], |r| r.get(0))
        .unwrap();
    let mut top = identified("top.txt", 8, 30);
    top.extension = Some("txt".into());
    top.family = "document".into();
    w.call(listing(run, proj_id, vec![top], vec![])).unwrap();
    end(w, run, "completed");
    let run = begin(w, grant, 5);
    let mut placeholder = proj;
    placeholder.attrs |= 0x1000;
    placeholder.flags = 1 << 3;
    w.call(listing(run, root, vec![], vec![placeholder]))
        .unwrap();
    end(w, run, "completed");
    let mut reader = c.reader().unwrap();
    let b = reader
        .breakdown(proj_id, "extension", Basis::Logical, 10, 1_000_000)
        .unwrap();
    assert!(b.buckets.is_empty(), "{:?}", b.buckets);
    assert_eq!((b.other.files, b.unknown.files), (0, 0));
    assert!(!b.complete);
    let sums = reader
        .inspect(NodeKey::Dir(proj_id))
        .unwrap()
        .subtree
        .unwrap();
    assert_eq!(
        (sums.unique_objects, sums.unique_allocated_bytes),
        (None, None),
        "unknown, never zero"
    );
}

#[test]
fn refresh_root_adopts_legacy_null_guid_once_without_replacing_grant() {
    for filesystem in ["NTFS", "exFAT"] {
        let temp = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(temp.path(), "synthetic").unwrap();
        let writer = catalog.writer();
        let mut observation = RootObservation {
            volume_guid: None,
            volume_key: "fixture-volume".into(),
            display_name: "Synthetic volume".into(),
            display_path: "Synthetic root".into(),
            root_file_id: Some(vec![1; 16]),
            filesystem: Some(filesystem.into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 1,
        };
        let WriteReply::Root {
            grant_id, dir_id, ..
        } = writer
            .call(WriteCommand::RegisterRoot(observation.clone()))
            .unwrap()
        else {
            panic!("root expected")
        };
        let state = Connection::open(temp.path().join("state.db")).unwrap();
        let revision = || {
            state
                .query_row("SELECT state_rev FROM revision", [], |r| r.get::<_, i64>(0))
                .unwrap()
        };
        let before = revision();
        observation.volume_guid = Some("0123456789abcdef0123456789abcdef".into());
        for field in ["volume", "id", "guid", "path"] {
            if field == "path" && filesystem == "NTFS" {
                continue;
            }
            let mut wrong = observation.clone();
            match field {
                "volume" => wrong.volume_key = "different-volume".into(),
                "id" => wrong.root_file_id = Some(vec![2; 16]),
                "path" => wrong.display_path = "Different synthetic root".into(),
                // A non-null stored binding must never be replaced.
                "guid" => {
                    state
                        .execute("UPDATE root_grant SET volume_guid='other-guid'", [])
                        .unwrap();
                }
                _ => unreachable!(),
            }
            assert!(
                writer
                    .call(WriteCommand::RefreshRoot {
                        grant_id,
                        observation: wrong
                    })
                    .is_err(),
                "{filesystem}: {field}"
            );
            state
                .execute("UPDATE root_grant SET volume_guid=NULL", [])
                .unwrap();
            assert_eq!(revision(), before);
        }
        for expected_revision in [before + 1, before + 1] {
            let WriteReply::Root {
                grant_id: refreshed,
                dir_id: refreshed_dir,
                ..
            } = writer
                .call(WriteCommand::RefreshRoot {
                    grant_id,
                    observation: observation.clone(),
                })
                .unwrap()
            else {
                panic!("root expected")
            };
            assert_eq!((refreshed, refreshed_dir), (grant_id, dir_id));
            assert_eq!(revision(), expected_revision);
            assert_eq!(
                state
                    .query_row(
                        "SELECT count(*) FROM root_grant WHERE state='active'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                1
            );
            assert_eq!(
                state
                    .query_row("SELECT volume_guid FROM root_grant", [], |r| r
                        .get::<_, String>(0))
                    .unwrap(),
                observation.volume_guid.as_ref().unwrap().as_str()
            );
        }
    }
}
