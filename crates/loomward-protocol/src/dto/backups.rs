//! Backups stage 1: imported, normalised restic receipts. Mirrors `contracts/v3/view-service.schema.json`.
//! Evidence is never permission, a restore, or a source-retirement gate (docs/42 ADR-V3-23).

use super::*;

/// One requested file version: a scope-projected manifest name compared lexically (never resolved,
/// opened or joined to a root) and its independently observed SHA-256 content version.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredVersion {
    pub relative_name: BoundedStr<1, 1024>,
    pub version: Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupTopology {
    IndependentTarget,
    SameVolume,
    SamePhysicalDevice,
    Unknown,
}

/// Where the topology came from. Volume identity tells one volume from another; it cannot yet tell
/// whether two volumes share a physical disk, so different volumes stay `unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupTopologyBasis {
    SameVolumeIdentity,
    DifferentVolumesDeviceUnknown,
    Undeclared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupRefusal {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupMetadataClass {
    Acl,
    AlternateStreams,
    HardLinks,
    Timestamps,
    Sparse,
    Compression,
    Efs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreDrillProof {
    RepositoryAndSnapshotBinding,
    IndependentContentSha256,
    RequiredMetadataComparison,
    CompleteManifest,
    FreshObservation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreDrillTarget {
    NewDisposableTarget,
}

/// A restore drill proposed as data. Nothing creates, opens, writes or restores its target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedRestoreDrill {
    pub files: BoundedVec<RequiredVersion, 1, 64>,
    pub target: RestoreDrillTarget,
    pub compare_metadata: BoundedVec<BackupMetadataClass, 0, 7>,
    pub proofs: BoundedVec<RestoreDrillProof, 0, 5>,
    pub executable: ConstFalse,
}

/// Normalised recovery evidence for one repository. Snapshot, integrity and restore stay separate;
/// `reported_recovery_evidence` is caller-reported self-consistency, never native proof.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupEvidence {
    pub evidence_id: BackupEvidenceId,
    pub repository_label: BackupLabel,
    pub snapshot_id: SnapshotId,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub snapshot_time: Option<Text<64>>,
    pub imported_at: Timestamp,
    pub snapshot_observed_at: Timestamp,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub integrity_observed_at: Option<Timestamp>,
    pub snapshot_taken: bool,
    pub integrity_checked: bool,
    pub integrity_read_all_data: bool,
    pub restore_drill_succeeded: bool,
    pub required_versions: BoundedVec<RequiredVersion, 1, 64>,
    pub restore_coverage: BoundedVec<RequiredVersion, 0, 64>,
    pub topology: BackupTopology,
    pub topology_basis: BackupTopologyBasis,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub snapshot_age_seconds: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub integrity_age_seconds: Option<Count>,
    pub max_age_seconds: Count,
    pub reported_recovery_evidence: bool,
    pub refusals: BoundedVec<BackupRefusal, 0, 10>,
    pub proposed_drill: ProposedRestoreDrill,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupEvidenceList {
    pub evidence: BoundedVec<BackupEvidence, 0, 64>,
}

/// Tauri adapter only. Rust opens the native open-file dialog; the webview never supplies a path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupImportRequest {
    pub repository_label: BackupLabel,
    pub snapshot_id: SnapshotId,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub source_volume_id: Option<VolumeId>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub target_volume_id: Option<VolumeId>,
    pub required: BoundedVec<RequiredVersion, 1, 64>,
    /// Also open the dialog a second time for a restic `check --json` output.
    pub integrity_check: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupImportOutcome {
    Imported,
    CancelledByUser,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupImportRefusal {
    SyntheticSession,
    NotARegularFile,
    Unreadable,
    TooLarge,
    InvalidJson,
    InvalidReceipt,
    InvalidSource,
    RepositoryLimit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupImportResult {
    pub outcome: BackupImportOutcome,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub evidence: Option<BackupEvidence>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub refusal: Option<BackupImportRefusal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupForgetRequest {
    pub evidence_id: BackupEvidenceId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupForgetResult {
    pub evidence_id: BackupEvidenceId,
    pub forgotten_at: Timestamp,
}
