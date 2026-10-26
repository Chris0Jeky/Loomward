use std::collections::BTreeSet;

use loomward_learn::labelling::{combine, votes, AbstainReason, Axis, Outcome, FUNCTIONS};
use loomward_learn::packet::validate_packet;
use serde_json::{json, Value};

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../fixtures/categoriser-v1/labelling-functions.json"
    ))
    .unwrap()
}

fn patched(base: &Value, patch: &Value) -> Value {
    let mut packet = base.clone();
    for (key, value) in patch.as_object().unwrap() {
        packet[key] = value.clone();
    }
    packet
}

#[test]
fn fixture_votes_outcomes_and_schema_labels() {
    let fixture = fixture();
    let schema: Value =
        serde_json::from_str(include_str!("../../../schemas/categoriser-record-v1.json")).unwrap();
    validate_packet(&fixture["base"]).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let packet = patched(&fixture["base"], &case["patch"]);
        validate_packet(&packet).unwrap_or_else(|error| panic!("{name}: {error}"));
        let actual_votes = votes(&packet).unwrap();
        assert!(
            actual_votes.windows(2).all(|pair| {
                (pair[0].axis, pair[0].function) < (pair[1].axis, pair[1].function)
            }),
            "{name}: vote ordering"
        );
        let mut ids: Vec<_> = actual_votes.iter().map(|vote| vote.function).collect();
        ids.sort_unstable();
        assert_eq!(json!(ids), case["votes"], "{name}: votes");

        let outcomes = combine(&packet).unwrap();
        for ((actual_axis, outcome), (axis, key)) in outcomes.iter().zip([
            (Axis::Meaning, "meaning"),
            (Axis::Lifecycle, "lifecycle"),
            (Axis::Recreatability, "recreatability"),
        ]) {
            assert_eq!(*actual_axis, axis, "{name}: axis ordering");
            let axis_votes: Vec<_> = actual_votes
                .iter()
                .filter(|vote| vote.axis == axis)
                .cloned()
                .collect();
            let allowed = schema["$defs"]["labels"]["items"]["oneOf"]
                .as_array()
                .unwrap()
                .iter()
                .find(|variant| variant["properties"]["axis"]["const"] == key)
                .unwrap()["properties"]["label"]["enum"]
                .as_array()
                .unwrap();
            for vote in &axis_votes {
                assert!(
                    FUNCTIONS.contains(&vote.function),
                    "{name}: unknown function"
                );
                assert_ne!(vote.label, "unknown", "{name}: abstain instead");
                assert!(allowed.contains(&json!(vote.label)), "{name}: {vote:?}");
            }
            let summary = match outcome {
                Outcome::Label { label, functions } => {
                    assert!(!axis_votes.is_empty(), "{name}: label needs votes");
                    assert!(axis_votes.iter().all(|vote| vote.label == *label), "{name}");
                    assert_eq!(
                        *functions,
                        axis_votes
                            .iter()
                            .map(|vote| vote.function)
                            .collect::<Vec<_>>(),
                        "{name}: sorted provenance"
                    );
                    *label
                }
                Outcome::Abstain(AbstainReason::NoVote) => {
                    assert!(axis_votes.is_empty(), "{name}: no vote");
                    "abstain:no_vote"
                }
                Outcome::Abstain(AbstainReason::Conflict(conflicts)) => {
                    assert_eq!(*conflicts, axis_votes, "{name}: conflict provenance");
                    assert!(
                        conflicts
                            .iter()
                            .map(|vote| vote.label)
                            .collect::<BTreeSet<_>>()
                            .len()
                            > 1,
                        "{name}: conflict requires different labels"
                    );
                    "abstain:conflict"
                }
                Outcome::Abstain(AbstainReason::Coverage(state)) => {
                    assert_eq!(
                        state,
                        packet["coverage"]["state"].as_str().unwrap(),
                        "{name}"
                    );
                    assert_ne!(state, "complete", "{name}");
                    assert!(
                        actual_votes.is_empty(),
                        "{name}: coverage suppresses all votes"
                    );
                    "abstain:coverage"
                }
            };
            assert_eq!(
                summary,
                case["expect"][key].as_str().unwrap(),
                "{name}: {key}"
            );
        }
    }
}

#[test]
fn every_function_has_a_vote_and_a_named_near_miss() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(
        FUNCTIONS.iter().collect::<BTreeSet<_>>().len(),
        FUNCTIONS.len()
    );
    for function in FUNCTIONS {
        assert!(
            cases
                .iter()
                .any(|case| case["votes"].as_array().unwrap().contains(&json!(function))),
            "{function}: missing positive fixture"
        );
        let prefix = format!("{function}:near_miss");
        assert!(
            cases.iter().any(|case| {
                case["name"].as_str().unwrap().starts_with(&prefix)
                    && !case["votes"].as_array().unwrap().contains(&json!(function))
            }),
            "{function}: missing near miss"
        );
    }
}

#[test]
fn invalid_packets_are_rejected_before_coverage_or_rules() {
    assert!(votes(&json!({})).is_err());
    assert!(combine(&json!({})).is_err());
    let mut packet = fixture()["base"].clone();
    packet["coverage"]["state"] = json!("denied");
    validate_packet(&packet).unwrap();
    packet["markers"] = json!(["cargo_toml"]);
    assert!(validate_packet(&packet).is_err());
    assert!(votes(&packet).is_err());
    assert!(combine(&packet).is_err());
}
