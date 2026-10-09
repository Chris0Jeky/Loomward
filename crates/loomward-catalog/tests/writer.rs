use loomward_catalog::*;
use rusqlite::Connection;

fn setup() -> (tempfile::TempDir, Catalog, i64, i64, i64) {
    let temp = tempfile::tempdir().unwrap();
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    let WriteReply::Root { grant_id, dir_id } = c
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_key: "fixture-volume".into(),
            display_name: "Synthetic volume".into(),
            display_path: "Synthetic root".into(),
            root_file_id: Some([1; 16]),
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
    o.file_id = Some([id; 16]);
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
        db.query_row("SELECT count(*) FROM dir WHERE id=?1", [child], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM file WHERE name='nested'", [], |r| r
            .get::<_, i64>(
            0
        ))
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
        db.query_row("SELECT state FROM dir WHERE id=?1", [root], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "stale"
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
    assert_eq!(c.writer().queued_entries(), 0);
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
        4
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
