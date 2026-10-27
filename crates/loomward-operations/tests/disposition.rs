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
    let epoch = r.epoch;
    r.apply(&r.row_for(&Entry { epoch, record })).unwrap();
}

fn at(r: &mut Replay, epoch: u64, record: Record) {
    r.apply(&r.row_for(&Entry { epoch, record })).unwrap();
}

fn cancel(r: &mut Replay) -> StagingDisposition {
    push(
        r,
        Record::Transition {
            to: OpState::Cancelling,
            code: None,
        },
    );
    r.staging_disposition().unwrap()
}

fn copying() -> Replay {
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
    ] {
        push(&mut r, record);
    }
    r
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
        "refusal_codes": [code], "stop_cause": code, "resumable_before_cancel": code.resumable(),
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
        ("stop_cause", json!("source_changed")),
        ("stop_cause", json!("emergency_stop")),
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

#[test]
fn a_fence_is_explained_by_its_own_cause_not_by_a_revoked_grant() {
    for (cause, stop, named, other) in [
        (
            FenceCause::EmergencyStop,
            StopCause::Other(StopKind::EmergencyStop),
            "emergency_stop",
            "grant_revoked",
        ),
        (
            FenceCause::GrantRevoked,
            StopCause::Refusal(Refusal::GrantRevoked),
            "grant_revoked",
            "emergency_stop",
        ),
    ] {
        let mut r = copying();
        at(&mut r, 1, Record::Fenced { cause });
        let facts = cancel(&mut r);
        assert_eq!(facts.stop_cause, stop);
        assert!(
            facts.refusal_codes.is_empty(),
            "a fence is not an item refusal"
        );
        let text = facts.explanation();
        assert!(text.contains(&format!("stopping cause: {named}")), "{text}");
        assert!(!text.contains(other), "{text}");
        assert!(
            text.contains("Refusals journaled during the run: none."),
            "{text}"
        );
    }
}

#[test]
fn the_stopping_cause_is_the_active_stop_not_every_refusal_the_run_resumed_past() {
    let fail = |code| Record::ItemFailed { item: 1, code };
    let resume = || Record::Transition {
        to: OpState::Copying,
        code: None,
    };
    // Resumed past source_in_use, then stopped by source_changed.
    let mut r = copying();
    push(&mut r, fail(Refusal::SourceInUse));
    push(&mut r, resume());
    push(&mut r, fail(Refusal::SourceChanged));
    let facts = cancel(&mut r);
    assert_eq!(facts.stop_cause, StopCause::Refusal(Refusal::SourceChanged));
    assert_eq!(
        facts.refusal_codes,
        [Refusal::SourceInUse, Refusal::SourceChanged]
    );
    let text = facts.explanation();
    assert!(text.contains("stopping cause: source_changed."), "{text}");
    assert!(
        text.contains("Refusals journaled during the run: source_in_use, source_changed."),
        "{text}"
    );
    // Resumed past source_in_use, then cancelled by the owner: no refusal stopped the run.
    let mut r = copying();
    push(&mut r, fail(Refusal::SourceInUse));
    push(&mut r, resume());
    let facts = cancel(&mut r);
    assert_eq!(facts.stop_cause, StopCause::Other(StopKind::Cancelled));
    assert!(!facts.resumable_before_cancel);
    let text = facts.explanation();
    assert!(text.contains("stopping cause: cancelled"), "{text}");
    assert!(
        text.contains("Refusals journaled during the run: source_in_use."),
        "{text}"
    );
    // Still blocked on it at cancel: it is the stopping cause.
    let mut r = copying();
    push(&mut r, fail(Refusal::SourceInUse));
    let facts = cancel(&mut r);
    assert_eq!(facts.stop_cause, StopCause::Refusal(Refusal::SourceInUse));
}

#[test]
fn the_explanation_stays_bounded_with_every_refusal_code_present() {
    use Refusal::*;
    let all = vec![
        SourceInUse,
        AccessDenied,
        InsufficientSpace,
        StagingInUse,
        PublishInUse,
        SpecialFile,
        SourceChanged,
        CloudPlaceholder,
        ReparsePoint,
        UnsupportedAttribute,
        ExtendedAttributes,
        AlternateStreamsUnsupported,
        TooManyStreams,
        StagingNameTaken,
        VerificationMismatch,
        ChangedAfterVerification,
        GrantRevoked,
        DestinationChanged,
    ];
    let facts = StagingDisposition {
        staging: id(1),
        items_total: u32::MAX,
        items_verified: u32::MAX,
        items_unverified: u32::MAX,
        refusal_codes: all.clone(),
        stop_cause: StopCause::Refusal(AlternateStreamsUnsupported),
        resumable_before_cancel: true,
        resume_possible: false,
    };
    let text = facts.explanation();
    assert!(text.len() <= 2048, "{}", text.len());
    assert!(all.len() <= 32, "wire bound on refusal_codes");
    for code in all {
        let name = serde_json::to_value(code).unwrap();
        assert!(text.contains(name.as_str().unwrap()), "{name}");
    }
}

/// The contract examples are real `explanation()` output: drift in the text fails here.
#[test]
fn contract_example_explanations_are_rendered_by_explanation() {
    let dto: serde_json::Value = serde_json::from_str(include_str!(
        "../../../contracts/v3/examples/dtos/copy-disposition.json"
    ))
    .unwrap();
    let abandon: serde_json::Value = serde_json::from_str(include_str!(
        "../../../contracts/v3/examples/commands/copy.abandon.result.json"
    ))
    .unwrap();
    for wire in [&dto["value"], &abandon["disposition"]] {
        let mut facts = wire.clone();
        let shown = facts
            .as_object_mut()
            .unwrap()
            .remove("explanation")
            .unwrap();
        facts["staging"] = json!(id(1));
        let facts: StagingDisposition = serde_json::from_value(facts).unwrap();
        assert_eq!(shown, facts.explanation());
    }
}
