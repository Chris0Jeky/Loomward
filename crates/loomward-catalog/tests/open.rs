use loomward_catalog::{Catalog, Error, APPLICATION_ID, SCHEMA_VERSION};
use rusqlite::{Connection, OpenFlags};

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
        assert_eq!(db.query_row("SELECT count(*) FROM pragma_table_list WHERE schema='main' AND name NOT LIKE 'sqlite_%' AND strict=0", [], |r| r.get::<_,i64>(0)).unwrap(), 0);
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
