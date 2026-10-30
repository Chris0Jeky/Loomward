use loomward_learn::explain::{explain, function_inputs, Line};
use loomward_learn::labelling::FUNCTIONS;
use loomward_learn::packet::{FAMILIES, MARKERS};
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

/// Substrings that must never appear in the renderer's own template text.
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
    "clean up",
    "cleanup",
    "unneeded",
    "unnecessary",
    "redundant",
    "waste",
    "free up",
];
/// Whole words (so "safe" is caught bare but a longer word is not mistaken for it).
const BANNED_WORDS: &[&str] = &["safe", "idle", "inactive"];

fn banned_hit(body: &str) -> Option<String> {
    let lowered = body.to_lowercase();
    BANNED
        .iter()
        .find(|phrase| lowered.contains(*phrase))
        .map(|phrase| (*phrase).to_owned())
        .or_else(|| {
            lowered
                .split(|c: char| !c.is_alphanumeric())
                .find(|word| BANNED_WORDS.contains(word))
                .map(str::to_owned)
        })
}

/// Small deterministic generator (a 64-bit LCG): no dependency, same packets on every run.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

/// Valid packets over every coverage state, online flag, link-count and allocation flag, date layout (missing,
/// old, equal to or after the observation), git state, duplicate and shape figure, family split (including
/// shares under 1%), extension and marker set. Name tokens and extensions stay neutral.
fn generated_packets(count: usize) -> Vec<(String, Value)> {
    const STATES: [&str; 8] = [
        "complete",
        "partial",
        "stale",
        "unscanned",
        "denied",
        "excluded",
        "cancelled",
        "unknown",
    ];
    const FAMILY_IDS: [&str; 6] = ["code", "image", "document", "archive", "executable", "data"];
    const EXTS: [&str; 5] = ["rs", "txt", "jpg", "zip", ""];
    const MARKER_IDS: [&str; 12] = [
        "cargo_toml",
        "cargo_target_dir",
        "package_json",
        "node_modules_dir",
        "git_dir",
        "hf_hub_layout",
        "lightroom_catalog",
        "lightroom_previews",
        "dcim_layout",
        "installer_files",
        "archive_files",
        "office_documents",
    ];
    let mut rng = Lcg(0x5EED_1048);
    let mut out = Vec::new();
    for index in 0..count {
        let state = *rng.pick(&STATES);
        let observed_state = matches!(state, "complete" | "partial" | "stale");
        let mut packet = if observed_state {
            rich()
        } else {
            fixture()["base"].clone()
        };
        let observed = 1_600_000_000 + rng.below(300_000_000);
        packet["observed_at"] = json!(observed);
        packet["coverage"]["state"] = json!(state);
        packet["coverage"]["online"] = json!(rng.below(2) == 0);
        packet["coverage"]["link_counts_observed"] = json!(rng.below(2) == 0);
        let allocation_known = rng.below(2) == 0;
        packet["coverage"]["allocation_known"] = json!(allocation_known);
        packet["subject"]["name_tokens"] = match rng.below(3) {
            0 => json!([]),
            1 => json!(["old", "tool"]),
            _ => json!(["photos", "2019"]),
        };
        if observed_state {
            let files = 1 + rng.below(5_000);
            packet["shape"] = json!({
                "files": files,
                "folders": rng.below(500),
                "logical_bytes": 1 + rng.below(1 << 42),
                "allocated_bytes": if allocation_known { json!(1 + rng.below(1 << 42)) } else { Value::Null },
                "max_depth": rng.below(40),
            });
            // Shares are basis points that sum to at most 10_000, so some land below 1%.
            let mut left = 10_000u64;
            let mut families = Vec::new();
            for family in FAMILY_IDS.iter().take(rng.below(7) as usize) {
                let take = match rng.below(3) {
                    0 => rng.below(100).min(left),
                    _ => rng.below(left + 1),
                };
                left -= take;
                families.push(json!({
                    "family": family,
                    "share_files": take as f64 / 10_000.0,
                    "share_bytes": if rng.below(4) == 0 { Value::Null } else { json!(rng.below(1_000) as f64 / 10_000.0) },
                }));
            }
            packet["families"] = json!(families);
            let extensions: Vec<_> = EXTS
                .iter()
                .take(rng.below(6) as usize)
                .map(|ext| json!({"ext": ext, "files": 1, "bytes": null}))
                .collect();
            packet["extensions"] = json!(extensions);
            let markers: Vec<_> = MARKER_IDS.iter().filter(|_| rng.below(3) == 0).collect();
            packet["markers"] = json!(markers);
            for key in [
                "newest_content_write",
                "newest_source_write",
                "newest_generated_write",
                "git_last_commit",
            ] {
                packet["time"][key] = match rng.below(4) {
                    0 => Value::Null,
                    1 => json!(observed + rng.below(90 * 86_400)),
                    2 => json!(observed),
                    _ => json!(observed.saturating_sub(rng.below(900 * 86_400))),
                };
            }
            packet["time"]["oldest_content_write"] = Value::Null;
            packet["time"]["git_dirty"] = match rng.below(3) {
                0 => Value::Null,
                1 => json!(true),
                _ => json!(false),
            };
            packet["duplicates"] = if rng.below(2) == 0 {
                json!({"duplicate_bytes": null, "groups": null})
            } else {
                json!({"duplicate_bytes": rng.below(1 << 30), "groups": rng.below(900)})
            };
            packet["media"] = if rng.below(2) == 0 {
                Value::Null
            } else {
                json!({"raw_files": 4, "sidecar_files": 3, "raw_with_sidecar": 2,
                       "capture_start": null, "capture_end": null, "camera_models": null})
            };
        }
        out.push((format!("generated-{index}-{state}"), packet));
    }
    out
}

#[test]
fn no_output_reads_as_approval_or_advice() {
    // Name tokens and extensions here are neutral, so every word in the output comes from the renderer's own
    // templates or from the closed marker, family and label vocabularies.
    let mut packets = all_packets();
    assert!(packets.len() > 80);
    packets.extend(generated_packets(400));
    for (name, packet) in &packets {
        let body = text(&explain(packet).unwrap());
        assert_eq!(banned_hit(&body), None, "{name}\n{body}");
    }
}

#[test]
fn closed_vocabularies_pass_the_banned_scan() {
    // Family and marker ids are echoed from packets (underscores become spaces for markers).
    for id in FAMILIES.iter().chain(MARKERS) {
        assert_eq!(banned_hit(&id.replace('_', " ")), None, "{id}");
    }
}

#[test]
fn banned_scan_can_fail() {
    for sample in [
        "This folder is idle.",
        "Looks safe.",
        "It is Redundant data",
        "free up space",
        "cleanup",
    ] {
        assert!(banned_hit(sample).is_some(), "{sample}");
    }
    assert_eq!(banned_hit("safeguard unsafe midpoint"), None);
}

#[test]
fn echoed_data_stays_inside_labelled_positions() {
    let hostile = [
        "waste",
        "junk",
        "idle",
        "inactive",
        "unused",
        "safe",
        "redundant",
        "obsolete",
    ];
    let mut packet = rich();
    packet["subject"]["name_tokens"] = json!(hostile);
    packet["extensions"] = json!([
        {"ext": "junk", "files": 1, "bytes": null},
        {"ext": "idle", "files": 1, "bytes": null},
    ]);
    let lines = explain(&packet).unwrap();
    let mut echoed = 0;
    for line in &lines {
        let lowered = line.text.to_lowercase();
        if line.text.starts_with("Folder name words: ") {
            assert_eq!(
                line.text,
                format!("Folder name words: {}.", hostile.join(", "))
            );
            echoed += 1;
        } else if line.text.starts_with("Most common extensions: ") {
            assert_eq!(
                line.text,
                "Most common extensions: .idle 1 files; .junk 1 files."
            );
            echoed += 1;
        } else {
            for word in hostile {
                assert!(
                    !lowered.contains(word),
                    "{word:?} echoed in {:?}",
                    line.text
                );
            }
            assert_eq!(banned_hit(&line.text), None, "{}", line.text);
        }
    }
    assert_eq!(echoed, 2);
}

#[test]
fn coldness_is_no_activity_evidence_seen_since_a_month() {
    let lines = explain(&rich()).unwrap();
    let cold = line_with(&lines, "No activity evidence seen since");
    assert_eq!(
        cold.text,
        "No activity evidence seen since May 2026 (newest content change, 238 days before this packet was observed)."
    );
    assert_eq!(
        cold.trace,
        [
            "/observed_at",
            "/time/git_dirty",
            "/time/git_last_commit",
            "/time/newest_content_write",
            "/time/newest_generated_write",
            "/time/newest_source_write"
        ]
    );
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
        "No dated change is known, so no statement about activity is made.",
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

fn time_of(observed: u64, patch: Value) -> Value {
    let mut packet = rich();
    packet["observed_at"] = json!(observed);
    for (key, value) in patch.as_object().unwrap() {
        packet["time"][key] = value.clone();
    }
    packet
}

fn coldness_line(packet: &Value) -> String {
    let lines = explain(packet).unwrap();
    lines
        .iter()
        .find(|l| {
            ["No activity", "Activity evidence", "No dated"]
                .iter()
                .any(|p| l.text.starts_with(p))
                || l.text.contains("after this packet")
        })
        .unwrap_or_else(|| panic!("no coldness line in {lines:#?}"))
        .text
        .clone()
}

#[test]
fn dirty_tree_is_activity_not_inactivity() {
    let fixture = fixture();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "dirty_and_old_conflict")
        .unwrap();
    let lines = explain(&patched(&fixture["base"], &case["patch"])).unwrap();
    assert!(!text(&lines).contains("No activity evidence"));
    assert_eq!(
        coldness_line(&patched(&fixture["base"], &case["patch"])),
        "Activity evidence is present: the git working tree has uncommitted changes."
    );
}

#[test]
fn cutoff_is_the_newest_of_all_dated_activity_fields() {
    let observed = 1_800_000_000u64;
    // Git commit newer than content, source and generated writes.
    let commit = time_of(
        observed,
        json!({"newest_content_write": 1_700_000_000u64, "git_last_commit": 1_790_000_000u64}),
    );
    assert_eq!(
        coldness_line(&commit),
        "No activity evidence seen since September 2026 (last git commit, 115 days before this packet was observed)."
    );
    // Source and generated writes newer than content.
    let source = time_of(
        observed,
        json!({"newest_content_write": 1_700_000_000u64, "newest_source_write": 1_790_000_000u64}),
    );
    assert!(coldness_line(&source).contains("September 2026 (newest source write,"));
    let generated = time_of(
        observed,
        json!({"newest_content_write": 1_700_000_000u64, "newest_generated_write": 1_795_000_000u64}),
    );
    assert!(coldness_line(&generated).contains("November 2026 (newest generated write,"));
    // Content only is unchanged from before.
    assert!(coldness_line(&time_of(
        observed,
        json!({"newest_content_write": 1_700_000_000u64})
    ))
    .contains("November 2023 (newest content change,"));
}

#[test]
fn every_inactivity_claim_matches_the_newest_field_in_every_packet() {
    for (name, packet) in all_packets() {
        let lines = explain(&packet).unwrap();
        let body = text(&lines);
        let time = &packet["time"];
        if body.contains("No activity evidence seen since") {
            assert_ne!(time["git_dirty"], json!(true), "{name}");
            let newest = [
                "newest_content_write",
                "newest_source_write",
                "newest_generated_write",
                "git_last_commit",
            ]
            .iter()
            .filter_map(|k| time[*k].as_u64())
            .max()
            .unwrap();
            let days = (packet["observed_at"].as_u64().unwrap() - newest) / 86_400;
            let unit = if days == 1 { "day" } else { "days" };
            assert!(
                body.contains(&format!(", {days} {unit} before this packet")),
                "{name}
{body}"
            );
        }
    }
}

#[test]
fn future_dates_make_no_activity_statement_and_singular_day() {
    let observed = 1_800_000_000u64;
    let future = coldness_line(&time_of(
        observed,
        json!({"newest_content_write": observed + 40 * 86_400}),
    ));
    assert_eq!(
        future,
        "The newest content change is dated February 2027, after this packet was observed, so no statement about activity is made."
    );
    assert!(!future.contains("No activity evidence"));
    assert!(coldness_line(&time_of(
        observed,
        json!({"newest_content_write": observed - 86_400 - 5})
    ))
    .contains("(newest content change, 1 day before this packet was observed)"));
}

#[test]
fn every_labelling_function_has_an_explicit_trace_mapping() {
    for function in FUNCTIONS {
        let inputs =
            function_inputs(function).unwrap_or_else(|| panic!("{function} has no trace mapping"));
        assert!(!inputs.is_empty());
    }
    assert!(function_inputs("lf_not_a_function").is_none());
}

#[test]
fn partial_and_stale_coverage_qualify_every_figure_line() {
    for state in ["partial", "stale"] {
        let mut packet = rich();
        packet["coverage"]["state"] = json!(state);
        let lines = explain(&packet).unwrap();
        let suffix = format!(" ({state} coverage).");
        let mut qualified = 0;
        for line in &lines {
            let is_figure = ![
                "Coverage is",
                "Folder name words",
                "Last-access",
                "Meaning:",
                "Lifecycle:",
                "Recreatability:",
            ]
            .iter()
            .any(|p| line.text.starts_with(p));
            assert_eq!(
                line.text.ends_with(&suffix),
                is_figure,
                "{state}: {}",
                line.text
            );
            qualified += usize::from(is_figure);
        }
        assert_eq!(qualified, 9, "{state}");
    }
    let complete = text(&explain(&rich()).unwrap());
    assert!(!complete.contains("coverage)."));
}

/// Every "<N>% of files|bytes" figure in one family-share line, split by column.
fn shown_shares(line: &str, column: &str) -> Vec<u64> {
    let words: Vec<_> = line.split([' ', '(', ')', ';']).collect();
    words
        .windows(3)
        .filter(|w| w[1] == "of" && w[2].trim_end_matches([';', '.', ')']) == column)
        .filter_map(|w| w[0].strip_suffix('%')?.parse().ok())
        .collect()
}

fn families_line(packet: &Value) -> String {
    explain(packet)
        .unwrap()
        .into_iter()
        .find(|line| line.text.starts_with("File types") || line.text.starts_with("No file types"))
        .unwrap()
        .text
}

#[test]
fn small_shares_never_read_as_zero_and_shown_shares_never_pass_100() {
    let mut packet = rich();
    packet["families"] = json!([
        {"family": "code", "share_files": 0.996, "share_bytes": 0.0},
        {"family": "image", "share_files": 0.004, "share_bytes": 0.0004},
    ]);
    assert_eq!(
        families_line(&packet),
        "File types by share: code 99% of files (0% of bytes); image <1% of files (<1% of bytes)."
    );

    // Whole percent rounds down: 3 x 33.5% reads 33% each, never 34% each (102% in total).
    packet["families"] = json!([
        {"family": "code", "share_files": 0.335, "share_bytes": null},
        {"family": "image", "share_files": 0.335, "share_bytes": null},
        {"family": "data", "share_files": 0.33, "share_bytes": null},
    ]);
    assert_eq!(
        families_line(&packet),
        "File types by share: code 33% of files (bytes share unknown); image 33% of files (bytes share unknown); data 33% of files (bytes share unknown)."
    );

    // Float noise (0.29 * 100 is 28.999...) does not shave a point off.
    packet["families"] = json!([{"family": "code", "share_files": 0.29, "share_bytes": 1.0}]);
    assert_eq!(
        families_line(&packet),
        "File types by share: code 29% of files (100% of bytes)."
    );

    // Not vacuous: percentages were really parsed (and some sums were really large).
    let (mut parsed, mut largest) = (0, 0);
    for (name, packet) in all_packets().into_iter().chain(generated_packets(400)) {
        let line = families_line(&packet);
        for column in ["files", "bytes"] {
            let shares = shown_shares(&line, column);
            let shown: u64 = shares.iter().sum();
            assert!(shown <= 100, "{name}: {column} sums to {shown} in {line}");
            parsed += shares.len();
            largest = largest.max(shown);
        }
    }
    assert!(parsed > 400, "only {parsed} percentages parsed");
    assert!(
        largest >= 90,
        "no generated column came near 100: {largest}"
    );
}

#[test]
fn unobserved_link_counts_qualify_logical_size_and_duplicate_figures() {
    let mut packet = rich();
    packet["duplicates"] = json!({"duplicate_bytes": 2048, "groups": 3});
    let observed = text(&explain(&packet).unwrap());
    assert!(!observed.contains("link counts"));

    packet["coverage"]["link_counts_observed"] = json!(false);
    let lines = explain(&packet).unwrap();
    let contents = line_with(&lines, "Contents:");
    assert!(
        contents
            .text
            .contains("logical size 1.5 GiB (link counts not observed); allocated size 1.5 GiB (link counts not observed); deepest"),
        "{}",
        contents.text
    );
    assert!(contents
        .trace
        .contains(&"/coverage/link_counts_observed".to_owned()));
    // Byte shares come from the same per-row sums as logical size.
    let types = line_with(&lines, "File types by share:");
    assert_eq!(
        types.text,
        "File types by share: code 75% of files (25% of bytes); executable 25% of files (75% of bytes) (link counts not observed)."
    );
    assert!(types
        .trace
        .contains(&"/coverage/link_counts_observed".to_owned()));
    let duplicates = line_with(&lines, "Duplicate bytes:");
    assert_eq!(
        duplicates.text,
        "Duplicate bytes: 2.0 KiB; duplicate groups: 3 (link counts not observed)."
    );
    assert!(duplicates
        .trace
        .contains(&"/coverage/link_counts_observed".to_owned()));

    // Under partial coverage both qualifiers read in order, and unknown figures get none.
    packet["coverage"]["state"] = json!("partial");
    let partial = text(&explain(&packet).unwrap());
    assert!(partial.contains("duplicate groups: 3 (link counts not observed) (partial coverage)."));
    // Only a stated byte figure is qualified: file-count shares alone and unknown sizes get none.
    packet["duplicates"] = json!({"duplicate_bytes": null, "groups": null});
    packet["shape"]["logical_bytes"] = Value::Null;
    packet["shape"]["allocated_bytes"] = Value::Null;
    packet["coverage"]["allocation_known"] = json!(false);
    for family in packet["families"].as_array_mut().unwrap() {
        family["share_bytes"] = Value::Null;
    }
    assert!(!text(&explain(&packet).unwrap()).contains("link counts"));
}

#[test]
fn month_boundaries_are_exact() {
    // (newest content write, expected month): the second before and the second of each boundary, in UTC.
    for (at, month) in [
        (1_704_067_199u64, "December 2023"),
        (1_704_067_200, "January 2024"),
        (1_709_164_800, "February 2024"), // 29 February 2024 00:00:00, a leap day
        (1_709_251_199, "February 2024"), // 29 February 2024 23:59:59
        (1_709_251_200, "March 2024"),
        (1_677_628_799, "February 2023"), // 28 February 2023 23:59:59, no leap day
        (1_677_628_800, "March 2023"),
        (0, "January 1970"),
    ] {
        let line = coldness_line(&time_of(
            1_800_000_000,
            json!({"newest_content_write": at, "oldest_content_write": null, "newest_source_write": null}),
        ));
        assert!(
            line.starts_with(&format!(
                "No activity evidence seen since {month} (newest content change,"
            )),
            "{at}: {line}"
        );
    }
}

#[test]
fn whole_second_and_day_ages_at_a_year_boundary() {
    // Observed at 2024-01-01 00:00:00 UTC, seconds = 0.
    let observed = 1_704_067_200u64;
    for (age, expected) in [
        (0u64, "0 days"),
        (1, "0 days"),
        (86_399, "0 days"),
        (86_400, "1 day"),
        (86_401, "1 day"),
        (2 * 86_400, "2 days"),
    ] {
        let line = coldness_line(&time_of(
            observed,
            json!({"newest_content_write": observed - age, "oldest_content_write": null}),
        ));
        let december = age > 0;
        assert_eq!(
            line,
            format!(
                "No activity evidence seen since {} (newest content change, {expected} before this packet was observed).",
                if december { "December 2023" } else { "January 2024" }
            ),
            "age {age}"
        );
    }
    // One second after the observation is the future, not a zero-day age.
    assert!(coldness_line(&time_of(
        observed,
        json!({"newest_content_write": observed + 1, "oldest_content_write": null})
    ))
    .starts_with("The newest content change is dated January 2024, after this packet"));
}

fn lines_of(packet: &Value) -> Vec<String> {
    explain(packet)
        .unwrap()
        .into_iter()
        .map(|line| line.text)
        .collect()
}

#[test]
fn partial_packet_golden() {
    let mut packet = rich();
    packet["coverage"]["state"] = json!("partial");
    assert_eq!(lines_of(&packet), [
        "Coverage is partial, so counts and dates may be incomplete.",
        "Folder name words: old, tool.",
        "Contents: 3 files; 2 folders; logical size 1.5 GiB; allocated size 1.5 GiB; deepest nesting 3 (partial coverage).",
        "File types by share: code 75% of files (25% of bytes); executable 25% of files (75% of bytes) (partial coverage).",
        "Most common extensions: .rs 2 files; (no extension) 1 files (partial coverage).",
        "Markers seen: cargo toml, cargo target dir (partial coverage).",
        "No activity evidence seen since May 2026 (newest content change, 238 days before this packet was observed) (partial coverage).",
        "Newest source write: November 2023; newest generated write: unknown (partial coverage).",
        "Git last commit: unknown; working tree: clean (partial coverage).",
        "Last-access time is system-managed and is ignored as evidence.",
        "Duplicate bytes: unknown; duplicate groups: unknown (partial coverage).",
        "No camera-file details are recorded (partial coverage).",
        "Meaning: no label is given because coverage is partial.",
        "Lifecycle: no label is given because coverage is partial.",
        "Recreatability: no label is given because coverage is partial.",
    ]);
}

#[test]
fn stale_packet_golden() {
    let mut packet = rich();
    packet["coverage"]["state"] = json!("stale");
    assert_eq!(lines_of(&packet), [
        "Coverage is stale: the figures describe an earlier scan.",
        "Folder name words: old, tool.",
        "Contents: 3 files; 2 folders; logical size 1.5 GiB; allocated size 1.5 GiB; deepest nesting 3 (stale coverage).",
        "File types by share: code 75% of files (25% of bytes); executable 25% of files (75% of bytes) (stale coverage).",
        "Most common extensions: .rs 2 files; (no extension) 1 files (stale coverage).",
        "Markers seen: cargo toml, cargo target dir (stale coverage).",
        "No activity evidence seen since May 2026 (newest content change, 238 days before this packet was observed) (stale coverage).",
        "Newest source write: November 2023; newest generated write: unknown (stale coverage).",
        "Git last commit: unknown; working tree: clean (stale coverage).",
        "Last-access time is system-managed and is ignored as evidence.",
        "Duplicate bytes: unknown; duplicate groups: unknown (stale coverage).",
        "No camera-file details are recorded (stale coverage).",
        "Meaning: no label is given because coverage is stale.",
        "Lifecycle: no label is given because coverage is stale.",
        "Recreatability: no label is given because coverage is stale.",
    ]);
}

#[test]
fn offline_packet_golden() {
    let mut packet = rich();
    packet["coverage"]["online"] = json!(false);
    assert_eq!(lines_of(&packet), [
        "Coverage is complete. It was offline when observed.",
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
    ]);
}

#[test]
fn dirty_packet_golden() {
    let mut packet = rich();
    packet["time"]["git_dirty"] = json!(true);
    packet["time"]["git_last_commit"] = json!(1_795_000_000u64);
    packet["time"]["newest_generated_write"] = json!(1_796_000_000u64);
    // Activity is stated, no inactivity is claimed, and the Lifecycle rules still disagree out loud: the
    // quiet-months rule ages only the content write (see `labelling::votes`).
    assert_eq!(lines_of(&packet), [
        "Coverage is complete.",
        "Folder name words: old, tool.",
        "Contents: 3 files; 2 folders; logical size 1.5 GiB; allocated size 1.5 GiB; deepest nesting 3.",
        "File types by share: code 75% of files (25% of bytes); executable 25% of files (75% of bytes).",
        "Most common extensions: .rs 2 files; (no extension) 1 files.",
        "Markers seen: cargo toml, cargo target dir.",
        "Activity evidence is present: the git working tree has uncommitted changes.",
        "Newest source write: November 2023; newest generated write: November 2026.",
        "Git last commit: November 2026; working tree: has uncommitted changes.",
        "Last-access time is system-managed and is ignored as evidence.",
        "Duplicate bytes: unknown; duplicate groups: unknown.",
        "No camera-file details are recorded.",
        "Meaning: rule evidence (lf_source_manifest) points to \"source project\". This is an inference from the facts above, not a recorded fact.",
        "Lifecycle: rules disagree (lf_git_dirty says \"active\"; lf_quiet_months says \"dormant\"), so no label is given.",
        "Recreatability: rule evidence (lf_project_with_outputs) points to \"mixed\". This is an inference from the facts above, not a recorded fact.",
    ]);
}
