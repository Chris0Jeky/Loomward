use loomward_catalog::{Catalog, Error, APPLICATION_ID, SCHEMA_VERSION};
use rusqlite::{Connection, OpenFlags};

#[test]
fn existing_catalogue_gains_partial_directory_gap_index() {
    let temp = tempfile::tempdir().unwrap();
    drop(Catalog::open(temp.path(), "synthetic").unwrap());
    let conn = Connection::open(temp.path().join("catalog.db")).unwrap();
    conn.execute_batch("DROP INDEX IF EXISTS dir_missing_identity")
        .unwrap();
    drop(conn);
    drop(Catalog::open(temp.path(), "synthetic").unwrap());
    let conn = Connection::open(temp.path().join("catalog.db")).unwrap();
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='dir_missing_identity'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        sql.contains("WHERE file_id IS NULL AND listing_state!='absent_pending'"),
        "{sql}"
    );
}

#[test]
fn fresh_pair_has_strict_schema_and_separate_data_classes() {
    let temp = tempfile::tempdir().unwrap();
    let _catalog = Catalog::open(temp.path(), "synthetic").unwrap();
    for name in ["catalog.db", "state.db"] {
        let db = Connection::open(temp.path().join(name)).unwrap();
        let id: i64 = db
            .pragma_query_value(None, "application_id", |r| r.get(0))
            .unwrap();
        let version: i64 = db
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!((id, version), (APPLICATION_ID, SCHEMA_VERSION));
        assert_eq!(
            db.query_row(
                "SELECT value FROM meta WHERE key='dataset_class'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "synthetic"
        );
        assert!(
            db.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
                .unwrap()
                == "ok"
        );
        assert_eq!(db.query_row("SELECT count(*) FROM pragma_table_list WHERE schema='main' AND type='table' AND name NOT LIKE 'sqlite_%' AND strict=0", [], |r| r.get::<_,i64>(0)).unwrap(), 0);
    }
    assert!(matches!(
        Catalog::open(temp.path(), "personal"),
        Err(Error::DatasetMismatch)
    ));
}

#[test]
fn foreign_newer_unowned_and_network_paths_are_refused_without_rewriting() {
    for (id, version, expected_foreign) in
        [(123, 0, true), (APPLICATION_ID, SCHEMA_VERSION + 1, false)]
    {
        let temp = tempfile::tempdir().unwrap();
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        db.pragma_update(None, "application_id", id).unwrap();
        db.pragma_update(None, "user_version", version).unwrap();
        drop(db);
        let result = Catalog::open(temp.path(), "synthetic");
        assert!(if expected_foreign {
            matches!(result, Err(Error::ForeignDatabase))
        } else {
            matches!(result, Err(Error::NewerDatabase))
        });
    }
    let temp = tempfile::tempdir().unwrap();
    Connection::open(temp.path().join("state.db"))
        .unwrap()
        .execute("CREATE TABLE somebody_elses_data(x)", [])
        .unwrap();
    assert!(matches!(
        Catalog::open(temp.path(), "synthetic"),
        Err(Error::ForeignDatabase)
    ));
    for path in [
        r"\\server\share\catalog",
        r"\\?\UNC\server\share",
        "//server/share",
        r"\\.\pipe\catalog",
    ] {
        assert!(matches!(
            Catalog::open(std::path::Path::new(path), "synthetic"),
            Err(Error::Invalid(_))
        ));
    }
}

#[test]
fn precious_migration_is_backed_up_and_a_failure_rolls_back() {
    let temp = tempfile::tempdir().unwrap();
    let db = Connection::open(temp.path().join("state.db")).unwrap();
    db.pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    db.execute("CREATE TABLE taxonomy(precious TEXT)", [])
        .unwrap();
    db.execute("INSERT INTO taxonomy VALUES ('synthetic-kept')", [])
        .unwrap();
    drop(db);
    assert!(Catalog::open(temp.path(), "synthetic").is_err());
    let db = Connection::open(temp.path().join("state.db")).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT precious FROM taxonomy", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "synthetic-kept"
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='meta'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    let backup = Connection::open(temp.path().join("state.before-v0.db")).unwrap();
    assert_eq!(
        backup
            .query_row("SELECT precious FROM taxonomy", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "synthetic-kept"
    );
}

#[test]
fn corrupt_derived_catalog_is_preserved_and_precious_corruption_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("catalog.db"),
        b"synthetic corrupt catalogue",
    )
    .unwrap();
    Catalog::validate_stores(temp.path(), "synthetic").unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("catalog.db")).unwrap(),
        b"synthetic corrupt catalogue"
    );
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    drop(c);
    let renamed = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("catalog.corrupt-")
        })
        .unwrap();
    assert_eq!(
        std::fs::read(renamed).unwrap(),
        b"synthetic corrupt catalogue"
    );
    let another = tempfile::tempdir().unwrap();
    std::fs::write(
        another.path().join("state.db"),
        b"synthetic precious corruption",
    )
    .unwrap();
    assert!(matches!(
        Catalog::validate_stores(another.path(), "synthetic"),
        Err(Error::CorruptDatabase)
    ));
    assert!(matches!(
        Catalog::open(another.path(), "synthetic"),
        Err(Error::CorruptDatabase)
    ));
    assert_eq!(
        std::fs::read(another.path().join("state.db")).unwrap(),
        b"synthetic precious corruption"
    );
    assert!(!another.path().join("catalog.db").exists());
}

#[test]
fn corrupt_schema_page_rebuilds_only_the_derived_database() {
    for page_size in [512, 4096, 65536] {
        for name in ["catalog.db", "state.db"] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join(name);
            let db = Connection::open(&path).unwrap();
            db.pragma_update(None, "page_size", page_size).unwrap();
            db.execute_batch("VACUUM").unwrap();
            db.close().unwrap();
            drop(Catalog::open(temp.path(), "synthetic").unwrap());
            // Exercise the same setup when a valid page-1 WAL survives a previous connection.
            let db = Connection::open(&path).unwrap();
            db.pragma_update(None, "user_version", SCHEMA_VERSION)
                .unwrap();
            let wal_path = path.with_file_name(format!("{name}-wal"));
            let wal = std::fs::read(&wal_path).unwrap();
            assert_eq!(u32::from_be_bytes(wal[32..36].try_into().unwrap()), 1);
            db.close().unwrap();
            std::fs::write(&wal_path, wal).unwrap();
            let db = Connection::open(&path).unwrap();
            db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
            let mode: String = db
                .pragma_update_and_check(None, "journal_mode", "DELETE", |r| r.get(0))
                .unwrap();
            assert_eq!(mode, "delete");
            assert!(
                db.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap()
                    > 0
            );
            db.close().unwrap();
            // A healthy WAL page must not mask (or checkpoint over) the injected main-file damage.
            assert!(!path.with_file_name(format!("{name}-wal")).exists());
            assert!(!path.with_file_name(format!("{name}-shm")).exists());
            let mut damaged = std::fs::read(&path).unwrap();
            assert_eq!(&damaged[..16], b"SQLite format 3\0");
            let encoded_size = u16::from_be_bytes([damaged[16], damaged[17]]);
            let header_page_size = if encoded_size == 1 {
                65536
            } else {
                usize::from(encoded_size)
            };
            assert_eq!(header_page_size, page_size as usize);
            // sqlite_schema is rooted at page 1; its b-tree header follows the 100-byte file header.
            assert!(matches!(damaged[100], 5 | 13));
            damaged[100] = 0;
            std::fs::write(&path, &damaged).unwrap();
            let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
            assert_eq!(
                db.pragma_query_value(None, "application_id", |r| r.get::<_, i64>(0))
                    .unwrap(),
                APPLICATION_ID
            );
            assert_eq!(
                db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                    .unwrap(),
                SCHEMA_VERSION
            );
            let schema_read = db.query_row("SELECT count(*) FROM sqlite_schema", [], |r| {
                r.get::<_, i64>(0)
            });
            assert!(
                matches!(
                    &schema_read,
                    Err(rusqlite::Error::SqliteFailure(e, _))
                        if e.code == rusqlite::ErrorCode::DatabaseCorrupt
                ),
                "{schema_read:?}"
            );
            db.close().unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), damaged);
            let precious = std::fs::read(temp.path().join("state.db")).unwrap();
            let result = Catalog::open(temp.path(), "synthetic");
            if name == "catalog.db" {
                drop(result.unwrap());
                let archive = std::fs::read_dir(temp.path())
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .find(|p| {
                        p.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with("catalog.corrupt-")
                            && p.extension().is_some_and(|ext| ext == "db")
                    })
                    .unwrap();
                assert_eq!(std::fs::read(archive).unwrap(), damaged);
                let db = Connection::open(&path).unwrap();
                assert_eq!(
                    db.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
                        .unwrap(),
                    "ok"
                );
            } else {
                assert!(matches!(result, Err(Error::CorruptDatabase)));
                assert_eq!(std::fs::read(&path).unwrap(), damaged);
            }
            assert_eq!(
                std::fs::read(temp.path().join("state.db")).unwrap(),
                precious
            );
        }
    }
}

#[test]
fn a_preexisting_backup_does_not_block_restart_before_migration() {
    let temp = tempfile::tempdir().unwrap();
    let db = Connection::open(temp.path().join("state.db")).unwrap();
    db.execute(
        "VACUUM INTO ?1",
        [temp
            .path()
            .join("state.before-v0.db")
            .to_string_lossy()
            .as_ref()],
    )
    .unwrap();
    drop(db);
    let old = std::fs::read(temp.path().join("state.before-v0.db")).unwrap();
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    drop(c);
    assert_eq!(
        std::fs::read(temp.path().join("state.before-v0.db")).unwrap(),
        old
    );
    assert!(temp.path().join("state.before-v0-1.db").exists());
}
#[test]
fn v1_migration_preserves_precious_history_and_moves_existing_consent_before_rebuild() {
    let temp = tempfile::tempdir().unwrap();
    for (name, schema) in [
        ("catalog.db", include_str!("fixtures/catalog-v1.sql")),
        ("state.db", include_str!("fixtures/state-v1.sql")),
    ] {
        let db = Connection::open(temp.path().join(name)).unwrap();
        db.execute_batch(schema).unwrap();
        db.pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        db.pragma_update(None, "user_version", 1).unwrap();
        db.execute("INSERT INTO meta VALUES('dataset_class','synthetic')", [])
            .unwrap();
    }
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    db.execute_batch("INSERT INTO volume VALUES(1,'fixture','Fixture','{}','{}','{}',1,0);INSERT INTO root_grant VALUES(1,1,zeroblob(16),'Fixture root','fixture','fixture','active',0,NULL);INSERT INTO root_grant VALUES(2,1,zeroblob(16),'Revoked fixture','fixture','fixture','revoked',0,1);").unwrap();
    drop(db);
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    st.execute_batch("INSERT INTO taxonomy VALUES(1,'[]',0);INSERT INTO object_ref VALUES(1,'fixture',zeroblob(16),'file');INSERT INTO human_feedback VALUES(1,'fixture-event',1,1,'archive',0,1,'{}',0);INSERT INTO collection VALUES(1,'Fixture collection','human',0);INSERT INTO collection_member VALUES(1,1,0);").unwrap();
    drop(st);
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    assert_eq!(
        st.query_row(
            "SELECT label FROM legacy_v1_human_feedback WHERE id=1",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "archive"
    );
    assert_eq!(
        st.query_row("SELECT count(*) FROM collection_member", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        st.query_row("SELECT state,retired FROM object_ref WHERE id=1", [], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
        })
        .unwrap(),
        ("unresolved".into(), true)
    );
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT grant_id FROM root", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        st.query_row(
            "SELECT state,volume_guid FROM root_grant WHERE id=1",
            [],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        )
        .unwrap(),
        ("identity_changed".into(), None)
    );
    assert_eq!(
        st.query_row("SELECT state FROM root_grant WHERE id=2", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "revoked"
    );
    assert!(temp.path().join("state.before-v1.db").exists());
    assert!(std::fs::read_dir(temp.path()).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("catalog.before-v1-")));
    drop(c);
}

#[test]
fn startup_keeps_seven_owned_backups_and_preserves_migration_backup() {
    let temp = tempfile::tempdir().unwrap();
    for _ in 0..9 {
        drop(Catalog::open(temp.path(), "synthetic").unwrap());
    }
    let count = std::fs::read_dir(temp.path())
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("state.start-")
        })
        .count();
    assert_eq!(count, 7);
    assert!(temp.path().join("state.before-v0.db").exists());
}

/// A serial-only legacy grant must not gain permission for the currently mounted volume.
#[test]
fn v8_grants_gain_unknown_guid_without_inventing_a_binding() {
    let temp = tempfile::tempdir().unwrap();
    drop(Catalog::open(temp.path(), "synthetic").unwrap());
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    let has_guid: bool = st
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('root_grant') WHERE name='volume_guid')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if has_guid {
        st.execute_batch("ALTER TABLE root_grant DROP COLUMN volume_guid")
            .unwrap();
    }
    st.execute_batch("INSERT INTO root_grant(volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES('vsn_42',zeroblob(16),'Synthetic root','fixture','fixture','active',0); PRAGMA user_version=8;").unwrap();
    drop(st);
    drop(Catalog::open(temp.path(), "synthetic").unwrap());
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    let has_guid: bool = st
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('root_grant') WHERE name='volume_guid')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(has_guid, "migration must persist the grant's volume GUID");
    let row: (String, Option<String>, String) = st
        .query_row(
            "SELECT volume_key,volume_guid,state FROM root_grant",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, ("vsn_42".into(), None, "identity_changed".into()));
    assert!(temp.path().join("state.before-v8.db").exists());
}

/// #219: a v4 state store gains `root_grant.identity_mode`, every existing grant `verified`.
#[test]
fn v4_state_store_grants_migrate_as_identity_verified() {
    let temp = tempfile::tempdir().unwrap();
    drop(Catalog::open(temp.path(), "synthetic").unwrap());
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    st.execute_batch(
        "ALTER TABLE root_grant DROP COLUMN identity_mode;
         INSERT INTO root_grant(volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns)
           VALUES('fixture',zeroblob(16),'Synthetic root','fixture','fixture','active',0);
         PRAGMA user_version=4;",
    )
    .unwrap();
    drop(st);
    drop(Catalog::open(temp.path(), "synthetic").unwrap());
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    assert_eq!(
        st.query_row(
            "SELECT identity_mode, (SELECT user_version FROM pragma_user_version) FROM root_grant",
            [],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        )
        .unwrap(),
        ("verified".into(), SCHEMA_VERSION)
    );
    assert!(st
        .execute("UPDATE root_grant SET identity_mode='maybe'", [])
        .is_err());
}

/// Backups stage 1: a v5 store (after #372) and a v4 store (before it) both reach v6 with
/// `backup_evidence`, keeping their rows, and the table refuses anything but a JSON document.
#[test]
fn v6_v5_and_v4_state_stores_reach_v7_with_backup_evidence_and_the_forget_record() {
    assert_eq!(SCHEMA_VERSION, 9);
    for (from, downgrade) in [
        // v6 (backups applied): only forget's step runs, and the evidence already there stays.
        (
            6,
            "DROP TABLE root_forgotten;
             INSERT INTO backup_evidence(repository_label,evidence_json,snapshot_observed_ns,imported_at_ns) VALUES('kept-repo','{}',0,0);",
        ),
        (5, "DROP TABLE root_forgotten; DROP TABLE backup_evidence;"),
        (
            4,
            "DROP TABLE root_forgotten; DROP TABLE backup_evidence; ALTER TABLE root_grant DROP COLUMN identity_mode;",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        drop(Catalog::open(temp.path(), "synthetic").unwrap());
        let st = Connection::open(temp.path().join("state.db")).unwrap();
        st.execute_batch(&format!(
            "{downgrade}
             INSERT INTO collection(name,kind,created_at_ns) VALUES('kept','human',0);
             PRAGMA user_version={from};"
        ))
        .unwrap();
        drop(st);
        drop(Catalog::open(temp.path(), "synthetic").unwrap());
        assert!(
            temp.path().join(format!("state.before-v{from}.db")).exists(),
            "from v{from}: the migration copy"
        );
        let st = Connection::open(temp.path().join("state.db")).unwrap();
        let (version, kept, mode, tables, evidence): (i64, i64, i64, i64, i64) = st
            .query_row(
                "SELECT (SELECT user_version FROM pragma_user_version),(SELECT count(*) FROM collection WHERE name='kept'),
                        (SELECT count(*) FROM pragma_table_info('root_grant') WHERE name='identity_mode'),
                        (SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN ('backup_evidence','root_forgotten')),
                        (SELECT count(*) FROM backup_evidence WHERE repository_label='kept-repo')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(
            (version, kept, mode, tables, evidence),
            (SCHEMA_VERSION, 1, 1, 2, i64::from(from == 6)),
            "from v{from}"
        );
        let insert = |label: &str, json: &str| {
            st.execute(
                "INSERT INTO backup_evidence(repository_label,evidence_json,snapshot_observed_ns,imported_at_ns) VALUES(?1,?2,0,0)",
                [label, json],
            )
        };
        st.execute("DELETE FROM backup_evidence", []).unwrap();
        insert("synthetic-repo", "{}").unwrap();
        assert!(
            insert("other", "raw bytes").is_err(),
            "only a JSON document"
        );
        assert!(
            insert("synthetic-repo", "{}").is_err(),
            "one record per label"
        );
    }
}

/// LW-067: a v7 store gains `client_grant` and keeps its rows; one live grant per client, and the
/// roots and fields are JSON arrays.
#[test]
fn a_v7_state_store_reaches_v8_with_client_grants() {
    let temp = tempfile::tempdir().unwrap();
    drop(Catalog::open(temp.path(), "synthetic").unwrap());
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    st.execute_batch(
        "DROP TABLE client_grant;
         INSERT INTO collection(name,kind,created_at_ns) VALUES('kept','human',0);
         PRAGMA user_version=7;",
    )
    .unwrap();
    drop(st);
    drop(Catalog::open(temp.path(), "synthetic").unwrap());
    assert!(temp.path().join("state.before-v7.db").exists());
    let st = Connection::open(temp.path().join("state.db")).unwrap();
    let (version, kept): (i64, i64) = st
        .query_row(
            "SELECT (SELECT user_version FROM pragma_user_version),(SELECT count(*) FROM collection WHERE name='kept')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((version, kept), (SCHEMA_VERSION, 1));
    let grant = |client: &str, roots: &str, revoked: Option<i64>| {
        st.execute(
            "INSERT INTO client_grant(client,roots_json,fields_json,created_at_ns,revoked_at_ns) VALUES(?1,?2,'[]',0,?3)",
            rusqlite::params![client, roots, revoked],
        )
    };
    grant("mcp:a", "[1]", None).unwrap();
    assert!(
        grant("mcp:a", "[2]", None).is_err(),
        "one live grant per client"
    );
    grant("mcp:a", "[2]", Some(1)).unwrap();
    grant("mcp:b", "[1]", None).unwrap();
    assert!(grant("mcp:c", "{}", None).is_err(), "roots are an array");
    assert!(grant("mcp:c", "1,2", None).is_err(), "roots are JSON");
}
