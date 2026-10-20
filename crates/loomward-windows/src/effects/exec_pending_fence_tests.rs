//! #738 metadata/admission regressions. Real SQLite and executor controls, with no native effects,
//! no laboratory/elevation early return, and no claim about Windows rename or content verification.
use super::*;
use loomward_operations::{Entry, FenceCause};

fn publishing(path: &Path) -> Mutex<Store> {
    publishing_with_refusal(path, None)
}

fn publishing_with_refusal(path: &Path, prior: Option<Refusal>) -> Mutex<Store> {
    let mut store = Store::open(path).unwrap();
    // Seed only the initial metadata row; this fixture never invokes an effect or an approval.
    let row = Replay::new(1).row_for(&Entry {
        epoch: 1,
        record: Record::Started {
            digest: [1; 32],
            items: 1,
        },
    });
    let raw = rusqlite::Connection::open(path).unwrap();
    raw.execute(
        "INSERT INTO op_journal(op_id,seq,body,chain) VALUES(1,1,?1,?2)",
        rusqlite::params![row.body, row.chain],
    )
    .unwrap();
    raw.execute(
        "INSERT INTO op_journal_head(op_id,seq,chain) VALUES(1,1,?1)",
        [&row.chain],
    )
    .unwrap();
    let id = |n| ObjectId {
        file_id: [n; 16],
        creation_ft: i64::from(n),
    };
    for record in [
        Record::StagingIntent {
            token: "lw-staging-738".into(),
        },
        Record::StagingCreated { id: id(1) },
        Record::CreateBatch {
            items: vec![(0, "fixture.bin".into())],
        },
        Record::Identities {
            items: vec![(0, id(2))],
        },
        Record::Verified { items: vec![0] },
        Record::Transition {
            to: OpState::Staged,
            code: None,
        },
        Record::PublishIntent {
            final_name: "Loomward copy".into(),
        },
    ] {
        if matches!(record, Record::Transition { to: OpState::Staged, .. }) {
            if let Some(code) = prior {
                store
                    .append(
                        1,
                        1,
                        Record::Transition {
                            to: OpState::Blocked,
                            code: Some(code),
                        },
                    )
                    .unwrap();
                store
                    .append(
                        1,
                        1,
                        Record::Transition {
                            to: OpState::Copying,
                            code: None,
                        },
                    )
                    .unwrap();
            }
        }
        store.append(1, 1, record).unwrap();
    }
    Mutex::new(store)
}

fn pending(store: &Mutex<Store>, control: &Control) {
    let step = control.admit().ok().unwrap();
    assert_eq!(
        fence_waiting(store, 1, control, FenceCause::GrantRevoked, Duration::ZERO).unwrap(),
        Ack::PendingInFlightStep
    );
    drop(step);
}

#[test]
fn a_foreign_commit_preserves_the_resident_pending_annotation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = publishing(&path);
    let control = Control::new(false);
    pending(&store, &control);
    Store::open(&path)
        .unwrap()
        .append(1, 1, Record::Committed)
        .unwrap();
    let before = status(&store, 1).unwrap();
    journal_fence(&store, 1, &control).unwrap();
    let reopened = Mutex::new(Store::open(&path).unwrap());
    let after = status(&reopened, 1).unwrap();
    assert_eq!(after.state, OpState::Committed);
    assert_eq!(after.items_verified, before.items_verified);
    assert_eq!(after.refusal, Some((Refusal::GrantRevoked, None)));
    assert!(matches!(control.admit(), Err(Stop::Cancel)));
}

#[test]
fn a_later_terminal_request_does_not_replace_the_eligible_pending_cause() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = publishing(&path);
    let control = Control::new(false);
    pending(&store, &control);
    Store::open(&path)
        .unwrap()
        .append(1, 1, Record::Committed)
        .unwrap();
    fence_waiting(&store, 1, &control, FenceCause::EmergencyStop, Duration::ZERO).unwrap();
    journal_fence(&store, 1, &control).unwrap();
    let rows = store.lock().unwrap().rows(1).unwrap();
    let annotations: Vec<_> = rows
        .iter()
        .map(|row| serde_json::from_str::<serde_json::Value>(&row.body).unwrap())
        .filter(|record| record["kind"] == "revoked_after_publish")
        .collect();
    assert_eq!(annotations.len(), 1);
    assert_eq!(annotations[0]["cause"], "grant_revoked");
}

#[test]
fn a_new_terminal_revoke_does_not_invent_a_pending_request() {
    let dir = tempfile::tempdir().unwrap();
    let store = publishing(&dir.path().join("state.db"));
    store.lock().unwrap().append(1, 1, Record::Committed).unwrap();
    let before = store.lock().unwrap().rows(1).unwrap();
    let control = Control::new(false);
    assert_eq!(
        fence_waiting(&store, 1, &control, FenceCause::GrantRevoked, Duration::ZERO).unwrap(),
        Ack::Settled
    );
    journal_fence(&store, 1, &control).unwrap();
    assert_eq!(store.lock().unwrap().rows(1).unwrap(), before);
    assert_eq!(status(&store, 1).unwrap().refusal, None);
}

#[test]
fn a_terminal_winner_cannot_acknowledge_an_admitted_resident_step() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = publishing(&path);
    let control = Control::new(false);
    let step = control.admit().ok().unwrap();
    Store::open(&path)
        .unwrap()
        .append(1, 1, Record::Committed)
        .unwrap();
    assert_eq!(
        fence_waiting(&store, 1, &control, FenceCause::GrantRevoked, Duration::ZERO).unwrap(),
        Ack::PendingInFlightStep
    );
    assert!(
        store.try_lock().is_ok(),
        "Store must be free while the step is pending"
    );
    drop(step);
    assert_eq!(
        fence_waiting(&store, 1, &control, FenceCause::GrantRevoked, Duration::ZERO).unwrap(),
        Ack::Settled
    );
    assert_eq!(status(&store, 1).unwrap().refusal, None);
}

#[test]
fn a_terminal_annotation_write_failure_keeps_the_cause_until_a_durable_retry() {
    use loomward_operations::store::fault::{Kind, Stage};
    for stage in [Stage::Row, Stage::Head, Stage::Ack] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let store = publishing(&path);
        let control = Control::new(false);
        pending(&store, &control);
        Store::open(&path).unwrap().append(1, 1, Record::Committed).unwrap();
        let before = store.lock().unwrap().rows(1).unwrap().len();
        store.lock().unwrap().set_fault(Some(Box::new(move |record, at| {
            (matches!(record, Record::RevokedAfterPublish { .. }) && at == stage)
                .then_some(Kind::Full)
        })));
        assert!(journal_fence(&store, 1, &control).is_err(), "{stage:?}");
        assert_eq!(control.unjournaled.lock().unwrap().len(), 1);
        assert!(control.has_pending_fence());
        assert!(matches!(control.admit(), Err(Stop::Cancel)));
        let expected = before + usize::from(stage == Stage::Ack);
        assert_eq!(Store::open(&path).unwrap().replay(1).unwrap().unwrap().seq as usize, expected);
        store.lock().unwrap().set_fault(None);
        journal_fence(&store, 1, &control).unwrap();
        assert!(control.unjournaled.lock().unwrap().is_empty());
        assert!(!control.has_pending_fence());
        assert_eq!(store.lock().unwrap().rows(1).unwrap().len(), before + 1);
        let reopened = Mutex::new(Store::open(&path).unwrap());
        assert_eq!(status(&reopened, 1).unwrap().refusal, Some((Refusal::GrantRevoked, None)));
    }
}

#[test]
fn metadata_retry_waits_for_quiescence_without_creating_a_new_cause() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = publishing(&path);
    let control = Control::new(false);
    let step = control.admit().ok().unwrap();
    assert_eq!(
        fence_waiting(&store, 1, &control, FenceCause::EmergencyStop, Duration::ZERO).unwrap(),
        Ack::PendingInFlightStep
    );
    Store::open(&path).unwrap().append(1, 1, Record::Committed).unwrap();
    let before = store.lock().unwrap().rows(1).unwrap();
    assert!(!retry_pending_fence(&store, 1, &control).unwrap());
    assert!(store.try_lock().is_ok());
    assert_eq!(store.lock().unwrap().rows(1).unwrap(), before);
    {
        // A service reaper must be able to inspect lifetime state while journaling owns this lock.
        let _journaling = control.unjournaled.lock().unwrap();
        assert!(control.has_pending_fence());
    }
    drop(step);
    assert!(retry_pending_fence(&store, 1, &control).unwrap());
    assert!(!control.has_pending_fence());
    let r = replay(&store, 1).unwrap();
    assert!(r.has_fence_cause(FenceCause::EmergencyStop));
    assert!(!r.has_fence_cause(FenceCause::GrantRevoked));
    assert_eq!(r.state, OpState::Committed);
    assert_eq!(r.epoch, 1);
    let after = store.lock().unwrap().rows(1).unwrap();
    let fresh = Control::new(false);
    assert!(retry_pending_fence(&store, 1, &fresh).unwrap());
    assert!(!fresh.fenced());
    assert_eq!(store.lock().unwrap().rows(1).unwrap(), after);
}

#[test]
fn distinct_nonterminal_pending_causes_are_both_preserved_after_commit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = publishing(&path);
    let control = Control::new(false);
    let step = control.admit().ok().unwrap();
    for cause in [FenceCause::GrantRevoked, FenceCause::EmergencyStop, FenceCause::GrantRevoked] {
        assert_eq!(
            fence_waiting(&store, 1, &control, cause, Duration::ZERO).unwrap(),
            Ack::PendingInFlightStep
        );
    }
    drop(step);
    assert_eq!(control.unjournaled.lock().unwrap().len(), 2);
    Store::open(&path).unwrap().append(1, 1, Record::Committed).unwrap();
    let before = store.lock().unwrap().rows(1).unwrap().len();
    journal_fence(&store, 1, &control).unwrap();
    assert_eq!(store.lock().unwrap().rows(1).unwrap().len(), before + 2);
    let r = replay(&store, 1).unwrap();
    assert!(r.has_fence_cause(FenceCause::GrantRevoked));
    assert!(r.has_fence_cause(FenceCause::EmergencyStop));
    assert_eq!(r.epoch, 1);
    assert_eq!(r.state, OpState::Committed);
}

#[test]
fn terminal_annotation_does_not_replace_an_earlier_specific_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = publishing_with_refusal(&path, Some(Refusal::SourceInUse));
    let control = Control::new(false);
    pending(&store, &control);
    Store::open(&path).unwrap().append(1, 1, Record::Committed).unwrap();
    journal_fence(&store, 1, &control).unwrap();
    let outcome = status(&Mutex::new(Store::open(&path).unwrap()), 1).unwrap();
    assert_eq!(outcome.state, OpState::Committed);
    assert_eq!(outcome.items_verified, 1);
    assert_eq!(outcome.refusal, Some((Refusal::SourceInUse, None)));
    assert!(replay(&store, 1).unwrap().has_fence_cause(FenceCause::GrantRevoked));
}

#[test]
fn a_pending_cause_never_moves_to_another_operation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = publishing(&path);
    let control = Control::new(false);
    pending(&store, &control);
    let before = store.lock().unwrap().rows(1).unwrap();
    // A second valid operation is essential: a nonexistent operation would fail even if the
    // pending cause were incorrectly transplanted to any caller-supplied operation number.
    let raw = rusqlite::Connection::open(&path).unwrap();
    let mut second = Replay::new(2);
    for original in &before {
        let row = second.row_for(&serde_json::from_str::<Entry>(&original.body).unwrap());
        second.apply(&row).unwrap();
        raw.execute(
            "INSERT INTO op_journal(op_id,seq,body,chain) VALUES(2,?1,?2,?3)",
            rusqlite::params![row.seq as i64, row.body, row.chain],
        ).unwrap();
    }
    raw.execute(
        "INSERT INTO op_journal_head(op_id,seq,chain) VALUES(2,?1,?2)",
        rusqlite::params![second.seq as i64, second.chain.to_vec()],
    ).unwrap();
    let other_before = store.lock().unwrap().rows(2).unwrap();
    assert!(journal_fence(&store, 2, &control).is_err());
    assert_eq!(store.lock().unwrap().rows(1).unwrap(), before);
    assert_eq!(store.lock().unwrap().rows(2).unwrap(), other_before);
    assert_eq!(control.unjournaled.lock().unwrap().len(), 1);
}

#[test]
fn a_foreign_equivalent_fence_is_not_duplicated_after_commit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = publishing(&path);
    let control = Control::new(false);
    pending(&store, &control);
    let mut foreign = Store::open(&path).unwrap();
    foreign.append(1, 2, Record::Fenced { cause: FenceCause::GrantRevoked }).unwrap();
    foreign.append(1, 1, Record::Committed).unwrap();
    let before = foreign.rows(1).unwrap();
    journal_fence(&store, 1, &control).unwrap();
    assert!(control.unjournaled.lock().unwrap().is_empty());
    assert_eq!(foreign.rows(1).unwrap(), before);
    assert_eq!(status(&store, 1).unwrap().refusal, Some((Refusal::GrantRevoked, None)));
}

#[test]
fn terminal_fence_wait_releases_store_until_the_resident_step_returns() {
    use std::sync::mpsc::{channel, RecvTimeoutError};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = publishing(&path);
    let control = Control::new(false);
    let (entered, ready) = channel();
    let (release, released) = channel();
    let (answer, received) = channel();
    std::thread::scope(|scope| {
        let store = &store;
        let control = &control;
        let worker = scope.spawn(move || {
            let _step = control.admit().ok().unwrap();
            entered.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            store.try_lock().is_ok()
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        Store::open(&path).unwrap().append(1, 1, Record::Committed).unwrap();
        scope.spawn(move || {
            answer.send(fence_waiting(
                store,
                1,
                control,
                FenceCause::GrantRevoked,
                Duration::from_secs(5),
            )).unwrap();
        });
        let until = std::time::Instant::now() + Duration::from_secs(3);
        let store_free = loop {
            if control.fenced() && store.try_lock().is_ok() {
                break true;
            }
            if std::time::Instant::now() >= until {
                break false;
            }
            std::thread::yield_now();
        };
        let early = received.recv_timeout(Duration::from_millis(25));
        release.send(()).unwrap();
        let worker_store_free = worker.join().unwrap();
        assert!(store_free, "terminal fence waited while holding Store");
        assert!(worker_store_free);
        assert!(matches!(early, Err(RecvTimeoutError::Timeout)), "{early:?}");
        assert_eq!(received.recv_timeout(Duration::from_secs(5)).unwrap().unwrap(), Ack::Settled);
    });
}
