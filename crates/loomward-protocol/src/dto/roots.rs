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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubtreeTotals {
    pub files: Count,
    pub dirs: Count,
    pub logical_bytes: Bytes,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub allocated_bytes: Option<Bytes>,
    pub allocation_unknown_files: Count,
    pub skipped: Count,
    pub failed: Count,
    pub complete: bool,
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
