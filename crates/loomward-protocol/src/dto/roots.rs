//! Roots and grants. Mirrors `contracts/v3/view-service.schema.json`; the contract tests keep them equal.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootScanStateState {
    NeverScanned,
    Scanning,
    Complete,
    Partial,
    Stale,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootScanState {
    pub state: RootScanStateState,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub last_job_id: Option<JobId>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub generation: Option<Generation>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub finished_at: Option<Timestamp>,
    pub coverage: CoverageState,
}

const_str!(
/// Alternate data streams and filesystem metadata are not observed in v0.3.
    pub SubtreeTotalsStreamCoverage = "default_stream_only"
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubtreeTotals {
    /// Directory entries (names), not objects.
    pub files: Count,
    pub dirs: Count,
    /// Bytes by directory entry: sum of default-stream end-of-file as listed. A hard-linked object counts once per observed name.
    pub logical_bytes: Bytes,
    /// Bytes by directory entry: sum of default-stream allocation as listed.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub allocated_bytes: Option<Bytes>,
    pub allocation_unknown_files: Count,
    pub skipped: Count,
    pub failed: Count,
    pub complete: bool,
    /// Alternate data streams and filesystem metadata are not observed in v0.3.
    pub stream_coverage: SubtreeTotalsStreamCoverage,
    /// Distinct (volume_key, file_id) objects within this subtree; null when any entry lacks a native ID.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub unique_objects: Option<Count>,
    /// Allocation counted once per unique object within this subtree; null when unique_objects is null or allocation is unknown.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub unique_allocated_bytes: Option<Bytes>,
    /// Entries whose object has another observed name anywhere in the dataset or an observed link count above 1.
    pub multi_link_entries: Count,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootOrigin {
    Fixture,
    LabGenerated,
    OwnerGranted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootGrantedVia {
    DesktopPicker,
    CliFlag,
    Fixture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootGrantState {
    Active,
    Revoked,
    IdentityChanged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Root {
    pub root_id: RootId,
    pub display_path: DisplayPath,
    pub origin: RootOrigin,
    pub dataset_class: DatasetClass,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub volume_id: Option<VolumeId>,
    pub granted_at: Timestamp,
    pub granted_via: RootGrantedVia,
    pub grant_state: RootGrantState,
    pub scan: RootScanState,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub totals: Option<SubtreeTotals>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootList {
    pub roots: BoundedVec<Root, 0, 64>,
}

const_str!(
    pub RootGrantRequestPurpose = "metadata_scan"
);

/// Tauri adapter only. Rust opens the native folder picker; the webview never supplies a path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootGrantRequest {
    pub purpose: RootGrantRequestPurpose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootGrantResultOutcome {
    Granted,
    CancelledByUser,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootGrantResultRefusal {
    VolumeRoot,
    ReparseOrPlaceholder,
    NotADirectory,
    ContainsStateDir,
    InsideStateDir,
    AlreadyGranted,
    NetworkOrRemovableUnsupported,
    IdentityUnavailable,
    SyntheticSessionRequiresLabRoot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootGrantResult {
    pub outcome: RootGrantResultOutcome,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub root: Option<Root>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub refusal: Option<RootGrantResultRefusal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootRevokeRequest {
    pub root_id: RootId,
    /// Deletes Loomward's own catalogue rows for the root. Never touches the scanned files.
    pub purge_catalog: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootRevokeResult {
    pub root_id: RootId,
    pub revoked_at: Timestamp,
    pub purged: bool,
}
