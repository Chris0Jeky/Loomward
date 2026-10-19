//! Backup observations are evidence, never permission or a source-retirement gate.

use serde::{de, Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::collections::HashSet;

pub const MAX_INPUT_BYTES: usize = 256 * 1024;
pub const MAX_RECORDS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topology {
    IndependentTarget,
    SameVolume,
    SamePhysicalDevice,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataClass {
    Acl,
    AlternateStreams,
    HardLinks,
    Timestamps,
    Sparse,
    Compression,
    Efs,
}

/// A relative, scope-projected path and independently observed SHA-256 content version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileVersion {
    pub path: String,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct Observation {
    pub repository_id: String,
    pub snapshot_id: String,
    pub observed_at: u64,
    pub exit_code: i32,
}

#[derive(Debug, Clone)]
pub struct IntegrityObservation {
    pub observation: Observation,
    /// Supplied restic `check --json` summary bytes, not human-readable log text.
    pub raw: Vec<u8>,
    pub read_all_data: bool,
}

#[derive(Debug, Clone)]
pub struct RestoredFile {
    pub file: FileVersion,
    pub compared_metadata: Vec<MetadataClass>,
}

#[derive(Debug, Clone)]
pub struct RestoreObservation {
    pub observation: Observation,
    pub complete: bool,
    /// Content versions measured at the disposable restore target, not job counts.
    pub files: Vec<RestoredFile>,
}

/// Supplied, scope-projected observations. No credentials or real target paths are accepted.
#[derive(Debug, Clone)]
pub struct BackupSource {
    pub repository_id: String,
    pub snapshot_id: String,
    pub observed_at: u64,
    pub now: u64,
    pub max_age_seconds: u64,
    pub topology: Topology,
    /// Missing credentials remain unknown; this is validity evidence, not the secret.
    pub credentials_valid_until: Option<u64>,
    pub required: Vec<FileVersion>,
    /// Bound to `repository_id` + `snapshot_id` by the future observation owner.
    /// `snapshots --json` alone does not contain file versions.
    pub snapshot_files: Vec<FileVersion>,
    pub required_metadata: Vec<MetadataClass>,
    pub integrity: Option<IntegrityObservation>,
    pub restore: Option<RestoreObservation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    SnapshotMissing,
    VersionNotCovered,
    TargetNotIndependent,
    CredentialsUnavailable,
    IntegrityNotChecked,
    RestoreNotSucceeded,
    PartialRestore,
    UnsupportedMetadata,
    StaleObservation,
    InvalidObservationTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptError {
    InputTooLarge,
    InvalidJson,
    /// UTF-16 with a byte-order mark, as Windows PowerShell 5.1 `>` redirection writes (#349).
    Utf16Bom,
    InvalidReceipt,
    InvalidSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedRestoreDrill {
    pub files: Vec<FileVersion>,
    pub target: &'static str,
    pub compare_metadata: Vec<MetadataClass>,
    pub proofs: &'static [&'static str],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryEvidence {
    pub repository_id: String,
    pub snapshot_id: String,
    pub snapshot_taken: bool,
    pub snapshot_time: Option<String>,
    pub integrity_checked: bool,
    pub integrity_read_all_data: bool,
    pub restore_drill_succeeded: bool,
    pub restored_versions: Vec<FileVersion>,
    pub required_versions: Vec<FileVersion>,
    pub restore_coverage: Vec<FileVersion>,
    pub topology: Topology,
    pub observation_age_seconds: Option<u64>,
    pub integrity_age_seconds: Option<u64>,
    pub restore_age_seconds: Option<u64>,
    /// Every predicate holds on the supplied, scoped evidence. Caller-reported self-consistency,
    /// never native proof, authorisation or a source-retirement gate (invariant 9, #349).
    pub reported_recovery_evidence: bool,
    pub refusals: Vec<Refusal>,
    pub proposed_drill: ProposedRestoreDrill,
}

/// Parses supplied restic `snapshots --json` bytes. No I/O, clocks, config or credentials.
/// Malformed input is refused; well-formed but insufficient evidence carries named refusals.
pub fn normalise_backup_receipt(
    raw: &[u8],
    source: &BackupSource,
) -> Result<RecoveryEvidence, ReceiptError> {
    validate_source(source)?;
    let value = parse_strict(raw)?;
    // The byte budget bounds the entry count. Every entry needs a unique digest ID; only the bound
    // snapshot is validated strictly, so one odd unrelated snapshot cannot hide it (#349).
    let items = value.as_array().ok_or(ReceiptError::InvalidReceipt)?;
    let mut ids = HashSet::new();
    let mut snapshot_time = None;
    for value in items {
        let id = value.get("id").and_then(Value::as_str).unwrap_or("");
        if !digest(id) || !ids.insert(id) {
            return Err(ReceiptError::InvalidReceipt);
        }
        if id != source.snapshot_id {
            continue;
        }
        let snapshot: Snapshot =
            serde_json::from_value(value.clone()).map_err(|_| ReceiptError::InvalidReceipt)?;
        if !display_label(&snapshot.time)
            || snapshot.paths.is_empty()
            || snapshot.paths.len() > MAX_RECORDS
            || snapshot.paths.iter().any(|p| !provider_path(p))
        {
            return Err(ReceiptError::InvalidReceipt);
        }
        snapshot_time = Some(snapshot.time);
    }
    let snapshot_taken = snapshot_time.is_some();
    // An empty, plain-text (restic fatal error) or non-summary check document keeps the snapshot
    // evidence and refuses as IntegrityNotChecked rather than failing the whole receipt (#349).
    let integrity_checked = source.integrity.as_ref().is_some_and(|check| {
        check_summary(&check.raw).is_some_and(|summary| {
            snapshot_taken
                && bound(&check.observation, source)
                && summary.num_errors == 0
                && summary.broken_packs.as_ref().is_none_or(Vec::is_empty)
                && !summary.suggest_repair_index
                && !summary.suggest_prune
        })
    });
    let integrity_read_all_data =
        integrity_checked && source.integrity.as_ref().is_some_and(|c| c.read_all_data);
    let restore_bound = snapshot_taken
        && source
            .restore
            .as_ref()
            .is_some_and(|r| bound(&r.observation, source));
    let restored_versions: Vec<_> = if restore_bound {
        source
            .restore
            .as_ref()
            .unwrap()
            .files
            .iter()
            .map(|r| r.file.clone())
            .collect()
    } else {
        Vec::new()
    };
    let restore_coverage: Vec<_> = source
        .required
        .iter()
        .filter(|f| {
            snapshot_taken
                && source.snapshot_files.iter().any(|s| same_file(s, f))
                && restored_versions.iter().any(|s| same_file(s, f))
        })
        .cloned()
        .collect();
    let versions_covered = restore_coverage.len() == source.required.len();
    let complete = restore_bound
        && source.restore.as_ref().is_some_and(|r| {
            r.complete
                && source
                    .required
                    .iter()
                    .all(|f| r.files.iter().any(|got| same_path(&got.file.path, &f.path)))
        });
    let metadata_supported = restore_bound
        && source.restore.as_ref().is_some_and(|r| {
            source.required.iter().all(|f| {
                r.files
                    .iter()
                    .find(|got| same_file(&got.file, f))
                    .is_some_and(|got| {
                        source
                            .required_metadata
                            .iter()
                            .all(|m| got.compared_metadata.contains(m))
                    })
            })
        });
    let restore_drill_succeeded = complete && versions_covered && metadata_supported;
    let observation_age_seconds = source.now.checked_sub(source.observed_at);
    let integrity_age_seconds = source
        .integrity
        .as_ref()
        .and_then(|c| source.now.checked_sub(c.observation.observed_at));
    let restore_age_seconds = source
        .restore
        .as_ref()
        .and_then(|r| source.now.checked_sub(r.observation.observed_at));
    let mut refusals = Vec::new();
    if !snapshot_taken {
        refusals.push(Refusal::SnapshotMissing);
    }
    if !versions_covered {
        refusals.push(Refusal::VersionNotCovered);
    }
    if source.topology != Topology::IndependentTarget {
        refusals.push(Refusal::TargetNotIndependent);
    }
    if !source
        .credentials_valid_until
        .is_some_and(|until| until > source.now)
    {
        refusals.push(Refusal::CredentialsUnavailable);
    }
    if !integrity_read_all_data {
        refusals.push(Refusal::IntegrityNotChecked);
    }
    if !restore_bound {
        refusals.push(Refusal::RestoreNotSucceeded);
    }
    if !complete {
        refusals.push(Refusal::PartialRestore);
    }
    if !metadata_supported {
        refusals.push(Refusal::UnsupportedMetadata);
    }
    let ages = [
        observation_age_seconds,
        integrity_age_seconds,
        restore_age_seconds,
    ];
    if ages
        .iter()
        .flatten()
        .any(|age| *age > source.max_age_seconds)
    {
        refusals.push(Refusal::StaleObservation);
    }
    if source.observed_at > source.now
        || source
            .integrity
            .as_ref()
            .is_some_and(|c| c.observation.observed_at > source.now)
        || source
            .restore
            .as_ref()
            .is_some_and(|r| r.observation.observed_at > source.now)
    {
        refusals.push(Refusal::InvalidObservationTime);
    }
    Ok(RecoveryEvidence {
        repository_id: source.repository_id.clone(),
        snapshot_id: source.snapshot_id.clone(),
        snapshot_taken,
        snapshot_time,
        integrity_checked,
        integrity_read_all_data,
        restore_drill_succeeded,
        restored_versions,
        required_versions: source.required.clone(),
        restore_coverage,
        topology: source.topology,
        observation_age_seconds,
        integrity_age_seconds,
        restore_age_seconds,
        reported_recovery_evidence: refusals.is_empty(),
        refusals,
        proposed_drill: ProposedRestoreDrill {
            files: source.required.clone(),
            target: "new-disposable-target",
            compare_metadata: source.required_metadata.clone(),
            proofs: &[
                "repository-and-snapshot-binding",
                "independent-content-sha256",
                "required-metadata-comparison",
                "complete-manifest",
                "fresh-observation",
            ],
        },
    })
}

#[derive(Deserialize)]
struct Snapshot {
    time: String,
    paths: Vec<String>,
}

#[derive(Deserialize)]
struct CheckSummary {
    message_type: String,
    num_errors: i64,
    // restic's checkSummary.BrokenPacks has no omitempty: a clean check prints `null`.
    #[serde(default)]
    broken_packs: Option<Vec<String>>,
    #[serde(default)]
    suggest_repair_index: bool,
    #[serde(default)]
    suggest_prune: bool,
}

fn check_summary(raw: &[u8]) -> Option<CheckSummary> {
    let value = parse_strict(raw).ok().filter(Value::is_object)?;
    serde_json::from_value::<CheckSummary>(value)
        .ok()
        .filter(|s| s.message_type == "summary" && s.num_errors >= 0)
}

/// A repository label: 1-64 of `[A-Za-z0-9._-]`, so a URL, path or credential is never echoed (#349).
pub fn repository_label(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// The provider's uninterpreted snapshot-time text: 1-64 of `[0-9A-Za-z:.+-]` (#349).
fn display_label(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b":.+-".contains(&b))
}

fn bound(observation: &Observation, source: &BackupSource) -> bool {
    observation.repository_id == source.repository_id
        && observation.snapshot_id == source.snapshot_id
        && observation.exit_code == 0
}

/// Relative names are Windows names: case folds exactly as `valid_files` folds duplicates, so
/// coverage and duplicate detection share one path identity.
fn same_path(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

fn same_file(a: &FileVersion, b: &FileVersion) -> bool {
    same_path(&a.path, &b.path) && a.version == b.version
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && !path.contains('\\')
        && path.split('/').all(|part| {
            // Windows ignores trailing spaces before the extension: `CON .txt` is `CON` (#349).
            let stem = part.split('.').next().unwrap_or("").trim_end_matches(' ');
            let stem = stem.to_ascii_uppercase();
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.ends_with(['.', ' '])
                && !part
                    .chars()
                    .any(|c| c.is_control() || "<>:\"|?*".contains(c))
                && !["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
                && !["COM", "LPT"].iter().any(|prefix| {
                    stem.strip_prefix(prefix).is_some_and(|n| {
                        ["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"].contains(&n)
                    })
                })
        })
}

// Provider roots are retained only as parse-validated labels; never restore operands.
fn provider_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && !path.chars().any(char::is_control)
        && path
            .split(['/', '\\'])
            .all(|part| part != ".." && part != ".")
}

fn valid_files<'a>(files: impl Iterator<Item = &'a FileVersion>, length: usize) -> bool {
    let mut paths = HashSet::new();
    length <= MAX_RECORDS
        && files.into_iter().all(|f| {
            relative_path(&f.path) && digest(&f.version) && paths.insert(f.path.to_lowercase())
        })
}

fn validate_source(source: &BackupSource) -> Result<(), ReceiptError> {
    const MAX_TIME: u64 = 253_402_300_799;
    let valid_observation = |o: &Observation| {
        o.observed_at <= MAX_TIME && repository_label(&o.repository_id) && digest(&o.snapshot_id)
    };
    if !repository_label(&source.repository_id)
        || !digest(&source.snapshot_id)
        || source.observed_at > MAX_TIME
        || source.now > MAX_TIME
        || source.max_age_seconds > MAX_TIME
        || source.credentials_valid_until.is_some_and(|t| t > MAX_TIME)
        || source.required.is_empty()
        || !valid_files(source.required.iter(), source.required.len())
        || !valid_files(source.snapshot_files.iter(), source.snapshot_files.len())
        || source.required_metadata.len() > 7
        || source
            .integrity
            .as_ref()
            .is_some_and(|c| !valid_observation(&c.observation))
        || source.restore.as_ref().is_some_and(|r| {
            !valid_observation(&r.observation)
                || !valid_files(r.files.iter().map(|f| &f.file), r.files.len())
                || r.files.iter().any(|f| f.compared_metadata.len() > 7)
        })
    {
        return Err(ReceiptError::InvalidSource);
    }
    Ok(())
}

/// Mirrors the protocol's private strict-JSON visitor without changing its wire surface.
/// serde_json's default recursion limit bounds nesting at 128; input bounds total allocation.
fn parse_strict(raw: &[u8]) -> Result<Value, ReceiptError> {
    if raw.len() > MAX_INPUT_BYTES {
        return Err(ReceiptError::InputTooLarge);
    }
    // Only the byte-order mark is inspected; nothing is transcoded.
    if raw.starts_with(&[0xff, 0xfe]) || raw.starts_with(&[0xfe, 0xff]) {
        return Err(ReceiptError::Utf16Bom);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(raw);
    let Strict(value) =
        Strict::deserialize(&mut deserializer).map_err(|_| ReceiptError::InvalidJson)?;
    deserializer.end().map_err(|_| ReceiptError::InvalidJson)?;
    Ok(value)
}

struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = Strict;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("bounded JSON without duplicate keys")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Strict, E> {
                Ok(Strict(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Strict, E> {
                if v > i64::MAX as u64 {
                    return Err(E::custom("number outside supported range"));
                }
                Ok(Strict(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Strict, E> {
                if !v.is_finite() || v.abs() >= i64::MAX as f64 {
                    return Err(E::custom("number outside supported range"));
                }
                serde_json::Number::from_f64(v)
                    .map(|n| Strict(Value::Number(n)))
                    .ok_or_else(|| E::custom("invalid number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Strict, E> {
                Ok(Strict(Value::String(v.to_owned())))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Strict, A::Error> {
                let mut items = Vec::new();
                while let Some(Strict(item)) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Strict(Value::Array(items)))
            }
            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Strict, A::Error> {
                let mut out = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if out.contains_key(&key) {
                        return Err(de::Error::custom("duplicate key"));
                    }
                    let Strict(value) = map.next_value()?;
                    out.insert(key, value);
                }
                Ok(Strict(Value::Object(out)))
            }
        }
        d.deserialize_any(Visitor)
    }
}
