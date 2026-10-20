//! #738: real SQLite coverage of terminal metadata, including competing Stores and commit faults.
use super::*;
use crate::{FenceCause, ObjectId, OpState};

fn publishing(path: &Path) -> Store {
    let mut store = Store::open(path).unwrap();
    let row = Replay::new(1).row_for(&Entry {
        epoch: 1,
        record: Record::Started {
            digest: [1; 32],
            items: 1,
        },
    });
    insert_row(&store.conn, 1, &row).unwrap();
    insert_head(&store.conn, 1, &row).unwrap();
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
        store.append(1, 1, record).unwrap();
    }
    store
}

fn committed(path: &Path) -> Store {
    let mut store = publishing(path);
    store.append(1, 1, Record::Committed).unwrap();
    store
}

#[test]
fn terminal_annotation_survives_reopen_without_new_effect_authority() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let mut store = committed(&path);
    let before = store.replay(1).unwrap().unwrap();
    assert_eq!(
        store.annotate_pending_fence(1, 1, FenceCause::GrantRevoked).unwrap(),
        FenceAnnotation::Recorded
    );
    assert_eq!(
        store.annotate_pending_fence(1, 1, FenceCause::GrantRevoked).unwrap(),
        FenceAnnotation::AlreadyPresent
    );
    assert_eq!(store.rows(1).unwrap().len(), before.seq as usize + 1);
    let ran = std::cell::Cell::new(false);
    let effect = store.while_current(1, OpState::Publishing, 1, || ran.set(true)).unwrap().unwrap();
    assert_eq!(effect, None);
    assert!(!ran.get());
    drop(store);
    let after = Store::open(&path).unwrap().replay(1).unwrap().unwrap();
    assert_eq!(after.state, before.state);
    assert_eq!(after.epoch, before.epoch);
    assert_eq!(after.fenced, before.fenced);
    assert_eq!(after.cancelled, before.cancelled);
    assert_eq!(after.items, before.items);
    assert_eq!(after.staging, before.staging);
    assert_eq!(after.publish, before.publish);
    assert!(after.has_fence_cause(FenceCause::GrantRevoked));
}

#[test]
fn two_stores_record_one_terminal_annotation_for_the_same_cause() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let mut first = committed(&path);
    let before = first.rows(1).unwrap().len();
    let mut second = Store::open(&path).unwrap();
    let go = std::sync::Barrier::new(2);
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            go.wait();
            first.annotate_pending_fence(1, 1, FenceCause::GrantRevoked).unwrap()
        });
        let b = scope.spawn(|| {
            go.wait();
            second.annotate_pending_fence(1, 1, FenceCause::GrantRevoked).unwrap()
        });
        (a.join().unwrap(), b.join().unwrap())
    });
    assert!(matches!(
        (a, b),
        (FenceAnnotation::Recorded, FenceAnnotation::AlreadyPresent)
            | (FenceAnnotation::AlreadyPresent, FenceAnnotation::Recorded)
    ));
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.rows(1).unwrap().len(), before + 1);
    assert_eq!(reopened.replay(1).unwrap().unwrap().state, OpState::Committed);
}

#[test]
fn an_existing_fence_deduplicates_only_its_own_cause() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = publishing(&dir.path().join("state.db"));
    store.append(1, 2, Record::Fenced { cause: FenceCause::GrantRevoked }).unwrap();
    store.append(1, 1, Record::Committed).unwrap();
    let before = store.rows(1).unwrap().len();
    assert_eq!(
        store.annotate_pending_fence(1, 1, FenceCause::GrantRevoked).unwrap(),
        FenceAnnotation::AlreadyPresent
    );
    assert_eq!(
        store.annotate_pending_fence(1, 1, FenceCause::EmergencyStop).unwrap(),
        FenceAnnotation::Recorded
    );
    let after = store.replay(1).unwrap().unwrap();
    assert_eq!(after.epoch, 2);
    assert!(after.fenced);
    assert!(after.has_fence_cause(FenceCause::GrantRevoked));
    assert!(after.has_fence_cause(FenceCause::EmergencyStop));
    assert_eq!(store.rows(1).unwrap().len(), before + 1);
}

#[test]
fn terminal_annotation_row_and_head_errors_roll_back_together() {
    for sql in [
        "CREATE TRIGGER fail_annotation BEFORE INSERT ON op_journal
         BEGIN SELECT RAISE(ABORT,'row failure'); END;",
        "CREATE TRIGGER fail_annotation BEFORE UPDATE ON op_journal_head
         BEGIN SELECT RAISE(ABORT,'head failure'); END;",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = committed(&path);
        let before = store.rows(1).unwrap();
        store.conn.execute_batch(sql).unwrap();
        assert!(store.annotate_pending_fence(1, 1, FenceCause::GrantRevoked).is_err());
        assert_eq!(store.rows(1).unwrap(), before);
        assert_eq!(store.replay(1).unwrap().unwrap().seq, before.len() as u64);
        store.conn.execute_batch("DROP TRIGGER fail_annotation").unwrap();
        assert_eq!(
            store.annotate_pending_fence(1, 1, FenceCause::GrantRevoked).unwrap(),
            FenceAnnotation::Recorded
        );
    }
}

#[test]
fn terminal_annotation_refuses_damaged_history_even_with_a_warm_cache() {
    for damage in [
        "DELETE FROM op_journal_head WHERE op_id=1;",
        "DROP TRIGGER op_journal_no_delete; DELETE FROM op_journal WHERE op_id=1 AND seq=9;",
        "DROP TRIGGER op_journal_no_update; UPDATE op_journal SET body='{}' WHERE op_id=1 AND seq=2;",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = committed(&dir.path().join("state.db"));
        assert_eq!(store.state_epoch(1).unwrap().unwrap().0, OpState::Committed);
        store.conn.execute_batch(damage).unwrap();
        let damaged = store.rows(1).unwrap();
        assert!(matches!(
            store.annotate_pending_fence(1, 1, FenceCause::GrantRevoked),
            Err(AppendError::Rejected(_))
        ));
        assert_eq!(store.rows(1).unwrap(), damaged);
    }
}

#[test]
fn terminal_annotation_requires_a_committed_receipt_and_an_observed_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = publishing(&dir.path().join("state.db"));
    let before = store.rows(1).unwrap();
    assert_eq!(
        store.annotate_pending_fence(1, 1, FenceCause::GrantRevoked).unwrap(),
        FenceAnnotation::Ineligible
    );
    assert_eq!(store.rows(1).unwrap(), before);
    store.append(1, 1, Record::Committed).unwrap();
    for epoch in [0, 2] {
        assert_eq!(
            store.annotate_pending_fence(1, epoch, FenceCause::GrantRevoked).unwrap(),
            FenceAnnotation::Ineligible
        );
    }
    assert_eq!(store.rows(1).unwrap().len(), before.len() + 1);
}

#[cfg(feature = "fault-injection")]
#[test]
fn a_lost_terminal_annotation_acknowledgement_deduplicates_on_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let mut store = committed(&path);
    let before = store.rows(1).unwrap().len();
    store.set_fault(Some(Box::new(|record, stage| {
        (matches!(record, Record::RevokedAfterPublish { .. }) && stage == fault::Stage::Ack)
            .then_some(fault::Kind::Full)
    })));
    assert!(store.annotate_pending_fence(1, 1, FenceCause::GrantRevoked).is_err());
    drop(store);
    let mut reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened.annotate_pending_fence(1, 1, FenceCause::GrantRevoked).unwrap(),
        FenceAnnotation::AlreadyPresent
    );
    assert_eq!(reopened.rows(1).unwrap().len(), before + 1);
}

#[test]
fn terminal_annotation_process_exit_child() {
    let Some(path) = std::env::var_os("LOOMWARD_TERMINAL_ANNOTATION_CHILD_PATH") else {
        return;
    };
    let mut store = Store::open(Path::new(&path)).unwrap();
    if std::env::var("LOOMWARD_TERMINAL_ANNOTATION_CHILD_PHASE").unwrap() == "after" {
        assert_eq!(
            store.annotate_pending_fence(1, 1, FenceCause::GrantRevoked).unwrap(),
            FenceAnnotation::Recorded
        );
    }
    // Exit without running destructors. This is process-death evidence, never power-loss proof.
    std::process::exit(87);
}

#[test]
fn terminal_annotation_reopens_honestly_after_process_exit_on_either_side() {
    for phase in ["before", "after"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let store = committed(&path);
        let before = store.rows(1).unwrap().len();
        drop(store);
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "store::terminal_tests::terminal_annotation_process_exit_child",
                "--nocapture",
            ])
            .env("LOOMWARD_TERMINAL_ANNOTATION_CHILD_PATH", &path)
            .env("LOOMWARD_TERMINAL_ANNOTATION_CHILD_PHASE", phase)
            .status()
            .unwrap();
        assert_eq!(child.code(), Some(87), "child did not reach the requested boundary");
        let mut reopened = Store::open(&path).unwrap();
        let r = reopened.replay(1).unwrap().unwrap();
        assert_eq!(r.state, OpState::Committed);
        assert_eq!(r.epoch, 1);
        assert_eq!(r.items.iter().filter(|item| item.verified).count(), 1);
        assert_eq!(r.has_fence_cause(FenceCause::GrantRevoked), phase == "after");
        assert_eq!(reopened.rows(1).unwrap().len(), before + usize::from(phase == "after"));
        let ran = std::cell::Cell::new(false);
        assert_eq!(
            reopened
                .while_current(1, OpState::Publishing, 1, || ran.set(true))
                .unwrap()
                .unwrap(),
            None
        );
        assert!(!ran.get());
    }
}
