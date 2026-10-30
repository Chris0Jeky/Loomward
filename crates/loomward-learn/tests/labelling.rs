use std::collections::BTreeSet;

use loomward_learn::labelling::{combine, votes, AbstainReason, Axis, Outcome, Vote, FUNCTIONS};
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

fn packet_for(patch: &Value) -> Value {
    let fixture = fixture();
    patched(&fixture["base"], patch)
}

fn sorted_functions(packet: &Value) -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = votes(packet)
        .unwrap()
        .iter()
        .map(|vote| vote.function)
        .collect();
    ids.sort_unstable();
    ids
}

fn exact_vote(packet: &Value, function: &'static str) -> Vote {
    votes(packet)
        .unwrap()
        .into_iter()
        .find(|vote| vote.function == function)
        .unwrap_or_else(|| panic!("expected vote for {function}"))
}

fn full_time(git_dirty: &Value, newest: &Value) -> Value {
    json!({
        "newest_content_write": newest,
        "oldest_content_write": null,
        "newest_source_write": null,
        "newest_generated_write": null,
        "git_last_commit": null,
        "git_dirty": git_dirty,
        "last_access": "not_evidence"
    })
}

#[test]
fn near_miss_pair_lf_source_manifest() {
    let near = packet_for(&json!({"markers": ["cargo_target_dir"]}));
    let positive = packet_for(&json!({"markers": ["cargo_toml", "cargo_target_dir"]}));
    assert_eq!(sorted_functions(&near), Vec::<&str>::new());
    assert_eq!(
        sorted_functions(&positive),
        vec!["lf_project_with_outputs", "lf_source_manifest"]
    );
    assert_eq!(
        exact_vote(&positive, "lf_source_manifest"),
        Vote {
            function: "lf_source_manifest",
            axis: Axis::Meaning,
            label: "source_project",
        }
    );
}

#[test]
fn near_miss_pair_lf_model_layout() {
    let near = packet_for(&json!({"markers": ["gguf_file", "cargo_toml"]}));
    let positive = packet_for(&json!({"markers": ["gguf_file"]}));
    assert_eq!(sorted_functions(&near), vec!["lf_source_manifest"]);
    assert_eq!(sorted_functions(&positive), vec!["lf_model_layout"]);
    assert_eq!(
        exact_vote(&positive, "lf_model_layout"),
        Vote {
            function: "lf_model_layout",
            axis: Axis::Meaning,
            label: "model_weights",
        }
    );
}

#[test]
fn near_miss_pair_lf_virtual_disk() {
    let near = packet_for(&json!({"markers": ["vhdx_file", "cargo_toml"]}));
    let positive = packet_for(&json!({"markers": ["vhdx_file"]}));
    assert_eq!(sorted_functions(&near), vec!["lf_source_manifest"]);
    assert_eq!(sorted_functions(&positive), vec!["lf_virtual_disk"]);
    assert_eq!(
        exact_vote(&positive, "lf_virtual_disk"),
        Vote {
            function: "lf_virtual_disk",
            axis: Axis::Meaning,
            label: "virtual_disk",
        }
    );
}

#[test]
fn near_miss_pair_lf_lightroom_catalogue() {
    let near = packet_for(&json!({"markers": ["lightroom_catalog", "lightroom_backups_dated"]}));
    let positive = packet_for(&json!({"markers": ["lightroom_catalog"]}));
    assert_eq!(
        sorted_functions(&near),
        vec!["lf_catalogue_only", "lf_lightroom_backups"]
    );
    assert_eq!(
        sorted_functions(&positive),
        vec!["lf_catalogue_only", "lf_lightroom_catalogue"]
    );
    assert_eq!(
        exact_vote(&positive, "lf_lightroom_catalogue"),
        Vote {
            function: "lf_lightroom_catalogue",
            axis: Axis::Meaning,
            label: "photo_catalogue",
        }
    );
}

#[test]
fn near_miss_pair_lf_lightroom_backups() {
    let near = packet_for(&json!({"markers": ["lightroom_catalog"]}));
    let positive =
        packet_for(&json!({"markers": ["lightroom_catalog", "lightroom_backups_dated"]}));
    assert_eq!(
        sorted_functions(&near),
        vec!["lf_catalogue_only", "lf_lightroom_catalogue"]
    );
    assert_eq!(
        sorted_functions(&positive),
        vec!["lf_catalogue_only", "lf_lightroom_backups"]
    );
    assert_eq!(
        exact_vote(&positive, "lf_lightroom_backups"),
        Vote {
            function: "lf_lightroom_backups",
            axis: Axis::Meaning,
            label: "catalogue_backup",
        }
    );
}

#[test]
fn near_miss_pair_lf_photo_shoot() {
    let families =
        |share: f64| json!([{"family": "image", "share_files": share, "share_bytes": null}]);
    let near = packet_for(&json!({"markers": ["dcim_layout"], "families": families(0.499999)}));
    let positive = packet_for(&json!({"markers": ["dcim_layout"], "families": families(0.5)}));
    assert_eq!(sorted_functions(&near), vec!["lf_camera_originals"]);
    assert_eq!(
        sorted_functions(&positive),
        vec!["lf_camera_originals", "lf_photo_shoot"]
    );
    assert_eq!(
        exact_vote(&positive, "lf_photo_shoot"),
        Vote {
            function: "lf_photo_shoot",
            axis: Axis::Meaning,
            label: "photo_shoot",
        }
    );
}

#[test]
fn near_miss_pair_lf_documents() {
    let families =
        |share: f64| json!([{"family": "document", "share_files": share, "share_bytes": null}]);
    let near =
        packet_for(&json!({"markers": ["office_documents"], "families": families(0.499999)}));
    let positive = packet_for(&json!({"markers": ["office_documents"], "families": families(0.5)}));
    assert_eq!(sorted_functions(&near), Vec::<&str>::new());
    assert_eq!(sorted_functions(&positive), vec!["lf_documents"]);
    assert_eq!(
        exact_vote(&positive, "lf_documents"),
        Vote {
            function: "lf_documents",
            axis: Axis::Meaning,
            label: "document_collection",
        }
    );
}

#[test]
fn near_miss_pair_lf_installers() {
    let families =
        |share: f64| json!([{"family": "executable", "share_files": 0.9, "share_bytes": share}]);
    let near = packet_for(&json!({"markers": ["installer_files"], "families": families(0.499999)}));
    let positive = packet_for(&json!({"markers": ["installer_files"], "families": families(0.5)}));
    assert_eq!(sorted_functions(&near), Vec::<&str>::new());
    assert_eq!(sorted_functions(&positive), vec!["lf_installers"]);
    assert_eq!(
        exact_vote(&positive, "lf_installers"),
        Vote {
            function: "lf_installers",
            axis: Axis::Meaning,
            label: "installer",
        }
    );
}

#[test]
fn near_miss_pair_lf_archives() {
    let families =
        |share: f64| json!([{"family": "archive", "share_files": 0.9, "share_bytes": share}]);
    let near = packet_for(&json!({"markers": ["archive_files"], "families": families(0.499999)}));
    let positive = packet_for(&json!({"markers": ["archive_files"], "families": families(0.5)}));
    assert_eq!(sorted_functions(&near), Vec::<&str>::new());
    assert_eq!(sorted_functions(&positive), vec!["lf_archives"]);
    assert_eq!(
        exact_vote(&positive, "lf_archives"),
        Vote {
            function: "lf_archives",
            axis: Axis::Meaning,
            label: "archive",
        }
    );
}

#[test]
fn near_miss_pair_lf_git_dirty() {
    let near_false = packet_for(&json!({"time": full_time(&json!(false), &json!(null))}));
    let null_patch = json!({"time": full_time(&json!(null), &json!(null))});
    assert!(!null_patch.as_object().unwrap().is_empty());
    assert!(!null_patch["time"].as_object().unwrap().is_empty());
    let near_null = packet_for(&null_patch);
    let positive = packet_for(&json!({"time": full_time(&json!(true), &json!(null))}));
    assert_eq!(sorted_functions(&near_false), Vec::<&str>::new());
    assert_eq!(sorted_functions(&near_null), Vec::<&str>::new());
    assert_eq!(sorted_functions(&positive), vec!["lf_git_dirty"]);
    assert_eq!(
        exact_vote(&positive, "lf_git_dirty"),
        Vote {
            function: "lf_git_dirty",
            axis: Axis::Lifecycle,
            label: "active",
        }
    );
}

#[test]
fn near_miss_pair_lf_recent_write() {
    let near = packet_for(&json!({"time": full_time(&json!(null), &json!(1797407999))}));
    let positive = packet_for(&json!({"time": full_time(&json!(null), &json!(1797408000))}));
    assert_eq!(sorted_functions(&near), Vec::<&str>::new());
    assert_eq!(sorted_functions(&positive), vec!["lf_recent_write"]);
    assert_eq!(
        exact_vote(&positive, "lf_recent_write"),
        Vote {
            function: "lf_recent_write",
            axis: Axis::Lifecycle,
            label: "active",
        }
    );
}

#[test]
fn near_miss_pair_lf_quiet_months() {
    let near = packet_for(&json!({"time": full_time(&json!(null), &json!(1792224000))}));
    let positive = packet_for(&json!({"time": full_time(&json!(null), &json!(1792223999))}));
    assert_eq!(sorted_functions(&near), Vec::<&str>::new());
    assert_eq!(sorted_functions(&positive), vec!["lf_quiet_months"]);
    assert_eq!(
        exact_vote(&positive, "lf_quiet_months"),
        Vote {
            function: "lf_quiet_months",
            axis: Axis::Lifecycle,
            label: "dormant",
        }
    );
}

#[test]
fn near_miss_pair_lf_quiet_years() {
    let near = packet_for(&json!({"time": full_time(&json!(null), &json!(1768464000))}));
    let positive = packet_for(&json!({"time": full_time(&json!(null), &json!(1768463999))}));
    assert_eq!(sorted_functions(&near), vec!["lf_quiet_months"]);
    assert_eq!(sorted_functions(&positive), vec!["lf_quiet_years"]);
    assert_eq!(
        exact_vote(&positive, "lf_quiet_years"),
        Vote {
            function: "lf_quiet_years",
            axis: Axis::Lifecycle,
            label: "historical",
        }
    );
}

#[test]
fn near_miss_pair_lf_hub_layout() {
    let near = packet_for(&json!({"markers": ["gguf_file"]}));
    let positive = packet_for(&json!({"markers": ["gguf_file", "hf_hub_layout"]}));
    assert_eq!(sorted_functions(&near), vec!["lf_model_layout"]);
    assert_eq!(
        sorted_functions(&positive),
        vec!["lf_hub_layout", "lf_model_layout"]
    );
    assert_eq!(
        exact_vote(&positive, "lf_hub_layout"),
        Vote {
            function: "lf_hub_layout",
            axis: Axis::Recreatability,
            label: "redownloadable",
        }
    );
}

#[test]
fn near_miss_pair_lf_project_with_outputs() {
    let near = packet_for(&json!({"markers": ["cargo_target_dir"]}));
    let positive = packet_for(&json!({"markers": ["cargo_toml", "cargo_target_dir"]}));
    assert_eq!(sorted_functions(&near), Vec::<&str>::new());
    assert_eq!(
        sorted_functions(&positive),
        vec!["lf_project_with_outputs", "lf_source_manifest"]
    );
    assert_eq!(
        exact_vote(&positive, "lf_project_with_outputs"),
        Vote {
            function: "lf_project_with_outputs",
            axis: Axis::Recreatability,
            label: "mixed",
        }
    );
}

#[test]
fn near_miss_pair_lf_catalogue_previews() {
    let near = packet_for(&json!({"markers": ["lightroom_catalog"]}));
    let positive = packet_for(&json!({"markers": ["lightroom_catalog", "lightroom_previews"]}));
    assert_eq!(
        sorted_functions(&near),
        vec!["lf_catalogue_only", "lf_lightroom_catalogue"]
    );
    assert_eq!(
        sorted_functions(&positive),
        vec!["lf_catalogue_previews", "lf_lightroom_catalogue"]
    );
    assert_eq!(
        exact_vote(&positive, "lf_catalogue_previews"),
        Vote {
            function: "lf_catalogue_previews",
            axis: Axis::Recreatability,
            label: "mixed",
        }
    );
}

#[test]
fn near_miss_pair_lf_catalogue_only() {
    let near_blocked = packet_for(&json!({"markers": ["lightroom_catalog", "lightroom_previews"]}));
    let missing_patch = json!({"markers": ["lightroom_previews"]});
    assert!(!missing_patch.as_object().unwrap().is_empty());
    let near_missing = packet_for(&missing_patch);
    let positive = packet_for(&json!({"markers": ["lightroom_catalog"]}));
    assert_eq!(
        sorted_functions(&near_blocked),
        vec!["lf_catalogue_previews", "lf_lightroom_catalogue"]
    );
    assert_eq!(sorted_functions(&near_missing), Vec::<&str>::new());
    assert_eq!(
        sorted_functions(&positive),
        vec!["lf_catalogue_only", "lf_lightroom_catalogue"]
    );
    assert_eq!(
        exact_vote(&positive, "lf_catalogue_only"),
        Vote {
            function: "lf_catalogue_only",
            axis: Axis::Recreatability,
            label: "unique",
        }
    );
}

#[test]
fn near_miss_pair_lf_camera_originals() {
    let families = json!([{"family": "image", "share_files": 1.0, "share_bytes": null}]);
    let near = packet_for(&json!({"families": families}));
    let positive = packet_for(&json!({"markers": ["dcim_layout"], "families": families}));
    assert_eq!(sorted_functions(&near), Vec::<&str>::new());
    assert_eq!(
        sorted_functions(&positive),
        vec!["lf_camera_originals", "lf_photo_shoot"]
    );
    assert_eq!(
        exact_vote(&positive, "lf_camera_originals"),
        Vote {
            function: "lf_camera_originals",
            axis: Axis::Recreatability,
            label: "unique",
        }
    );
}

#[test]
fn three_way_office_shares_at_decimal_half_vote() {
    let packet = packet_for(&json!({
        "markers": ["office_documents"],
        "families": [
            {"family": "document", "share_files": 0.1, "share_bytes": null},
            {"family": "spreadsheet", "share_files": 0.2, "share_bytes": null},
            {"family": "presentation", "share_files": 0.2, "share_bytes": null}
        ]
    }));
    assert_eq!(sorted_functions(&packet), vec!["lf_documents"]);
    assert_eq!(
        exact_vote(&packet, "lf_documents"),
        Vote {
            function: "lf_documents",
            axis: Axis::Meaning,
            label: "document_collection",
        }
    );
    let outcomes = combine(&packet).unwrap();
    assert!(matches!(
        &outcomes[0],
        (Axis::Meaning, Outcome::Label { label: "document_collection", functions })
            if functions == &vec!["lf_documents"]
    ));
}

#[test]
fn office_share_sum_above_one_is_rejected() {
    let packet = packet_for(&json!({
        "markers": ["office_documents"],
        "families": [
            {"family": "document", "share_files": 0.6, "share_bytes": null},
            {"family": "spreadsheet", "share_files": 0.6, "share_bytes": null}
        ]
    }));
    assert!(votes(&packet).is_err());
    assert!(combine(&packet).is_err());
}
