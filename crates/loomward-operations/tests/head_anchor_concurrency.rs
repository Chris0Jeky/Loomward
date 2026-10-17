//! Lane store-569 (#569 residual): a second `Store` on the same `state.db` while the first appends to the same
//! operation, the shape a detached copy worker leaves behind after `Copies::drop` stops waiting. Real SQLite,
//! bounded waits only (fixed iteration counts and the store's own 5 s busy timeout). Every outcome must be one of:
//! the second store reads a consistent journal (contiguous, replays, head equals tail), or a diverging append is
//! refused. Never a forked or silently truncated chain.
use loomward_operations::journal::{Invalid, ObjectId};
use loomward_operations::recovery::Decision;
use loomward_operations::tree::Kind;
use loomward_operations::*;
use rusqlite::Connection;
use std::thread;

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

/// A retry of the open batch is a legal repeat in `copying`, so it can be appended any number of times.
fn batch() -> Record {
    Record::CreateBatch {
        items: vec![(0, "n".into())],
    }
}

/// Recovery reads the journal alone: nothing is on disk.
struct EmptyDisk;

impl Disk for EmptyDisk {
    fn in_parent(&self, _name: &str) -> Option<ObjectId> {
        None
    }
    fn in_staging(&self, _staging: ObjectId, _name: &str) -> Option<Observed> {
        None
    }
}

/// The stored rows form one linear chain (seq 1..=n, no gap, duplicate or reorder) and the journal replays,
/// which checks the hash chain and the head anchor. Returns the row count.
fn assert_linear(store: &Store, op: u64) -> usize {
    let rows = store.rows(op).unwrap();
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(
            row.seq,
            i as u64 + 1,
            "gap, duplicate or reorder at row {i}"
        );
    }
    if let Err(e) = store.replay(op).unwrap() {
        panic!("journal does not replay: {e:?}");
    }
    rows.len()
}

/// A reader opened while a writer appends to the same operation sees a consistent journal at every
/// observation: the head and tail are read in one snapshot, so a half-committed append is never visible.
#[test]
fn a_second_store_reads_a_consistent_journal_while_the_first_appends() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let mut writer = Store::open(&path).unwrap();
    let op = writer.start(1, admitted()).unwrap();
    writer.append(op, 1, intent()).unwrap();
    writer.append(op, 1, created()).unwrap();
    const APPENDS: usize = 200;
    let appender = thread::spawn(move || {
        for _ in 0..APPENDS {
            writer
                .append(op, 1, batch())
                .expect("the writer's append succeeds while a reader is open");
        }
    });
    let reader = Store::open(&path).unwrap();
    let mut last = 0;
    for _ in 0..200 {
        let n = assert_linear(&reader, op);
        assert!(n >= last, "the journal shrank from {last} to {n} rows");
        last = n;
    }
    appender.join().unwrap();
    assert_eq!(assert_linear(&reader, op), 3 + APPENDS);
    let recovered = reader.recover(op, &EmptyDisk).unwrap();
    assert_eq!(recovered.state, Some(loomward_operations::OpState::Copying));
}

/// Two stores on one operation append alternately from two threads. Each append validates against the
/// journal as it stands under the write lock, so the result is one linear chain: no fork, no lost record.
#[test]
fn two_stores_appending_to_one_operation_make_one_linear_chain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let mut first = Store::open(&path).unwrap();
    let op = first.start(1, admitted()).unwrap();
    first.append(op, 1, intent()).unwrap();
    first.append(op, 1, created()).unwrap();
    let mut second = Store::open(&path).unwrap();
    const EACH: usize = 100;
    let a = thread::spawn(move || {
        for _ in 0..EACH {
            first.append(op, 1, batch()).expect("first store append");
        }
    });
    let b = thread::spawn(move || {
        for _ in 0..EACH {
            second.append(op, 1, batch()).expect("second store append");
        }
    });
    a.join().unwrap();
    b.join().unwrap();
    let check = Store::open(&path).unwrap();
    assert_eq!(assert_linear(&check, op), 3 + 2 * EACH);
}

/// A stale store (its cached replay is one row behind) has a record that its own view would accept. A second
/// store fences the run in between. The stale append is refused by the journal as it now stands, so nothing
/// forks; a record at the fenced epoch is refused too.
#[test]
fn a_stale_store_append_is_refused_once_a_second_store_fences_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let mut stale = Store::open(&path).unwrap();
    let op = stale.start(1, admitted()).unwrap();
    stale.append(op, 1, intent()).unwrap();
    let mut revoker = Store::open(&path).unwrap();
    revoker
        .append(
            op,
            2,
            Record::Fenced {
                cause: FenceCause::GrantRevoked,
            },
        )
        .unwrap();
    match stale.append(op, 1, created()) {
        Err(AppendError::Rejected(Invalid { why, .. })) => assert_eq!(why, "stale epoch"),
        other => panic!("stale append not refused as stale: {other:?}"),
    }
    assert_eq!(
        assert_linear(&stale, op),
        3,
        "refused append stored nothing"
    );
    match stale.append(op, 2, created()) {
        Err(AppendError::Rejected(Invalid { why, .. })) => {
            assert_eq!(why, "record advances a fenced run")
        }
        other => panic!("append at the fenced epoch not refused: {other:?}"),
    }
    assert_eq!(
        assert_linear(&revoker, op),
        3,
        "still no record past the fence"
    );
}

/// A tail row lost behind the head anchor (the append-only triggers are dropped here to simulate damage
/// outside the store). The writer refuses to extend a journal whose head and tail disagree, and recovery
/// reports it as an invalid journal rather than resuming from the surviving prefix.
#[test]
fn a_tail_lost_behind_the_head_is_refused_by_the_writer_and_flagged_by_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let mut store = Store::open(&path).unwrap();
    let op = store.start(1, admitted()).unwrap();
    store.append(op, 1, intent()).unwrap();
    store.append(op, 1, created()).unwrap();
    store.append(op, 1, batch()).unwrap();
    let raw = Connection::open(&path).unwrap();
    raw.execute_batch(&format!(
        "DROP TRIGGER op_journal_no_delete;
         DELETE FROM op_journal WHERE op_id={op} AND seq=4;"
    ))
    .unwrap();
    drop(raw);
    match store.append(op, 1, batch()) {
        Err(AppendError::Rejected(Invalid { why, .. })) => {
            assert_eq!(why, "journal head does not match tail")
        }
        other => panic!("append over a lost tail not refused: {other:?}"),
    }
    assert_eq!(
        store.rows(op).unwrap().len(),
        3,
        "the writer stored nothing"
    );
    let reopened = Store::open(&path).unwrap();
    assert!(
        reopened.replay(op).unwrap().is_err(),
        "a lost tail must not replay"
    );
    let recovered = reopened.recover(op, &EmptyDisk).unwrap();
    assert_eq!(recovered.state, None);
    assert!(
        matches!(
            recovered.decision,
            Decision::NeedsAttention(loomward_operations::recovery::Attention::InvalidJournal(_))
        ),
        "recovery must flag the lost tail, got {:?}",
        recovered.decision
    );
}
