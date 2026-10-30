use loomward_learn::explain::{explain, Line};
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

/// A fully observed synthetic project folder: manifest, build output, quiet for 238 days.
fn rich() -> Value {
    let observed = 1_800_000_000u64;
    patched(
        &fixture()["base"],
        &json!({
            "observed_at": observed,
            "subject": {"kind": "folder", "name_tokens": ["old", "tool"], "depth": 0},
            "shape": {"files": 3, "folders": 2, "logical_bytes": 1_610_612_736u64,
                      "allocated_bytes": 1_700_000_000u64, "max_depth": 3},
            "families": [
                {"family": "executable", "share_files": 0.25, "share_bytes": 0.75},
                {"family": "code", "share_files": 0.75, "share_bytes": 0.25}
            ],
            "extensions": [{"ext": "rs", "files": 2, "bytes": 30}, {"ext": "", "files": 1, "bytes": null}],
            "markers": ["cargo_toml", "cargo_target_dir"],
            "time": {
                "newest_content_write": observed - 238 * 86_400,
                "oldest_content_write": 1_600_000_000u64,
                "newest_source_write": 1_700_000_000u64,
                "newest_generated_write": null,
                "git_last_commit": null, "git_dirty": false, "last_access": "not_evidence"
            },
            "coverage": {"state": "complete", "allocation_known": true,
                         "link_counts_observed": true, "online": true},
        }),
    )
}

fn all_packets() -> Vec<(String, Value)> {
    let fixture = fixture();
    let mut packets = vec![("rich".to_owned(), rich())];
    for case in fixture["cases"].as_array().unwrap() {
        packets.push((
            case["name"].as_str().unwrap().to_owned(),
            patched(&fixture["base"], &case["patch"]),
        ));
    }
    // Conflicts, partial/stale coverage and unobserved coverage are fixture cases; add offline too.
    let mut offline = rich();
    offline["coverage"]["online"] = json!(false);
    packets.push(("offline".into(), offline));
    packets
}

fn text(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_with<'a>(lines: &'a [Line], prefix: &str) -> &'a Line {
    lines
        .iter()
        .find(|l| l.text.starts_with(prefix))
        .unwrap_or_else(|| panic!("no line starting {prefix:?} in {lines:#?}"))
}

#[test]
fn no_output_reads_as_approval_or_advice() {
    const BANNED: &[&str] = &[
        "unused",
        "not used",
        "safe to",
        "can be removed",
        "can be deleted",
        "remove it",
        "delete it",
        "should delete",
        "recommend",
        "advise",
        "approv",
        "obsolete",
        "abandoned",
        "junk",
        "useless",
        "no longer needed",
        "probab",
        "confiden",
        "certain",
        "definitely",
    ];
    let packets = all_packets();
    assert!(packets.len() > 80);
    for (name, packet) in &packets {
        let lowered = text(&explain(packet).unwrap()).to_lowercase();
        for phrase in BANNED {
            assert!(
                !lowered.contains(phrase),
                "{name}: {phrase:?} in\n{lowered}"
            );
        }
    }
}

#[test]
fn coldness_is_no_activity_evidence_seen_since_a_month() {
    let lines = explain(&rich()).unwrap();
    let cold = line_with(&lines, "No activity evidence seen since");
    assert_eq!(
        cold.text,
        "No activity evidence seen since May 2026 (newest content change, 238 days before this packet was observed)."
    );
    assert_eq!(cold.trace, ["/observed_at", "/time/newest_content_write"]);
    assert!(line_with(&lines, "Last-access time is system-managed")
        .text
        .contains("ignored as evidence"));
}

#[test]
fn unknown_stays_unknown() {
    // The fixture base is complete coverage with every optional fact null.
    let lines = explain(&fixture()["base"]).unwrap();
    let body = text(&lines);
    assert!(!body.contains("No activity evidence seen since"));
    for expected in [
        "The newest content change is unknown, so no statement about activity is made.",
        "Contents: files unknown; folders unknown; logical size unknown; allocated size unknown; deepest nesting unknown.",
        "Newest source write: unknown; newest generated write: unknown.",
        "Git last commit: unknown; working tree: unknown.",
        "Duplicate bytes: unknown; duplicate groups: unknown.",
    ] {
        assert!(body.contains(expected), "missing {expected:?} in\n{body}");
    }
    assert!(!body.contains(" 0 files") && !body.contains("0 B"));

    for state in ["denied", "unscanned", "excluded", "cancelled", "unknown"] {
        let mut packet = fixture()["base"].clone();
        packet["coverage"]["state"] = json!(state);
        let lines = explain(&packet).unwrap();
        let body = text(&lines);
        assert!(body.contains(&format!("Nothing was observed (coverage is {state})")));
        for expected in [
            "File types are unknown.",
            "File extensions are unknown.",
            "Project and layout markers are unknown.",
            "Camera-file details are unknown.",
        ] {
            assert!(body.contains(expected), "{state}: {expected}");
        }
    }
}

#[test]
fn abstention_and_conflict_are_said() {
    let fixture = fixture();
    let case = |name: &str| {
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap();
        explain(&patched(&fixture["base"], &case["patch"])).unwrap()
    };

    let none = case("both_times_null");
    for axis in ["Meaning", "Lifecycle", "Recreatability"] {
        assert_eq!(
            line_with(&none, &format!("{axis}:")).text,
            format!("{axis}: no rule had enough evidence, so no label is given.")
        );
    }

    let conflict = case("dirty_and_old_conflict");
    let lifecycle = line_with(&conflict, "Lifecycle:");
    assert!(
        lifecycle.text.contains("rules disagree (lf_git_dirty says \"active\"; lf_quiet_years says \"historical\"), so no label is given."),
        "{}",
        lifecycle.text
    );
    assert!(lifecycle.trace.contains(&"/time/git_dirty".to_owned()));

    for (name, state) in [
        ("coverage_partial_with_markers", "partial"),
        ("coverage_denied", "denied"),
    ] {
        let lines = case(name);
        assert_eq!(
            line_with(&lines, "Meaning:").text,
            format!("Meaning: no label is given because coverage is {state}.")
        );
    }
    assert!(text(&case("coverage_partial_with_markers"))
        .contains("Coverage is partial, so counts and dates may be incomplete."));

    let label = line_with(&explain(&rich()).unwrap(), "Meaning:")
        .text
        .clone();
    assert_eq!(
        label,
        "Meaning: rule evidence (lf_source_manifest) points to \"source project\". This is an inference from the facts above, not a recorded fact."
    );
}

#[test]
fn output_is_byte_stable_for_a_fixed_input() {
    let expected = [
        "Coverage is complete.",
        "Folder name words: old, tool.",
        "Contents: 3 files; 2 folders; logical size 1.5 GiB; allocated size 1.5 GiB; deepest nesting 3.",
        "File types by share: code 75% of files (25% of bytes); executable 25% of files (75% of bytes).",
        "Most common extensions: .rs 2 files; (no extension) 1 files.",
        "Markers seen: cargo toml, cargo target dir.",
        "No activity evidence seen since May 2026 (newest content change, 238 days before this packet was observed).",
        "Newest source write: November 2023; newest generated write: unknown.",
        "Git last commit: unknown; working tree: clean.",
        "Last-access time is system-managed and is ignored as evidence.",
        "Duplicate bytes: unknown; duplicate groups: unknown.",
        "No camera-file details are recorded.",
        "Meaning: rule evidence (lf_source_manifest) points to \"source project\". This is an inference from the facts above, not a recorded fact.",
        "Lifecycle: rule evidence (lf_quiet_months) points to \"dormant\". This is an inference from the facts above, not a recorded fact.",
        "Recreatability: rule evidence (lf_project_with_outputs) points to \"mixed\". This is an inference from the facts above, not a recorded fact.",
    ]
    .join("\n");
    let first = text(&explain(&rich()).unwrap());
    assert_eq!(first, expected);
    assert_eq!(first, text(&explain(&rich()).unwrap()));
}

#[test]
fn every_trace_names_real_packet_fields() {
    for (name, packet) in all_packets() {
        let lines = explain(&packet).unwrap();
        assert!(lines.len() >= 15, "{name}");
        for line in &lines {
            assert!(!line.trace.is_empty(), "{name}: untraced {:?}", line.text);
            let mut sorted = line.trace.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted, line.trace, "{name}: trace order");
            for pointer in &line.trace {
                assert!(
                    packet.pointer(pointer).is_some(),
                    "{name}: {pointer} does not resolve for {:?}",
                    line.text
                );
            }
        }
    }
}

#[test]
fn invalid_packets_are_rejected_not_explained() {
    let mut packet = rich();
    packet["time"]["last_access"] = json!("evidence");
    assert!(explain(&packet).is_err());
}
