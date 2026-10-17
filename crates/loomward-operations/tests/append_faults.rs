//! Lane C6 (#535): a journal append that fails as the database would (SQLITE_FULL, SQLITE_BUSY) stores
//! nothing, row and head anchor together, and leaves a journal that replays and accepts the next append.
//! The real-SQLite tests need no feature; the injected-stage tests need `fault-injection` (test builds only).
use loomward_operations::tree::Kind;
use loomward_operations::*;
use rusqlite::{Connection, ErrorCode};

const TOKEN: &str = "lw-staging-1";

fn id(b: u8) -> Identity {
    Identity {
        volume: 1,
        file_id: [b; 16],
    }
}

/// A consumed approval through the public consent path: the only way to start a journal.
fn admitted() -> Admitted {
    let dest = DestinationFacts {
        grant: 1,
        parent: id(200),
        ancestors: vec![id(200)],
        verified_volume: true,
        cloud_synced: false,
        unsupported_volume: false,
        protected: false,
        case_sensitive: false,
        existing_names: vec![],
        parent_path_units: 10,
        available_bytes: 10 << 30,
        volume_bytes: 100 << 30,
        cluster_bytes: 4096,
    };
    let ctx = Context {
        elevated: false,
        grants_active: true,
        named_streams: false,
        date: "2026-10-17".into(),
    };
    let entry = SourceEntry {
        name: "a.bin".into(),
        kind: Kind::File,
        identity: id(5),
        size: 1,
        creation_ft: 1,
        last_write_ft: 1,
        change_ft: 1,
        attributes: 0x20,
        reparse_tag: None,
        ea_size: 0,
        streams: vec![],
        link_count: 1,
        explicit_aces: false,
        verified_volume: true,
    };
    let p = preview(vec![entry], &dest, &ctx);
    approve(&p, p.manifest.digest(), 0, 0)
        .unwrap()
        .admit(&p, 0, 1)
        .unwrap()
}

fn intent() -> Record {
    Record::StagingIntent {
        token: TOKEN.into(),
    }
}

fn created() -> Record {
    Record::StagingCreated {
        id: ObjectId {
            file_id: [100; 16],
            creation_ft: 1_100,
        },
    }
}

fn code(e: AppendError) -> ErrorCode {
    match e {
        AppendError::Sql(rusqlite::Error::SqliteFailure(f, _)) => f.code,
        other => panic!("not a SQLite failure: {other:?}"),
    }
}

/// The journal replays (head and tail agree) and holds exactly `rows` rows.
fn assert_intact(store: &Store, op: u64, rows: usize) {
    assert_eq!(store.rows(op).unwrap().len(), rows, "rows stored");
    store
        .replay(op)
        .unwrap()
        .unwrap_or_else(|e| panic!("journal no longer replays: {e:?}"));
}

/// Another connection holds the write lock past the busy timeout: SQLITE_BUSY, nothing stored, and the
/// same record appends once the lock is released.
#[test]
fn a_real_busy_append_stores_nothing_and_succeeds_after_the_lock_clears() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let mut store = Store::open(&path).unwrap();
    let op = store.start(1, admitted()).unwrap();
    let locker = Connection::open(&path).unwrap();
    locker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let e = store.append(op, 1, intent()).unwrap_err();
    assert_eq!(code(e), ErrorCode::DatabaseBusy);
    locker.execute_batch("ROLLBACK").unwrap();
    assert_intact(&store, op, 1);
    store.append(op, 1, intent()).unwrap();
    assert_intact(&store, op, 2);
}

/// The database cannot grow: SQLITE_FULL on some append, that append stores neither row nor head, every
/// earlier record is intact, and once there is room the journal carries on from where it stopped.
#[test]
fn a_real_full_database_rejects_an_append_whole_and_keeps_the_journal_intact() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let op = {
        let mut store = Store::open(&path).unwrap();
        let op = store.start(1, admitted()).unwrap();
        store.append(op, 1, intent()).unwrap();
        store.append(op, 1, created()).unwrap();
        op
    };
    let capped = Connection::open(&path).unwrap();
    let pages: i64 = capped
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    capped
        .query_row(&format!("PRAGMA max_page_count={pages}"), [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
    let mut store = Store::new(capped).unwrap();
    // A retry of the open batch is a legal repeat, so a fat record can be appended until the cap bites.
    let fat = || Record::CreateBatch {
        items: vec![(0, "n".repeat(1000))],
    };
    let mut stored = 3;
    let full = loop {
        assert!(stored < 5_000, "the cap never bit");
        match store.append(op, 1, fat()) {
            Ok(_) => stored += 1,
            Err(e) => break e,
        }
    };
    assert_eq!(code(full), ErrorCode::DiskFull);
    assert_intact(&store, op, stored);
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_intact(&store, op, stored);
    store.append(op, 1, fat()).unwrap();
    assert_intact(&store, op, stored + 1);
}

#[cfg(feature = "fault-injection")]
mod injected {
    use super::*;
    use loomward_operations::store::fault::{Kind as Fail, Stage};

    fn failing(at: Stage, kind: Fail) -> store::fault::Hook {
        Box::new(move |_, stage| (stage == at).then_some(kind))
    }

    /// Every stage and kind: the error is the one injected; Row and Head leave no trace (the head anchor
    /// and the tail still agree), Ack keeps the durable record the caller was told failed; either way the
    /// next append continues the chain.
    #[test]
    fn every_stage_and_kind_leaves_a_journal_that_replays_and_continues() {
        for kind in [Fail::Full, Fail::Busy] {
            for (stage, stored) in [(Stage::Row, 1), (Stage::Head, 1), (Stage::Ack, 2)] {
                let dir = tempfile::tempdir().unwrap();
                let mut store = Store::open(&dir.path().join("state.db")).unwrap();
                let op = store.start(1, admitted()).unwrap();
                store.set_fault(Some(failing(stage, kind)));
                let e = store.append(op, 1, intent()).unwrap_err();
                let want = match kind {
                    Fail::Full => ErrorCode::DiskFull,
                    Fail::Busy => ErrorCode::DatabaseBusy,
                };
                assert_eq!(code(e), want, "{stage:?} {kind:?}");
                assert_intact(&store, op, stored);
                store.set_fault(None);
                let next = if stored == 1 { intent() } else { created() };
                store.append(op, 1, next).unwrap();
                assert_intact(&store, op, stored + 1);
            }
        }
    }

    /// The hook sees each record at each stage, in order, and decides per call.
    #[test]
    fn the_hook_sees_every_stage_of_every_append() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("state.db")).unwrap();
        let op = store.start(1, admitted()).unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        store.set_fault(Some(Box::new(move |_, stage| {
            log.lock().unwrap().push(stage);
            None
        })));
        store.append(op, 1, intent()).unwrap();
        assert_eq!(*seen.lock().unwrap(), [Stage::Row, Stage::Head, Stage::Ack]);
    }
}
