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
