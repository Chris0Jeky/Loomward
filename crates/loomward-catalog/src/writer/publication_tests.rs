use super::*;

fn depth_fixture() -> (tempfile::TempDir, Connection) {
    let (temp, conn, _) = fixture();
    conn.execute_batch(
        "DELETE FROM st.collection_member; DELETE FROM st.object_ref; DELETE FROM stage_entry;
         DELETE FROM file; DELETE FROM dir WHERE id!=1;
         UPDATE dir SET own_files=0,own_logical=0,own_allocated=0,own_noid=0;
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<511)
         INSERT INTO dir(id,root_id,parent_id,name,file_id,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run)
         SELECT i+9,1,CASE i WHEN 1 THEN 1 ELSE i+8 END,'spine-'||i,
                CASE i WHEN 1 THEN x'0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a'
                       WHEN 2 THEN x'0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c'
                       WHEN 511 THEN x'0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b' END,
                CASE WHEN i IN (1,2,511) THEN 'native_file_id_128' ELSE 'path_observation' END,
                CASE WHEN i IN (1,2,511) THEN 'listed' ELSE 'none' END,i,0,'complete',2,2 FROM n;
         INSERT INTO dir(id,root_id,parent_id,name,file_id,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run)
         VALUES(600,1,1,'destination',x'14141414141414141414141414141414','native_file_id_128','listed',1,0,'complete',2,2),
               (601,1,1,'control',NULL,'path_observation','none',1,0,'complete',2,2);
         UPDATE dir SET created_ft=0;"
    ).unwrap();
    (temp, conn)
}

fn depth_listing(dir_id: i64, files: Vec<Observation>, dirs: Vec<Observation>) -> WriteCommand {
    WriteCommand::DirListing(DirListing {
        run_id: 2,
        dir_id,
        files,
        dirs,
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    })
}

fn depth_batch(
    conn: Connection,
    commands: Vec<WriteCommand>,
) -> (Vec<Result<WriteReply>>, WriterTimings) {
    let (tx, rx) = bounded(commands.len());
    let mut replies = Vec::new();
    for command in commands {
        let (reply, result) = bounded(1);
        tx.send(Pending {
            command,
            reply,
            _permit: BytePermit {
                quota: Arc::new((Mutex::new(0), Condvar::new())),
                bytes: 0,
            },
        })
        .unwrap();
        replies.push(result);
    }
    drop(tx);
    let timings = Arc::new(Mutex::new(WriterTimings::default()));
    let (alive, _) = bounded(0);
    work(conn, rx, timings.clone(), alive, None);
    let results = replies.into_iter().map(|r| r.recv().unwrap()).collect();
    let timing = timings.lock().unwrap().clone();
    (results, timing)
}

fn depth_end_run() -> WriteCommand {
    WriteCommand::EndRun {
        run_id: 2,
        state: "completed".into(),
        finished_at_ns: 6,
    }
}

#[test]
fn reparent_bounds_the_whole_subtree_and_keeps_the_batch_and_end_run_usable() {
    let (temp, conn) = depth_fixture();
    let (mut results, timing) = depth_batch(
        conn,
        vec![
            depth_listing(601, vec![observation("before", 30, 42)], vec![]),
            depth_listing(
                600,
                vec![observation("rolled-back", 31, 42)],
                vec![observation("spine-1", 10, 0)],
            ),
            depth_listing(520, vec![observation("after", 32, 42)], vec![]),
            // Moving spine-2 instead lands its deepest descendant at exactly depth 511.
            depth_listing(600, vec![], vec![observation("spine-2", 12, 0)]),
            depth_end_run(),
        ],
    );
    assert!(matches!(
        results.remove(1),
        Err(Error::Invalid("tree depth limit"))
    ));
    for result in results {
        result.unwrap();
    }
    assert_eq!(timing.max_transaction_commands, 4);
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT parent_id FROM dir WHERE id=10", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT parent_id FROM dir WHERE id=11", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        600
    );
    assert_eq!(
        db.query_row("SELECT max(depth) FROM dir", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        511
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        db.query_row(
            "SELECT sub_files,sub_logical,agg_valid_rev=dirty_rev FROM dir WHERE id=1",
            [],
            |r| Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, bool>(2)?
            ))
        )
        .unwrap(),
        (2, 20, true)
    );
}

#[test]
fn a_shallower_intra_batch_reparent_does_not_trust_stored_depth() {
    let (temp, conn) = depth_fixture();
    assert_eq!(
        conn.query_row("SELECT depth FROM dir WHERE id=520", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        511
    );
    let (results, timing) = depth_batch(
        conn,
        vec![
            depth_listing(600, vec![], vec![observation("spine-511", 11, 0)]),
            depth_listing(520, vec![], vec![observation("new-child", 13, 42)]),
            depth_end_run(),
        ],
    );
    for result in results {
        result.unwrap();
    }
    assert_eq!(timing.max_transaction_commands, 2);
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT depth FROM dir WHERE name='new-child'", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
}

#[test]
fn rejected_reparent_clears_parent_cache_before_the_next_chain_check() {
    let (temp, conn) = depth_fixture();
    let (mut results, timing) = depth_batch(
        conn,
        vec![
            depth_listing(
                600,
                vec![],
                vec![observation("spine-511", 11, 0), observation("cycle", 20, 0)],
            ),
            // A stale cache says this is shallow; SQLite still has the 511-deep spine.
            depth_listing(520, vec![], vec![observation("too-deep", 13, 42)]),
            depth_listing(520, vec![observation("accepted", 30, 42)], vec![]),
            depth_end_run(),
        ],
    );
    assert!(matches!(
        results.remove(0),
        Err(Error::Invalid("directory identity cycle"))
    ));
    assert!(matches!(
        results.remove(0),
        Err(Error::Invalid("tree depth limit"))
    ));
    for result in results {
        result.unwrap();
    }
    assert_eq!(timing.max_transaction_commands, 3);
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT parent_id FROM dir WHERE id=520", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        519
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM dir WHERE name='too-deep'", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT sub_logical FROM dir WHERE id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        10
    );
}

#[test]
fn chain_lengths_reuse_proven_seeds_but_count_the_unproven_prefix() {
    let conn = Connection::open_in_memory().unwrap();
    let mut ctx = Context {
        timings: Arc::new(Mutex::new(WriterTimings::default())),
        parents: (1..=513)
            .map(|id| (id, (id > 1).then_some(id - 1)))
            .collect(),
        dirty: Vec::new(),
        extensions: HashMap::new(),
        revision: 1,
        fenced_listings: HashSet::new(),
    };
    let mut proven = HashMap::new();
    assert_eq!(ctx.chain_length(&conn, 512, &mut proven).unwrap(), 512);
    assert_eq!(proven.len(), 512);
    // No table exists: revisiting this proven seed must need neither a parent walk nor SQL.
    ctx.parents.remove(&512);
    assert_eq!(ctx.chain_length(&conn, 512, &mut proven).unwrap(), 512);
    assert!(matches!(
        ctx.chain_length(&conn, 513, &mut proven),
        Err(Error::Invalid("tree depth limit"))
    ));
}

fn observation(name: &str, identity: u8, creation: i64) -> Observation {
    let mut o = Observation::file(name, 10, Some(10));
    o.file_id = Some(vec![identity; 16]);
    o.created_ft = Some(creation);
    o
}

fn fixture() -> (tempfile::TempDir, Connection, WriteCommand) {
    let temp = tempfile::tempdir().unwrap();
    let writer = Writer::start(crate::db::open_pair(temp.path(), "synthetic").unwrap()).unwrap();
    writer
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_key: "fixture".into(),
            display_name: "Fixture".into(),
            display_path: "Synthetic root".into(),
            root_file_id: Some(vec![1; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 1,
        }))
        .unwrap();
    writer
        .call(WriteCommand::BeginRun {
            grant_id: 1,
            mode: "full".into(),
            strategy: "fixture".into(),
            started_at_ns: 2,
        })
        .unwrap();
    let listing = |dir_id, files, dirs| {
        WriteCommand::DirListing(DirListing {
            run_id: 1,
            dir_id,
            files,
            dirs,
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        })
    };
    writer
        .call(listing(
            1,
            vec![
                observation("a", 2, 42),
                observation("link", 2, 42),
                observation("omitted", 3, 42),
            ],
            vec![observation("child", 4, 42)],
        ))
        .unwrap();
    writer
        .call(listing(2, vec![observation("nested", 5, 42)], vec![]))
        .unwrap();
    for node in [
        NodeKey::File(1),
        NodeKey::File(3),
        NodeKey::Dir(2),
        NodeKey::File(4),
    ] {
        writer
            .call(WriteCommand::ObjectReference {
                node,
                observed_at_ns: 3,
            })
            .unwrap();
    }
    writer
        .call(WriteCommand::EndRun {
            run_id: 1,
            state: "completed".into(),
            finished_at_ns: 4,
        })
        .unwrap();
    writer
        .call(WriteCommand::BeginRun {
            grant_id: 1,
            mode: "refresh".into(),
            strategy: "fixture".into(),
            started_at_ns: 5,
        })
        .unwrap();
    writer
        .call(WriteCommand::StageChunk {
            run_id: 2,
            dir_id: 1,
            seq: 0,
            files: vec![observation("link", 2, 42)],
            dirs: vec![observation("child", 4, 100)],
        })
        .unwrap();
    drop(writer);
    let conn = Connection::open(temp.path().join("catalog.db")).unwrap();
    conn.execute(
        "ATTACH DATABASE ?1 AS st",
        [temp.path().join("state.db").to_string_lossy().as_ref()],
    )
    .unwrap();
    conn.execute_batch(
        "PRAGMA main.foreign_keys=ON; PRAGMA main.synchronous=NORMAL; PRAGMA st.synchronous=FULL;
        INSERT INTO st.collection VALUES(1,'Kept','human',1);
        INSERT INTO st.collection_member SELECT 1,id,1 FROM st.object_ref;",
    )
    .unwrap();
    crate::db::grant_view(&conn).unwrap();
    (
        temp,
        conn,
        WriteCommand::ListingDone {
            run_id: 2,
            dir_id: 1,
            outcome: ListingOutcome::Complete,
            skipped: 0,
            errors: 0,
        },
    )
}

fn publish(
    conn: Connection,
    command: WriteCommand,
    crash: Option<PublicationCrash>,
) -> Result<WriteReply> {
    let (tx, rx) = bounded(1);
    let (reply, result) = bounded(1);
    tx.send(Pending {
        command,
        reply,
        _permit: BytePermit {
            quota: Arc::new((Mutex::new(0), Condvar::new())),
            bytes: 0,
        },
    })
    .unwrap();
    drop(tx);
    let (alive, _) = bounded(0);
    work(
        conn,
        rx,
        Arc::new(Mutex::new(WriterTimings::default())),
        alive,
        crash,
    );
    result.recv().unwrap_or(Err(Error::Closed))
}

fn bindings(dir: &std::path::Path) -> Vec<(i64, String, bool, i64, i64)> {
    let st = Connection::open(dir.join("state.db")).unwrap();
    let mut s = st
        .prepare("SELECT id,state,retired,row_id,born_run FROM object_ref ORDER BY id")
        .unwrap();
    s.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })
    .unwrap()
    .collect::<rusqlite::Result<Vec<_>>>()
    .unwrap()
}

type DirMark = (i64, Option<i64>, Option<i64>, Option<i64>, i64);

fn dir_marks(dir: &std::path::Path) -> Vec<DirMark> {
    let db = Connection::open(dir.join("catalog.db")).unwrap();
    let mut s = db
        .prepare("SELECT id,dirty_rev,subtree_rev,dirty_run,sub_complete FROM dir ORDER BY id")
        .unwrap();
    s.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })
    .unwrap()
    .collect::<rusqlite::Result<Vec<_>>>()
    .unwrap()
}

fn old_listing(dir: &std::path::Path) -> (i64, Vec<(i64, String)>) {
    let db = Connection::open(dir.join("catalog.db")).unwrap();
    let rev = db
        .query_row("SELECT catalog_rev FROM revision", [], |r| r.get(0))
        .unwrap();
    let mut s = db.prepare("SELECT id,name FROM file ORDER BY id").unwrap();
    (
        rev,
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap(),
    )
}

fn assert_memberships(dir: &std::path::Path) {
    let mut reader = Reader::open(dir).unwrap();
    for node in [
        NodeKey::File(1),
        NodeKey::File(3),
        NodeKey::Dir(2),
        NodeKey::File(4),
    ] {
        assert_eq!(reader.inspect(node).unwrap().memberships[0].name, "Kept");
    }
    let page = reader
        .search(&SearchRequest {
            root_id: Some(1),
            text: String::new(),
            extension: None,
            min_bytes: None,
            kind: "file".into(),
            limit: 10,
            cursor: None,
            work_budget: 2_000_000,
        })
        .unwrap();
    assert_eq!(page.items.len(), 4);
}

fn assert_recovered_slice(dir: &std::path::Path) {
    let slice = Reader::open(dir)
        .unwrap()
        .slice(&SliceRequest {
            anchor: NodeKey::Dir(1),
            depth: 2,
            max_nodes: 20,
            min_share: 0.0,
            basis: Basis::Logical,
            include_files: true,
        })
        .unwrap();
    assert_eq!(slice.nodes[0].logical_bytes, "40");
}

fn pending_count(dir: &std::path::Path) -> i64 {
    Connection::open(dir.join("state.db"))
        .unwrap()
        .query_row("SELECT count(*) FROM pending_reference", [], |r| r.get(0))
        .unwrap()
}

fn assert_publication_sync(conn: &Connection, after_commit: bool) {
    let pending: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM st.pending_reference)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mode: i64 = conn
        .query_row("PRAGMA main.synchronous", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, if pending && !after_commit { 2 } else { 1 });
    assert_eq!(
        conn.query_row("PRAGMA st.synchronous", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(conn.is_autocommit(), after_commit);
}

#[test]
fn publication_sync_is_full_only_with_intents_and_normal_is_restored() {
    for intents in [true, false] {
        let (_temp, conn, command) = fixture();
        if !intents {
            conn.execute_batch("DELETE FROM st.collection_member; DELETE FROM st.object_ref")
                .unwrap();
        }
        publish(
            conn,
            command,
            Some(PublicationCrash::Observe(assert_publication_sync)),
        )
        .unwrap();
    }
}

#[test]
fn resolved_references_keep_two_queued_publications_in_singleton_transactions() {
    let (temp, conn, first) = fixture();
    let second = WriteCommand::DirListing(DirListing {
        run_id: 2,
        dir_id: 1,
        files: vec![],
        dirs: vec![],
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    });
    let (tx, rx) = bounded(2);
    let mut replies = Vec::new();
    for command in [first, second] {
        let (reply, result) = bounded(1);
        tx.send(Pending {
            command,
            reply,
            _permit: BytePermit {
                quota: Arc::new((Mutex::new(0), Condvar::new())),
                bytes: 0,
            },
        })
        .unwrap();
        replies.push(result);
    }
    drop(tx);
    let timings = Arc::new(Mutex::new(WriterTimings::default()));
    let (alive, _) = bounded(0);
    work(
        conn,
        rx,
        timings.clone(),
        alive,
        Some(PublicationCrash::Observe(assert_publication_sync)),
    );
    for reply in replies {
        reply.recv().unwrap().unwrap();
    }
    let timings = timings.lock().unwrap();
    assert_eq!(timings.transactions, 2);
    assert_eq!(timings.max_transaction_commands, 1);
    assert_eq!(pending_count(temp.path()), 0);
    let after = bindings(temp.path());
    assert!(after.iter().all(|r| r.1 == "unresolved" && r.2));
    assert_eq!(old_listing(temp.path()).1, vec![(4, "nested".into())]);
}

#[test]
fn auto_rollback_never_commits_a_failed_receipt_or_acknowledges_lost_rows() {
    for full in [true, false] {
        let (temp, conn, _) = fixture();
        conn.execute_batch(
            "DELETE FROM st.collection_member; DELETE FROM st.object_ref;
        CREATE TRIGGER auto_rollback BEFORE INSERT ON file WHEN NEW.name='fault'
        BEGIN SELECT RAISE(ROLLBACK,'injected transaction rollback'); END;",
        )
        .unwrap();
        if full {
            conn.execute_batch(
                "DROP TRIGGER auto_rollback; CREATE TABLE fault_growth(value BLOB);
            CREATE TRIGGER auto_rollback BEFORE INSERT ON file WHEN NEW.name='fault'
            BEGIN INSERT INTO fault_growth VALUES(zeroblob(10000000)); END;",
            )
            .unwrap();
            let pages: i64 = conn
                .query_row("PRAGMA main.page_count", [], |r| r.get(0))
                .unwrap();
            conn.pragma_update(None, "max_page_count", pages + 32)
                .unwrap();
            let error = conn
                .execute("INSERT INTO fault_growth VALUES(zeroblob(10000000))", [])
                .unwrap_err();
            assert_eq!(
                error.sqlite_error_code(),
                Some(rusqlite::ErrorCode::DiskFull)
            );
        }
        let before = old_listing(temp.path());
        let marks = dir_marks(temp.path());
        let (tx, rx) = bounded(3);
        let mut replies = Vec::new();
        for name in ["before", "fault", "after"] {
            let (reply, result) = bounded(1);
            tx.send(Pending {
                command: WriteCommand::DirListing(DirListing {
                    run_id: 2,
                    dir_id: 1,
                    files: vec![observation(name, 6, 42)],
                    dirs: vec![observation("child", 4, 42)],
                    state: "complete".into(),
                    skipped: 0,
                    errors: 0,
                }),
                reply,
                _permit: BytePermit {
                    quota: Arc::new((Mutex::new(0), Condvar::new())),
                    bytes: 0,
                },
            })
            .unwrap();
            replies.push(result);
        }
        drop(tx);
        let (alive, _) = bounded(0);
        work(
            conn,
            rx,
            Arc::new(Mutex::new(WriterTimings::default())),
            alive,
            if full {
                None
            } else {
                Some(PublicationCrash::Observe(|conn, _| {
                    assert!(conn.is_autocommit(), "fault did not auto-rollback")
                }))
            },
        );
        if full {
            // This FULL fails only the statement/savepoint; the ROLLBACK case below ends the batch.
            let results: Vec<_> = replies.into_iter().map(|r| r.recv().unwrap()).collect();
            assert!(results[0].is_ok());
            assert!(
                matches!(&results[1], Err(Error::Sql(e)) if e.sqlite_error_code()==Some(rusqlite::ErrorCode::DiskFull))
            );
            assert!(results[2].is_ok());
            assert_eq!(
                old_listing(temp.path()).1,
                vec![(4, "nested".into()), (5, "after".into())]
            );
            continue;
        }
        for reply in replies {
            assert!(
                reply.recv().unwrap_or(Err(Error::Closed)).is_err(),
                "rolled-back listing acknowledged"
            );
        }
        assert_eq!(
            old_listing(temp.path()),
            before,
            "failed caller's listing persisted"
        );
        assert_eq!(
            dir_marks(temp.path()),
            marks,
            "dirty marks written after the auto-rollback"
        );
        drop(crate::db::open_pair(temp.path(), "synthetic").unwrap());
        assert_eq!(old_listing(temp.path()).1, before.1);
    }
}

#[test]
fn auto_rollback_keeps_the_failing_callers_original_error() {
    let (temp, conn, _) = fixture();
    conn.execute_batch(
        "DELETE FROM st.collection_member; DELETE FROM st.object_ref;
        CREATE TRIGGER abort_transaction BEFORE INSERT ON file WHEN NEW.name='fault'
        BEGIN SELECT RAISE(ROLLBACK,'first synthetic failure'); END;",
    )
    .unwrap();
    let before = old_listing(temp.path());
    let (sender, receiver) = bounded(3);
    let mut replies = Vec::new();
    for name in ["before", "fault", "after"] {
        let (reply, result) = bounded(1);
        sender
            .send(Pending {
                command: WriteCommand::DirListing(DirListing {
                    run_id: 2,
                    dir_id: 1,
                    files: vec![observation(name, 6, 42)],
                    dirs: vec![],
                    state: "complete".into(),
                    skipped: 0,
                    errors: 0,
                }),
                reply,
                _permit: BytePermit {
                    quota: Arc::new((Mutex::new(0), Condvar::new())),
                    bytes: 0,
                },
            })
            .unwrap();
        replies.push(result);
    }
    drop(sender);
    let (alive, _) = bounded(0);
    work(
        conn,
        receiver,
        Arc::new(Mutex::new(WriterTimings::default())),
        alive,
        None,
    );
    let error = replies[1].recv().unwrap().unwrap_err();
    assert!(
        matches!(error, Error::Sql(rusqlite::Error::SqliteFailure(code, Some(ref message)))
        if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER && message == "first synthetic failure"),
        "original failure lost: {error:?}"
    );
    assert!(matches!(replies[0].recv().unwrap(), Err(Error::Closed)));
    assert!(replies[2].recv().unwrap_or(Err(Error::Closed)).is_err());
    assert_eq!(old_listing(temp.path()), before);
}

#[test]
fn a_savepoint_after_auto_rollback_would_start_and_commit_a_fresh_transaction() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE probe(value TEXT);
        CREATE TRIGGER abort_transaction BEFORE INSERT ON probe WHEN NEW.value='fault'
        BEGIN SELECT RAISE(ROLLBACK,'synthetic auto rollback'); END;",
    )
    .unwrap();
    let mut tx = conn.transaction().unwrap();
    tx.execute("INSERT INTO probe VALUES('lost')", []).unwrap();
    {
        let save = tx.savepoint().unwrap();
        let error = save
            .execute("INSERT INTO probe VALUES('fault')", [])
            .unwrap_err();
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::ConstraintViolation)
        );
    }
    assert!(tx.is_autocommit());
    {
        let save = tx.savepoint().unwrap();
        assert!(
            !save.is_autocommit(),
            "SAVEPOINT did not start a fresh transaction"
        );
        save.execute("INSERT INTO probe VALUES('fresh')", [])
            .unwrap();
        save.commit().unwrap();
    }
    assert!(tx.is_autocommit());
    assert!(tx.commit().is_err());
    assert_eq!(
        conn.query_row("SELECT group_concat(value) FROM probe", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "fresh"
    );
}

#[test]
fn real_disk_full_auto_rollback_also_allows_a_new_implicit_transaction() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE growth(value BLOB);")
        .unwrap();
    let pages: i64 = conn
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    conn.pragma_update(None, "max_page_count", pages + 8)
        .unwrap();
    let mut tx = conn.transaction().unwrap();
    tx.execute("INSERT INTO growth VALUES(zeroblob(1))", [])
        .unwrap();
    let error = tx
        .execute("INSERT INTO growth VALUES(zeroblob(10000000))", [])
        .unwrap_err();
    assert_eq!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DiskFull)
    );
    assert!(
        tx.is_autocommit(),
        "FULL did not roll back the outer transaction"
    );
    let save = tx.savepoint().unwrap();
    assert!(!save.is_autocommit());
    save.execute("INSERT INTO growth VALUES(zeroblob(2))", [])
        .unwrap();
    save.commit().unwrap();
    assert!(tx.is_autocommit());
    assert!(tx.commit().is_err());
    assert_eq!(
        conn.query_row("SELECT length(value) FROM growth", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn hardlink_membership_survives_publication_before_confirmation() {
    for crash in [
        PublicationCrash::BeforeCatalog,
        PublicationCrash::AfterCatalog,
    ] {
        let (temp, conn, command) = fixture();
        let before = bindings(temp.path());
        publish(conn, command, Some(crash)).unwrap_err();
        assert_eq!(bindings(temp.path()), before);
        assert_eq!(pending_count(temp.path()), 4);
        let membership = Reader::open(temp.path())
            .unwrap()
            .inspect(NodeKey::File(2))
            .unwrap()
            .memberships;
        if matches!(crash, PublicationCrash::BeforeCatalog) {
            assert_memberships(temp.path());
            // An uncommitted witness must not expose the pending new binding.
            assert!(membership.is_empty());
        } else {
            assert_eq!(membership[0].name, "Kept");
            drop(crate::db::open_pair(temp.path(), "synthetic").unwrap());
            assert_eq!(
                Reader::open(temp.path())
                    .unwrap()
                    .inspect(NodeKey::File(2))
                    .unwrap()
                    .memberships[0]
                    .name,
                "Kept"
            );
        }
    }
}

#[test]
fn catalogue_commit_failure_reverts_pending_references_and_preserves_listing() {
    let (temp, conn, command) = fixture();
    let before = bindings(temp.path());
    let listing = old_listing(temp.path());
    // Deferred constraint passes the rollback-only publication but fails the real COMMIT.
    conn.execute_batch("CREATE TABLE fault_parent(id INTEGER PRIMARY KEY);
        CREATE TABLE fault_child(id INTEGER REFERENCES fault_parent(id) DEFERRABLE INITIALLY DEFERRED);
        CREATE TRIGGER fail_commit AFTER UPDATE ON revision BEGIN INSERT INTO fault_child VALUES(1);END;").unwrap();
    assert!(matches!(
        publish(
            conn,
            command,
            Some(PublicationCrash::Observe(assert_publication_sync))
        ),
        Err(Error::Sql(_))
    ));
    assert_eq!(bindings(temp.path()), before);
    assert_eq!(old_listing(temp.path()), listing);
    assert_memberships(temp.path());
    assert_eq!(pending_count(temp.path()), 0);
}

#[test]
fn crash_before_catalogue_commit_recovery_discards_pending_references() {
    let (temp, conn, command) = fixture();
    let before = bindings(temp.path());
    let listing = old_listing(temp.path());
    assert!(matches!(
        publish(conn, command, Some(PublicationCrash::BeforeCatalog)),
        Err(Error::Closed)
    ));
    assert_eq!(old_listing(temp.path()), listing);
    assert_eq!(bindings(temp.path()), before);
    assert_memberships(temp.path());
    assert_eq!(pending_count(temp.path()), 4);
    drop(crate::db::open_pair(temp.path(), "synthetic").unwrap());
    assert_eq!(bindings(temp.path()), before);
    assert_memberships(temp.path());
    assert_recovered_slice(temp.path());
    assert_eq!(pending_count(temp.path()), 0);
}

#[test]
fn crash_after_catalogue_commit_recovery_confirms_pending_references() {
    let (temp, conn, command) = fixture();
    let before = bindings(temp.path());
    assert!(matches!(
        publish(conn, command, Some(PublicationCrash::AfterCatalog)),
        Err(Error::Closed)
    ));
    assert_eq!(
        old_listing(temp.path()).1,
        vec![(2, "link".into()), (4, "nested".into())]
    );
    assert_eq!(bindings(temp.path()), before);
    assert_eq!(pending_count(temp.path()), 4);
    drop(crate::db::open_pair(temp.path(), "synthetic").unwrap());
    assert_eq!(pending_count(temp.path()), 0);
    let after = bindings(temp.path());
    assert_eq!(after[0], (1, "resolved".into(), false, 2, 1));
    assert!(after[1..].iter().all(|r| r.1 == "unresolved" && r.2));
    assert_eq!(
        Reader::open(temp.path())
            .unwrap()
            .inspect(NodeKey::File(2))
            .unwrap()
            .memberships[0]
            .name,
        "Kept"
    );
}

#[test]
fn pending_reference_recovery_twice_is_idempotent() {
    for crash in [
        PublicationCrash::BeforeCatalog,
        PublicationCrash::AfterCatalog,
    ] {
        let (temp, conn, command) = fixture();
        let before = bindings(temp.path());
        publish(conn, command, Some(crash)).unwrap_err();
        drop(crate::db::open_pair(temp.path(), "synthetic").unwrap());
        let once = bindings(temp.path());
        assert_eq!(pending_count(temp.path()), 0);
        if matches!(crash, PublicationCrash::BeforeCatalog) {
            assert_eq!(once, before);
        } else {
            assert!(once[1..].iter().all(|r| r.1 == "unresolved" && r.2));
        }
        let st = Connection::open(temp.path().join("state.db")).unwrap();
        let revision: i64 = st
            .query_row("SELECT state_rev FROM revision", [], |r| r.get(0))
            .unwrap();
        drop(crate::db::open_pair(temp.path(), "synthetic").unwrap());
        assert_eq!(bindings(temp.path()), once);
        assert_eq!(
            st.query_row("SELECT state_rev FROM revision", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            revision
        );
    }
}

#[test]
fn v2_upgrade_preserves_bindings_membership_and_catalogue_instance() {
    let (temp, conn, _) = fixture();
    let before = bindings(temp.path());
    let instance: String = conn
        .query_row("SELECT value FROM meta WHERE key='instance_id'", [], |r| {
            r.get(0)
        })
        .unwrap();
    conn.execute_batch(
        "DROP TABLE st.pending_reference; PRAGMA main.user_version=2; PRAGMA st.user_version=2",
    )
    .unwrap();
    drop(conn);
    let conn = crate::db::open_pair(temp.path(), "synthetic").unwrap();
    assert_eq!(
        conn.query_row("SELECT value FROM meta WHERE key='instance_id'", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        instance
    );
    assert_eq!(bindings(temp.path()), before);
    assert_eq!(pending_count(temp.path()), 0);
    assert!(temp.path().join("state.before-v2.db").exists());
    assert_memberships(temp.path());
    assert_recovered_slice(temp.path());
}

#[test]
fn confirmation_failure_closes_writer_and_recovery_retries() {
    let (temp, conn, command) = fixture();
    let before = bindings(temp.path());
    conn.execute_batch("CREATE TRIGGER st.fail_confirmation BEFORE UPDATE ON object_ref BEGIN SELECT RAISE(ABORT,'test confirmation failure'); END").unwrap();
    let writer = Writer::start(conn).unwrap();
    assert!(matches!(writer.call(command), Err(Error::Sql(_))));
    assert!(matches!(
        writer.send(WriteCommand::Barrier),
        Err(Error::Closed)
    ));
    drop(writer);
    assert_eq!(bindings(temp.path()), before);
    assert_eq!(pending_count(temp.path()), 4);
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    st.execute_batch("DROP TRIGGER fail_confirmation").unwrap();
    drop(st);
    drop(crate::db::open_pair(temp.path(), "synthetic").unwrap());
    let after = bindings(temp.path());
    assert_eq!(after[0], (1, "resolved".into(), false, 2, 1));
    assert!(after[1..].iter().all(|r| r.1 == "unresolved" && r.2));
    assert_eq!(pending_count(temp.path()), 0);
}

#[test]
fn batch_dirtying_uses_latest_revisions_and_both_move_chains_after_rejection() {
    let (temp, conn, _) = fixture();
    conn.execute_batch(
        "DELETE FROM st.collection_member; DELETE FROM st.object_ref; DELETE FROM stage_entry",
    )
    .unwrap();
    let (tx, rx) = bounded(16);
    let mut replies = Vec::new();
    let listing = |dir_id, files, dirs| {
        WriteCommand::DirListing(DirListing {
            run_id: 2,
            dir_id,
            files,
            dirs,
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        })
    };
    let cycle = observation("cycle", 7, 42);
    for command in [
        listing(
            1,
            vec![],
            vec![observation("A", 6, 42), observation("B", 7, 42)],
        ),
        listing(3, vec![], vec![observation("x", 8, 42)]),
        listing(5, vec![observation("item", 9, 42)], vec![]),
        listing(4, vec![], vec![observation("x", 8, 42)]),
        listing(4, vec![], vec![observation("rolled-back", 10, 42), cycle]),
        listing(5, vec![observation("new-item", 9, 42)], vec![]),
    ] {
        let (reply, result) = bounded(1);
        tx.send(Pending {
            command,
            reply,
            _permit: BytePermit {
                quota: Arc::new((Mutex::new(0), Condvar::new())),
                bytes: 0,
            },
        })
        .unwrap();
        replies.push(result);
    }
    drop(tx);
    let timings = Arc::new(Mutex::new(WriterTimings::default()));
    let (alive, _) = bounded(0);
    work(conn, rx, timings.clone(), alive, None);
    let mut revisions = Vec::new();
    for (i, reply) in replies.into_iter().enumerate() {
        let result = reply.recv().unwrap();
        if i == 4 {
            assert!(matches!(
                result,
                Err(Error::Invalid("directory identity cycle"))
            ));
        } else if let WriteReply::Listing { revision } = result.unwrap() {
            revisions.push(revision);
        } else {
            panic!();
        }
    }
    let timing = timings.lock().unwrap();
    assert_eq!(timing.transactions, 1);
    assert_eq!(
        timing.dirty_updates, 4,
        "each surviving directory is dirtied once per transaction"
    );
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    for (id, expected) in [
        (1, revisions[4]),
        (3, revisions[3]),
        (4, revisions[4]),
        (5, revisions[4]),
    ] {
        assert_eq!(
            db.query_row(
                "SELECT dirty_rev,subtree_rev,dirty_run,sub_complete FROM dir WHERE id=?1",
                [id],
                |r| Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, bool>(3)?
                ))
            )
            .unwrap(),
            (expected, expected, 2, false)
        );
    }
    assert_eq!(
        db.query_row("SELECT parent_id FROM dir WHERE id=5", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        4
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM dir WHERE name='rolled-back'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn extension_cache_cannot_reuse_a_rolled_back_id_or_family() {
    let (temp, conn, _) = fixture();
    conn.execute_batch(
        "DELETE FROM st.collection_member; DELETE FROM st.object_ref; DELETE FROM stage_entry",
    )
    .unwrap();
    let (tx, rx) = bounded(2);
    let mut replies = Vec::new();
    for families in [vec!["data", "image"], vec!["document"]] {
        let files = families
            .into_iter()
            .enumerate()
            .map(|(i, family)| {
                let mut o = observation(&format!("file-{i}"), i as u8 + 10, 42);
                o.extension = Some("new-extension".into());
                o.family = family.into();
                o
            })
            .collect();
        let (reply, result) = bounded(1);
        tx.send(Pending {
            command: WriteCommand::DirListing(DirListing {
                run_id: 2,
                dir_id: 1,
                files,
                dirs: vec![],
                state: "complete".into(),
                skipped: 0,
                errors: 0,
            }),
            reply,
            _permit: BytePermit {
                quota: Arc::new((Mutex::new(0), Condvar::new())),
                bytes: 0,
            },
        })
        .unwrap();
        replies.push(result);
    }
    drop(tx);
    let (alive, _) = bounded(0);
    work(
        conn,
        rx,
        Arc::new(Mutex::new(WriterTimings::default())),
        alive,
        None,
    );
    assert!(matches!(
        replies.remove(0).recv().unwrap(),
        Err(Error::Invalid("conflicting extension family"))
    ));
    replies.remove(0).recv().unwrap().unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT family FROM ext JOIN file ON file.ext_id=ext.id",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "document"
    );
}

#[test]
fn rejected_single_pass_publication_discards_intents_before_the_next_witness() {
    let (temp, conn, command) = fixture();
    let before = bindings(temp.path());
    let old = old_listing(temp.path());
    // Publication fails after pending intents have committed; a later unrelated witness must not confirm them.
    conn.execute_batch("CREATE TRIGGER reject_publication BEFORE UPDATE ON dir BEGIN SELECT RAISE(ABORT,'synthetic rejection'); END").unwrap();
    let writer = Writer::start(conn).unwrap();
    assert!(matches!(writer.call(command), Err(Error::Sql(_))));
    assert_eq!(pending_count(temp.path()), 0);
    assert_eq!(bindings(temp.path()), before);
    assert_eq!(old_listing(temp.path()), old);
    assert_eq!(writer.timings().validation_dry_run_seconds, 0.0);
    writer.call(WriteCommand::Barrier).unwrap();
    assert_eq!(bindings(temp.path()), before);
    assert_memberships(temp.path());
}

#[test]
fn prepared_file_matches_survive_until_publication_without_recopied_or_rematched_rows() {
    let (temp, mut conn, command) = fixture();
    let timings = Arc::new(Mutex::new(WriterTimings::default()));
    let prepared = prepare_state(&mut conn, &command, &HashSet::new()).unwrap();
    assert!(matches!(prepared, Some(Prepared::Listing)));
    assert_eq!(pending_count(temp.path()), 4);
    conn.execute_batch("CREATE TEMP TRIGGER no_rematch BEFORE UPDATE ON old_file BEGIN SELECT RAISE(ABORT,'second match'); END;
        CREATE TEMP TRIGGER no_recopy BEFORE DELETE ON old_file BEGIN SELECT RAISE(ABORT,'second copy'); END;
        PRAGMA main.synchronous=FULL").unwrap();
    let mut context = Context {
        timings,
        parents: HashMap::new(),
        dirty: Vec::new(),
        extensions: HashMap::new(),
        revision: 0,
        fenced_listings: HashSet::new(),
    };
    let tx = conn.transaction().unwrap();
    tx.execute("UPDATE revision SET catalog_rev=catalog_rev+1", [])
        .unwrap();
    context.revision = tx
        .query_row("SELECT catalog_rev FROM revision", [], |r| r.get(0))
        .unwrap();
    apply(&tx, &command, &mut context, prepared).unwrap();
    context.flush_dirty(&tx).unwrap();
    tx.execute("INSERT INTO meta(key,value) SELECT 'reference_publication',publication_token FROM st.pending_reference LIMIT 1 ON CONFLICT(key) DO UPDATE SET value=excluded.value", []).unwrap();
    tx.commit().unwrap();
    conn.execute_batch("PRAGMA main.synchronous=NORMAL")
        .unwrap();
    crate::db::reconcile_references(&mut conn).unwrap();
    assert_eq!(pending_count(temp.path()), 0);
    assert_eq!(
        bindings(temp.path())[0],
        (1, "resolved".into(), false, 2, 1)
    );
    assert_eq!(
        old_listing(temp.path()).1,
        vec![(2, "link".into()), (4, "nested".into())]
    );
}

#[test]
fn file_matching_prefers_exact_name_and_consumes_each_old_row_once() {
    let (_temp, conn, _) = fixture();
    old_files(&conn, 1).unwrap();
    let identity = [2; 16];
    assert_eq!(
        match_file(&conn, "link", None, Some(&identity), Some(42))
            .unwrap()
            .unwrap()
            .0,
        2
    );
    assert_eq!(
        match_file(&conn, "renamed", None, Some(&identity), Some(42))
            .unwrap()
            .unwrap()
            .0,
        1
    );
    assert!(match_file(&conn, "a", None, Some(&identity), Some(42))
        .unwrap()
        .is_none());
    assert!(
        match_file(&conn, "omitted", None, Some(&[3; 16]), Some(100))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        conn.query_row("SELECT used FROM old_file WHERE id=3", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        -1
    );
}
