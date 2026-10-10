use super::*;

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
