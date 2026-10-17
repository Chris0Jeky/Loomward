//! Synthetic post-forget rewrite probe (#434). Run prepare, then measure a fresh forget process.
use loomward_catalog::{Catalog, RootObservation, WriteCommand, WriteReply};
use rusqlite::Connection;
use std::{path::PathBuf, time::Instant};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(
        args.len(),
        3,
        "forget_perf <prepare|prepare-10m|forget> <new synthetic store>"
    );
    let dir = PathBuf::from(&args[2]);
    if matches!(args[1].as_str(), "prepare" | "prepare-10m") {
        assert!(!dir.exists(), "prepare only creates a new disposable store");
        let c = Catalog::open(&dir, "synthetic").unwrap();
        let WriteReply::Root {
            grant_id, dir_id, ..
        } = c
            .writer()
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "synthetic-forget".into(),
                display_name: "Synthetic forget probe".into(),
                display_path: "Synthetic forget probe path".into(),
                root_file_id: Some(vec![1; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap()
        else {
            panic!()
        };
        c.writer()
            .call(WriteCommand::RevokeGrant {
                grant_id,
                revoked_at_ns: 1,
            })
            .unwrap();
        drop(c);
        let mut db = Connection::open(dir.join("catalog.db")).unwrap();
        if args[1] == "prepare-10m" {
            for batch in 0..100 {
                db.execute(
                    "INSERT INTO file(dir_id,name,logical,allocated,attrs,born_run,seen_run)
                    WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<100000)
                    SELECT ?1,printf('synthetic-%08d.dat',i+?2),1,4096,0,0,0 FROM n",
                    rusqlite::params![dir_id, batch * 100000],
                )
                .unwrap();
            }
            assert_eq!(
                db.query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                10000000
            );
        } else {
            db.execute_batch(
                "CREATE TABLE synthetic_padding(id INTEGER PRIMARY KEY, bytes BLOB NOT NULL)",
            )
            .unwrap();
            let tx = db.transaction().unwrap();
            for _ in 0..1024 {
                tx.execute(
                    "INSERT INTO synthetic_padding(bytes) VALUES(zeroblob(1048576))",
                    [],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        println!(
            "{}",
            serde_json::json!({"catalog_bytes": std::fs::metadata(dir.join("catalog.db")).unwrap().len()})
        );
    } else {
        assert_eq!(args[1], "forget");
        let c = Catalog::open(&dir, "synthetic").unwrap();
        println!("ready");
        let started = Instant::now();
        c.writer()
            .call(WriteCommand::ForgetGrant {
                grant_id: 1,
                forgotten_at_ns: 2,
                expected_state_rev: None,
            })
            .unwrap();
        let elapsed = started.elapsed().as_secs_f64();
        drop(c);
        let db = Connection::open(dir.join("catalog.db")).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM root", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        if db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='synthetic_padding')",
                [],
                |r| r.get::<_, bool>(0),
            )
            .unwrap()
        {
            assert_eq!(
                db.query_row("SELECT count(*) FROM synthetic_padding", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1024
            );
        }
        println!(
            "{}",
            serde_json::json!({"forget_seconds": elapsed, "catalog_bytes": std::fs::metadata(dir.join("catalog.db")).unwrap().len()})
        );
    }
}
