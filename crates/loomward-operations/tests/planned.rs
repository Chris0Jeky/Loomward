//! Lane C5 additions to the C3 journal (#506): planned creation times journaled before metadata
//! preservation changes an identity, and the Q1 default "leave the staging to the owner" outcome.
use loomward_operations::*;
use std::collections::HashMap;

const OP: u64 = 3;
const TOKEN: &str = "lw-staging-c5";
const FINAL: &str = "Copy";

fn id(n: u8, ft: i64) -> ObjectId {
    ObjectId {
        file_id: [n; 16],
        creation_ft: ft,
    }
}

struct J(Replay, Vec<Row>);
impl J {
    fn new() -> J {
        J(Replay::new(OP), Vec::new())
    }
    fn try_push(&mut self, epoch: u64, record: Record) -> Result<(), Invalid> {
        let row = self.0.row_for(&Entry { epoch, record });
        let mut next = self.0.clone();
        next.apply(&row)?;
        self.0 = next;
        self.1.push(row);
        Ok(())
    }
    fn push(&mut self, record: Record) -> &mut J {
        let shown = format!("{record:?}");
        self.try_push(1, record)
            .unwrap_or_else(|e| panic!("{shown} rejected: {e:?}"));
        self
    }
}

#[derive(Default)]
struct Disk(HashMap<String, ObjectId>, HashMap<String, Observed>);
impl loomward_operations::Disk for Disk {
    fn in_parent(&self, name: &str) -> Option<ObjectId> {
        self.0.get(name).copied()
    }
    fn in_staging(&self, _: ObjectId, name: &str) -> Option<Observed> {
        self.1.get(name).copied()
    }
}

/// Staging created, one file created and identity-committed, then its creation time planned.
fn planned() -> J {
    let mut j = J::new();
    j.push(Record::Started {
        digest: [1; 32],
        items: 1,
    })
    .push(Record::StagingIntent {
        token: TOKEN.into(),
    })
    .push(Record::StagingCreated { id: id(9, 10) })
    .push(Record::CreateBatch {
        items: vec![(0, "f".into())],
    })
    .push(Record::Identities {
        items: vec![(0, id(1, 20))],
    })
    .push(Record::Planned {
        items: vec![(0, 777)],
        staging: Some(888),
    });
    j
}

fn disk(staging_ft: i64, file_ft: i64) -> Disk {
    let mut d = Disk::default();
    d.0.insert(TOKEN.into(), id(9, staging_ft));
    d.1.insert(
        "f".into(),
        Observed {
            id: id(1, file_ft),
            links: 1,
        },
    );
    d
}

#[test]
fn identity_matches_original_or_planned_creation_time_only() {
    let original = id(1, 20);
    assert!(original.matches(&id(1, 20), None));
    assert!(original.matches(&id(1, 777), Some(777)));
    assert!(!original.matches(&id(1, 777), None));
    assert!(!original.matches(&id(1, 5), Some(777)));
    assert!(!original.matches(&id(2, 20), Some(777)));
}

#[test]
fn recovery_attaches_with_either_creation_time_and_refuses_any_other() {
    let j = planned();
    for (s, f) in [(10, 20), (888, 777), (888, 20), (10, 777)] {
        let r = recover(OP, &j.1, &disk(s, f));
        let Decision::Resume(resume) = r.decision else {
            panic!("{s} {f}: {:?}", r.decision)
        };
        assert_eq!(resume.reverify.len(), 1, "{s} {f}");
        assert_eq!(resume.reverify[0].planned_ft, Some(777));
    }
    // Same name, other creation time: someone else's object, never adopted or removed.
    let r = recover(OP, &j.1, &disk(10, 21));
    assert_eq!(
        r.decision,
        Decision::NeedsAttention(Attention::EntryMismatch(vec![0]))
    );
    let r = recover(OP, &j.1, &disk(11, 20));
    assert_eq!(
        r.decision,
        Decision::NeedsAttention(Attention::StagingMismatch)
    );
}

#[test]
fn published_staging_with_planned_creation_time_is_confirmed() {
    let mut j = planned();
    j.push(Record::Verified { items: vec![0] })
        .push(Record::Transition {
            to: OpState::Staged,
            code: None,
        })
        .push(Record::PublishIntent {
            final_name: FINAL.into(),
        });
    let mut d = Disk::default();
    d.0.insert(FINAL.into(), id(9, 888));
    assert_eq!(
        recover(OP, &j.1, &d).decision,
        Decision::Confirm {
            staging: id(9, 10),
            final_name: FINAL.into()
        }
    );
}

#[test]
fn planned_needs_copying_an_identity_and_an_open_run() {
    let mut j = planned();
    // Unknown item, or an item verified already.
    assert!(j
        .try_push(
            1,
            Record::Planned {
                items: vec![(5, 1)],
                staging: None
            }
        )
        .is_err());
    assert!(j
        .try_push(
            1,
            Record::Planned {
                items: vec![],
                staging: None
            }
        )
        .is_err());
    j.push(Record::Verified { items: vec![0] });
    assert!(j
        .try_push(
            1,
            Record::Planned {
                items: vec![(0, 1)],
                staging: None
            }
        )
        .is_err());
    // A fenced run never advances, so it never plans either.
    let mut f = planned();
    f.try_push(
        2,
        Record::Fenced {
            cause: FenceCause::GrantRevoked,
        },
    )
    .unwrap();
    assert!(f
        .try_push(
            2,
            Record::Planned {
                items: vec![],
                staging: Some(1)
            }
        )
        .is_err());
}

#[test]
fn after_a_create_failure_the_identity_commit_covers_only_what_was_created() {
    let mut j = J::new();
    j.push(Record::Started {
        digest: [1; 32],
        items: 3,
    })
    .push(Record::StagingIntent {
        token: TOKEN.into(),
    })
    .push(Record::StagingCreated { id: id(9, 10) })
    .push(Record::CreateBatch {
        items: vec![(0, "a".into()), (1, "b".into()), (2, "c".into())],
    });
    // While copying, the commit must still cover the batch exactly.
    assert!(j
        .try_push(
            1,
            Record::Identities {
                items: vec![(0, id(1, 1))]
            }
        )
        .is_err());
    j.push(Record::ItemFailed {
        item: 1,
        code: Refusal::StagingNameTaken,
    });
    // Never the failed item, never twice, never outside the batch.
    for bad in [
        vec![(0, id(1, 1)), (1, id(2, 2))],
        vec![(0, id(1, 1)), (0, id(3, 3))],
        vec![(5, id(1, 1))],
    ] {
        assert!(j.try_push(1, Record::Identities { items: bad }).is_err());
    }
    j.push(Record::Identities {
        items: vec![(0, id(1, 1))],
    });
    assert!(j.0.open_batch.is_empty());
    assert_eq!(j.0.items[2].name.as_deref(), Some("c"));
    assert!(j.0.items[2].id.is_none());
}

#[test]
fn staging_left_ends_a_cancel_without_removal_and_ends_cleanup_authority() {
    let mut j = planned();
    // Not before cancelling.
    assert!(j
        .try_push(1, Record::StagingLeft { id: id(9, 10) })
        .is_err());
    j.push(Record::Transition {
        to: OpState::Cancelling,
        code: None,
    });
    // Not for another identity.
    assert!(j
        .try_push(1, Record::StagingLeft { id: id(9, 11) })
        .is_err());
    // Without it, cancelled is refused while journaled staging remains.
    assert!(j
        .try_push(
            1,
            Record::Transition {
                to: OpState::Cancelled,
                code: None
            }
        )
        .is_err());
    j.push(Record::StagingLeft { id: id(9, 10) });
    // A crash here: recovery has nothing left to remove.
    let r = recover(OP, &j.1, &disk(10, 20));
    assert_eq!(
        r.decision,
        Decision::Cleanup(Cleanup {
            staging: Some(id(9, 10)),
            ..Cleanup::default()
        })
    );
    for record in [
        Record::ItemRemoved {
            item: 0,
            id: id(1, 20),
        },
        Record::StagingRemoved { id: id(9, 10) },
        Record::StagingLeft { id: id(9, 10) },
    ] {
        assert!(j.try_push(1, record).is_err());
    }
    j.push(Record::Transition {
        to: OpState::Cancelled,
        code: None,
    });
    assert_eq!(j.0.state, OpState::Cancelled);
    assert_eq!(
        recover(OP, &j.1, &Disk::default()).decision,
        Decision::Settled
    );
}
