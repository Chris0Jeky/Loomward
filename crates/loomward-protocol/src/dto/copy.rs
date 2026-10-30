//! Copy-and-verify commands (docs/47 lanes C4, C5; LW-038, LW-040): preview, approve, start, cancel,
//! abandon, status. Served for synthetic sessions on registered lab roots only (C5); every personal
//! session answers `capability_unavailable` until the effect gate (docs/47 lane C8) opens. Node IDs
//! select what the engine observes; the approval binds the digest of the freshly observed native
//! identities, never a node ID (invariant 5).

use super::*;

/// docs/47 section 5, the operation state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyState {
    Draft,
    Refused,
    Approved,
    Expired,
    Preparing,
    Copying,
    Staged,
    Blocked,
    NeedsAttention,
    Publishing,
    Committed,
    Cancelling,
    Cancelled,
}

/// docs/47 section 7, every refusal code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyRefusalCode {
    IdentityUnverified,
    CloudPlaceholder,
    ReparsePoint,
    UnsupportedAttribute,
    ExtendedAttributes,
    AlternateStreamsUnsupported,
    TooManyStreams,
    SpecialFile,
    SourceChanged,
    StagingNameTaken,
    StagingInUse,
    ChangedAfterVerification,
    SourceInUse,
    AccessDenied,
    InsufficientSpace,
    DestinationInsideSource,
    DestinationCloudSynced,
    DestinationUnsupportedVolume,
    DestinationProtected,
    DestinationChanged,
    CaseCollision,
    NameUnrepresentable,
    PathTooLong,
    PublishInUse,
    VerificationMismatch,
    GrantRevoked,
    Elevated,
    ApprovalExpired,
    DigestMismatch,
    ManifestLimit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyRefusal {
    pub code: CopyRefusalCode,
    /// The refused item, or `null` for an operation-wide refusal.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub node_id: Option<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyPreviewRequest {
    /// Owner-selected items, each copied with everything beneath it.
    pub sources: UniqueVec<NodeId, 1, 1000>,
    /// A folder under an active grant; v1 creates exactly one new folder inside it.
    pub destination_node_id: NodeId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyPreview {
    pub operation_id: CopyOperationId,
    /// `draft` (approvable) or `refused` (deselect and preview again).
    pub state: CopyState,
    /// The canonical manifest digest: membership, destination, bytes and identities.
    pub digest: Digest,
    /// The proposed new folder; " (2)", " (3)" when the parent already has the name.
    pub folder_name: BoundedStr<1, 255>,
    pub files: Count,
    pub folders: Count,
    pub bytes: Bytes,
    pub space_needed: Bytes,
    pub space_reserve: Bytes,
    pub same_volume: bool,
    pub explicit_acl_items: Count,
    pub hard_link_names: Count,
    pub refusals: BoundedVec<CopyRefusal, 0, 256>,
    pub refusal_count: Count,
}

/// `approve` and `start` name the digest the owner saw; any other digest is refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyConsentRequest {
    pub operation_id: CopyOperationId,
    pub digest: Digest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyApproval {
    pub operation_id: CopyOperationId,
    pub digest: Digest,
    pub approved_at: Timestamp,
    /// Single use; expires unused after 30 minutes.
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyOperationRequest {
    pub operation_id: CopyOperationId,
}

/// What ended a stopped run when it is not a refusal code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyStopKind {
    EmergencyStop,
    Cancelled,
}

/// A refusal code or a `CopyStopKind`, as one string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CopyStopCause {
    Refusal(CopyRefusalCode),
    Other(CopyStopKind),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyDisposition {
    pub explanation: BoundedStr<1, 2048>,
    pub items_total: Count,
    pub items_verified: Count,
    pub items_unverified: Count,
    /// Every distinct refusal journaled during the run, including ones it resumed past.
    pub refusal_codes: BoundedVec<CopyRefusalCode, 0, 32>,
    /// The refusal or fence active at cancellation, else `cancelled`. Absent from older services.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_cause: Option<CopyStopCause>,
    pub resumable_before_cancel: bool,
    pub resume_possible: bool,
}

/// LW-132 / ADR R1. A vocabulary is not evidence that any higher label was earned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyReceiptLabel {
    ReadBack,
    CrashQualified,
    PowerLossQualified,
    ArchiveVerified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyReceiptState {
    Unverified,
    ReadBack,
    PublishedUnverified,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyReceipt {
    pub state: CopyReceiptState,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub highest_label: Option<CopyReceiptLabel>,
    pub items_read_back: Count,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub archive_verified_at: Option<Timestamp>,
    pub explanation: BoundedStr<1, 2048>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyStatus {
    pub operation_id: CopyOperationId,
    pub state: CopyState,
    pub digest: Digest,
    pub items_total: Count,
    /// Deprecated compatibility count; not a qualification label.
    pub items_verified: Count,
    /// Absent from older services; clients must not infer a label from the legacy count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<CopyReceipt>,
    /// Pre-removal journal facts and deterministic explanation; absent until disposition is committed.
    /// Native staging identities stay in the journal, never become effect authority in this DTO.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposition: Option<CopyDisposition>,
    /// docs/47 Q1 default: the name (never a path) of the staging folder left in the destination
    /// parent on cancel or abandon, for the owner to remove; `null` otherwise.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub staging_folder: Option<BoundedStr<1, 255>>,
    /// Staging folders intended but never identity-journaled: possibly empty leftovers, never adopted
    /// or removed by Loomward; names only.
    pub leftover_staging_folders: BoundedVec<BoundedStr<1, 255>, 0, 64>,
    /// First 64 staging-relative manifest names observed without journaled identities; never removed.
    pub unjournaled_items: BoundedVec<BoundedStr<1, 32768>, 0, 64>,
    /// Total observed, including names beyond the bounded list.
    pub unjournaled_item_count: Count,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub refusal: Option<CopyRefusal>,
}
