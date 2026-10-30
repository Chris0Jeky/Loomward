//! Evidence packets contain facts only: never an approval, never a probability.
//! Human and teacher labels stay separate by provenance (invariant 3).
//! Null is unknown, never zero (invariant 4).

use std::collections::BTreeSet;

use serde_json::Value;

use crate::features::{casefold, integer, word_runs, SIZE_MAX};
use crate::LearnError;

const UNIX_MAX: u64 = 32_503_680_000;
pub const FAMILIES: &[&str] = &[
    "document",
    "spreadsheet",
    "presentation",
    "image",
    "video",
    "audio",
    "archive",
    "code",
    "data",
    "model",
    "executable",
    "system",
    "font",
    "other",
    "none",
];
pub const MARKERS: &[&str] = &[
    "cargo_toml",
    "cargo_target_dir",
    "package_json",
    "node_modules_dir",
    "pnpm_lock",
    "pyproject_toml",
    "requirements_txt",
    "python_venv_dir",
    "pycache_dir",
    "dotnet_project",
    "dotnet_bin_obj_dirs",
    "gradle_build",
    "gradle_cache_dir",
    "git_dir",
    "git_file_worktree",
    "cachedir_tag",
    "gguf_file",
    "safetensors_file",
    "hf_hub_layout",
    "lmstudio_models_layout",
    "ollama_blobs_layout",
    "vhdx_file",
    "lightroom_catalog",
    "lightroom_catalog_lock",
    "lightroom_previews",
    "lightroom_backups_dated",
    "dcim_layout",
    "raw_xmp_pairs",
    "installer_files",
    "archive_files",
    "office_documents",
    "loomward_staging",
];
pub const MEANINGS: &[&str] = &[
    "source_project",
    "generated_build",
    "dependency_install",
    "package_cache",
    "model_weights",
    "virtual_disk",
    "photo_shoot",
    "photo_catalogue",
    "catalogue_backup",
    "document_collection",
    "archive",
    "download",
    "installer",
    "agent_worktree",
    "unknown",
];
pub const LIFECYCLES: &[&str] = &["active", "dormant", "historical", "unknown"];
pub const RECREATABILITIES: &[&str] =
    &["generated", "redownloadable", "unique", "mixed", "unknown"];
pub const COVERAGE_STATES: &[&str] = &[
    "complete",
    "partial",
    "stale",
    "unscanned",
    "denied",
    "excluded",
    "cancelled",
    "unknown",
];
pub const MAX_NAME_TOKENS: usize = 16;
pub const MAX_FAMILIES: usize = 15;
pub const MAX_EXTENSIONS: usize = 16;
pub const MAX_MARKERS: usize = 32;
pub const MAX_LABELS: usize = 3;
pub const MAX_SUMMARY_FACTS: usize = 12;

fn exact_keys(value: &Value, name: &str, keys: &[&str]) -> Result<(), LearnError> {
    match value.as_object() {
        Some(object)
            if object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key)) =>
        {
            Ok(())
        }
        _ => Err(LearnError::new(format!("Invalid keys for {name}"))),
    }
}

fn array<'a>(value: &'a Value, name: &str, maximum: usize) -> Result<&'a [Value], LearnError> {
    value
        .as_array()
        .filter(|items| items.len() <= maximum)
        .map(Vec::as_slice)
        .ok_or_else(|| LearnError::new(format!("Invalid {name}: expected at most {maximum} items")))
}

fn text<'a>(
    value: &'a Value,
    name: &str,
    minimum: usize,
    maximum: usize,
) -> Result<&'a str, LearnError> {
    value
        .as_str()
        .filter(|text| (minimum..=maximum).contains(&text.chars().count()))
        .ok_or_else(|| LearnError::new(format!("Invalid {name}")))
}

fn id(value: &Value, name: &str) -> Result<(), LearnError> {
    let text = text(value, name, 1, 64)?;
    if !text.as_bytes()[0].is_ascii_lowercase() && !text.as_bytes()[0].is_ascii_digit()
        || !text
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
    {
        return Err(LearnError::new(format!("Invalid {name}")));
    }
    Ok(())
}

fn choice<'a>(value: &'a Value, name: &str, choices: &[&str]) -> Result<&'a str, LearnError> {
    value
        .as_str()
        .filter(|found| choices.contains(found))
        .ok_or_else(|| LearnError::new(format!("Invalid {name}")))
}

fn boolean(value: &Value, name: &str) -> Result<bool, LearnError> {
    value
        .as_bool()
        .ok_or_else(|| LearnError::new(format!("Invalid {name}")))
}

fn unsigned(value: &Value, name: &str, maximum: u64) -> Result<u64, LearnError> {
    integer(Some(value), name, 0, maximum)
}

fn nullable_unsigned(value: &Value, name: &str, maximum: u64) -> Result<Option<u64>, LearnError> {
    if value.is_null() {
        Ok(None)
    } else {
        unsigned(value, name, maximum).map(Some)
    }
}

fn share(value: &Value, name: &str) -> Result<f64, LearnError> {
    value
        .as_f64()
        .filter(|number| (0.0..=1.0).contains(number))
        .ok_or_else(|| LearnError::new(format!("Invalid {name}: expected a share in [0, 1]")))
}

fn no_path(text: &str, name: &str) -> Result<(), LearnError> {
    // This also excludes drive-relative paths and both Windows and slash-form UNC paths.
    if text.contains(['/', '\\', ':']) {
        return Err(LearnError::new(format!("Invalid path-like {name}")));
    }
    Ok(())
}

fn validate_families(value: &Value) -> Result<(), LearnError> {
    let mut seen = BTreeSet::new();
    let mut files = 0.0;
    let mut bytes = 0.0;
    for family in array(value, "families", MAX_FAMILIES)? {
        exact_keys(family, "family", &["family", "share_files", "share_bytes"])?;
        let name = choice(&family["family"], "family", FAMILIES)?;
        if !seen.insert(name) {
            return Err(LearnError::new("Duplicate family"));
        }
        files += share(&family["share_files"], "share_files")?;
        if !family["share_bytes"].is_null() {
            bytes += share(&family["share_bytes"], "share_bytes")?;
        }
    }
    if files > 1.000001 {
        return Err(LearnError::new("Family share_files sum exceeds 1.000001"));
    }
    if bytes > 1.000001 {
        return Err(LearnError::new("Family share_bytes sum exceeds 1.000001"));
    }
    Ok(())
}

fn validate_time(value: &Value) -> Result<(), LearnError> {
    exact_keys(
        value,
        "time",
        &[
            "newest_content_write",
            "oldest_content_write",
            "newest_source_write",
            "newest_generated_write",
            "git_last_commit",
            "git_dirty",
            "last_access",
        ],
    )?;
    let newest = nullable_unsigned(
        &value["newest_content_write"],
        "newest_content_write",
        UNIX_MAX,
    )?;
    let oldest = nullable_unsigned(
        &value["oldest_content_write"],
        "oldest_content_write",
        UNIX_MAX,
    )?;
    for key in [
        "newest_source_write",
        "newest_generated_write",
        "git_last_commit",
    ] {
        nullable_unsigned(&value[key], key, UNIX_MAX)?;
    }
    if !value["git_dirty"].is_null() {
        boolean(&value["git_dirty"], "git_dirty")?;
    }
    choice(&value["last_access"], "last_access", &["not_evidence"])?;
    if matches!((oldest, newest), (Some(oldest), Some(newest)) if oldest > newest) {
        return Err(LearnError::new(
            "oldest_content_write exceeds newest_content_write",
        ));
    }
    Ok(())
}

fn validate_media(value: &Value) -> Result<(), LearnError> {
    if value.is_null() {
        return Ok(());
    }
    exact_keys(
        value,
        "media",
        &[
            "raw_files",
            "sidecar_files",
            "raw_with_sidecar",
            "capture_start",
            "capture_end",
            "camera_models",
        ],
    )?;
    let raw = unsigned(&value["raw_files"], "raw_files", SIZE_MAX)?;
    let sidecars = unsigned(&value["sidecar_files"], "sidecar_files", SIZE_MAX)?;
    let paired = unsigned(&value["raw_with_sidecar"], "raw_with_sidecar", SIZE_MAX)?;
    let start = nullable_unsigned(&value["capture_start"], "capture_start", UNIX_MAX)?;
    let end = nullable_unsigned(&value["capture_end"], "capture_end", UNIX_MAX)?;
    nullable_unsigned(&value["camera_models"], "camera_models", SIZE_MAX)?;
    if matches!((start, end), (Some(start), Some(end)) if start > end) {
        return Err(LearnError::new("capture_start exceeds capture_end"));
    }
    if paired > raw.min(sidecars) {
        return Err(LearnError::new(
            "raw_with_sidecar exceeds raw_files or sidecar_files",
        ));
    }
    Ok(())
}

/// Validate the closed packet contract and the privacy, consistency and honesty invariants.
pub fn validate_packet(value: &Value) -> Result<(), LearnError> {
    exact_keys(
        value,
        "packet",
        &[
            "schema",
            "synthetic",
            "packet_id",
            "group_id",
            "observed_at",
            "subject",
            "shape",
            "families",
            "extensions",
            "markers",
            "time",
            "duplicates",
            "media",
            "coverage",
        ],
    )?;
    choice(
        &value["schema"],
        "packet schema",
        &["loomward.evidence-packet.v1"],
    )?;
    boolean(&value["synthetic"], "packet synthetic")?;
    id(&value["packet_id"], "packet_id")?;
    id(&value["group_id"], "group_id")?;
    unsigned(&value["observed_at"], "observed_at", UNIX_MAX)?;

    let subject = &value["subject"];
    exact_keys(subject, "subject", &["kind", "name_tokens", "depth"])?;
    choice(&subject["kind"], "subject.kind", &["folder", "file_set"])?;
    unsigned(&subject["depth"], "depth", 4096)?;
    for token in array(&subject["name_tokens"], "name_tokens", MAX_NAME_TOKENS)? {
        let token = text(token, "name_token", 1, 64)?;
        no_path(token, "name_token")?;
        if casefold(token) != token {
            return Err(LearnError::new("name_tokens must already be casefolded"));
        }
        // One word run each, so a raw name ("tax return 2024.pdf") never passes as a token.
        if word_runs(token) != [token] {
            return Err(LearnError::new("name_tokens must be single word runs"));
        }
    }

    let shape = &value["shape"];
    exact_keys(
        shape,
        "shape",
        &[
            "files",
            "folders",
            "logical_bytes",
            "allocated_bytes",
            "max_depth",
        ],
    )?;
    let files = nullable_unsigned(&shape["files"], "files", SIZE_MAX)?;
    nullable_unsigned(&shape["folders"], "folders", SIZE_MAX)?;
    nullable_unsigned(&shape["logical_bytes"], "logical_bytes", SIZE_MAX)?;
    let allocated = nullable_unsigned(&shape["allocated_bytes"], "allocated_bytes", SIZE_MAX)?;
    nullable_unsigned(&shape["max_depth"], "max_depth", 4096)?;

    validate_families(&value["families"])?;
    let mut extensions = BTreeSet::new();
    let mut extension_files = 0u64;
    for extension in array(&value["extensions"], "extensions", MAX_EXTENSIONS)? {
        exact_keys(extension, "extension", &["ext", "files", "bytes"])?;
        let ext = text(&extension["ext"], "ext", 0, 32)?;
        no_path(ext, "ext")?;
        if !ext
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_+-".contains(&c))
        {
            return Err(LearnError::new("Invalid ext"));
        }
        if !extensions.insert(ext) {
            return Err(LearnError::new("Duplicate ext"));
        }
        // At most 16 JS-safe counts fit comfortably in u64.
        extension_files += unsigned(&extension["files"], "extension files", SIZE_MAX)?;
        nullable_unsigned(&extension["bytes"], "extension bytes", SIZE_MAX)?;
    }
    if files.is_some_and(|files| extension_files > files) {
        return Err(LearnError::new("Extension files sum exceeds shape.files"));
    }
    let mut markers = BTreeSet::new();
    for marker in array(&value["markers"], "markers", MAX_MARKERS)? {
        let marker = choice(marker, "marker", MARKERS)?;
        if !markers.insert(marker) {
            return Err(LearnError::new("Duplicate marker"));
        }
    }
    validate_time(&value["time"])?;
    exact_keys(
        &value["duplicates"],
        "duplicates",
        &["duplicate_bytes", "groups"],
    )?;
    for key in ["duplicate_bytes", "groups"] {
        nullable_unsigned(&value["duplicates"][key], key, SIZE_MAX)?;
    }
    validate_media(&value["media"])?;

    let coverage = &value["coverage"];
    exact_keys(
        coverage,
        "coverage",
        &[
            "state",
            "allocation_known",
            "link_counts_observed",
            "online",
        ],
    )?;
    let state = choice(&coverage["state"], "coverage.state", COVERAGE_STATES)?;
    let allocation_known = boolean(&coverage["allocation_known"], "allocation_known")?;
    boolean(&coverage["link_counts_observed"], "link_counts_observed")?;
    boolean(&coverage["online"], "online")?;
    if matches!(
        state,
        "denied" | "unscanned" | "excluded" | "cancelled" | "unknown"
    ) {
        for pointer in [
            "/shape/files",
            "/shape/folders",
            "/shape/logical_bytes",
            "/shape/allocated_bytes",
            "/shape/max_depth",
            "/duplicates/duplicate_bytes",
            "/duplicates/groups",
            "/media",
            "/time/newest_content_write",
            "/time/oldest_content_write",
            "/time/newest_source_write",
            "/time/newest_generated_write",
            "/time/git_last_commit",
            "/time/git_dirty",
        ] {
            if !value.pointer(pointer).unwrap().is_null() {
                return Err(LearnError::new(format!(
                    "{pointer} must be unknown for unobserved coverage"
                )));
            }
        }
        for key in ["families", "extensions", "markers"] {
            if !value[key].as_array().unwrap().is_empty() {
                return Err(LearnError::new(format!(
                    "/{key} must be unknown for unobserved coverage"
                )));
            }
        }
    }
    if !allocation_known && allocated.is_some() {
        return Err(LearnError::new(
            "allocated_bytes must be null when allocation is unknown",
        ));
    }
    if allocation_known && files.is_some_and(|files| files > 0) && allocated.is_none() {
        return Err(LearnError::new(
            "allocated_bytes must be present when allocation is known and files is positive",
        ));
    }
    if state != "complete"
        && allocation_known
        && allocated == Some(0)
        && files.is_some_and(|files| files > 0)
    {
        return Err(LearnError::new(
            "Incomplete coverage cannot claim zero allocation for non-empty subjects",
        ));
    }
    Ok(())
}

fn validate_labels(value: &Value, name: &str) -> Result<(), LearnError> {
    let mut axes = BTreeSet::new();
    for label in array(value, name, MAX_LABELS)? {
        exact_keys(label, "label", &["axis", "label"])?;
        let axis = choice(
            &label["axis"],
            "label axis",
            &["meaning", "lifecycle", "recreatability"],
        )?;
        let labels = match axis {
            "meaning" => MEANINGS,
            "lifecycle" => LIFECYCLES,
            _ => RECREATABILITIES,
        };
        choice(&label["label"], "label for axis", labels)?;
        if !axes.insert(axis) {
            return Err(LearnError::new(format!("Duplicate label axis in {name}")));
        }
    }
    Ok(())
}

/// Keep labels by provenance and resolve summary evidence only inside the validated packet.
pub fn validate_record(value: &Value) -> Result<(), LearnError> {
    exact_keys(
        value,
        "record",
        &[
            "schema",
            "synthetic",
            "record_id",
            "packet",
            "human_labels",
            "teacher_labels",
            "summary_facts",
        ],
    )?;
    choice(
        &value["schema"],
        "record schema",
        &["loomward.categoriser-record.v1"],
    )?;
    let synthetic = boolean(&value["synthetic"], "record synthetic")?;
    id(&value["record_id"], "record_id")?;
    let packet = &value["packet"];
    validate_packet(packet)?;
    if packet["synthetic"] != synthetic {
        return Err(LearnError::new(
            "Record and packet synthetic flags must match",
        ));
    }
    validate_labels(&value["human_labels"], "human_labels")?;
    validate_labels(&value["teacher_labels"], "teacher_labels")?;
    for pointer in array(&value["summary_facts"], "summary_facts", MAX_SUMMARY_FACTS)? {
        let pointer = text(pointer, "summary_facts pointer", 2, 96)?;
        if !pointer.starts_with('/')
            || !pointer
                .bytes()
                .skip(1)
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_/".contains(&c))
        {
            return Err(LearnError::new("Invalid summary_facts pointer"));
        }
        if packet.pointer(pointer).is_none() {
            return Err(LearnError::new(
                "summary_facts pointer does not resolve inside packet",
            ));
        }
    }
    Ok(())
}

/// Accept only bounded, explicitly synthetic corpora with unique record and packet identities.
pub fn validate_corpus(value: &Value) -> Result<usize, LearnError> {
    let records = value
        .as_array()
        .filter(|records| !records.is_empty() && records.len() <= 10_000)
        .ok_or_else(|| LearnError::new("Corpus needs 1..10000 records"))?;
    let mut record_ids = BTreeSet::new();
    let mut packet_ids = BTreeSet::new();
    for record in records {
        if record["synthetic"] != true {
            return Err(LearnError::new(
                "Only explicitly synthetic corpora are accepted",
            ));
        }
        validate_record(record)?;
        if !record_ids.insert(record["record_id"].as_str().unwrap()) {
            return Err(LearnError::new("Duplicate record_id"));
        }
        if !packet_ids.insert(record["packet"]["packet_id"].as_str().unwrap()) {
            return Err(LearnError::new("Duplicate packet_id"));
        }
    }
    Ok(records.len())
}
