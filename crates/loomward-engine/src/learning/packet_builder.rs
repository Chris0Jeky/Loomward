//! Pure projection from catalogue facts to the closed evidence-packet-v1 contract.
//! No filesystem, process, Git, EXIF or network probes; unknown evidence stays null.

use loomward_catalog::EvidenceFacts;
use loomward_learn::{
    features::{casefold, word_runs},
    packet::validate_packet,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
/// Caller-supplied identity, lineage and observation metadata; never derived from a path.
pub struct PacketIds {
    /// Opaque packet identifier in the packet schema's bounded ID vocabulary.
    pub packet_id: String,
    /// Shared lineage identifier for grouped evaluation splits.
    pub group_id: String,
    /// Unix seconds when the catalogue facts were observed.
    pub observed_at: u64,
    /// Whether these observations belong to a synthetic dataset.
    pub synthetic: bool,
}

/// Build and validate before returning. In particular, a validator predating nullable
/// unobserved counts rejects denied/unscanned packets here, rather than inventing zeroes.
pub fn build(facts: &EvidenceFacts, ids: PacketIds) -> Result<Value, String> {
    let mut seen = BTreeSet::new();
    let tokens: Vec<_> = word_runs(&casefold(&facts.name))
        .into_iter()
        .filter(|word| word.chars().count() <= 64 && seen.insert(word.clone()))
        .take(16)
        .collect();
    let observed = !matches!(
        facts.coverage.as_str(),
        "denied" | "unscanned" | "excluded" | "cancelled" | "unknown"
    );
    let bytes_known = facts.allocation_known
        && facts.logical_bytes.is_some()
        && facts.allocated_bytes.is_some()
        && facts.families.iter().all(|f| f.bytes.is_some());
    let file_shares = shares(facts.families.iter().map(|f| f.files), facts.files);
    let byte_shares = shares(
        facts.families.iter().map(|f| f.bytes.unwrap_or(0)),
        facts.logical_bytes.unwrap_or(0),
    );
    let families: Vec<_> = if observed {
        facts
            .families
            .iter()
            .enumerate()
            .map(|(i, f)| {
                json!({
                    "family":f.family, "share_files":file_shares[i],
                    "share_bytes":bytes_known.then_some(byte_shares[i]),
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let extensions: Vec<_> = if observed {
        facts
            .extensions
            .iter()
            .map(|e| json!({"ext":e.ext,"files":e.files,"bytes":e.bytes}))
            .collect()
    } else {
        Vec::new()
    };
    let media = if observed {
        facts.media.as_ref().map(|m| json!({
        "raw_files":m.raw_files, "sidecar_files":m.sidecar_files, "raw_with_sidecar":m.raw_with_sidecar,
        "capture_start":m.capture_start, "capture_end":m.capture_end, "camera_models":m.camera_models,
    }))
    } else {
        None
    };
    let packet = json!({
        "schema":"loomward.evidence-packet.v1", "synthetic":ids.synthetic,
        "packet_id":ids.packet_id, "group_id":ids.group_id, "observed_at":ids.observed_at,
        "subject":{"kind":"folder","name_tokens":tokens,"depth":0},
        "shape":{
            "files":observed.then_some(facts.files), "folders":observed.then_some(facts.folders),
            "logical_bytes":observed.then_some(facts.logical_bytes).flatten(),
            "allocated_bytes":(observed && facts.allocation_known).then_some(facts.allocated_bytes).flatten(),
            "max_depth":observed.then_some(facts.max_depth),
        },
        "families":families, "extensions":extensions,
        "markers":if observed { facts.markers.clone() } else { Vec::new() },
        "time":{
            "newest_content_write":observed.then_some(facts.time.newest_content_write).flatten(),
            "oldest_content_write":observed.then_some(facts.time.oldest_content_write).flatten(),
            "newest_source_write":observed.then_some(facts.time.newest_source_write).flatten(),
            "newest_generated_write":observed.then_some(facts.time.newest_generated_write).flatten(),
            "git_last_commit":null, "git_dirty":null, "last_access":"not_evidence",
        },
        "duplicates":{"duplicate_bytes":null,"groups":null}, "media":media,
        "coverage":{
            "state":facts.coverage, "allocation_known":observed && facts.allocation_known,
            "link_counts_observed":observed && facts.link_counts_observed, "online":facts.online,
        },
    });
    validate_packet(&packet).map_err(|error| error.to_string())?;
    Ok(packet)
}

fn shares(values: impl Iterator<Item = u64>, total: u64) -> Vec<f64> {
    let mut shares: Vec<_> = values
        .map(|value| {
            if total == 0 {
                0.0
            } else {
                value as f64 / total as f64
            }
        })
        .collect();
    // Correct floating point accumulation only; inconsistent facts still fail validation.
    let sum: f64 = shares.iter().sum();
    if sum > 1.0 && sum <= 1.0 + f64::EPSILON * shares.len() as f64 {
        let mut remaining = 1.0_f64;
        for share in &mut shares {
            *share = share.min(remaining);
            remaining = (remaining - *share).max(0.0);
        }
    }
    shares
}
