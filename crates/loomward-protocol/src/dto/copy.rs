//! Copy-and-verify commands (docs/47 lane C4, LW-038): preview, approve, start, cancel, abandon, status.
//! Contract only: every copy command answers `capability_unavailable` until the effect gate (docs/47
//! lane C8) opens. Node IDs select what the engine observes; the approval binds the digest of the
//! freshly observed native identities, never a node ID (invariant 5).

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyStatus {
    pub operation_id: CopyOperationId,
    pub state: CopyState,
    pub digest: Digest,
    pub items_total: Count,
    pub items_verified: Count,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub refusal: Option<CopyRefusal>,
}
