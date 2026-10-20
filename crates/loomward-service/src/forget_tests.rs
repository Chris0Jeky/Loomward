//! `roots.forget` (#288, owner decision q-9): a revoked root leaves Loomward's own state, never the disk.
use crate::tests::*;
use loomward_catalog::{NodeKey, RootObservation, WriteCommand, WriteReply};

const PATH_A: &str = "ForgetServicePathAlpha";
const NAME_A: &str = "ForgetServiceFolderAlpha";
const PATH_B: &str = "KeepServicePathBeta";

/// A fixture root with three listed files and a name of its own.
fn named(svc: &Service, id: u8, name: &str, path: &str) -> (i64, i64) {
    let WriteReply::Root {
        grant_id,
        root_id,
        dir_id,
    } = svc
        .inner
        .catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "fixture".into(),
            display_name: name.into(),
            display_path: path.into(),
            root_file_id: Some(vec![id; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))
        .unwrap()
    else {
        panic!()
    };
    relist(svc, grant_id, dir_id, 3);
    (root_id, grant_id)
}

/// The first file of `root` as a sealed node ID.
fn first_node(svc: &Service, root: i64) -> String {
    let (id, born): (i64, i64) = svc
        .inner
        .db()
        .query_row(
            "SELECT f.id,f.born_run FROM main.file f JOIN main.dir d ON d.id=f.dir_id WHERE d.root_id=?1 ORDER BY f.id LIMIT 1",
            [root],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    svc.inner
        .ids
        .seal(NodeKey::File(id), born, &svc.inner.instance)
}

fn forget(svc: &Service, root: i64, expected: Option<u64>) -> ResponseEnvelope {
    let mut r = request("roots.forget", json!({"root_id": svc.inner.root_id(root)}));
    r.expected_state_rev = expected.map(Generation);
    svc.call(r, &CallContext::http())
}

fn state_rev(svc: &Service) -> u64 {
    svc.inner.revisions().unwrap().1 as u64
}

fn count(svc: &Service, sql: &str) -> i64 {
    svc.inner
        .db()
        .query_row(sql, [], |r| r.get::<_, i64>(0))
        .unwrap()
}

/// Whether the database files or their WALs still hold `needle`.
fn on_disk(dir: &Path, needle: &str) -> bool {
    ["state.db", "state.db-wal", "catalog.db", "catalog.db-wal"]
        .iter()
        .filter_map(|n| std::fs::read(dir.join(n)).ok())
        .any(|bytes| bytes.windows(needle.len()).any(|w| w == needle.as_bytes()))
}

#[test]
fn forgetting_a_revoked_root_purges_its_names_and_keeps_an_anonymised_record() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (a, grant_a) = named(&svc, 1, NAME_A, PATH_A);
    let (b, grant_b) = named(&svc, 2, "KeepServiceFolderBeta", PATH_B);
    svc.inner
        .db()
        .execute_batch("UPDATE main.file SET file_id=cast(printf('%016d',id) AS BLOB),created_ft=1")
        .unwrap();
    let (node_a, node_b) = (first_node(&svc, a), first_node(&svc, b));
    for (n, node) in [&node_a, &node_b].into_iter().enumerate() {
        ok(
            "feedback.record",
            call(
                &svc,
                "feedback.record",
                json!({"node_id": node, "client_event_id": format!("ce_forget_{n}"), "label": "Documents", "retract": false}),
            ),
        );
    }
    let (made, _) = ok(
        "collections.create",
        call(&svc, "collections.create", json!({"name": "Mixed"})),
    );
    let collection = made["collection_id"].as_str().unwrap().to_owned();
    let mut add = request(
        "collections.update_members",
        json!({"collection_id": collection, "add": [node_a, node_b], "remove": []}),
    );
    add.expected_state_rev = Some(Generation(state_rev(&svc)));
    ok(
        "collections.update_members",
        svc.call(add, &CallContext::http()),
    );
    let root_b_before = ok("roots.list", call(&svc, "roots.list", json!({}))).0["roots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["root_id"] == json!(svc.inner.root_id(b)))
        .cloned()
        .unwrap();
    // The names are on disk before the forget, in the file or its WAL.
    assert!(on_disk(dir.path(), PATH_A) && on_disk(dir.path(), NAME_A));

    // An active root is refused, with a reason, and nothing changes.
    let rev = state_rev(&svc);
    let refused = err(forget(&svc, a, None));
    assert_eq!(refused.code, ErrorCode::InvalidRequest);
    assert_eq!(reason(&refused), Some("root_active"));
    assert_eq!(state_rev(&svc), rev);
    assert_eq!(count(&svc, "SELECT count(*) FROM main.root"), 2);

    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": svc.inner.root_id(a), "purge_catalog": false}),
        ),
    );
    let mut events = svc.subscribe(None);

    // A stale precondition writes nothing.
    let rev = state_rev(&svc);
    let stale = err(forget(&svc, a, Some(rev + 7)));
    assert_eq!(stale.code, ErrorCode::StaleGeneration);
    assert_eq!(reason(&stale), Some("state_rev_changed"));
    assert_eq!(state_rev(&svc), rev);
    assert_eq!(count(&svc, "SELECT count(*) FROM main.root"), 2);

    // The same request_id replays the stored answer; a new one finds the root gone.
    let request_once = request("roots.forget", json!({"root_id": svc.inner.root_id(a)}));
    let first = svc.call(request_once.clone(), &CallContext::http());
    let (done, meta) = ok("roots.forget", first.clone());
    assert_eq!(done["root_id"], json!(svc.inner.root_id(a)));
    assert_eq!(svc.call(request_once, &CallContext::http()), first);
    assert_eq!(state_rev(&svc), meta.state_rev.unwrap().get());
    assert_eq!(err(forget(&svc, a, None)).code, ErrorCode::NotFound);

    // roots.changed follows the commit and no longer lists the root.
    let event = loop {
        match events.recv_timeout(Duration::from_secs(2)) {
            RecvOutcome::Event(e)
                if e.event == EventName::RootsChanged
                    && e.data["roots"].as_array().unwrap().len() == 1 =>
            {
                break e
            }
            RecvOutcome::Event(_) => continue,
            other => panic!("{other:?}"),
        }
    };
    event.validate().unwrap();
    assert_eq!(event.state_rev, meta.state_rev);

    // Every row that named the root is gone; the other root is exactly as it was.
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    assert_eq!(roots["roots"].as_array().unwrap(), &vec![root_b_before]);
    assert_eq!(count(&svc, "SELECT count(*) FROM st.object_ref"), 1);
    assert_eq!(count(&svc, "SELECT count(*) FROM st.human_feedback"), 1);
    assert_eq!(count(&svc, "SELECT count(*) FROM st.collection_member"), 1);
    assert_eq!(count(&svc, "SELECT count(*) FROM st.learning_receipt"), 1);
    assert_eq!(count(&svc, "SELECT count(*) FROM st.learning_node"), 1);
    assert_eq!(
        count(&svc, "SELECT count(*) FROM main.file"),
        count(
            &svc,
            &format!("SELECT count(*) FROM main.file f JOIN main.dir d ON d.id=f.dir_id WHERE d.root_id={b}")
        )
    );
    assert_eq!(
        count(
            &svc,
            "SELECT count(*) FROM main.dir WHERE name LIKE 'Forget%'"
        ),
        0
    );
    let (members, _) = ok(
        "collections.members",
        call(
            &svc,
            "collections.members",
            json!({"collection_id": collection, "limit": 10, "cursor": null}),
        ),
    );
    assert_eq!(members["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        ok("learning.status", call(&svc, "learning.status", json!({}))).0["human_events"],
        1
    );
    // The audit record: dates and an ID, no path, name or volume.
    let (grants, _) = ok("grants.list", call(&svc, "grants.list", json!({})));
    let listed = grants["grants"].as_array().unwrap();
    let record = listed
        .iter()
        .find(|g| g["grant_id"] == json!(format!("gr_{grant_a}")))
        .unwrap();
    assert!(record["forgotten_at"].is_string() && record["revoked_at"].is_string());
    assert!(listed
        .iter()
        .any(|g| g["grant_id"] == json!(format!("gr_{grant_b}")) && g["forgotten_at"].is_null()));
    let text = json!(grants).to_string();
    assert!(!text.contains(PATH_A) && !text.contains(NAME_A));
    assert_eq!(
        count(
            &svc,
            &format!("SELECT display_path='forgotten' FROM st.root_grant WHERE id={grant_a}")
        ),
        1
    );
    // Nothing of it is left in the files or their WALs; the other root's name is.
    drop(svc);
    for needle in [PATH_A, NAME_A] {
        assert!(!on_disk(dir.path(), needle), "{needle} survived on disk");
    }
    assert!(on_disk(dir.path(), PATH_B));
    // The rolling start-up backups predate the forget and would still hold it.
    let backups = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("state.start-")
        })
        .count();
    assert_eq!(backups, 0, "start-up backups must be removed");
}

#[test]
fn a_forgotten_root_stays_forgotten_across_a_restart() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (a, grant_a) = named(&svc, 1, NAME_A, PATH_A);
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": svc.inner.root_id(a), "purge_catalog": false}),
        ),
    );
    ok("roots.forget", forget(&svc, a, None));
    drop(svc);
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    assert!(roots["roots"].as_array().unwrap().is_empty());
    let (grants, _) = ok("grants.list", call(&svc, "grants.list", json!({})));
    assert_eq!(
        grants["grants"][0]["grant_id"],
        json!(format!("gr_{grant_a}"))
    );
    assert!(grants["grants"][0]["forgotten_at"].is_string());
    // Revoking the forgotten grant again is the monotone no-op it always was.
    ok(
        "grants.revoke",
        call(
            &svc,
            "grants.revoke",
            json!({"grant_id": format!("gr_{grant_a}")}),
        ),
    );
    // Forgetting never reactivates it: a new grant of the same folder is a new record.
    assert_eq!(
        count(
            &svc,
            "SELECT count(*) FROM st.root_grant WHERE state='active'"
        ),
        0
    );
}

/// Every file in the state directory that holds `needle`, as UTF-8 or UTF-16LE.
fn holders(dir: &Path, needle: &str) -> Vec<String> {
    let utf16: Vec<u8> = needle.encode_utf16().flat_map(u16::to_le_bytes).collect();
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_file())
        .filter(|e| {
            let bytes = std::fs::read(e.path()).unwrap_or_default();
            [needle.as_bytes(), utf16.as_slice()]
                .iter()
                .any(|n| bytes.windows(n.len()).any(|w| w == *n))
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn forgetting_recovers_migration_journals_before_removing_sidecars() {
    const CHILD_COPY: &str = "LOOMWARD_FORGET_JOURNAL_FIXTURE";
    const UNCOMMITTED: &str = "UncommittedMigrationPath";
    if let Some(copy) = std::env::var_os(CHILD_COPY) {
        let db = rusqlite::Connection::open(copy).unwrap();
        db.execute_batch(
            "PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;
             PRAGMA cache_size=1; PRAGMA cache_spill=ON;
             CREATE TABLE rollback_fixture(payload BLOB);
             WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<32)
             INSERT INTO rollback_fixture SELECT zeroblob(4096) FROM n;
             BEGIN IMMEDIATE",
        )
        .unwrap();
        assert_eq!(
            db.execute(
                "UPDATE root_grant SET display_path=?1 WHERE display_path=?2",
                [UNCOMMITTED, PATH_B],
            )
            .unwrap(),
            1
        );
        // Evict the changed grant page to disk, leaving its committed value only in the journal.
        db.execute_batch("UPDATE rollback_fixture SET payload=randomblob(4096)")
            .unwrap();
        // An interrupted process must not run SQLite's rollback-on-drop destructor.
        std::process::exit(17);
    }
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    let (_, kept) = named(&svc, 2, "KeptRoot", PATH_B);
    svc.inner.revoke(grant).unwrap();
    // Orphan sidecars and sidecars beside a real migration copy are both owned state.
    let copy = dir.path().join("state.before-v6.db");
    svc.inner
        .db()
        .execute("VACUUM st INTO ?1", [copy.to_string_lossy().as_ref()])
        .unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::forget_tests::forgetting_recovers_migration_journals_before_removing_sidecars",
            "--nocapture",
        ])
        .env(CHILD_COPY, &copy)
        .status()
        .unwrap();
    assert_eq!(child.code(), Some(17));
    let utf16: Vec<u8> = PATH_A.encode_utf16().flat_map(u16::to_le_bytes).collect();
    for stem in [
        "state.before-v6.db",
        "state.before-v0.db",
        "state.start-123.db",
    ] {
        for suffix in ["-wal", "-shm", "-journal"] {
            if stem == "state.before-v6.db" && suffix == "-journal" {
                continue; // The child left a real hot journal beside this migration copy.
            }
            let mut bytes = PATH_A.as_bytes().to_vec();
            bytes.extend_from_slice(&utf16);
            std::fs::write(dir.path().join(format!("{stem}{suffix}")), bytes).unwrap();
        }
    }
    let journal = std::fs::read(copy.with_extension("db-journal")).unwrap();
    assert_eq!(
        &journal[..8],
        &[0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7],
        "the migration fixture must contain a hot SQLite rollback journal"
    );
    assert!(journal
        .windows(PATH_B.len())
        .any(|w| w == PATH_B.as_bytes()));
    assert!(
        crate::handlers::holds(&copy, UNCOMMITTED),
        "the uncommitted grant page must have reached the database file"
    );
    ok("roots.forget", forget(&svc, root, None));
    assert!(holders(dir.path(), PATH_A).is_empty());
    for stem in [
        "state.before-v6.db",
        "state.before-v0.db",
        "state.start-123.db",
    ] {
        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(!dir.path().join(format!("{stem}{suffix}")).exists());
        }
    }
    assert!(copy.exists(), "keep the scrubbed migration copy");
    let db = rusqlite::Connection::open(&copy).unwrap();
    let paths: (String, String) = db
        .query_row(
            "SELECT (SELECT display_path FROM root_grant WHERE id=?1),
                (SELECT display_path FROM root_grant WHERE id=?2)",
            [grant, kept],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        paths,
        ("forgotten".into(), PATH_B.into()),
        "SQLite must roll back the kept grant before scrubbing and removing sidecars"
    );
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}

/// #434: a rollback journal beside a migration copy that the forget never writes (an unknown v1 shape
/// with nothing to scrub) is swept with the copy's sidecars. SQLite leaves a zero-length journal alone.
#[test]
fn forgetting_removes_a_journal_beside_an_unwritten_migration_copy() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    let (_, _kept) = named(&svc, 2, "KeptRoot", PATH_B);
    svc.inner.revoke(grant).unwrap();
    let copy = dir.path().join("state.before-v1.db");
    rusqlite::Connection::open(&copy)
        .unwrap()
        .execute_batch(
            "PRAGMA user_version=1; CREATE TABLE root_grant(id INTEGER PRIMARY KEY, display_path TEXT NOT NULL);
             INSERT INTO root_grant VALUES(1,'SyntheticUnrelatedPath');",
        )
        .unwrap();
    let journal = dir.path().join("state.before-v1.db-journal");
    std::fs::write(&journal, b"").unwrap();
    ok("roots.forget", forget(&svc, root, None));
    assert!(
        !journal.exists(),
        "a migration copy's journal must not survive the forget"
    );
    assert!(copy.exists(), "keep the unrelated migration copy");
}

#[test]
fn wal_reader_keeps_forget_pending_until_the_copy_can_be_checkpointed() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    svc.inner.revoke(grant).unwrap();
    let copy = dir.path().join("state.before-v6.db");
    svc.inner
        .db()
        .execute("VACUUM st INTO ?1", [copy.to_string_lossy().as_ref()])
        .unwrap();
    let reader = rusqlite::Connection::open(&copy).unwrap();
    reader
        .execute_batch("PRAGMA journal_mode=WAL; BEGIN; SELECT * FROM root_grant")
        .unwrap();
    let error = err(forget(&svc, root, None));
    assert_eq!(reason(&error), Some("forget_backup_scrub_failed"));
    assert!(svc.inner.backup_scrub_pending().unwrap().is_some());
    assert!(!holders(dir.path(), PATH_A).is_empty());
    drop(reader);
    // The committed live forget is not repeated; settle only the outstanding copy obligation.
    let _guard = svc.inner.mutation.lock().unwrap();
    svc.inner.scrub_backups(None).unwrap();
    assert!(holders(dir.path(), PATH_A).is_empty());
    assert!(svc.inner.backup_scrub_pending().unwrap().is_none());
}

#[test]
fn health_reports_live_store_scrub_pending_without_a_path() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    // Hold the mutation lock so the service's backup retry cannot mask the writer marker.
    let _guard = svc.inner.mutation.lock().unwrap();
    svc.inner
        .db()
        .execute("INSERT INTO st.meta VALUES('scrub_pending','1')", [])
        .unwrap();
    let (health, _) = ok("health.get", call(&svc, "health.get", json!({})));
    assert_eq!(health["last_error"]["code"], "internal_error");
    assert_eq!(
        health["last_error"]["detail"]["reason"],
        "forget_scrub_pending"
    );
    assert!(!health["last_error"]
        .to_string()
        .contains(&dir.path().display().to_string()));
    svc.inner
        .db()
        .execute("DELETE FROM st.meta WHERE key='scrub_pending'", [])
        .unwrap();
    let (health, _) = ok("health.get", call(&svc, "health.get", json!({})));
    assert!(health["last_error"].is_null());
}

#[test]
fn forget_deletes_only_the_forgotten_grants_freshness() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    let (_, kept) = named(&svc, 2, "KeepServiceFolderBeta", PATH_B);
    for id in [grant, kept] {
        svc.inner.db().execute("INSERT INTO st.root_freshness(grant_id,catalog_instance,journal_id,next_usn,at_ns) VALUES(?1,?2,zeroblob(8),0,0)", rusqlite::params![id, svc.inner.instance]).unwrap();
    }
    svc.inner.revoke(grant).unwrap();
    ok("roots.forget", forget(&svc, root, None));
    assert_eq!(
        count(
            &svc,
            &format!("SELECT count(*) FROM st.root_freshness WHERE grant_id={grant}")
        ),
        0
    );
    assert_eq!(
        count(
            &svc,
            &format!("SELECT count(*) FROM st.root_freshness WHERE grant_id={kept}")
        ),
        1
    );
}

#[test]
fn forgetting_scrubs_the_migration_copies_so_no_file_keeps_the_name() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (a, _) = named(&svc, 1, NAME_A, PATH_A);
    named(&svc, 2, "KeepServiceFolderBeta", PATH_B);
    svc.inner
        .db()
        .execute_batch("UPDATE main.file SET file_id=cast(printf('%016d',id) AS BLOB),created_ft=1")
        .unwrap();
    let node_a = first_node(&svc, a);
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            json!({"node_id": node_a, "client_event_id": "ce_copy_forget", "label": "Documents", "retract": false}),
        ),
    );
    // The copies a v5 and a v6 install's upgrades leave (older schemas: no root_forgotten, and at v5
    // no backup_evidence either), and a copy in a shape the scrub does not know that names the root.
    let copies = [
        (
            "state.before-v5.db",
            "DROP TABLE root_forgotten; DROP TABLE backup_evidence",
        ),
        ("state.before-v6.db", "DROP TABLE root_forgotten"),
    ];
    for (name, downgrade) in copies {
        let copy = dir.path().join(name);
        svc.inner
            .db()
            .execute("VACUUM st INTO ?1", [copy.to_string_lossy().as_ref()])
            .unwrap();
        rusqlite::Connection::open(&copy)
            .unwrap()
            .execute_batch(downgrade)
            .unwrap();
    }
    let unknown = dir.path().join("state.before-v1.db");
    let legacy = rusqlite::Connection::open(&unknown).unwrap();
    legacy
        .execute_batch("CREATE TABLE legacy_root(path TEXT)")
        .unwrap();
    legacy
        .execute("INSERT INTO legacy_root VALUES(?1)", [PATH_A])
        .unwrap();
    drop(legacy);
    for name in [
        "state.before-v5.db",
        "state.before-v6.db",
        "state.before-v1.db",
    ] {
        assert!(
            holders(dir.path(), PATH_A).contains(&name.to_owned()),
            "{name}"
        );
    }

    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": svc.inner.root_id(a), "purge_catalog": false}),
        ),
    );
    ok("roots.forget", forget(&svc, a, None));
    drop(svc);
    let left = holders(dir.path(), PATH_A);
    assert!(left.is_empty(), "the root's path survived in {left:?}");
    // The known copies are scrubbed, not discarded: the other root and the owner's rows stay in them.
    for (name, _) in copies {
        assert!(
            holders(dir.path(), PATH_B).contains(&name.to_owned()),
            "{name}"
        );
        let copy = rusqlite::Connection::open(dir.path().join(name)).unwrap();
        let kept: (i64, i64) = copy
            .query_row(
                "SELECT (SELECT count(*) FROM root_grant),(SELECT count(*) FROM human_feedback)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kept, (2, 0), "{name}");
    }
    assert!(
        !unknown.exists(),
        "a copy the scrub cannot clear is removed"
    );
}

#[test]
fn failed_migration_scrub_preserves_rollback_copy_and_reports_failure() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    svc.inner.revoke(grant).unwrap();
    let copy = dir.path().join("state.before-v6.db");
    svc.inner
        .db()
        .execute("VACUUM st INTO ?1", [copy.to_string_lossy().as_ref()])
        .unwrap();
    let rollback = rusqlite::Connection::open(&copy).unwrap();
    rollback
        .execute_batch("CREATE TRIGGER refuse_scrub BEFORE UPDATE ON root_grant BEGIN SELECT RAISE(ABORT, 'synthetic scrub failure'); END")
        .unwrap();
    drop(rollback);
    let before = std::fs::read(&copy).unwrap();
    // A start-up copy sorting after the failing one must still be swept.
    let start = dir.path().join("state.start-zz.db");
    svc.inner
        .db()
        .execute("VACUUM st INTO ?1", [start.to_string_lossy().as_ref()])
        .unwrap();

    let response = forget(&svc, root, None);
    assert!(copy.exists(), "a failed scrub deleted the rollback copy");
    assert_eq!(std::fs::read(&copy).unwrap(), before);
    assert!(
        !start.exists(),
        "a failed scrub stopped the sweep of the other copies"
    );
    let error = err(response);
    assert_eq!(error.code, ErrorCode::InternalError);
    assert_eq!(
        error.detail.unwrap()["reason"],
        "forget_backup_scrub_failed"
    );
    // The live forget committed and was announced: the root is gone from roots.list.
    let listed = serde_json::Value::Object(ok("roots.list", call(&svc, "roots.list", json!({}))).0)
        .to_string();
    assert!(
        !listed.contains(NAME_A),
        "the forgotten root is still listed"
    );
}

#[test]
fn busy_migration_scrub_preserves_rollback_copy_and_reports_failure() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    svc.inner.revoke(grant).unwrap();
    let copy = dir.path().join("state.before-v6.db");
    svc.inner
        .db()
        .execute("VACUUM st INTO ?1", [copy.to_string_lossy().as_ref()])
        .unwrap();
    let rollback = rusqlite::Connection::open(&copy).unwrap();
    rollback.execute_batch("BEGIN IMMEDIATE").unwrap();
    let before = std::fs::read(&copy).unwrap();

    let response = forget(&svc, root, None);
    rollback.execute_batch("ROLLBACK").unwrap();
    drop(rollback);
    assert!(copy.exists(), "a busy scrub deleted the rollback copy");
    assert_eq!(std::fs::read(&copy).unwrap(), before);
    assert_eq!(err(response).code, ErrorCode::InternalError);
}

#[test]
fn migration_copy_keeps_regranted_and_nested_paths() {
    for kept in [
        PATH_A.to_owned(),
        format!("{PATH_A}\\Nested"),
        format!("{PATH_A}Suffix"),
    ] {
        let dir = temp_root();
        let svc = open(dir.path(), DatasetClass::Synthetic);
        let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
        svc.inner.revoke(grant).unwrap();
        let (_, keep) = named(&svc, if kept == PATH_A { 1 } else { 2 }, "KeptRoot", &kept);
        let copy = dir.path().join("state.before-v6.db");
        svc.inner
            .db()
            .execute("VACUUM st INTO ?1", [copy.to_string_lossy().as_ref()])
            .unwrap();
        ok("roots.forget", forget(&svc, root, None));
        assert!(
            copy.exists(),
            "the active path {kept:?} caused rollback-copy deletion"
        );
        let db = rusqlite::Connection::open(copy).unwrap();
        let rows: (String, String) = db.query_row("SELECT (SELECT display_path FROM root_grant WHERE id=?1),(SELECT display_path FROM root_grant WHERE id=?2)", [grant, keep], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(rows, ("forgotten".into(), kept));
    }
}

#[test]
fn failed_backup_removal_reports_and_restart_retries_every_owned_copy() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    svc.inner.revoke(grant).unwrap();
    let copy = dir.path().join("state.start-blocked.db");
    std::fs::create_dir(&copy).unwrap();
    let response = forget(&svc, root, None);
    assert_eq!(
        err(response).detail.unwrap()["reason"],
        "forget_backup_scrub_failed"
    );
    std::fs::remove_dir(&copy).unwrap();
    for name in [
        "state.start-interrupted.db",
        "catalog.corrupt-1.db",
        "catalog.corrupt-1.db-wal",
        "catalog.before-v1-2.db",
    ] {
        std::fs::write(dir.path().join(name), PATH_A).unwrap();
    }
    drop(svc);
    let svc = open(dir.path(), DatasetClass::Synthetic);
    assert!(holders(dir.path(), PATH_A).is_empty());
    drop(svc);
}

#[test]
fn preserved_catalogue_copies_are_swept_with_their_sidecars() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    svc.inner.revoke(grant).unwrap();
    for name in [
        "catalog.corrupt-1.db",
        "catalog.corrupt-1.db-wal",
        "catalog.corrupt-1.db-shm",
        "catalog.corrupt-1.db-journal",
        "catalog.before-v1-2.db",
        "catalog.before-v1-2.db-journal",
    ] {
        std::fs::write(dir.path().join(name), PATH_A).unwrap();
    }
    let unrelated = dir.path().join("catalog.corrupt-notes.db");
    std::fs::write(&unrelated, PATH_B).unwrap();
    ok("roots.forget", forget(&svc, root, None));
    assert!(holders(dir.path(), PATH_A).is_empty());
    assert_eq!(std::fs::read_to_string(unrelated).unwrap(), PATH_B);
}

#[test]
fn forget_removes_the_archive_created_by_corrupt_catalogue_recovery() {
    let dir = temp_root();
    drop(loomward_catalog::Catalog::open(dir.path(), "synthetic").unwrap());
    std::fs::write(dir.path().join("catalog.db"), PATH_A).unwrap();
    drop(loomward_catalog::Catalog::open(dir.path(), "synthetic").unwrap());
    let archived = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("catalog.corrupt-")
        })
        .expect("corrupt catalogue was not archived");
    assert!(crate::handlers::holds(&archived, PATH_A));
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    svc.inner.revoke(grant).unwrap();
    ok("roots.forget", forget(&svc, root, None));
    assert!(!archived.exists());
    assert!(holders(dir.path(), PATH_A).is_empty());
}

#[test]
fn backup_directory_open_failure_is_not_an_empty_success() {
    let dir = temp_root();
    let error =
        crate::handlers::forget_backups(&dir.path().join("missing"), Some(PATH_A)).unwrap_err();
    assert_eq!(
        error.detail.as_ref().unwrap()["reason"],
        "forget_backup_scrub_failed"
    );
    assert_eq!(error.detail.as_ref().unwrap()["copy_kind"], "directory");
}

#[test]
fn migration_hard_link_never_changes_the_external_database() {
    let dir = temp_root();
    let external = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    svc.inner.revoke(grant).unwrap();
    let other = external.path().join("external.db");
    svc.inner
        .db()
        .execute("VACUUM st INTO ?1", [other.to_string_lossy().as_ref()])
        .unwrap();
    std::fs::hard_link(&other, dir.path().join("state.before-v6.db")).unwrap();
    let before = std::fs::read(&other).unwrap();
    let _response = forget(&svc, root, None);
    assert!(
        std::fs::read(&other).unwrap() == before,
        "scrub followed a backup alias outside its owned directory"
    );
}

#[test]
fn pending_catalogue_scrub_is_visible_to_the_forget_caller() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    svc.inner.revoke(grant).unwrap();
    let reader = rusqlite::Connection::open(dir.path().join("state.db")).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM root_grant")
        .unwrap();
    let response = forget(&svc, root, None);
    let error = err(response);
    assert_eq!(error.detail.unwrap()["reason"], "forget_scrub_pending");
    assert!(error.message.contains("forgotten"));
    reader.execute_batch("COMMIT").unwrap();
}

#[test]
fn restart_repeats_the_backup_sweep_after_live_forget_committed() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (_root, grant) = named(&svc, 1, NAME_A, PATH_A);
    let copy = dir.path().join("state.before-v6.db");
    svc.inner
        .db()
        .execute("VACUUM st INTO ?1", [copy.to_string_lossy().as_ref()])
        .unwrap();
    svc.inner.revoke(grant).unwrap();
    svc.inner
        .catalog
        .writer()
        .call(WriteCommand::ForgetGrant {
            grant_id: grant,
            forgotten_at_ns: 2,
            expected_state_rev: None,
        })
        .unwrap();
    assert!(holders(dir.path(), PATH_A).contains(&"state.before-v6.db".into()));
    drop(svc);
    let svc = open(dir.path(), DatasetClass::Synthetic);
    assert!(!holders(dir.path(), PATH_A).contains(&"state.before-v6.db".into()));
    assert!(copy.exists());
    drop(svc);
}

#[test]
fn a_copy_that_silently_refuses_the_grant_scrub_is_reported() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    svc.inner.revoke(grant).unwrap();
    let copy = dir.path().join("state.before-v6.db");
    svc.inner
        .db()
        .execute("VACUUM st INTO ?1", [copy.to_string_lossy().as_ref()])
        .unwrap();
    rusqlite::Connection::open(&copy).unwrap().execute_batch("CREATE TRIGGER refuse_scrub BEFORE UPDATE ON root_grant BEGIN SELECT RAISE(IGNORE); END;").unwrap();
    assert_eq!(
        err(forget(&svc, root, None)).detail.unwrap()["reason"],
        "forget_backup_scrub_failed"
    );
    assert!(copy.exists());
}

#[test]
fn v1_migration_forget_preserves_the_other_grant_and_clears_every_copy() {
    let dir = temp_root();
    for (name, schema) in [
        (
            "catalog.db",
            include_str!("../../loomward-catalog/tests/fixtures/catalog-v1.sql"),
        ),
        (
            "state.db",
            include_str!("../../loomward-catalog/tests/fixtures/state-v1.sql"),
        ),
    ] {
        let db = rusqlite::Connection::open(dir.path().join(name)).unwrap();
        db.execute_batch(schema).unwrap();
        db.pragma_update(None, "application_id", loomward_catalog::APPLICATION_ID)
            .unwrap();
        db.pragma_update(None, "user_version", 1).unwrap();
        db.execute("INSERT INTO meta VALUES('dataset_class','synthetic')", [])
            .unwrap();
    }
    let db = rusqlite::Connection::open(dir.path().join("catalog.db")).unwrap();
    db.execute_batch("INSERT INTO volume VALUES(1,'fixture','Fixture','{}','{}','{}',1,0)")
        .unwrap();
    for (id, path) in [(41, PATH_A), (7, PATH_B)] {
        db.execute(
            "INSERT INTO root_grant VALUES(?1,1,?2,?3,'fixture','fixture','active',0,NULL)",
            rusqlite::params![id, vec![id as u8; 16], path],
        )
        .unwrap();
    }
    db.execute_batch("CREATE TABLE fixture_utf16(bytes BLOB)")
        .unwrap();
    let utf16: Vec<u8> = PATH_A.encode_utf16().flat_map(u16::to_le_bytes).collect();
    db.execute("INSERT INTO fixture_utf16 VALUES(?1)", [utf16])
        .unwrap();
    drop(db);
    let svc = open(dir.path(), DatasetClass::Synthetic);
    assert!(dir.path().join("state.before-v1.db").exists());
    assert_eq!(
        count(
            &svc,
            "SELECT id FROM st.root_grant WHERE display_path='ForgetServicePathAlpha'"
        ),
        41
    );
    // A legacy grant stays active until its first matching native admission adopts a GUID.
    let kept = "SELECT count(*) FROM st.root_grant WHERE id=7 AND display_path='KeepServicePathBeta' AND state='active' AND volume_guid IS NULL";
    assert_eq!(count(&svc, kept), 1);
    let root = count(&svc, "SELECT id FROM main.root WHERE grant_id=41");
    svc.inner.revoke(41).unwrap();
    ok("roots.forget", forget(&svc, root, None));
    assert_eq!(count(&svc, kept), 1);
    drop(svc);
    assert!(holders(dir.path(), PATH_A).is_empty());
}

#[test]
fn an_unrecognised_grant_copy_is_byte_checked_before_any_id_scrub() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    let (_, kept) = named(&svc, 2, "KeptRoot", PATH_B);
    svc.inner.revoke(grant).unwrap();
    let copy = dir.path().join("state.before-v1.db");
    svc.inner
        .db()
        .execute("VACUUM st INTO ?1", [copy.to_string_lossy().as_ref()])
        .unwrap();
    let db = rusqlite::Connection::open(&copy).unwrap();
    // A legacy-shaped grant table is not a promise that its IDs share the live namespace.
    db.pragma_update(None, "user_version", 1).unwrap();
    db.execute("UPDATE root_grant SET id=id+100", []).unwrap();
    db.execute(
        "UPDATE root_grant SET id=?1 WHERE display_path=?2",
        rusqlite::params![grant, PATH_B],
    )
    .unwrap();
    drop(db);
    ok("roots.forget", forget(&svc, root, None));
    assert!(
        !copy.exists(),
        "an unrecognised copy kept the forgotten path"
    );
    assert_eq!(
        count(
            &svc,
            &format!(
                "SELECT display_path='KeepServicePathBeta' FROM st.root_grant WHERE id={kept}"
            )
        ),
        1
    );
}

#[test]
fn an_unknown_v2_copy_never_scrubs_a_grant_in_an_unproven_id_namespace() {
    for path in [PATH_B, PATH_A] {
        let dir = temp_root();
        let svc = open(dir.path(), DatasetClass::Synthetic);
        let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
        svc.inner.revoke(grant).unwrap();
        let copy = dir.path().join("state.before-v2.db");
        let db = rusqlite::Connection::open(&copy).unwrap();
        // No object_ref table: v2 alone cannot establish that the grant IDs match live state.
        db.execute_batch("PRAGMA user_version=2; CREATE TABLE root_grant(id INTEGER PRIMARY KEY,display_path TEXT,volume_key TEXT,root_file_id BLOB)").unwrap();
        db.execute(
            "INSERT INTO root_grant VALUES(?1,?2,'copy-volume',X'0102')",
            rusqlite::params![grant, path],
        )
        .unwrap();
        drop(db);
        ok("roots.forget", forget(&svc, root, None));
        if path == PATH_A {
            assert!(
                !copy.exists(),
                "an unknown copy with forgotten bytes was retained"
            );
        } else {
            assert!(copy.exists(), "an unrelated unknown copy was removed");
            let db = rusqlite::Connection::open(&copy).unwrap();
            let unchanged: bool = db.query_row("SELECT display_path=?1 AND volume_key='copy-volume' AND root_file_id=X'0102' FROM root_grant", [PATH_B], |r| r.get(0)).unwrap();
            assert!(
                unchanged,
                "the unknown copy's grant ID was treated as live authority"
            );
        }
    }
}

#[test]
fn startup_backup_failures_are_visible_and_retried_without_a_restart() {
    for hard_link in [false, true] {
        let dir = temp_root();
        let external = temp_root();
        let svc = open(dir.path(), DatasetClass::Synthetic);
        let (_, grant) = named(&svc, 1, NAME_A, PATH_A);
        let copy = dir.path().join("state.before-v6.db");
        svc.inner
            .db()
            .execute("VACUUM st INTO ?1", [copy.to_string_lossy().as_ref()])
            .unwrap();
        svc.inner.revoke(grant).unwrap();
        svc.inner
            .catalog
            .writer()
            .call(WriteCommand::ForgetGrant {
                grant_id: grant,
                forgotten_at_ns: 2,
                expected_state_rev: None,
            })
            .unwrap();
        drop(svc);
        let alias = external.path().join("external.db");
        let lock = if hard_link {
            std::fs::hard_link(&copy, &alias).unwrap();
            None
        } else {
            let lock = rusqlite::Connection::open(&copy).unwrap();
            lock.execute_batch("BEGIN IMMEDIATE").unwrap();
            Some(lock)
        };
        let before = std::fs::read(&copy).unwrap();
        let svc = open(dir.path(), DatasetClass::Synthetic);
        let health = ok("health.get", call(&svc, "health.get", json!({}))).0;
        assert_eq!(
            health["last_error"]["detail"]["reason"],
            "forget_backup_scrub_failed"
        );
        assert_eq!(health["last_error"]["detail"]["copy_kind"], "migration");
        assert_eq!(
            count(
                &svc,
                "SELECT count(*) FROM st.meta WHERE key='forget_backup_scrub_pending'"
            ),
            1
        );
        assert_eq!(std::fs::read(&copy).unwrap(), before);
        if let Some(lock) = lock {
            lock.execute_batch("ROLLBACK").unwrap();
        } else {
            std::fs::remove_file(alias).unwrap();
        }
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            let health = ok("health.get", call(&svc, "health.get", json!({}))).0;
            if health["last_error"].is_null() {
                break;
            }
            assert!(
                Instant::now() < until,
                "backup cleanup did not recover: {health:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            count(
                &svc,
                "SELECT count(*) FROM st.meta WHERE key='forget_backup_scrub_pending'"
            ),
            0
        );
        assert!(!holders(dir.path(), PATH_A).contains(&"state.before-v6.db".into()));
    }
}

#[test]
fn another_forget_cannot_clear_an_earlier_unknown_copy_obligation() {
    let dir = temp_root();
    let external = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (a, grant_a) = named(&svc, 1, NAME_A, PATH_A);
    svc.inner.revoke(grant_a).unwrap();
    let copy = dir.path().join("state.before-v1.db");
    svc.inner
        .db()
        .execute("VACUUM st INTO ?1", [copy.to_string_lossy().as_ref()])
        .unwrap();
    rusqlite::Connection::open(&copy)
        .unwrap()
        .pragma_update(None, "user_version", 1)
        .unwrap();
    let (_, grant_b) = named(&svc, 2, "KeepServiceFolderBeta", PATH_B);
    svc.inner.revoke(grant_b).unwrap();
    let alias = external.path().join("external.db");
    std::fs::hard_link(&copy, &alias).unwrap();
    assert_eq!(
        err(forget(&svc, a, None)).detail.unwrap()["reason"],
        "forget_backup_scrub_failed"
    );
    // Hold the retry worker off so the second forget must finish the first one's cleanup itself.
    let _mutation = svc.inner.mutation.lock().unwrap();
    std::fs::remove_file(alias).unwrap();
    svc.inner
        .catalog
        .writer()
        .call(WriteCommand::ForgetGrant {
            grant_id: grant_b,
            forgotten_at_ns: 3,
            expected_state_rev: None,
        })
        .unwrap();
    svc.inner.scrub_backups(Some(PATH_B)).unwrap();
    assert!(
        !copy.exists(),
        "second forget cleared pending work for the first path"
    );
    assert!(svc.inner.backup_scrub_pending().unwrap().is_none());
    assert!(!holders(dir.path(), PATH_A).contains(&"state.before-v1.db".into()));
}

/// #391: the `roots.changed` the revoke published still names the root in the event replay
/// buffer. After the forget, neither a reader that had not drained it yet nor a client resuming
/// from the start of the epoch may receive that name.
#[test]
fn forget_redacts_the_root_from_retained_roots_events() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    named(&svc, 2, "KeptRoot", PATH_B);
    let mut slow = svc.subscribe(None);
    let RecvOutcome::Event(hello) = slow.recv_timeout(Duration::from_secs(5)) else {
        panic!("expected hello")
    };
    let epoch = hello.data["epoch"].as_str().unwrap().to_string();
    svc.inner.revoke(grant).unwrap();
    ok("roots.forget", forget(&svc, root, None));
    for mut stream in [slow, svc.subscribe(Some((epoch, 0)))] {
        let mut roots_events = 0;
        while let RecvOutcome::Event(e) = stream.recv_timeout(Duration::from_millis(300)) {
            let text = serde_json::to_string(&e).unwrap();
            assert!(
                !text.contains(PATH_A),
                "a retained event names the forgotten root"
            );
            if e.event == EventName::RootsChanged {
                e.validate().unwrap();
                assert!(text.contains(PATH_B), "the kept root stays in the event");
                roots_events += 1;
            }
        }
        assert!(
            roots_events >= 2,
            "both the revoke and the forget events replay"
        );
    }
}

/// The state half of ForgetGrant commits before the catalogue half. Its error receipt must not
/// leave a forgotten name readable from the retained revoke event.
#[test]
fn a_post_commit_catalogue_purge_error_still_redacts_the_retained_root_events() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    let (kept, _) = named(&svc, 2, "KeptRoot", PATH_B);
    let mut slow = svc.subscribe(None);
    let RecvOutcome::Event(hello) = slow.recv_timeout(Duration::from_secs(5)) else {
        panic!("expected hello");
    };
    svc.inner.revoke(grant).unwrap();
    svc.inner
        .db()
        .execute_batch(&format!(
            "CREATE TRIGGER main.refuse_forget_purge BEFORE DELETE ON root WHEN OLD.id={root} BEGIN SELECT RAISE(ABORT, 'synthetic post-commit purge failure'); END"
        ))
        .unwrap();
    let failed = err(forget(&svc, root, None));
    assert_eq!(failed.code, ErrorCode::InternalError);
    assert_eq!(
        count(
            &svc,
            &format!("SELECT count(*) FROM st.root_forgotten WHERE grant_id={grant}")
        ),
        1,
        "the state half really committed before the error"
    );
    assert_eq!(
        count(
            &svc,
            &format!("SELECT count(*) FROM main.root WHERE id={root}")
        ),
        1,
        "the catalogue half really rolled back"
    );
    for mut stream in [
        slow,
        svc.subscribe(Some((hello.epoch.to_string(), hello.seq.get()))),
    ] {
        let retained = loop {
            let RecvOutcome::Event(event) = stream.recv_timeout(Duration::from_secs(5)) else {
                panic!("expected retained roots.changed");
            };
            if event.event == EventName::RootsChanged {
                break event;
            }
        };
        retained.validate().unwrap();
        let text = serde_json::to_string(&retained).unwrap();
        assert!(
            !text.contains(PATH_A),
            "the durable forget survived an error, but its name replayed"
        );
        assert!(text.contains(PATH_B), "the other root remains in replay");
    }
    // A later grant response uses the live root-list source after the delivery suspension ended.
    // The durable marker must exclude the old root even though its derived rows still exist.
    let mut live = svc.subscribe(None);
    assert!(matches!(
        live.recv_timeout(Duration::from_secs(5)),
        RecvOutcome::Event(_)
    ));
    let admission = live.delivery_policy().stamp();
    svc.inner.granted_root_reply(kept).unwrap();
    let RecvOutcome::Event(future) = live.recv_timeout(Duration::from_secs(5)) else {
        panic!("fresh root-list event after failed purge");
    };
    assert!(
        admission.admit(),
        "delivery resumed after the failed RPC settled"
    );
    assert_eq!(future.event, EventName::RootsChanged);
    future.validate().unwrap();
    let text = serde_json::to_string(&future).unwrap();
    assert!(
        !text.contains(PATH_A),
        "a fresh snapshot reintroduced the durably forgotten root"
    );
    assert!(text.contains(PATH_B));
}

/// The picker bypasses the mutation FIFO while waiting on the owner. Drive its real post-picker
/// publication helper on a synthetic catalogue so Linux can prove the same ordering as Windows.
#[test]
fn a_grant_snapshot_cannot_republish_a_root_after_a_concurrent_forget() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (forgotten, grant) = named(&svc, 1, NAME_A, PATH_A);
    let (kept, _) = named(&svc, 2, "KeptRoot", PATH_B);
    svc.inner.revoke(grant).unwrap();
    const GATE: (Command, &str) = (Command::RootsRequestGrant, "grant_snapshot");
    svc.inner.hooks.closed.lock().unwrap().push(GATE);
    struct Release(Arc<crate::tests::Hooks>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.open(GATE.0, GATE.1);
        }
    }
    let (held, early) = std::thread::scope(|scope| {
        let publishing = scope.spawn(|| svc.inner.granted_root_reply(kept));
        let release = Release(svc.inner.hooks.clone());
        svc.inner
            .hooks
            .until("the grant owns its root-list snapshot", |h| {
                h.arrived.lock().unwrap().contains(&GATE)
            });
        // This deterministic observation complements the concurrent operation below: it cannot
        // pass merely because a slow scheduler delayed the forget beyond the bounded wait.
        let held = svc.inner.mutation.try_lock().is_err();
        let (sent, received) = std::sync::mpsc::channel();
        let forgetting_service = &svc;
        let forgetting = scope.spawn(move || {
            let response = forget(forgetting_service, forgotten, None);
            sent.send(response).unwrap();
        });
        svc.inner
            .hooks
            .until("the forget entered its production handler", |h| {
                h.runs.lock().unwrap().contains_key(&Command::RootsForget)
            });
        let early = received.recv_timeout(Duration::from_millis(50)).ok();
        drop(release);
        publishing.join().unwrap().unwrap();
        forgetting.join().unwrap();
        let was_early = early.is_some();
        ok(
            "roots.forget",
            early.unwrap_or_else(|| received.recv_timeout(Duration::from_secs(5)).unwrap()),
        );
        (held, was_early)
    });
    let mut replay = svc.subscribe(None);
    let RecvOutcome::Event(hello) = replay.recv_timeout(Duration::from_secs(5)) else {
        panic!("hello");
    };
    let mut replay = svc.subscribe(Some((hello.epoch.to_string(), 0)));
    while let RecvOutcome::Event(event) = replay.recv_timeout(Duration::from_millis(100)) {
        let text = serde_json::to_string(&event).unwrap();
        assert!(
            !text.contains(PATH_A),
            "the grant republished its pre-forget snapshot after redaction"
        );
    }
    assert!(
        held,
        "root-list snapshot and publication must hold the mutation turn"
    );
    assert!(
        !early,
        "forget may not commit between the grant snapshot and its event"
    );
}

// The volume-label and session-receipt surfaces deliberately retained by #769/#801.
// These use synthetic catalogue rows; the native folder picker is never invoked.
fn retained_privacy_volume(svc: &Service) -> String {
    ok("volumes.list", call(svc, "volumes.list", json!({}))).0["volumes"][0]["volume_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn retained_privacy_tier_request(svc: &Service) -> RequestEnvelope {
    request(
        "volumes.declare_tier",
        json!({"volume_id": retained_privacy_volume(svc), "tier": 2}),
    )
}

fn retained_privacy_has_name(response: &impl serde::Serialize, needle: &str) -> bool {
    serde_json::to_string(response).unwrap().contains(needle)
}

/// Seed only the completed native-picker boundary. Snapshot construction, validation, metadata,
/// Settle::drop, cache lookup and the retry's service ingress are the production paths.
fn retained_privacy_grant_completion(svc: &Service, root: i64) -> (RequestEnvelope, crate::Settle) {
    let req = request("roots.request_grant", json!({"purpose": "metadata_scan"}));
    let fingerprint = json!({
        "command": req.command,
        "payload": req.payload,
        "expected_state_rev": req.expected_state_rev,
        "expected_generation": req.expected_generation,
    });
    assert_eq!(
        svc.inner.idem.claim(&req.request_id, &fingerprint),
        crate::idem::Claim::Run
    );
    let out = svc.inner.granted_root_reply(root).unwrap();
    let map = out.result.as_object().unwrap().clone();
    Command::RootsRequestGrant.validate_result(&map).unwrap();
    let meta = svc
        .inner
        .meta(Instant::now(), &map, out.catalog_rev, out.state_rev);
    let outcome = ResponseEnvelope::ok_object(req.request_id.clone(), map, meta);
    assert!(retained_privacy_has_name(&outcome, PATH_A));
    let settle = crate::Settle {
        inner: svc.inner.clone(),
        key: req.request_id.to_string(),
        ticket: None,
        outcome: Some(outcome),
    };
    (req, settle)
}

fn retained_privacy_tauri() -> CallContext {
    CallContext {
        adapter: Adapter::Tauri,
        dialogs: None,
        principal: Principal::Owner,
    }
}

#[test]
fn retained_privacy_volume_events_drop_a_forgotten_root_derived_name() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    named(&svc, 2, "KeptVolumeRoot", PATH_B);
    svc.inner
        .catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "fixture-unrelated-volume".into(),
            display_name: "UnrelatedVolumeName".into(),
            display_path: "SyntheticUnrelatedVolumeRoot".into(),
            root_file_id: Some(vec![3; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))
        .unwrap();
    let mut before = svc.subscribe(None);
    let RecvOutcome::Event(hello) = before.recv_timeout(Duration::from_secs(5)) else {
        panic!("expected hello");
    };
    let tier = svc.call(retained_privacy_tier_request(&svc), &CallContext::http());
    ok("volumes.declare_tier", tier.clone());
    assert!(retained_privacy_has_name(&tier, NAME_A));
    let mut expected = loop {
        let RecvOutcome::Event(event) = before.recv_timeout(Duration::from_secs(5)) else {
            panic!("expected the declaration's original volume event");
        };
        if event.event == EventName::VolumesChanged {
            break event;
        }
    };
    assert_eq!(expected.data["volumes"].as_array().unwrap().len(), 2);
    expected.data["volumes"][0]["display_name"] = json!("Volume");
    svc.inner.revoke(grant).unwrap();
    ok("roots.forget", forget(&svc, root, None));
    let fresh = call(&svc, "volumes.list", json!({}));
    ok("volumes.list", fresh.clone());
    assert!(!retained_privacy_has_name(&fresh, NAME_A));
    let mut replay = svc.subscribe(Some((hello.epoch.to_string(), hello.seq.get())));
    for _ in 0..10 {
        let stamp = replay.delivery_policy().stamp();
        let RecvOutcome::Event(event) = replay.recv_timeout(Duration::from_secs(5)) else {
            panic!("expected retained volume event");
        };
        if event.event == EventName::VolumesChanged {
            assert!(stamp.admit(), "a fresh replay is admissible");
            event.validate().unwrap();
            assert!(
                !retained_privacy_has_name(&event, NAME_A),
                "retained volumes.changed discloses the forgotten root-derived name"
            );
            assert_eq!(event, expected, "only the affected display name changes; sequence, revisions, physics, tiers and the other volume remain exact");
            return;
        }
    }
    panic!("the declaration's volume event was not replayed");
}

#[test]
fn retained_privacy_cached_tier_reply_drops_the_forgotten_name_without_reexecution() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    named(&svc, 2, "KeptVolumeRoot", PATH_B);
    let req = retained_privacy_tier_request(&svc);
    let first = svc.call(req.clone(), &CallContext::http());
    ok("volumes.declare_tier", first.clone());
    assert!(retained_privacy_has_name(&first, NAME_A));
    svc.inner.revoke(grant).unwrap();
    ok("roots.forget", forget(&svc, root, None));
    let replay = svc.call(req.clone(), &CallContext::http());
    assert_eq!(count(&svc, "SELECT count(*) FROM st.tier_declaration"), 1);
    assert_eq!(
        svc.inner.hooks.runs.lock().unwrap()[&Command::VolumesDeclareTier],
        1
    );
    let mut changed = req;
    changed.payload.insert("tier".into(), json!(5));
    assert_eq!(
        reason(&err(svc.call(changed, &CallContext::http()))),
        Some("idempotency_conflict")
    );
    assert!(
        !retained_privacy_has_name(&replay, NAME_A),
        "a retained tier receipt discloses the forgotten root-derived name"
    );
}

#[test]
fn retained_privacy_fresh_volumes_are_safe_after_a_durable_forget_with_purge_error() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    named(&svc, 2, "KeptVolumeRoot", PATH_B);
    svc.inner.revoke(grant).unwrap();
    svc.inner.db().execute_batch(&format!(
        "CREATE TRIGGER main.retained_privacy_refuse_purge BEFORE DELETE ON root WHEN OLD.id={root} BEGIN SELECT RAISE(ABORT, 'synthetic post-commit purge failure'); END"
    )).unwrap();
    let failed = err(forget(&svc, root, None));
    assert_eq!(failed.code, ErrorCode::InternalError);
    assert_eq!(
        count(
            &svc,
            &format!("SELECT count(*) FROM st.root_forgotten WHERE grant_id={grant}")
        ),
        1
    );
    assert_eq!(
        count(
            &svc,
            &format!("SELECT count(*) FROM main.root WHERE id={root}")
        ),
        1
    );
    let fresh = call(&svc, "volumes.list", json!({}));
    let (view, _) = ok("volumes.list", fresh.clone());
    assert_eq!(
        view["volumes"].as_array().unwrap().len(),
        1,
        "the other root's volume remains visible"
    );
    assert!(
        !retained_privacy_has_name(&fresh, NAME_A),
        "fresh volumes.list discloses a root whose durable forget already committed"
    );
    // A later live publication also reads through the durable-marker projection while the
    // catalogue row still exists. It must not restore the label that replay just removed.
    let mut live = svc.subscribe(None);
    live.recv_timeout(Duration::from_secs(5));
    let declaration = svc.call(retained_privacy_tier_request(&svc), &CallContext::http());
    ok("volumes.declare_tier", declaration.clone());
    assert!(!retained_privacy_has_name(&declaration, NAME_A));
    let RecvOutcome::Event(event) = live.recv_timeout(Duration::from_secs(5)) else {
        panic!("expected the new volume event")
    };
    assert_eq!(event.event, EventName::VolumesChanged);
    event.validate().unwrap();
    assert!(
        !retained_privacy_has_name(&event, NAME_A),
        "a later mutation republished forgotten volume metadata"
    );
}

#[test]
fn retained_privacy_cached_grant_reply_drops_forgotten_root_json() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    let (req, settle) = retained_privacy_grant_completion(&svc, root);
    drop(settle);
    let before = svc.call(req.clone(), &retained_privacy_tauri());
    ok("roots.request_grant", before.clone());
    assert!(retained_privacy_has_name(&before, PATH_A));
    svc.inner.revoke(grant).unwrap();
    ok("roots.forget", forget(&svc, root, None));
    let replay = svc.call(req, &retained_privacy_tauri());
    assert_eq!(
        count(
            &svc,
            "SELECT count(*) FROM st.root_grant WHERE state='active'"
        ),
        0
    );
    assert_eq!(count(&svc, "SELECT count(*) FROM st.root_forgotten"), 1);
    assert!(!svc
        .inner
        .hooks
        .runs
        .lock()
        .unwrap()
        .contains_key(&Command::RootsRequestGrant));
    assert!(
        !retained_privacy_has_name(&replay, PATH_A),
        "a cached roots.request_grant reply discloses the forgotten root JSON"
    );
}

#[test]
fn retained_privacy_a_late_grant_completion_cannot_restore_forgotten_root_json() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    let (req, settle) = retained_privacy_grant_completion(&svc, root);
    svc.inner.revoke(grant).unwrap();
    ok("roots.forget", forget(&svc, root, None));
    // The response snapshot existed before forget; the actual production completion now runs
    // after its cache scrub. No timing sleeps or fabricated response payloads are involved.
    drop(settle);
    let replay = svc.call(req, &retained_privacy_tauri());
    assert_eq!(
        count(
            &svc,
            "SELECT count(*) FROM st.root_grant WHERE state='active'"
        ),
        0
    );
    assert_eq!(count(&svc, "SELECT count(*) FROM st.root_forgotten"), 1);
    assert!(!svc
        .inner
        .hooks
        .runs
        .lock()
        .unwrap()
        .contains_key(&Command::RootsRequestGrant));
    assert!(
        !retained_privacy_has_name(&replay, PATH_A),
        "Settle::drop restored forgotten root JSON after the privacy boundary"
    );
}

#[test]
fn retained_privacy_forget_also_removes_a_volume_label_from_before_a_root_rename() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    named(&svc, 2, "KeptVolumeRoot", PATH_B);
    // A top directory can be renamed after its name initialized the volume row. There is no
    // persistent label-origin ID; matching the current root name does not prove this label safe.
    svc.inner.db().execute(
        "UPDATE main.dir SET name='SyntheticRenamedRoot' WHERE root_id=?1 AND parent_id IS NULL",
        [root],
    ).unwrap();
    assert!(retained_privacy_has_name(
        &call(&svc, "volumes.list", json!({})),
        NAME_A
    ));
    svc.inner.revoke(grant).unwrap();
    ok("roots.forget", forget(&svc, root, None));
    let fresh = call(&svc, "volumes.list", json!({}));
    ok("volumes.list", fresh.clone());
    assert!(
        !retained_privacy_has_name(&fresh, NAME_A),
        "normal purge retained the root's older name as the surviving volume label"
    );
}

fn retained_privacy_alert_settings(svc: &Service, low_percent: u64) -> Value {
    let mut settings = ok("alerts.list", call(svc, "alerts.list", json!({}))).0["settings"].clone();
    settings["low_space_free_percent"] = json!(low_percent);
    settings
}

#[test]
fn retained_privacy_alerts_cached_configuration_never_replays_forgotten_labels_or_old_settings() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    named(&svc, 2, "KeptVolumeRoot", PATH_B);
    let req = request("alerts.configure", retained_privacy_alert_settings(&svc, 7));
    let first = svc.call(req.clone(), &CallContext::http());
    ok("alerts.configure", first.clone());
    assert!(retained_privacy_has_name(&first, NAME_A));
    assert!(retained_privacy_has_name(&first, PATH_A));
    // A newer owner choice makes replaying the old mutation observable even if its old reply
    // were accidentally hidden. Forget must preserve both choices' keys and the latest setting.
    ok(
        "alerts.configure",
        call(
            &svc,
            "alerts.configure",
            retained_privacy_alert_settings(&svc, 11),
        ),
    );
    svc.inner.revoke(grant).unwrap();
    ok("roots.forget", forget(&svc, root, None));
    let replay = svc.call(req.clone(), &CallContext::http());
    assert_eq!(
        svc.inner.hooks.runs.lock().unwrap()[&Command::AlertsConfigure],
        2
    );
    let fresh = ok("alerts.list", call(&svc, "alerts.list", json!({}))).0;
    assert_eq!(fresh["settings"]["low_space_free_percent"], 11);
    assert!(!retained_privacy_has_name(&fresh, NAME_A));
    assert!(!retained_privacy_has_name(&fresh, PATH_A));
    let mut changed = req;
    changed
        .payload
        .insert("low_space_free_percent".into(), json!(13));
    assert_eq!(
        reason(&err(svc.call(changed, &CallContext::http()))),
        Some("idempotency_conflict")
    );
    assert!(
        !retained_privacy_has_name(&replay, NAME_A) && !retained_privacy_has_name(&replay, PATH_A),
        "a cached alerts.configure report disclosed forgotten root or volume labels"
    );
    let error = err(replay);
    assert_eq!(error.code, ErrorCode::PermissionDenied);
    assert_eq!(reason(&error), Some("forgotten_result"));
    assert!(!error.retryable);
}

#[test]
fn retained_privacy_alerts_late_configuration_settlement_cannot_restore_forgotten_labels() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    named(&svc, 2, "KeptVolumeRoot", PATH_B);
    let req = request("alerts.configure", retained_privacy_alert_settings(&svc, 7));
    let fingerprint = json!({
        "command": req.command,
        "payload": req.payload,
        "expected_state_rev": req.expected_state_rev,
        "expected_generation": req.expected_generation,
    });
    assert_eq!(
        svc.inner.idem.claim(&req.request_id, &fingerprint),
        crate::idem::Claim::Run
    );
    // Execute the actual settings handler and its response validation, then retain the same
    // Settle boundary a mutation worker reaches after the handler releases the mutation mutex.
    let reply = svc.inner.respond(
        &req,
        Command::AlertsConfigure,
        &CallContext::http(),
        Instant::now(),
        None,
        None,
    );
    ok("alerts.configure", reply.clone());
    assert!(retained_privacy_has_name(&reply, NAME_A));
    assert!(retained_privacy_has_name(&reply, PATH_A));
    let settle = crate::Settle {
        inner: svc.inner.clone(),
        key: req.request_id.to_string(),
        ticket: None,
        outcome: Some(reply),
    };
    svc.inner.revoke(grant).unwrap();
    ok("roots.forget", forget(&svc, root, None));
    drop(settle);
    let replay = svc.call(req, &CallContext::http());
    assert_eq!(
        svc.inner.hooks.runs.lock().unwrap()[&Command::AlertsConfigure],
        1
    );
    assert_eq!(
        ok("alerts.list", call(&svc, "alerts.list", json!({}))).0["settings"]
            ["low_space_free_percent"],
        7
    );
    assert!(
        !retained_privacy_has_name(&replay, NAME_A) && !retained_privacy_has_name(&replay, PATH_A),
        "late alerts.configure settlement restored forgotten root or volume labels"
    );
    assert_eq!(reason(&err(replay)), Some("forgotten_result"));
}

fn rejected_forget_preserves_pending_replies(writer_failure: bool) {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    named(&svc, 2, "KeptVolumeRoot", PATH_B);
    svc.inner.revoke(grant).unwrap();
    let req = retained_privacy_tier_request(&svc);
    let original = svc.call(req.clone(), &CallContext::http());
    ok("volumes.declare_tier", original.clone());
    let pending_tier = svc.call_pending(req.clone(), &CallContext::http());
    let pending_roots = svc.call_pending(request("roots.list", json!({})), &CallContext::http());
    let rev = state_rev(&svc);
    if writer_failure {
        // Reject after the writer has changed rows inside its transaction. Rollback must be
        // distinguished from a durable marker followed by a derived catalogue purge failure.
        svc.inner
            .db()
            .execute_batch(
                "CREATE TRIGGER st.reject_response_forget BEFORE INSERT ON root_forgotten
             BEGIN SELECT RAISE(ABORT, 'synthetic pre-commit forget failure'); END",
            )
            .unwrap();
    }
    let rejected = err(forget(&svc, root, (!writer_failure).then_some(rev + 7)));
    assert_eq!(
        rejected.code,
        if writer_failure {
            ErrorCode::InternalError
        } else {
            ErrorCode::StaleGeneration
        }
    );
    assert_eq!(count(&svc, "SELECT count(*) FROM st.root_forgotten"), 0);
    assert_eq!(state_rev(&svc), rev);
    assert_eq!(
        pending_tier.admit(),
        original,
        "a confirmed rejected forget must resume the original response stamp"
    );
    let roots = pending_roots.admit();
    ok("roots.list", roots.clone());
    assert!(retained_privacy_has_name(&roots, PATH_A));
    assert_eq!(svc.call(req.clone(), &CallContext::http()), original);
    assert_eq!(count(&svc, "SELECT count(*) FROM st.tier_declaration"), 1);
    assert_eq!(
        svc.inner.hooks.runs.lock().unwrap()[&Command::VolumesDeclareTier],
        1
    );
    let mut changed = req;
    changed.payload.insert("tier".into(), json!(5));
    assert_eq!(
        reason(&err(svc.call(changed, &CallContext::http()))),
        Some("idempotency_conflict")
    );
}

#[test]
fn retained_privacy_rejected_forget_resumes_pending_replies_after_a_stale_revision() {
    rejected_forget_preserves_pending_replies(false);
}

#[test]
fn retained_privacy_rejected_forget_resumes_pending_replies_after_writer_rollback() {
    rejected_forget_preserves_pending_replies(true);
}

#[derive(Clone, Copy, PartialEq)]
enum ResponseForgetOutcome {
    Rejected,
    Durable,
    Unknown,
}

fn response_waits_for_forget_decision(outcome: ResponseForgetOutcome) {
    use std::future::Future;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll, Wake, Waker};

    struct ObserveWake {
        inner: Arc<crate::Inner>,
        held_mutation: AtomicBool,
        wakes: AtomicUsize,
    }
    impl Wake for ObserveWake {
        fn wake(self: Arc<Self>) {
            if self.inner.mutation.try_lock().is_err() {
                self.held_mutation.store(true, Ordering::SeqCst);
            }
            self.wakes.fetch_add(1, Ordering::SeqCst);
        }
    }

    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    named(&svc, 2, "KeptVolumeRoot", PATH_B);
    svc.inner.revoke(grant).unwrap();
    let pending = svc.call_pending(request("roots.list", json!({})), &CallContext::http());
    let mut admission = std::pin::pin!(pending.admit_async());
    let wake = Arc::new(ObserveWake {
        inner: svc.inner.clone(),
        held_mutation: AtomicBool::new(false),
        wakes: AtomicUsize::new(0),
    });
    let waker = Waker::from(wake.clone());
    let mut cx = Context::from_waker(&waker);
    let expected = (outcome != ResponseForgetOutcome::Durable).then_some(state_rev(&svc) + 7);
    const GATE: (Command, &str) = (Command::RootsForget, "forget_written");
    svc.inner.hooks.closed.lock().unwrap().push(GATE);
    struct Release(Arc<crate::tests::Hooks>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.open(GATE.0, GATE.1);
        }
    }
    let forgotten = std::thread::scope(|scope| {
        let worker = scope.spawn(|| forget(&svc, root, expected));
        let release = Release(svc.inner.hooks.clone());
        svc.inner
            .hooks
            .until("the writer returned before the marker decision", |h| {
                h.arrived.lock().unwrap().contains(&GATE)
            });
        assert!(
            admission.as_mut().poll(&mut cx).is_pending(),
            "admission must wait while forget has not classified the durable outcome"
        );
        assert_eq!(
            count(&svc, "SELECT count(*) FROM st.root_forgotten"),
            i64::from(outcome == ResponseForgetOutcome::Durable)
        );
        if outcome == ResponseForgetOutcome::Unknown {
            svc.inner
                .db()
                .execute_batch("ALTER TABLE st.root_forgotten RENAME TO hidden_response_marker")
                .unwrap();
        }
        drop(release);
        worker.join().unwrap()
    });
    if outcome == ResponseForgetOutcome::Unknown {
        svc.inner
            .db()
            .execute_batch("ALTER TABLE st.hidden_response_marker RENAME TO root_forgotten")
            .unwrap();
        assert_eq!(count(&svc, "SELECT count(*) FROM st.root_forgotten"), 0);
    }
    if outcome == ResponseForgetOutcome::Durable {
        ok("roots.forget", forgotten);
    } else {
        assert_eq!(err(forgotten).code, ErrorCode::StaleGeneration);
    }
    assert!(wake.wakes.load(Ordering::SeqCst) > 0);
    assert!(
        !wake.held_mutation.load(Ordering::SeqCst),
        "a response wake callback ran while the service mutation lock was held"
    );
    let Poll::Ready(delivered) = admission.as_mut().poll(&mut cx) else {
        panic!("the completed forget left a response paused")
    };
    if outcome == ResponseForgetOutcome::Rejected {
        ok("roots.list", delivered.clone());
        assert!(retained_privacy_has_name(&delivered, PATH_A));
    } else {
        assert!(!retained_privacy_has_name(&delivered, PATH_A));
        assert_eq!(reason(&err(delivered)), Some("forgotten_result"));
    }
}

#[test]
fn retained_privacy_response_waits_for_a_rejected_forget_decision() {
    response_waits_for_forget_decision(ResponseForgetOutcome::Rejected);
}

#[test]
fn retained_privacy_response_waits_for_a_durable_forget_decision() {
    response_waits_for_forget_decision(ResponseForgetOutcome::Durable);
}

#[test]
fn retained_privacy_unreadable_marker_retires_the_pending_response() {
    response_waits_for_forget_decision(ResponseForgetOutcome::Unknown);
}
