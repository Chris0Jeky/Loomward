//! LW-146: bounded, replay-validated facts survive removal of unpublished staging.
use loomward_operations::*;
use serde_json::json;

fn id(n: u8) -> ObjectId {
    ObjectId {
        file_id: [n; 16],
        creation_ft: 42,
    }
}

fn push(r: &mut Replay, record: Record) {
    r.apply(&r.row_for(&Entry { epoch: 0, record })).unwrap();
}

fn stopped(code: Refusal) -> Replay {
    let mut r = Replay::new(1);
    for record in [
        Record::Started {
            digest: [0; 32],
            items: 2,
        },
        Record::StagingIntent {
            token: "lab-staging".into(),
        },
        Record::StagingCreated { id: id(1) },
        Record::CreateBatch {
            items: vec![(0, "a".into()), (1, "b".into())],
        },
        Record::Identities {
            items: vec![(0, id(2)), (1, id(3))],
        },
        Record::Verified { items: vec![0] },
        Record::ItemFailed { item: 1, code },
        Record::Transition {
            to: OpState::Cancelling,
            code: None,
        },
    ] {
        push(&mut r, record);
    }
    r
}

fn record(code: Refusal) -> serde_json::Value {
    json!({"kind": "staging_disposition", "facts": {
        "staging": id(1), "items_total": 2, "items_verified": 1, "items_unverified": 1,
        "refusal_codes": [code], "resumable_before_cancel": code.resumable(),
        "resume_possible": false
    }})
}

#[test]
fn disposition_is_a_bounded_replay_checked_record() {
    let mut r = stopped(Refusal::SourceInUse);
    let valid: Record =
        serde_json::from_value(record(Refusal::SourceInUse)).expect("LW-146 record kind");
    push(&mut r, valid.clone());
    assert!(
        r.apply(&r.row_for(&Entry {
            epoch: 0,
            record: valid
        }))
        .is_err(),
        "one snapshot only"
    );
}

#[test]
fn disposition_refuses_invented_counts_identity_codes_or_resume_authority() {
    for (field, value) in [
        ("items_verified", json!(2)),
        ("items_unverified", json!(0)),
        ("items_total", json!(3)),
        ("staging", json!(id(9))),
        ("refusal_codes", json!(["source_changed"])),
        ("refusal_codes", json!(vec!["source_in_use"; 200])),
        ("resume_possible", json!(true)),
        ("resumable_before_cancel", json!(false)),
    ] {
        let mut r = stopped(Refusal::SourceInUse);
        let mut value_record = record(Refusal::SourceInUse);
        value_record["facts"][field] = value;
        let record: Record = serde_json::from_value(value_record).expect("LW-146 record kind");
        assert!(
            r.apply(&r.row_for(&Entry { epoch: 0, record })).is_err(),
            "{field}"
        );
    }
}

#[test]
fn explanation_is_deterministic_distinguishes_stops_and_survives_cleanup() {
    let mut explanations = Vec::new();
    for code in [Refusal::SourceInUse, Refusal::SourceChanged] {
        let mut r = stopped(code);
        let facts = r.staging_disposition().unwrap();
        let text = facts.explanation();
        assert_eq!(text, facts.explanation());
        assert!(text.contains("1 of 2 items verified; 1 unverified"));
        assert!(text.contains("cannot resume"));
        assert!(text.contains("remain in state.db"));
        assert!(text.len() <= 2048);
        push(
            &mut r,
            Record::StagingDisposition {
                facts: facts.clone(),
            },
        );
        for (item, id) in [(0, id(2)), (1, id(3))] {
            push(&mut r, Record::ItemRemoved { item, id });
        }
        push(&mut r, Record::StagingRemoved { id: id(1) });
        push(
            &mut r,
            Record::Transition {
                to: OpState::Cancelled,
                code: None,
            },
        );
        assert_eq!(r.disposition.as_ref(), Some(&facts));
        assert!(r.items.iter().all(|i| !i.verified));
        explanations.push(text);
    }
    assert_ne!(explanations[0], explanations[1]);
    assert!(explanations[0].contains("marked the stopped run resumable"));
    assert!(explanations[1].contains("No resumable refusal was active"));
}

#[test]
fn disposition_requires_cancellation_and_an_unpublished_journaled_identity() {
    let mut r = Replay::new(2);
    for record in [
        Record::Started {
            digest: [0; 32],
            items: 1,
        },
        Record::StagingIntent {
            token: "lab-staging".into(),
        },
        Record::StagingCreated { id: id(1) },
        Record::CreateBatch {
            items: vec![(0, "a".into())],
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
            final_name: "copy".into(),
        },
        Record::Committed,
    ] {
        push(&mut r, record);
        if let Some(facts) = r.staging_disposition() {
            let mut check = r.clone();
            assert!(check
                .apply(&check.row_for(&Entry {
                    epoch: 0,
                    record: Record::StagingDisposition { facts }
                }))
                .is_err());
        }
    }
}
