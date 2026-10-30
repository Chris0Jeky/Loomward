//! Fact-only explanation lines for one evidence packet (LW-138, docs/49 section 3, docs/51 section 4.5).
//!
//! Every line carries `trace`: JSON pointers into the packet it was read from. Unknown stays unknown, an
//! abstaining or conflicting axis says so, and a rule vote is worded as an inference, never as a decision.
//! Coldness reads "no activity evidence seen since <month>". Nothing here approves or advises an effect.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::labelling::{combine, AbstainReason, Axis, Outcome};
use crate::packet::validate_packet;
use crate::LearnError;

/// One human-readable sentence and the packet fields (JSON pointers) it rests on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub trace: Vec<String>,
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAY: u64 = 86_400;

/// "March 2024" from Unix seconds (proleptic Gregorian, UTC; Hinnant's civil-from-days).
fn month(seconds: u64) -> String {
    let z = (seconds / DAY) as i64 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(m <= 2);
    format!("{} {year}", MONTHS[m as usize - 1])
}

fn size(bytes: u64) -> String {
    let mut unit = 1u128;
    for name in ["B", "KiB", "MiB", "GiB", "TiB", "PiB"] {
        if u128::from(bytes) < unit * 1024 || name == "PiB" {
            if unit == 1 {
                return format!("{bytes} B");
            }
            let tenths = u128::from(bytes) % unit * 10 / unit;
            return format!("{}.{tenths} {name}", u128::from(bytes) / unit);
        }
        unit *= 1024;
    }
    unreachable!()
}

/// Whole-percent share for display, always rounded DOWN so the shown shares of one column can never sum past
/// 100 (2 x 33.5% reads 33% + 33%, not 34% + 34%). A non-zero share below 1% reads "<1%", never "0%", because
/// "0%" reads as absence; only an exact zero prints "0%". Basis points absorb float noise (0.29 * 100 < 29).
fn percent(share: f64) -> String {
    let basis_points = (share * 10_000.0).round() as u64;
    match basis_points / 100 {
        0 if share > 0.0 => "<1%".to_owned(),
        whole => format!("{whole}%"),
    }
}

/// Qualifier for any figure that double-counts hard-linked files when link counts were not observed.
const LINKS_UNOBSERVED: &str = " (link counts not observed)";

fn words(id: &str) -> String {
    id.replace('_', " ")
}

fn line(text: String, trace: &[&str]) -> Line {
    let trace: BTreeSet<_> = trace.iter().map(|pointer| (*pointer).to_owned()).collect();
    Line {
        text,
        trace: trace.into_iter().collect(),
    }
}

fn count(value: &Value, noun: &str) -> String {
    value
        .as_u64()
        .map_or_else(|| format!("{noun} unknown"), |n| format!("{n} {noun}"))
}

fn when(value: &Value) -> String {
    value.as_u64().map_or_else(|| "unknown".into(), month)
}

/// The packet fields each labelling function reads (kept beside `labelling::votes`).
pub fn function_inputs(function: &str) -> Option<&'static [&'static str]> {
    Some(match function {
        "lf_photo_shoot" | "lf_documents" | "lf_installers" | "lf_archives" => {
            &["/families", "/markers"]
        }
        "lf_git_dirty" => &["/time/git_dirty"],
        "lf_recent_write" | "lf_quiet_months" | "lf_quiet_years" => &[
            "/observed_at",
            "/time/newest_content_write",
            "/time/newest_source_write",
        ],
        "lf_source_manifest"
        | "lf_model_layout"
        | "lf_virtual_disk"
        | "lf_lightroom_catalogue"
        | "lf_lightroom_backups"
        | "lf_hub_layout"
        | "lf_project_with_outputs"
        | "lf_catalogue_previews"
        | "lf_catalogue_only"
        | "lf_camera_originals" => &["/markers"],
        _ => return None,
    })
}

fn inputs(function: &str) -> &'static [&'static str] {
    function_inputs(function)
        .unwrap_or_else(|| panic!("labelling function {function} has no explain trace mapping"))
}

fn axis_name(axis: Axis) -> &'static str {
    match axis {
        Axis::Meaning => "Meaning",
        Axis::Lifecycle => "Lifecycle",
        Axis::Recreatability => "Recreatability",
    }
}

fn axis_line(axis: Axis, outcome: &Outcome) -> Line {
    let name = axis_name(axis);
    match outcome {
        Outcome::Label { label, functions } => {
            let mut trace = BTreeSet::new();
            trace.extend(functions.iter().flat_map(|f| inputs(f)));
            line(
                format!(
                    "{name}: rule evidence ({}) points to \"{}\". This is an inference from the facts above, not a recorded fact.",
                    functions.join(", "),
                    words(label)
                ),
                &trace.into_iter().collect::<Vec<_>>(),
            )
        }
        Outcome::Abstain(AbstainReason::NoVote) => line(
            format!("{name}: no rule had enough evidence, so no label is given."),
            &[
                "/coverage/state",
                "/families",
                "/markers",
                "/observed_at",
                "/time/git_dirty",
                "/time/newest_content_write",
                "/time/newest_source_write",
            ],
        ),
        Outcome::Abstain(AbstainReason::Conflict(votes)) => {
            let mut trace = BTreeSet::new();
            trace.extend(votes.iter().flat_map(|v| inputs(v.function)));
            let detail: Vec<_> = votes
                .iter()
                .map(|v| format!("{} says \"{}\"", v.function, words(v.label)))
                .collect();
            line(
                format!(
                    "{name}: rules disagree ({}), so no label is given.",
                    detail.join("; ")
                ),
                &trace.into_iter().collect::<Vec<_>>(),
            )
        }
        Outcome::Abstain(AbstainReason::Coverage(state)) => line(
            format!("{name}: no label is given because coverage is {state}."),
            &["/coverage/state"],
        ),
    }
}

/// Newest of every dated activity field, never content writes alone; a dirty working tree is itself
/// activity evidence, so inactivity is not claimed then. Future dates make no statement.
///
/// This is deliberately wider than the `lf_recent_write` / `lf_quiet_*` labelling rules (`labelling::votes`),
/// which age only the newest content write (falling back to the source write): they are the frozen
/// categoriser-v1 oracle shared with Python. This line is a plain fact about every dated field, so a recent
/// generated write or commit can make it newer than the age those rules vote on; the two are not reconciled.
fn coldness(packet: &Value) -> Line {
    const TRACE: [&str; 6] = [
        "/observed_at",
        "/time/git_dirty",
        "/time/git_last_commit",
        "/time/newest_content_write",
        "/time/newest_generated_write",
        "/time/newest_source_write",
    ];
    let time = &packet["time"];
    if time["git_dirty"] == true {
        return line(
            "Activity evidence is present: the git working tree has uncommitted changes."
                .to_owned(),
            &TRACE,
        );
    }
    let newest = [
        ("newest content change", "newest_content_write"),
        ("newest source write", "newest_source_write"),
        ("newest generated write", "newest_generated_write"),
        ("last git commit", "git_last_commit"),
    ]
    .into_iter()
    .filter_map(|(label, key)| time[key].as_u64().map(|at| (at, label)))
    .reduce(|best, next| if next.0 > best.0 { next } else { best });
    let text = match newest {
        None => "No dated change is known, so no statement about activity is made.".to_owned(),
        Some((at, label)) => match packet["observed_at"].as_u64().unwrap().checked_sub(at) {
            None => format!(
                "The {label} is dated {}, after this packet was observed, so no statement about activity is made.",
                month(at)
            ),
            Some(seconds) => {
                let days = seconds / DAY;
                let unit = if days == 1 { "day" } else { "days" };
                format!(
                    "No activity evidence seen since {} ({label}, {days} {unit} before this packet was observed).",
                    month(at)
                )
            }
        },
    };
    line(text, &TRACE)
}

/// Validate the packet, then render facts first and the labelling outcome last. Deterministic.
pub fn explain(packet: &Value) -> Result<Vec<Line>, LearnError> {
    validate_packet(packet)?;
    let outcomes = combine(packet)?;
    let state = packet["coverage"]["state"].as_str().unwrap();
    let observed = !matches!(
        state,
        "denied" | "unscanned" | "excluded" | "cancelled" | "unknown"
    );
    let mut lines = Vec::new();

    let coverage = &packet["coverage"];
    let mut text = match state {
        "complete" => "Coverage is complete.".to_owned(),
        "partial" => "Coverage is partial, so counts and dates may be incomplete.".to_owned(),
        "stale" => "Coverage is stale: the figures describe an earlier scan.".to_owned(),
        other => {
            format!("Nothing was observed (coverage is {other}), so every figure below is unknown.")
        }
    };
    if coverage["online"] == false {
        text.push_str(" It was offline when observed.");
    }
    if observed && coverage["allocation_known"] == false {
        text.push_str(" Allocated size was not measured.");
    }
    lines.push(line(text, &["/coverage"]));

    let tokens: Vec<_> = packet["subject"]["name_tokens"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    lines.push(line(
        if tokens.is_empty() {
            "No folder name words are recorded.".to_owned()
        } else {
            format!("Folder name words: {}.", tokens.join(", "))
        },
        &["/subject/name_tokens"],
    ));

    let links_unobserved = coverage["link_counts_observed"] == false;
    let shape = &packet["shape"];
    lines.push(line(
        format!(
            "Contents: {}; {}; logical size {}{}; allocated size {}; deepest nesting {}.",
            count(&shape["files"], "files"),
            count(&shape["folders"], "folders"),
            shape["logical_bytes"]
                .as_u64()
                .map_or("unknown".into(), size),
            if links_unobserved && shape["logical_bytes"].is_u64() {
                LINKS_UNOBSERVED
            } else {
                ""
            },
            shape["allocated_bytes"]
                .as_u64()
                .map_or("unknown".into(), size),
            shape["max_depth"]
                .as_u64()
                .map_or("unknown".into(), |d| d.to_string()),
        ),
        &[
            "/coverage/link_counts_observed",
            "/shape/allocated_bytes",
            "/shape/files",
            "/shape/folders",
            "/shape/logical_bytes",
            "/shape/max_depth",
        ],
    ));

    let mut families: Vec<_> = packet["families"].as_array().unwrap().iter().collect();
    families.sort_by(|a, b| {
        b["share_files"]
            .as_f64()
            .partial_cmp(&a["share_files"].as_f64())
            .unwrap()
            .then_with(|| a["family"].as_str().cmp(&b["family"].as_str()))
    });
    lines.push(line(
        if families.is_empty() {
            if observed {
                "No file types are recorded.".to_owned()
            } else {
                "File types are unknown.".to_owned()
            }
        } else {
            let parts: Vec<_> = families
                .iter()
                .map(|f| {
                    let bytes = f["share_bytes"].as_f64().map_or_else(
                        || "bytes share unknown".to_owned(),
                        |s| format!("{} of bytes", percent(s)),
                    );
                    format!(
                        "{} {} of files ({bytes})",
                        f["family"].as_str().unwrap(),
                        percent(f["share_files"].as_f64().unwrap())
                    )
                })
                .collect();
            format!("File types by share: {}.", parts.join("; "))
        },
        &["/families"],
    ));

    let mut extensions: Vec<_> = packet["extensions"].as_array().unwrap().iter().collect();
    extensions.sort_by(|a, b| {
        b["files"]
            .as_u64()
            .cmp(&a["files"].as_u64())
            .then_with(|| a["ext"].as_str().cmp(&b["ext"].as_str()))
    });
    lines.push(line(
        if extensions.is_empty() {
            if observed {
                "No file extensions are recorded.".to_owned()
            } else {
                "File extensions are unknown.".to_owned()
            }
        } else {
            let parts: Vec<_> = extensions
                .iter()
                .take(5)
                .map(|e| match e["ext"].as_str().unwrap() {
                    "" => format!("(no extension) {} files", e["files"]),
                    ext => format!(".{ext} {} files", e["files"]),
                })
                .collect();
            format!("Most common extensions: {}.", parts.join("; "))
        },
        &["/extensions"],
    ));

    let markers: Vec<_> = packet["markers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| words(m.as_str().unwrap()))
        .collect();
    lines.push(line(
        if markers.is_empty() {
            if observed {
                "No project or layout markers were seen.".to_owned()
            } else {
                "Project and layout markers are unknown.".to_owned()
            }
        } else {
            format!("Markers seen: {}.", markers.join(", "))
        },
        &["/markers"],
    ));

    let time = &packet["time"];
    lines.push(coldness(packet));
    lines.push(line(
        format!(
            "Newest source write: {}; newest generated write: {}.",
            when(&time["newest_source_write"]),
            when(&time["newest_generated_write"])
        ),
        &["/time/newest_generated_write", "/time/newest_source_write"],
    ));
    lines.push(line(
        format!(
            "Git last commit: {}; working tree: {}.",
            when(&time["git_last_commit"]),
            match time["git_dirty"].as_bool() {
                Some(true) => "has uncommitted changes",
                Some(false) => "clean",
                None => "unknown",
            }
        ),
        &["/time/git_dirty", "/time/git_last_commit"],
    ));
    lines.push(line(
        "Last-access time is system-managed and is ignored as evidence.".to_owned(),
        &["/time/last_access"],
    ));

    let duplicates = &packet["duplicates"];
    lines.push(line(
        format!(
            "Duplicate bytes: {}; duplicate groups: {}{}.",
            duplicates["duplicate_bytes"]
                .as_u64()
                .map_or("unknown".into(), size),
            duplicates["groups"]
                .as_u64()
                .map_or("unknown".into(), |g| g.to_string()),
            if links_unobserved
                && (duplicates["duplicate_bytes"].is_u64() || duplicates["groups"].is_u64())
            {
                LINKS_UNOBSERVED
            } else {
                ""
            },
        ),
        &[
            "/coverage/link_counts_observed",
            "/duplicates/duplicate_bytes",
            "/duplicates/groups",
        ],
    ));

    let media = &packet["media"];
    lines.push(line(
        if media.is_null() {
            if observed {
                "No camera-file details are recorded.".to_owned()
            } else {
                "Camera-file details are unknown.".to_owned()
            }
        } else {
            format!(
                "Camera files: {} raw, {} sidecar, {} raw with a sidecar; capture dates {} to {}; camera models {}.",
                media["raw_files"],
                media["sidecar_files"],
                media["raw_with_sidecar"],
                when(&media["capture_start"]),
                when(&media["capture_end"]),
                media["camera_models"].as_u64().map_or("unknown".into(), |n| n.to_string()),
            )
        },
        &["/media"],
    ));

    if let Some(qualifier) = match state {
        "partial" => Some("partial"),
        "stale" => Some("stale"),
        _ => None,
    } {
        for fact in lines
            .iter_mut()
            .skip(2)
            .filter(|fact| fact.trace != ["/time/last_access"])
        {
            let body = fact.text.strip_suffix('.').unwrap_or(&fact.text);
            fact.text = format!("{body} ({qualifier} coverage).");
        }
    }

    lines.extend(
        outcomes
            .iter()
            .map(|(axis, outcome)| axis_line(*axis, outcome)),
    );
    Ok(lines)
}
