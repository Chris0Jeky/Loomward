use super::*;

#[test]
fn listing_scratch_is_file_backed_with_a_bounded_cache() {
    let (_temp, conn) = first_scan_fixture("full", false);
    assert_eq!(
        conn.query_row("PRAGMA temp_store", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("PRAGMA temp.cache_size", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        -16384
    );
    assert_eq!(
        conn.query_row("PRAGMA main.cache_size", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        -131072
    );
    conn.execute_batch(
        "SELECT count(*) FROM temp.root_grant; SELECT count(*) FROM temp.visible_dir",
    )
    .unwrap();
}

#[test]
fn first_scan_byte_charge_is_bounded_and_run_local() {
    let (reply, _) = bounded(1);
    let mut p = Pending {
        command: WriteCommand::StageChunk {
            run_id: 7,
            dir_id: 1,
            seq: 0,
            files: vec![],
            dirs: vec![],
        },
        reply,
        _permit: BytePermit {
            quota: Arc::new((Mutex::new(4096), Condvar::new())),
            bytes: 4096,
        },
    };
    assert_eq!(transaction_bytes(&p, Some(7)), 1024);
    assert_eq!(transaction_bytes(&p, Some(8)), 4096);
    assert_eq!(transaction_bytes(&p, None), 4096);
    p.command = WriteCommand::Barrier;
    assert_eq!(transaction_bytes(&p, None), 4096);
    p.command = WriteCommand::StageChunk {
        run_id: 7,
        dir_id: 1,
        seq: 0,
        files: vec![],
        dirs: vec![],
    };
    p._permit.bytes = 512;
    assert_eq!(transaction_bytes(&p, Some(7)), 512);
    p._permit.bytes = 4096;
    p.command = WriteCommand::StageChunk {
        run_id: 7,
        dir_id: 1,
        seq: -1,
        files: vec![],
        dirs: vec![],
    };
    assert_eq!(transaction_bytes(&p, Some(7)), 4096);
}

fn first_scan_fixture(mode: &str, prior: bool) -> (tempfile::TempDir, Connection) {
    let temp = tempfile::tempdir().unwrap();
    let conn = crate::db::open_pair(temp.path(), "synthetic").unwrap();
    conn.execute_batch("INSERT INTO volume(id,volume_key,display_name,identity_json,capabilities_json,device_json,online,observed_at_ns) VALUES(1,'fixture','Fixture','{}','{}','{}',1,0);
            INSERT INTO st.root_grant(id,volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES(1,'fixture',zeroblob(16),'Synthetic root','fixture','fixture','active',0);
            INSERT INTO root(id,grant_id,volume_id,root_file_id,active_run,state) VALUES(1,1,1,zeroblob(16),2,'scanning');
            INSERT INTO dir(id,root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run) VALUES(1,1,NULL,'Synthetic root','path_observation','none',0,0,'unlisted',0,0);").unwrap();
    if prior {
        conn.execute("INSERT INTO scan_run(id,root_id,mode,strategy,state,started_at_ns,counters_json,budget_json) VALUES(1,1,'full','fixture','cancelled',0,'{}','{}')", []).unwrap();
    }
    conn.execute("INSERT INTO scan_run(id,root_id,mode,strategy,state,started_at_ns,counters_json,budget_json) VALUES(2,1,?1,'fixture','running',0,'{}','{}')", [mode]).unwrap();
    (temp, conn)
}

#[test]
fn multilink_finalisation_does_not_group_singleton_file_rows() {
    let (_temp, conn) = first_scan_fixture("full", false);
    conn.execute_batch(
        "WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<10000)
        INSERT INTO file(dir_id,name,file_id,logical,allocated,attrs,born_run,seen_run)
        SELECT 1,'synthetic-'||i,CAST(printf('%016d',i) AS BLOB),1,1,0,2,2 FROM n;
        INSERT INTO file(dir_id,name,file_id,logical,allocated,attrs,born_run,seen_run)
        VALUES(1,'synthetic-link',CAST('0000000000000001' AS BLOB),1,7,0,2,2),
              (1,'synthetic-unknown',NULL,1,NULL,0,2,2);",
    )
    .unwrap();
    let steps = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = steps.clone();
    conn.progress_handler(
        100,
        Some(move || {
            counted.fetch_add(100, Ordering::Relaxed);
            false
        }),
    )
    .unwrap();
    rebuild_multilink(&conn, 1).unwrap();
    conn.progress_handler(0, None::<fn() -> bool>).unwrap();
    assert_eq!(
        conn.query_row("SELECT names,allocated FROM multilink", [], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })
        .unwrap(),
        (2, 7)
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM file WHERE flags & ?1 != 0",
            [HARDLINK_SUSPECTED],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    let steps = steps.load(Ordering::Relaxed);
    assert!(
        steps < 400_000,
        "singleton-heavy finalisation took {steps} SQLite VM steps"
    );
}

#[test]
fn first_scan_batches_staged_payloads_without_charging_unused_reservations() {
    for (mode, prior, run_id, expected) in [
        ("full", false, 2, 1),
        ("refresh", false, 2, 2),
        ("targeted", false, 2, 2),
        ("full", true, 2, 2),
        ("full", false, 999, 2),
    ] {
        let (temp, conn) = first_scan_fixture(mode, prior);
        let reservation = 24 * 1024 * 1024;
        let quota = Arc::new((Mutex::new(3 * reservation), Condvar::new()));
        let (tx, rx) = bounded(4);
        let mut replies = Vec::new();
        for seq in 0..3 {
            let (reply, result) = bounded(1);
            tx.send(Pending {
                command: WriteCommand::StageChunk {
                    run_id,
                    dir_id: 1,
                    seq,
                    files: vec![],
                    dirs: vec![],
                },
                reply,
                _permit: BytePermit {
                    quota: quota.clone(),
                    bytes: reservation,
                },
            })
            .unwrap();
            replies.push(result);
        }
        drop(tx);
        let timings = Arc::new(Mutex::new(WriterTimings::default()));
        let (alive, _) = bounded(0);
        super::hold_batch_window_open();
        work(conn, rx, timings.clone(), alive, None);
        for reply in replies {
            let result = reply.recv().unwrap();
            if run_id == 999 {
                assert!(result.is_err());
            } else {
                result.unwrap();
            }
        }
        assert_eq!(
            timings.lock().unwrap().transactions,
            expected,
            "mode={mode}, prior={prior}"
        );
        assert_eq!(*quota.0.lock().unwrap(), 0);
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM stage_entry", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            if run_id == 999 { 0 } else { 3 }
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(db);
        let reopened = crate::db::open_pair(temp.path(), "synthetic").unwrap();
        assert_eq!(
            reopened
                .query_row("SELECT count(*) FROM stage_entry", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn first_scan_ignores_another_roots_prior_run() {
    let (temp, conn) = first_scan_fixture("full", false);
    conn.execute_batch("INSERT INTO st.root_grant(id,volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES(2,'fixture',zeroblob(16),'Other synthetic root','fixture','fixture','active',0);
        INSERT INTO root(id,grant_id,volume_id,root_file_id,state) VALUES(2,2,1,zeroblob(16),'complete');
        INSERT INTO scan_run(id,root_id,mode,strategy,state,started_at_ns,counters_json,budget_json) VALUES(1,2,'full','fixture','completed',0,'{}','{}');").unwrap();
    let reservation = 24 * 1024 * 1024;
    let quota = Arc::new((Mutex::new(3 * reservation), Condvar::new()));
    let (tx, rx) = bounded(3);
    let mut replies = Vec::new();
    for seq in 0..3 {
        let (reply, result) = bounded(1);
        tx.send(Pending {
            command: WriteCommand::StageChunk {
                run_id: 2,
                dir_id: 1,
                seq,
                files: vec![],
                dirs: vec![],
            },
            reply,
            _permit: BytePermit {
                quota: quota.clone(),
                bytes: reservation,
            },
        })
        .unwrap();
        replies.push(result);
    }
    drop(tx);
    let timings = Arc::new(Mutex::new(WriterTimings::default()));
    let (alive, _) = bounded(0);
    super::hold_batch_window_open();
    work(conn, rx, timings.clone(), alive, None);
    for reply in replies {
        reply.recv().unwrap().unwrap();
    }
    assert_eq!(timings.lock().unwrap().transactions, 1);
    assert_eq!(*quota.0.lock().unwrap(), 0);
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT state FROM scan_run WHERE id=1", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "completed"
    );
}

#[test]
fn first_scan_pipeline_preserves_atomic_listings_other_roots_and_recovery() {
    fn observe(conn: &Connection, committed: bool) {
        let path = conn.path().unwrap();
        let reader = Connection::open(path).unwrap();
        let count: i64 = reader
            .query_row("SELECT count(*) FROM file WHERE dir_id=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, if committed { 3 } else { 0 });
    }
    for (crash, cancel) in [
        (Some(PublicationCrash::BeforeCatalog), false),
        (Some(PublicationCrash::AfterCatalog), false),
        (Some(PublicationCrash::Observe(observe)), false),
        (None, true),
    ] {
        let (temp, conn) = first_scan_fixture("full", false);
        conn.execute_batch("INSERT INTO st.root_grant(id,volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES(2,'fixture',zeroblob(16),'Other synthetic root','fixture','fixture','active',0);
            INSERT INTO root(id,grant_id,volume_id,root_file_id,state) VALUES(2,2,1,zeroblob(16),'complete');
            INSERT INTO dir(id,root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run,own_files,own_logical) VALUES(100,2,NULL,'Other synthetic root','path_observation','none',0,0,'complete',0,0,1,99);
            INSERT INTO file(id,dir_id,name,logical,attrs,born_run,seen_run) VALUES(100,100,'untouched',99,0,0,0);").unwrap();
        let mut commands: Vec<_> = (0..3)
            .map(|seq| WriteCommand::StageChunk {
                run_id: 2,
                dir_id: 1,
                seq,
                files: vec![Observation::file(format!("item-{seq}"), 10, Some(10))],
                dirs: vec![],
            })
            .collect();
        if cancel {
            commands.push(WriteCommand::EndRun {
                run_id: 2,
                state: "cancelled".into(),
                finished_at_ns: 1,
            });
        } else {
            commands.push(WriteCommand::ListingDone {
                run_id: 2,
                dir_id: 1,
                outcome: ListingOutcome::Complete,
                skipped: 0,
                errors: 0,
            });
        }
        let (tx, rx) = bounded(commands.len());
        let quota = Arc::new((
            Mutex::new(commands.len() * 24 * 1024 * 1024),
            Condvar::new(),
        ));
        let mut replies = Vec::new();
        for command in commands {
            let (reply, result) = bounded(1);
            tx.send(Pending {
                command,
                reply,
                _permit: BytePermit {
                    quota: quota.clone(),
                    bytes: 24 * 1024 * 1024,
                },
            })
            .unwrap();
            replies.push(result);
        }
        drop(tx);
        let timings = Arc::new(Mutex::new(WriterTimings::default()));
        let (alive, _) = bounded(0);
        work(conn, rx, timings.clone(), alive, crash);
        let before = matches!(crash, Some(PublicationCrash::BeforeCatalog));
        if !before {
            assert_eq!(
                timings.lock().unwrap().transactions,
                if cancel { 2 } else { 1 }
            );
        }
        for reply in replies {
            if matches!(
                crash,
                Some(PublicationCrash::BeforeCatalog | PublicationCrash::AfterCatalog)
            ) {
                assert!(reply.recv().is_err());
            } else {
                reply.recv().unwrap().unwrap();
            }
        }
        assert_eq!(*quota.0.lock().unwrap(), 0);
        let reopened = crate::db::open_pair(temp.path(), "synthetic").unwrap();
        let count: i64 = reopened
            .query_row("SELECT count(*) FROM file WHERE dir_id=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, if before || cancel { 0 } else { 3 });
        assert_eq!(
            reopened
                .query_row("SELECT logical FROM file WHERE id=100", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            99
        );
        assert_eq!(
            reopened
                .query_row(
                    "SELECT own_files,own_logical FROM dir WHERE id=100",
                    [],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                )
                .unwrap(),
            (1, 99)
        );
        assert_eq!(
            reopened
                .query_row("SELECT count(*) FROM stage_entry", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

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
    super::hold_batch_window_open();
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
fn flattening_ancestor_and_descendant_is_independent_of_listing_order() {
    for reverse in [false, true] {
        let (temp, conn) = depth_fixture();
        let mut dirs = vec![observation("spine-1", 10, 0), observation("spine-2", 12, 0)];
        if reverse {
            dirs.reverse();
        }
        let (results, _) = depth_batch(
            conn,
            vec![depth_listing(600, vec![], dirs), depth_end_run()],
        );
        for result in results {
            result.unwrap();
        }
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        for id in [10, 11] {
            assert_eq!(
                db.query_row("SELECT parent_id FROM dir WHERE id=?1", [id], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                600
            );
        }
        assert_eq!(
            db.query_row("SELECT max(depth) FROM dir", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            511
        );
    }
}

#[test]
fn aborted_ok_batch_reports_the_failure_to_every_caller() {
    for flush in [false, true] {
        let (temp, conn) = first_scan_fixture("full", false);
        if flush {
            conn.execute_batch("CREATE TRIGGER reject_dirty BEFORE UPDATE OF dirty_rev ON dir BEGIN SELECT RAISE(ABORT,'synthetic flush failure'); END;").unwrap();
        } else {
            conn.execute_batch("PRAGMA foreign_keys=ON;
                CREATE TABLE fault_parent(id INTEGER PRIMARY KEY);
                CREATE TABLE fault_child(id INTEGER REFERENCES fault_parent(id) DEFERRABLE INITIALLY DEFERRED);
                CREATE TRIGGER reject_commit AFTER INSERT ON file BEGIN INSERT INTO fault_child VALUES(1); END;").unwrap();
        }
        let (results, _) = depth_batch(
            conn,
            vec![
                depth_listing(1, vec![Observation::file("before", 10, Some(10))], vec![]),
                depth_listing(1, vec![Observation::file("after", 20, Some(20))], vec![]),
            ],
        );
        assert_eq!(results.len(), 2);
        for result in results {
            let error = result.unwrap_err();
            assert!(
                !matches!(error, Error::Closed),
                "batch failure lost: {error:?}"
            );
            assert!(error.to_string().contains(if flush {
                "synthetic flush failure"
            } else {
                "FOREIGN KEY"
            }));
        }
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
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
fn reparent_depth_ignores_hidden_remnants_but_preserves_them() {
    for excluded in [false, true] {
        let (temp, conn) = depth_fixture();
        conn.execute(
            "UPDATE dir SET listing_state='absent_pending' WHERE id=11",
            [],
        )
        .unwrap();
        let mut moved = observation("spine-1", 10, 0);
        if excluded {
            moved.flags = EXCLUDED_FLAGS;
        }
        let (results, _) = depth_batch(
            conn,
            vec![depth_listing(600, vec![], vec![moved]), depth_end_run()],
        );
        for result in results {
            result.expect("hidden remnants must not reject a shallow visible subtree");
        }
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        assert_eq!(
            db.query_row("SELECT parent_id FROM dir WHERE id=10", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            600
        );
        assert_eq!(
            db.query_row("SELECT listing_state FROM dir WHERE id=11", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "absent_pending"
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM dir WHERE id BETWEEN 11 AND 520",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            510
        );
        assert_eq!(
            db.query_row("SELECT depth FROM dir WHERE id=10", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}

#[test]
fn restoring_a_hidden_subtree_rechecks_its_descendant_depth() {
    let (temp, conn) = depth_fixture();
    conn.execute_batch(
        "UPDATE dir SET parent_id=600 WHERE id=10;
        UPDATE dir SET listing_state='absent_pending' WHERE id=11;",
    )
    .unwrap();
    let (mut results, _) = depth_batch(
        conn,
        vec![
            depth_listing(10, vec![], vec![observation("spine-2", 12, 0)]),
            depth_end_run(),
        ],
    );
    assert!(matches!(
        results.remove(0),
        Err(Error::Invalid("tree depth limit"))
    ));
    results
        .remove(0)
        .expect("the rejected restoration must leave rollup usable");
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT listing_state FROM dir WHERE id=11", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "absent_pending"
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
        bulk: HashMap::new(),
        bulk_enabled: Arc::default(),
    };
    let mut proven = HashMap::new();
    assert_eq!(
        ctx.chain_length(&conn, LIVE, 512, &mut proven).unwrap(),
        512
    );
    assert_eq!(proven.len(), 512);
    // No table exists: revisiting this proven seed must need neither a parent walk nor SQL.
    ctx.parents.remove(&512);
    assert_eq!(
        ctx.chain_length(&conn, LIVE, 512, &mut proven).unwrap(),
        512
    );
    assert!(matches!(
        ctx.chain_length(&conn, LIVE, 513, &mut proven),
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
            max_bytes: None,
            modified_after: None,
            modified_before: None,
            kind: "file".into(),
            limit: 10,
            cursor: None,
            work_budget: 2_000_000,
            time_budget: None,
            scope: None,
        })
        .unwrap();
    assert_eq!(page.hits.len(), 4);
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
    // The three listings must share one transaction even on a loaded host (`work` runs here).
    super::hold_batch_window_open();
    for full in [true, false] {
        let (temp, conn, _) = fixture();
        // This fault probe uses direct listings, not the fixture's queued staged payload.
        conn.execute_batch(
            "DELETE FROM st.collection_member; DELETE FROM st.object_ref; DELETE FROM stage_entry;
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
    // All three listings must share one transaction; a loaded host must not close the window early.
    super::hold_batch_window_open();
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
fn publication_temp_tables_roll_back_creation_and_rows_with_the_savepoint() {
    let mut conn = Connection::open_in_memory().unwrap();
    for (name, columns) in [
        ("listing_seen", "kind INTEGER,id INTEGER,PRIMARY KEY(kind,id)"),
        ("listing_names", "name TEXT,raw BLOB,kind INTEGER"),
        ("continuity_entry", "kind INTEGER,name TEXT,raw BLOB,identity BLOB,created_ft INTEGER,matched_id INTEGER"),
        ("old_file", "id INTEGER PRIMARY KEY,name TEXT,raw BLOB,identity BLOB,created_ft INTEGER,used INTEGER"),
    ] {
        let create = format!("CREATE TEMP TABLE {name}({columns})");
        let insert = format!("INSERT INTO {name} DEFAULT VALUES");
        {
            let save = conn.savepoint().unwrap();
            save.execute_batch(&create).unwrap();
            save.execute(&insert, []).unwrap();
        }
        assert!(!conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_temp_master WHERE name=?1)",
            [name],
            |r| r.get::<_, bool>(0),
        ).unwrap(), "{name} creation survived rollback");
        conn.execute_batch(&create).unwrap();
        conn.execute(&insert, []).unwrap();
        {
            let save = conn.savepoint().unwrap();
            save.execute(&format!("DELETE FROM {name}"), []).unwrap();
            for _ in 0..2 {
                save.execute(&insert, []).unwrap();
            }
        }
        assert_eq!(
            conn.query_row(&format!("SELECT count(*) FROM {name}"), [], |r| r.get::<_, i64>(0)).unwrap(),
            1,
            "{name} rows survived rollback"
        );
    }
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
    let before_listing = old_listing(temp.path());
    conn.execute_batch("CREATE TRIGGER st.fail_confirmation BEFORE UPDATE ON object_ref BEGIN SELECT RAISE(ABORT,'test confirmation failure'); END").unwrap();
    let writer = Writer::start(conn).unwrap();
    assert!(matches!(
        writer.call(command),
        Ok(WriteReply::Listing { .. })
    ));
    assert!(matches!(
        writer.send(WriteCommand::Barrier),
        Err(Error::Closed)
    ));
    drop(writer);
    let committed = old_listing(temp.path());
    assert_eq!(committed.0, before_listing.0 + 1);
    assert!(committed.1.iter().any(|(_, name)| name == "link"));
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
    super::hold_batch_window_open();
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
    let prepared = prepare_state(&mut conn, &command, &HashSet::new(), &HashMap::new()).unwrap();
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
        bulk: HashMap::new(),
        bulk_enabled: Arc::default(),
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
    old_files(&conn, LIVE, 1).unwrap();
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

/// #356: `fixture()` without the references on "a" (row 1) and "omitted" (row 3), published on a
/// writer thread that pauses inside the pending-publication window. While it is paused, another
/// connection binds new references to both rows exactly as `collections.update_members` does: one
/// IMMEDIATE transaction over both files. The publication then drops both rows; "a" survives as
/// its hard link "link" (row 2). With `late_commit` that transaction is still open when the window
/// is released, so the writer reaches its fence while the binding holds both write locks.
fn publish_with_window_bind(
    crash: Option<PublicationCrash>,
    late_commit: bool,
) -> (tempfile::TempDir, Result<WriteReply>) {
    let (temp, conn, command) = fixture();
    conn.execute_batch(
        "DELETE FROM st.collection_member WHERE object_ref_id IN (1,2);
        DELETE FROM st.object_ref WHERE id IN (1,2);",
    )
    .unwrap();
    let window = Arc::new(PublicationWindow::default());
    window.arm();
    let paused = window.clone();
    let publication = std::thread::spawn(move || {
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
        serve(
            conn,
            rx,
            Arc::new(Mutex::new(WriterTimings::default())),
            alive,
            Arc::default(),
            crash,
            Some(paused),
        );
        result.recv().unwrap_or(Err(Error::Closed))
    });
    window.wait_paused();
    let mut other = Connection::open(temp.path().join("catalog.db")).unwrap();
    other
        .execute(
            "ATTACH DATABASE ?1 AS st",
            [temp.path().join("state.db").to_string_lossy().as_ref()],
        )
        .unwrap();
    let tx = other
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    tx.execute_batch(
        "INSERT INTO st.object_ref(volume_key,file_id,creation_ft,kind,state,last_resolved_ns,catalog_instance,row_id,born_run)
            SELECT 'fixture',file_id,created_ft,'file','resolved',0,(SELECT value FROM main.meta WHERE key='instance_id'),id,born_run
            FROM main.file WHERE id IN (1,3) ORDER BY id;
        INSERT INTO st.collection_member SELECT 1,id,1 FROM st.object_ref WHERE id>4;",
    )
    .unwrap();
    if late_commit {
        window.release();
        // Ordering only: the assertions hold whether the writer is already waiting or not.
        std::thread::sleep(std::time::Duration::from_millis(200));
        tx.commit().unwrap();
    } else {
        tx.commit().unwrap();
        window.release();
    }
    (temp, publication.join().unwrap())
}

fn pending_references(dir: &std::path::Path) -> Vec<(i64, Option<i64>)> {
    let st = Connection::open(dir.join("state.db")).unwrap();
    let mut s = st
        .prepare("SELECT object_ref_id,row_id FROM pending_reference ORDER BY object_ref_id")
        .unwrap();
    s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

#[test]
fn a_reference_bound_inside_the_publication_window_is_restaged_before_publication() {
    for late_commit in [false, true] {
        let (temp, result) = publish_with_window_bind(None, late_commit);
        result.unwrap();
        assert_eq!(pending_count(temp.path()), 0);
        let after = bindings(temp.path());
        // The window's reference to "a" follows the identity to its surviving hard link, row 2.
        assert_eq!(after[2], (5, "resolved".into(), false, 2, 1));
        // The one to "omitted" is retired with the row, never left resolved to a deleted row id.
        assert_eq!(after[3], (6, "unresolved".into(), true, 3, 1));
        assert_eq!(
            old_listing(temp.path()).1,
            vec![(2, "link".into()), (4, "nested".into())]
        );
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

#[test]
fn crash_after_a_window_restage_recovers_like_any_publication() {
    // Before the catalogue commit: the re-staged intents are durable, the witness is not, and
    // recovery discards them; both window references keep their still-current rows.
    let (temp, result) = publish_with_window_bind(Some(PublicationCrash::BeforeCatalog), false);
    assert!(matches!(result, Err(Error::Closed)));
    assert_eq!(
        pending_references(temp.path()),
        vec![(3, None), (4, None), (5, Some(2)), (6, None)]
    );
    let listing = old_listing(temp.path());
    drop(crate::db::open_pair(temp.path(), "synthetic").unwrap());
    assert_eq!(pending_count(temp.path()), 0);
    assert_eq!(old_listing(temp.path()).1, listing.1);
    let after = bindings(temp.path());
    assert_eq!(after[2], (5, "resolved".into(), false, 1, 1));
    assert_eq!(after[3], (6, "resolved".into(), false, 3, 1));
    assert_recovered_slice(temp.path());
    // After the catalogue commit: the witness names the re-staged token, so recovery confirms it.
    let (temp, result) = publish_with_window_bind(Some(PublicationCrash::AfterCatalog), false);
    assert!(matches!(result, Err(Error::Closed)));
    assert_eq!(pending_count(temp.path()), 4);
    drop(crate::db::open_pair(temp.path(), "synthetic").unwrap());
    assert_eq!(pending_count(temp.path()), 0);
    let after = bindings(temp.path());
    assert_eq!(after[2], (5, "resolved".into(), false, 2, 1));
    assert_eq!(after[3], (6, "unresolved".into(), true, 3, 1));
}

/// #389: a batch led by `StageChunk` stages no references, yet a queued `ListingDone` may join it
/// when no resolved reference exists. A reference bound after that check but before the batch's
/// transaction (here: an IMMEDIATE transaction over both files, still uncommitted when the writer
/// checks and committed while it is paused in the window) must still be retired with the row the
/// publication removes. Before the fix the batch never paused, so `wait_paused` fails the test.
#[test]
fn a_reference_bound_before_a_stage_chunk_led_batch_is_restaged_before_the_joined_listing() {
    let (temp, conn, done) = fixture();
    conn.execute_batch("DELETE FROM st.collection_member; DELETE FROM st.object_ref")
        .unwrap();
    let chunk = WriteCommand::StageChunk {
        run_id: 2,
        dir_id: 1,
        seq: 1,
        files: vec![observation("fresh", 9, 42)],
        dirs: vec![],
    };
    let mut other = Connection::open(temp.path().join("catalog.db")).unwrap();
    other
        .execute(
            "ATTACH DATABASE ?1 AS st",
            [temp.path().join("state.db").to_string_lossy().as_ref()],
        )
        .unwrap();
    let bind = other
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    bind.execute_batch(
        "INSERT INTO st.object_ref(volume_key,file_id,creation_ft,kind,state,last_resolved_ns,catalog_instance,row_id,born_run)
            SELECT 'fixture',file_id,created_ft,'file','resolved',0,(SELECT value FROM main.meta WHERE key='instance_id'),id,born_run
            FROM main.file WHERE id IN (1,3) ORDER BY id;
        INSERT INTO st.collection_member SELECT 1,id,1 FROM st.object_ref;",
    )
    .unwrap();
    let (tx, rx) = bounded(2);
    let mut replies = Vec::new();
    for command in [chunk, done] {
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
    let measured = timings.clone();
    let window = Arc::new(PublicationWindow::default());
    window.arm();
    let paused = window.clone();
    let writer = std::thread::spawn(move || {
        super::hold_batch_window_open();
        let (alive, _) = bounded(0);
        serve(
            conn,
            rx,
            measured,
            alive,
            Arc::default(),
            None,
            Some(paused),
        );
    });
    // The writer has taken its mark and decided batching with the binding still uncommitted.
    window.wait_paused();
    bind.commit().unwrap();
    window.release();
    writer.join().unwrap();
    for reply in replies {
        reply.recv().unwrap().unwrap();
    }
    // Rows 1 ("a") and 3 ("omitted") are gone.
    assert_eq!(
        old_listing(temp.path()).1,
        vec![
            (2, "link".into()),
            (4, "nested".into()),
            (5, "fresh".into())
        ]
    );
    assert_eq!(pending_count(temp.path()), 0);
    let after = bindings(temp.path());
    // "a" follows its identity to the surviving hard link; "omitted" is retired with its row.
    assert_eq!(after[0], (5, "resolved".into(), false, 2, 1));
    assert_eq!(after[1], (6, "unresolved".into(), true, 3, 1));
    // The fence failed, so the re-staged chunk no longer let the listing join it.
    assert_eq!(timings.lock().unwrap().transactions, 2);
}

#[cfg(test)]
mod hardlink_regressions {
    use super::*;

    fn add_survivor_reference(conn: &Connection) {
        conn.execute_batch("INSERT INTO st.object_ref(volume_key,file_id,creation_ft,kind,state,incarnation,catalog_instance,row_id,born_run)
            SELECT volume_key,file_id,creation_ft,kind,state,2,catalog_instance,2,born_run FROM st.object_ref WHERE id=1;
            INSERT INTO st.collection_member VALUES(1,5,1);
            INSERT INTO st.taxonomy VALUES(1,'[]',0);
            INSERT INTO st.human_feedback(event_id,client_event_id,content_digest,object_ref_id,revision,label,retracted,taxonomy_version,features_json,recorded_at_ns)
            SELECT 'event-'||id,'client-'||id,'digest',id,1,'keep',0,1,'{}',0 FROM st.object_ref WHERE id IN (1,5);").unwrap();
    }

    fn assert_preserved(dir: &std::path::Path) {
        let after = bindings(dir);
        assert_eq!(after[0], (1, "unresolved".into(), true, 1, 1));
        assert_eq!(after[4], (5, "resolved".into(), false, 2, 1));
        let st = Connection::open(dir.join("state.db")).unwrap();
        for table in ["collection_member", "human_feedback"] {
            let count: i64 = st
                .query_row(
                    &format!("SELECT count(*) FROM {table} WHERE object_ref_id IN (1,5)"),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 2, "{table} must retain both references");
        }
        assert_eq!(pending_count(dir), 0);
    }

    #[test]
    fn hardlink_collision_preserves_survivor_and_recovers_twice() {
        for crash in [
            None,
            Some(PublicationCrash::BeforeCatalog),
            Some(PublicationCrash::AfterCatalog),
        ] {
            let (temp, conn, command) = fixture();
            add_survivor_reference(&conn);
            let before = bindings(temp.path());
            let result = publish(conn, command, crash);
            if crash.is_none() {
                result.unwrap();
            } else {
                assert!(matches!(result, Err(Error::Closed)));
            }
            if matches!(crash, Some(PublicationCrash::AfterCatalog)) {
                // Readers in the witness/confirmation window see only the survivor's binding.
                let st = Connection::open(temp.path().join("state.db")).unwrap();
                let target: Option<i64> = st
                    .query_row(
                        "SELECT row_id FROM pending_reference WHERE object_ref_id=1",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(target, None);
            }
            for _ in 0..2 {
                drop(crate::db::open_pair(temp.path(), "synthetic").unwrap());
                if matches!(crash, Some(PublicationCrash::BeforeCatalog)) {
                    assert_eq!(bindings(temp.path()), before);
                } else {
                    assert_preserved(temp.path());
                }
            }
        }
    }

    #[test]
    fn legacy_conflicting_pending_intent_recovers() {
        let (temp, conn, command) = fixture();
        add_survivor_reference(&conn);
        publish(conn, command, Some(PublicationCrash::AfterCatalog)).unwrap_err();
        let st = Connection::open(temp.path().join("state.db")).unwrap();
        // The intent written by versions predating #363.
        st.execute(
            "UPDATE pending_reference SET row_id=2,born_run=1 WHERE object_ref_id=1",
            [],
        )
        .unwrap();
        drop(st);
        for _ in 0..2 {
            drop(crate::db::open_pair(temp.path(), "synthetic").unwrap());
            assert_preserved(temp.path());
        }
    }

    #[test]
    fn competing_rebinds_elect_the_lowest_reference_id() {
        let (temp, conn, command) = fixture();
        conn.execute_batch("UPDATE file SET file_id=(SELECT file_id FROM file WHERE id=1) WHERE id=3;
            UPDATE st.object_ref SET file_id=(SELECT file_id FROM st.object_ref WHERE id=1),incarnation=2 WHERE id=2;").unwrap();
        publish(conn, command, None).unwrap();
        for _ in 0..2 {
            drop(crate::db::open_pair(temp.path(), "synthetic").unwrap());
            let after = bindings(temp.path());
            assert_eq!(after[0], (1, "resolved".into(), false, 2, 1));
            assert_eq!(after[1], (2, "unresolved".into(), true, 3, 1));
            assert_eq!(after.len(), 4);
            let st = Connection::open(temp.path().join("state.db")).unwrap();
            let count: i64 = st
                .query_row("SELECT count(*) FROM collection_member", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 4);
            assert_eq!(pending_count(temp.path()), 0);
        }
    }

    #[test]
    fn continuous_reference_churn_fails_unpublished_and_writer_remains_usable() {
        fn churn(conn: &Connection) {
            let count: i64 = conn
                .query_row("SELECT count(*) FROM st.object_ref", [], |r| r.get(0))
                .unwrap();
            assert!(count < 20, "publication retried without a bound");
            conn.execute_batch("INSERT INTO st.object_ref(volume_key,file_id,creation_ft,kind,state,incarnation,row_id,born_run)
                SELECT 'churn',zeroblob(16),0,'file','unresolved',coalesce(max(incarnation),0)+1,0,0 FROM st.object_ref WHERE volume_key='churn'").unwrap();
        }
        let (temp, conn, command) = fixture();
        let before = bindings(temp.path());
        let listing = old_listing(temp.path());
        let (tx, rx) = bounded(2);
        let mut replies = Vec::new();
        for command in [command, WriteCommand::Barrier] {
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
        let (alive, _) = bounded(0);
        work(
            conn,
            rx,
            Arc::new(Mutex::new(WriterTimings::default())),
            alive,
            Some(PublicationCrash::BeforeFence(churn)),
        );
        assert!(matches!(
            replies[0].recv().unwrap(),
            Err(Error::Invalid("reference publication retry limit"))
        ));
        replies[1].recv().unwrap().unwrap();
        // Only the succeeding barrier advances the revision; the rejected publication changes no rows.
        assert_eq!(old_listing(temp.path()), (listing.0 + 1, listing.1));
        assert_eq!(&bindings(temp.path())[..4], &before);
        assert_eq!(bindings(temp.path()).len(), 12, "eight attempts only");
        assert_eq!(pending_count(temp.path()), 0);
        let writer =
            Writer::start(crate::db::open_pair(temp.path(), "synthetic").unwrap()).unwrap();
        writer.call(WriteCommand::Barrier).unwrap();
    }
}

#[test]
fn rollup_postorder_crosses_batches_and_ignores_cached_depths() {
    let (_temp, conn) = first_scan_fixture("full", false);
    conn.execute_batch("UPDATE dir SET own_files=2,own_logical=2,listing_state='complete' WHERE id=1;
        INSERT INTO dir(id,root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run,own_files,own_logical)
        VALUES(5000,1,1,'parent','path_observation','none',99,0,'complete',2,2,3,3),
              (2,1,5000,'child','path_observation','none',0,0,'complete',2,2,7,7);
        WITH RECURSIVE n(i) AS (VALUES(3) UNION ALL SELECT i+1 FROM n WHERE i<2050)
        INSERT INTO dir(id,root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run,own_files,own_logical)
        SELECT i,1,1,'leaf-'||i,'path_observation','none',99,0,'complete',2,2,1,1 FROM n;
        UPDATE dir SET listing_state='excluded' WHERE id=3;
        UPDATE dir SET listing_state='absent_pending' WHERE id=4;").unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let mut callbacks = 0;
    // Bound VM work so a root-wide scan per directory fails deterministically.
    conn.progress_handler(
        1000,
        Some(move || {
            callbacks += 1;
            callbacks > 10_000
        }),
    )
    .unwrap();
    rollup(&tx, 1, true).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT sub_files,sub_dirs,sub_logical,sub_complete FROM dir WHERE id=1",
            [],
            |r| Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, bool>(3)?
            ))
        )
        .unwrap(),
        (2058, 2048, 2058, false)
    );
    assert_eq!(
        conn.query_row("SELECT depth FROM dir WHERE id=2", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row("SELECT sub_files FROM dir WHERE id=3", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT depth FROM dir WHERE id=4", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        99
    );

    conn.execute("UPDATE dir SET own_logical=?1 WHERE id=2", [i64::MAX])
        .unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    assert!(rollup(&tx, 1, true).is_err());
    tx.rollback().unwrap();
    assert_eq!(
        conn.query_row("SELECT sub_logical FROM dir WHERE id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2058
    );
}

#[test]
fn rollup_scratch_disk_full_is_atomic_and_restart_recovers() {
    let (temp, conn) = first_scan_fixture("full", false);
    conn.execute_batch(
        "UPDATE dir SET own_files=7,own_logical=11,listing_state='complete' WHERE id=1;
        PRAGMA temp.max_page_count=2;",
    )
    .unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let Err(Error::Sql(error)) = rollup(&tx, 1, true) else {
        panic!("scratch should fill");
    };
    assert_eq!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DiskFull)
    );
    // SQLITE_FULL may already have rolled the transaction back.
    drop(tx);
    assert_eq!(
        conn.query_row("SELECT sub_files FROM dir WHERE id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(conn);
    let conn = crate::db::open_pair(temp.path(), "synthetic").unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT sub_files,sub_logical,sub_complete FROM dir WHERE id=1",
            [],
            |r| Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, bool>(2)?
            ))
        )
        .unwrap(),
        (7, 11, false)
    );
    assert_eq!(
        conn.query_row("SELECT state FROM scan_run WHERE id=2", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "failed"
    );
}
