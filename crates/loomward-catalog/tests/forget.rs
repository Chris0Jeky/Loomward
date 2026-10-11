//! Forgetting a revoked root (#288): Loomward's own rows go, the audit record stays, nothing else moves.
use loomward_catalog::*;
use rusqlite::Connection;
use std::path::Path;

const PATH_A: &str = r"C:\Synthetic\ForgetPathAlpha";
const NAME_A: &str = "ForgetFolderAlpha";
const FILE_A: &str = "forgetfileA.zzqx";
const PATH_B: &str = r"C:\Synthetic\KeepPathBeta";
const NAME_B: &str = "KeepFolderBeta";
const FILE_B: &str = "keepfileB.yyqx";

struct Fixture {
    temp: tempfile::TempDir,
    catalog: Catalog,
    grant_a: i64,
    grant_b: i64,
    root_a: i64,
    root_b: i64,
}

fn root(c: &Catalog, volume: &str, name: &str, path: &str, id: u8, file: &str) -> (i64, i64, i64) {
    let w = c.writer();
    let WriteReply::Root {
        grant_id, dir_id, ..
    } = w
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_key: volume.into(),
            display_name: name.into(),
            display_path: path.into(),
            root_file_id: Some([id; 16].to_vec()),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 1,
        }))
        .unwrap()
    else {
        panic!()
    };
    let WriteReply::Run(run) = w
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
    let mut f = Observation::file(file, 10, Some(4096));
    f.file_id = Some([id.wrapping_add(100); 16].to_vec());
    f.created_ft = Some(1);
    f.extension = file.rsplit('.').next().map(str::to_owned);
    w.call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id,
        files: vec![f],
        dirs: vec![],
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }))
    .unwrap();
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 3,
    })
    .unwrap();
    (grant_id, dir_id, run)
}

/// Two roots on one volume, each with one file, one durable reference, one human label (its features
/// name the file), one collection membership and the learning tables the engine creates.
fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(temp.path(), "synthetic").unwrap();
    let (grant_a, dir_a, _) = root(&catalog, "vol", NAME_A, PATH_A, 1, FILE_A);
    let (grant_b, dir_b, _) = root(&catalog, "vol", NAME_B, PATH_B, 2, FILE_B);
    let main = Connection::open(temp.path().join("catalog.db")).unwrap();
    let file_of = |dir: i64| -> i64 {
        main.query_row("SELECT id FROM file WHERE dir_id=?1", [dir], |r| r.get(0))
            .unwrap()
    };
    let root_of = |grant: i64| -> i64 {
        main.query_row("SELECT id FROM root WHERE grant_id=?1", [grant], |r| {
            r.get(0)
        })
        .unwrap()
    };
    let state = Connection::open(temp.path().join("state.db")).unwrap();
    state
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS learning_receipt(client_event_id TEXT PRIMARY KEY, result_json TEXT NOT NULL) STRICT;
             CREATE TABLE IF NOT EXISTS learning_node(node_id TEXT PRIMARY KEY, object_ref_id INTEGER NOT NULL REFERENCES object_ref(id)) STRICT;
             INSERT INTO taxonomy VALUES(1,'[]',0);
             INSERT INTO collection(name,kind,created_at_ns) VALUES('mixed','human',0);",
        )
        .unwrap();
    for (n, (dir, file, grant)) in [(dir_a, FILE_A, grant_a), (dir_b, FILE_B, grant_b)]
        .into_iter()
        .enumerate()
    {
        let reference = match catalog
            .writer()
            .call(WriteCommand::ObjectReference {
                node: NodeKey::File(file_of(dir)),
                observed_at_ns: 5,
            })
            .unwrap()
        {
            WriteReply::Reference(id) => id,
            other => panic!("{other:?} for grant {grant}"),
        };
        state
            .execute(
                "INSERT INTO human_feedback(event_id,client_event_id,content_digest,object_ref_id,revision,label,retracted,taxonomy_version,features_json,recorded_at_ns) VALUES (?1,?2,'d',?3,1,'Documents',0,1,?4,0)",
                rusqlite::params![format!("fe{n}"), format!("ce{n}"), reference, format!("{{\"name\":\"{file}\"}}")],
            )
            .unwrap();
        state
            .execute(
                "INSERT INTO learning_receipt VALUES (?1, ?2)",
                rusqlite::params![format!("ce{n}"), format!("{{\"name\":\"{file}\"}}")],
            )
            .unwrap();
        state
            .execute(
                "INSERT INTO learning_node VALUES (?1,?2)",
                rusqlite::params![format!("nd{n}"), reference],
            )
            .unwrap();
        state
            .execute("INSERT INTO collection_member VALUES (1,?1,0)", [reference])
            .unwrap();
    }
    let (root_a, root_b) = (root_of(grant_a), root_of(grant_b));
    drop((main, state));
    Fixture {
        temp,
        catalog,
        grant_a,
        grant_b,
        root_a,
        root_b,
    }
}

fn revoke(f: &Fixture, grant: i64) {
    f.catalog
        .writer()
        .call(WriteCommand::RevokeGrant {
            grant_id: grant,
            revoked_at_ns: 10,
        })
        .unwrap();
}
fn forget(f: &Fixture, grant: i64, expected: Option<i64>) -> Result<WriteReply> {
    f.catalog.writer().call(WriteCommand::ForgetGrant {
        grant_id: grant,
        forgotten_at_ns: 20,
        expected_state_rev: expected,
    })
}
fn count(db: &Path, sql: &str) -> i64 {
    Connection::open(db)
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}
fn state_count(f: &Fixture, sql: &str) -> i64 {
    count(&f.temp.path().join("state.db"), sql)
}
fn catalog_count(f: &Fixture, sql: &str) -> i64 {
    count(&f.temp.path().join("catalog.db"), sql)
}
/// Whether any byte of the database or its WAL still holds `needle`, as UTF-8 or UTF-16.
fn on_disk(f: &Fixture, needle: &str) -> bool {
    let utf16: Vec<u8> = needle.encode_utf16().flat_map(u16::to_le_bytes).collect();
    ["state.db", "state.db-wal", "catalog.db", "catalog.db-wal"]
        .iter()
        .filter_map(|n| std::fs::read(f.temp.path().join(n)).ok())
        .any(|bytes| {
            [needle.as_bytes(), utf16.as_slice()]
                .iter()
                .any(|n| bytes.windows(n.len()).any(|w| w == *n))
        })
}
fn fold_wal(f: &Fixture) {
    for db in ["state.db", "catalog.db"] {
        Connection::open(f.temp.path().join(db))
            .unwrap()
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
    }
}

#[test]
fn forgetting_purges_every_named_row_and_leaves_the_other_root_untouched() {
    let f = fixture();
    fold_wal(&f);
    // The check below means something only if the names really are on disk before the purge.
    for needle in [PATH_A, NAME_A, FILE_A, "zzqx"] {
        assert!(
            on_disk(&f, needle),
            "{needle} should be stored before the forget"
        );
    }
    revoke(&f, f.grant_a);
    let before = state_count(&f, "SELECT state_rev FROM revision");
    forget(&f, f.grant_a, Some(before)).unwrap();

    // Nothing of root A survives on disk: not in state.db, catalog.db or either WAL.
    for needle in [PATH_A, NAME_A, FILE_A, "zzqx"] {
        assert!(!on_disk(&f, needle), "{needle} survived the forget");
    }
    // The catalogue half: rows, aggregates, vocabulary.
    assert_eq!(
        catalog_count(
            &f,
            &format!("SELECT count(*) FROM root WHERE id={}", f.root_a)
        ),
        0
    );
    assert_eq!(
        catalog_count(
            &f,
            &format!("SELECT count(*) FROM dir WHERE root_id={}", f.root_a)
        ),
        0
    );
    assert_eq!(
        catalog_count(
            &f,
            &format!("SELECT count(*) FROM scan_run WHERE root_id={}", f.root_a)
        ),
        0
    );
    assert_eq!(
        catalog_count(&f, "SELECT count(*) FROM file WHERE name LIKE 'forget%'"),
        0
    );
    assert_eq!(
        catalog_count(&f, "SELECT count(*) FROM ext WHERE ext='zzqx'"),
        0
    );
    // The state half: references, labels, memberships and the learning rows keyed to them.
    assert_eq!(state_count(&f, "SELECT count(*) FROM object_ref"), 1);
    assert_eq!(state_count(&f, "SELECT count(*) FROM human_feedback"), 1);
    assert_eq!(state_count(&f, "SELECT count(*) FROM collection_member"), 1);
    assert_eq!(state_count(&f, "SELECT count(*) FROM learning_receipt"), 1);
    assert_eq!(state_count(&f, "SELECT count(*) FROM learning_node"), 1);
    // The collection itself is the owner's and stays.
    assert_eq!(state_count(&f, "SELECT count(*) FROM collection"), 1);
    // The audit record: dates and a literal word, no name.
    let db = Connection::open(f.temp.path().join("state.db")).unwrap();
    let audit: (String, String, Vec<u8>, i64, i64, i64) = db
        .query_row(
            "SELECT g.display_path,g.volume_key,g.root_file_id,g.granted_at_ns,g.revoked_at_ns,f.forgotten_at_ns FROM root_grant g JOIN root_forgotten f ON f.grant_id=g.id WHERE g.id=?1",
            [f.grant_a],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!(audit.0, "forgotten");
    assert_eq!((audit.3, audit.4, audit.5), (1, 10, 20));
    assert_eq!(audit.2, vec![0; 8]);
    assert_eq!(
        state_count(&f, "SELECT state_rev FROM revision"),
        before + 1
    );

    // Root B: row for row what it was, and its volume still exists under a name that is not A's.
    assert_eq!(
        catalog_count(
            &f,
            &format!(
                "SELECT count(*) FROM file f JOIN dir d ON d.id=f.dir_id WHERE d.root_id={}",
                f.root_b
            )
        ),
        1
    );
    assert_eq!(
        catalog_count(&f, "SELECT count(*) FROM ext WHERE ext='yyqx'"),
        1
    );
    assert_eq!(catalog_count(&f, "SELECT count(*) FROM volume WHERE volume_key='vol' AND display_name!='ForgetFolderAlpha'"), 1);
    let kept: String = db
        .query_row(
            "SELECT display_path FROM root_grant WHERE id=?1",
            [f.grant_b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kept, PATH_B);
    assert!(on_disk(&f, PATH_B) && on_disk(&f, FILE_B));
}

#[test]
fn an_active_grant_is_refused_and_nothing_changes() {
    let f = fixture();
    let before = state_count(&f, "SELECT state_rev FROM revision");
    let refused = forget(&f, f.grant_a, None).unwrap_err();
    assert!(
        matches!(refused, Error::Invalid("grant is not revoked")),
        "{refused:?}"
    );
    assert_eq!(state_count(&f, "SELECT state_rev FROM revision"), before);
    assert_eq!(state_count(&f, "SELECT count(*) FROM root_forgotten"), 0);
    assert_eq!(state_count(&f, "SELECT count(*) FROM human_feedback"), 2);
    assert_eq!(catalog_count(&f, "SELECT count(*) FROM file"), 2);
    assert!(matches!(
        forget(&f, 999, None).unwrap_err(),
        Error::NotFound
    ));
}

#[test]
fn a_stale_state_revision_writes_nothing_and_forgetting_twice_is_a_no_op() {
    let f = fixture();
    revoke(&f, f.grant_a);
    let rev = state_count(&f, "SELECT state_rev FROM revision");
    assert!(matches!(
        forget(&f, f.grant_a, Some(rev + 1)).unwrap_err(),
        Error::StaleGeneration
    ));
    assert_eq!(state_count(&f, "SELECT count(*) FROM root_forgotten"), 0);
    assert_eq!(catalog_count(&f, "SELECT count(*) FROM file"), 2);

    forget(&f, f.grant_a, Some(rev)).unwrap();
    let after = state_count(&f, "SELECT state_rev FROM revision");
    forget(&f, f.grant_a, None).unwrap();
    assert_eq!(state_count(&f, "SELECT state_rev FROM revision"), after);
    assert_eq!(
        state_count(&f, "SELECT forgotten_at_ns FROM root_forgotten"),
        20
    );
}

#[test]
fn the_last_root_of_a_volume_takes_the_volume_row_with_it() {
    let f = fixture();
    for grant in [f.grant_a, f.grant_b] {
        revoke(&f, grant);
        forget(&f, grant, None).unwrap();
    }
    assert_eq!(catalog_count(&f, "SELECT count(*) FROM volume"), 0);
    assert_eq!(catalog_count(&f, "SELECT count(*) FROM root"), 0);
    assert_eq!(catalog_count(&f, "SELECT count(*) FROM file"), 0);
    assert_eq!(state_count(&f, "SELECT count(*) FROM object_ref"), 0);
    for needle in [PATH_A, NAME_A, FILE_A, PATH_B, NAME_B, FILE_B] {
        assert!(!on_disk(&f, needle), "{needle} survived");
    }
    assert_eq!(
        state_count(
            &f,
            "SELECT count(*) FROM root_grant WHERE display_path='forgotten'"
        ),
        2
    );
}

#[test]
fn a_forget_that_stopped_after_its_state_half_finishes_on_the_next_open() {
    let f = fixture();
    revoke(&f, f.grant_a);
    let Fixture {
        temp,
        catalog,
        grant_a,
        root_a,
        ..
    } = f;
    drop(catalog);
    // The state half alone, as a crash between the two commits would leave it.
    let state = Connection::open(temp.path().join("state.db")).unwrap();
    state
        .execute_batch(&format!(
            "UPDATE root_grant SET display_path='forgotten' WHERE id={grant_a}; INSERT INTO root_forgotten VALUES({grant_a},{root_a},20);"
        ))
        .unwrap();
    drop(state);
    let _reopened = Catalog::open(temp.path(), "synthetic").unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let rows: i64 = db
        .query_row(
            &format!("SELECT count(*) FROM root WHERE id={root_a}"),
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn a_very_deep_tree_is_deleted_without_a_recursion_limit() {
    let f = fixture();
    {
        let db = Connection::open(f.temp.path().join("catalog.db")).unwrap();
        db.pragma_update(None, "foreign_keys", "ON").unwrap();
        let top: i64 = db
            .query_row(
                "SELECT id FROM dir WHERE root_id=?1 AND parent_id IS NULL",
                [f.root_a],
                |r| r.get(0),
            )
            .unwrap();
        db.execute(
            "INSERT INTO dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run)
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<5000)
             SELECT ?1, ?2+i-1, 'deep', 'path_observation','none', i, 0, 'complete', 0, 0 FROM n",
            rusqlite::params![f.root_a, top],
        )
        .unwrap();
    }
    revoke(&f, f.grant_a);
    forget(&f, f.grant_a, None).unwrap();
    assert_eq!(
        catalog_count(
            &f,
            &format!("SELECT count(*) FROM dir WHERE root_id={}", f.root_a)
        ),
        0
    );
}

fn ref_of(f: &Fixture, grant: i64) -> i64 {
    // The fixture records references in grant order: the n-th reference belongs to the n-th root.
    let n = if grant == f.grant_a { 0 } else { 1 };
    state_count(
        f,
        &format!("SELECT id FROM object_ref ORDER BY id LIMIT 1 OFFSET {n}"),
    )
}

#[test]
fn a_reference_after_a_catalogue_rebuild_is_matched_by_native_identity() {
    let f = fixture();
    // As after a rebuild: A's reference is unresolved and bound to no row, but its object is A's.
    let a = ref_of(&f, f.grant_a);
    Connection::open(f.temp.path().join("state.db"))
        .unwrap()
        .execute(
            "UPDATE object_ref SET state='unresolved',retired=1,catalog_instance=NULL,row_id=NULL,born_run=NULL WHERE id=?1",
            [a],
        )
        .unwrap();
    revoke(&f, f.grant_a);
    forget(&f, f.grant_a, None).unwrap();
    assert_eq!(
        state_count(&f, &format!("SELECT count(*) FROM object_ref WHERE id={a}")),
        0
    );
    assert_eq!(state_count(&f, "SELECT count(*) FROM object_ref"), 1);
    assert_eq!(state_count(&f, "SELECT count(*) FROM human_feedback"), 1);
    assert_eq!(state_count(&f, "SELECT count(*) FROM collection_member"), 1);
}

#[test]
fn an_object_another_root_still_observes_keeps_its_reference() {
    let f = fixture();
    // Root B also lists the object A's reference names (a hard link across roots).
    let (a, b) = (ref_of(&f, f.grant_a), ref_of(&f, f.grant_b));
    Connection::open(f.temp.path().join("catalog.db"))
        .unwrap()
        .execute(
            "UPDATE file SET file_id=?1 WHERE dir_id IN (SELECT id FROM dir WHERE root_id=?2)",
            rusqlite::params![vec![101u8; 16], f.root_b],
        )
        .unwrap();
    let state = Connection::open(f.temp.path().join("state.db")).unwrap();
    state
        .execute(
            "UPDATE object_ref SET file_id=?2,creation_ft=2 WHERE id=?1",
            rusqlite::params![b, vec![101u8; 16]],
        )
        .unwrap();
    state
        .execute(
            "UPDATE object_ref SET state='unresolved',retired=1,catalog_instance=NULL,row_id=NULL,born_run=NULL WHERE id=?1",
            [a],
        )
        .unwrap();
    drop(state);
    revoke(&f, f.grant_a);
    forget(&f, f.grant_a, None).unwrap();
    // A's unresolved reference names an object B still observes: it is not "only A's".
    assert_eq!(state_count(&f, "SELECT count(*) FROM object_ref"), 2);
    assert_eq!(state_count(&f, "SELECT count(*) FROM human_feedback"), 2);
    assert_eq!(state_count(&f, "SELECT count(*) FROM collection_member"), 2);
}

#[test]
fn a_reader_that_outlasts_the_forget_only_delays_the_log_cut() {
    let f = fixture();
    fold_wal(&f);
    revoke(&f, f.grant_a);
    // An older snapshot of state.db keeps the pre-purge pages alive; SQLite cannot cut the log under it.
    let reader = Connection::open(f.temp.path().join("state.db")).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM root_grant")
        .unwrap();
    let started = std::time::Instant::now();
    forget(&f, f.grant_a, None).unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the forget must not wait for the reader"
    );
    assert!(
        on_disk(&f, PATH_A),
        "the reader's snapshot still needs those pages"
    );
    reader.execute_batch("COMMIT").unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while on_disk(&f, PATH_A) && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        !on_disk(&f, PATH_A),
        "the writer cuts the log once the reader is gone"
    );
}

#[test]
fn a_bound_object_another_root_still_observes_keeps_its_labels_and_membership() {
    let f = fixture();
    // Root B lists the very object A's reference is bound to (a hard link seen under both roots);
    // A's reference stays resolved and bound to A's row, as it is right after labelling.
    let (a, b) = (ref_of(&f, f.grant_a), ref_of(&f, f.grant_b));
    Connection::open(f.temp.path().join("catalog.db"))
        .unwrap()
        .execute(
            "UPDATE file SET file_id=?1 WHERE dir_id IN (SELECT id FROM dir WHERE root_id=?2)",
            rusqlite::params![vec![101u8; 16], f.root_b],
        )
        .unwrap();
    Connection::open(f.temp.path().join("state.db"))
        .unwrap()
        .execute(
            "UPDATE object_ref SET file_id=?2,creation_ft=2 WHERE id=?1",
            rusqlite::params![b, vec![101u8; 16]],
        )
        .unwrap();
    assert_eq!(
        state_count(
            &f,
            &format!("SELECT state='resolved' FROM object_ref WHERE id={a}")
        ),
        1
    );
    revoke(&f, f.grant_a);
    forget(&f, f.grant_a, None).unwrap();
    assert_eq!(state_count(&f, "SELECT count(*) FROM object_ref"), 2);
    assert_eq!(state_count(&f, "SELECT count(*) FROM human_feedback"), 2);
    assert_eq!(state_count(&f, "SELECT count(*) FROM collection_member"), 2);
    // Its row in A is gone, so the binding is no longer trusted: unresolved, as after a rebuild.
    assert_eq!(
        state_count(
            &f,
            &format!("SELECT state='unresolved' AND retired=1 FROM object_ref WHERE id={a}")
        ),
        1
    );
    // Once no other root observes it, forgetting B takes it with the rest.
    revoke(&f, f.grant_b);
    forget(&f, f.grant_b, None).unwrap();
    assert_eq!(state_count(&f, "SELECT count(*) FROM object_ref"), 0);
    assert_eq!(state_count(&f, "SELECT count(*) FROM human_feedback"), 0);
    assert_eq!(state_count(&f, "SELECT count(*) FROM collection_member"), 0);
}
