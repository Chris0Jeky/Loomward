//! #738 metadata/admission regressions. Real SQLite and executor controls, with no native effects,
//! no laboratory/elevation early return, and no claim about Windows rename or content verification.
use super::*;
use loomward_operations::{Entry, FenceCause};

fn publishing(path: &Path) -> Mutex<Store> {
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
