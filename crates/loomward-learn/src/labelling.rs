//! Rules vote or abstain; no output is calibrated or an approval.

use serde_json::Value;

use crate::{packet::validate_packet, LearnError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Axis {
    Meaning,
    Lifecycle,
    Recreatability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vote {
    pub function: &'static str,
    pub axis: Axis,
    pub label: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Label {
        label: &'static str,
        functions: Vec<&'static str>,
    },
    Abstain(AbstainReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbstainReason {
    NoVote,
    Conflict(Vec<Vote>),
    Coverage(String),
}

pub const FUNCTIONS: &[&str] = &[
    "lf_source_manifest",
    "lf_model_layout",
    "lf_virtual_disk",
    "lf_lightroom_catalogue",
    "lf_lightroom_backups",
    "lf_photo_shoot",
    "lf_documents",
    "lf_installers",
    "lf_archives",
    "lf_git_dirty",
    "lf_recent_write",
    "lf_quiet_months",
    "lf_quiet_years",
    "lf_hub_layout",
    "lf_project_with_outputs",
    "lf_catalogue_previews",
    "lf_catalogue_only",
    "lf_camera_originals",
];

fn share(packet: &Value, family: &str, key: &str) -> Option<f64> {
    packet["families"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["family"] == family)
        .and_then(|entry| entry[key].as_f64())
}

/// Validate first, then return only non-abstaining votes, ordered by axis and function.
pub fn votes(packet: &Value) -> Result<Vec<Vote>, LearnError> {
    validate_packet(packet)?;
    if packet["coverage"]["state"] != "complete" {
        return Ok(Vec::new());
    }

    use Axis::{Lifecycle, Meaning, Recreatability};

    let markers = packet["markers"].as_array().unwrap();
    let has = |marker: &str| markers.iter().any(|value| value == marker);
    let has_any = |wanted: &[&str]| wanted.iter().any(|marker| has(marker));
    let manifest = has_any(&[
        "cargo_toml",
        "package_json",
        "pyproject_toml",
        "requirements_txt",
        "dotnet_project",
        "gradle_build",
    ]);
    let generated = has_any(&[
        "cargo_target_dir",
        "node_modules_dir",
        "python_venv_dir",
        "pycache_dir",
        "dotnet_bin_obj_dirs",
        "gradle_cache_dir",
    ]);
    let hub = has_any(&[
        "hf_hub_layout",
        "lmstudio_models_layout",
        "ollama_blobs_layout",
    ]);
    let catalogue = has("lightroom_catalog");
    let previews = has("lightroom_previews");
    let backups = has("lightroom_backups_dated");
    let camera = has_any(&["dcim_layout", "raw_xmp_pairs"]);
    let office_share: f64 = ["document", "spreadsheet", "presentation"]
        .iter()
        .map(|family| share(packet, family, "share_files").unwrap_or(0.0))
        .sum();

    let time = &packet["time"];
    let age = time["newest_content_write"]
        .as_u64()
        .or_else(|| time["newest_source_write"].as_u64())
        .and_then(|written| packet["observed_at"].as_u64().unwrap().checked_sub(written));
    const DAY: u64 = 86_400;

    let rules = [
        ("lf_source_manifest", Meaning, "source_project", manifest),
        (
            "lf_model_layout",
            Meaning,
            "model_weights",
            (hub || has_any(&["gguf_file", "safetensors_file"])) && !manifest,
        ),
        (
            "lf_virtual_disk",
            Meaning,
            "virtual_disk",
            has("vhdx_file") && !manifest,
        ),
        (
            "lf_lightroom_catalogue",
            Meaning,
            "photo_catalogue",
            catalogue && !backups,
        ),
        ("lf_lightroom_backups", Meaning, "catalogue_backup", backups),
        (
            "lf_photo_shoot",
            Meaning,
            "photo_shoot",
            camera && share(packet, "image", "share_files").is_some_and(|share| share >= 0.5),
        ),
        (
            "lf_documents",
            Meaning,
            "document_collection",
            has("office_documents") && office_share >= 0.5 && !manifest,
        ),
        (
            "lf_installers",
            Meaning,
            "installer",
            has("installer_files")
                && share(packet, "executable", "share_bytes").is_some_and(|share| share >= 0.5),
        ),
        (
            "lf_archives",
            Meaning,
            "archive",
            has("archive_files")
                && share(packet, "archive", "share_bytes").is_some_and(|share| share >= 0.5),
        ),
        (
            "lf_git_dirty",
            Lifecycle,
            "active",
            time["git_dirty"] == true,
        ),
        (
            "lf_recent_write",
            Lifecycle,
            "active",
            age.is_some_and(|age| age <= 30 * DAY),
        ),
        (
            "lf_quiet_months",
            Lifecycle,
            "dormant",
            age.is_some_and(|age| age > 90 * DAY && age <= 365 * DAY),
        ),
        (
            "lf_quiet_years",
            Lifecycle,
            "historical",
            age.is_some_and(|age| age > 365 * DAY),
        ),
        ("lf_hub_layout", Recreatability, "redownloadable", hub),
        (
            "lf_project_with_outputs",
            Recreatability,
            "mixed",
            manifest && generated,
        ),
        (
            "lf_catalogue_previews",
            Recreatability,
            "mixed",
            catalogue && previews,
        ),
        (
            "lf_catalogue_only",
            Recreatability,
            "unique",
            catalogue && !previews,
        ),
        ("lf_camera_originals", Recreatability, "unique", camera),
    ];
    let mut votes: Vec<_> = rules
        .into_iter()
        .filter(|(_, _, _, applies)| *applies)
        .map(|(function, axis, label, _)| Vote {
            function,
            axis,
            label,
        })
        .collect();
    votes.sort_unstable_by_key(|vote| (vote.axis, vote.function));
    Ok(votes)
}

/// Agree per axis or abstain, retaining the sorted provenance of labels and conflicts.
pub fn combine(packet: &Value) -> Result<[(Axis, Outcome); 3], LearnError> {
    let votes = votes(packet)?;
    let state = packet["coverage"]["state"].as_str().unwrap();
    Ok(
        [Axis::Meaning, Axis::Lifecycle, Axis::Recreatability].map(|axis| {
            let outcome = if state != "complete" {
                Outcome::Abstain(AbstainReason::Coverage(state.to_owned()))
            } else {
                let axis_votes: Vec<_> = votes
                    .iter()
                    .filter(|vote| vote.axis == axis)
                    .cloned()
                    .collect();
                match axis_votes.first() {
                    None => Outcome::Abstain(AbstainReason::NoVote),
                    Some(first) if axis_votes.iter().all(|vote| vote.label == first.label) => {
                        Outcome::Label {
                            label: first.label,
                            functions: axis_votes.iter().map(|vote| vote.function).collect(),
                        }
                    }
                    Some(_) => Outcome::Abstain(AbstainReason::Conflict(axis_votes)),
                }
            };
            (axis, outcome)
        }),
    )
}
