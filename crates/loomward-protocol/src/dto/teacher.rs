//! Teacher disclosure and grants. Mirrors `contracts/v3/view-service.schema.json`; the contract tests keep them equal.

use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeacherPreviewRequest {
    pub node_ids: UniqueVec<NodeId, 1, 25>,
    pub recipient: TeacherRecipient,
}

const_str!(
    pub TeacherPreviewSerializationVersion = "loomward-teacher-request/1"
);

const_str!(
    pub TeacherPreviewModel = "gpt-6.1-sol"
);

const_str!(
    pub TeacherPreviewReasoningEffort = "medium"
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeacherPreviewItemMetadata {
    pub name: Text<256>,
    pub extension: Text<32>,
    pub context: Text<512>,
    pub size_bucket: Int<0, 16>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeacherPreviewItem {
    /// Request-local handle; catalogue IDs never leave the machine.
    pub handle: TeacherHandle,
    pub metadata: TeacherPreviewItemMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeacherPreviewExcludedReason {
    SensitiveName,
    SensitiveContext,
    Excluded,
    CloudPlaceholder,
    DirectoryNotSupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeacherPreviewExcluded {
    pub node_id: NodeId,
    pub reason: TeacherPreviewExcludedReason,
}

/// Immutable once created. Shows the complete request that would leave the machine. Creating a preview sends nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeacherPreview {
    pub preview_id: PreviewId,
    pub serialization_version: TeacherPreviewSerializationVersion,
    /// The exact system instructions sent.
    pub instructions: Text<4000>,
    pub allowed_labels: UniqueVec<Label, 1, 32>,
    pub output_schema_digest: Digest,
    /// Pinned Codex executable hash, version, argv and effective configuration overrides (docs/41 section 9.2).
    pub runner_profile_digest: Digest,
    pub recipient: TeacherRecipient,
    pub model: TeacherPreviewModel,
    pub reasoning_effort: TeacherPreviewReasoningEffort,
    pub dataset_class: DatasetClass,
    pub fields: UniqueVec<TeacherField, 1, 4>,
    pub items: BoundedVec<TeacherPreviewItem, 0, 25>,
    pub excluded: BoundedVec<TeacherPreviewExcluded, 0, 25>,
    pub taxonomy_version: Int<1, 9007199254740991>,
    pub payload_bytes: Count,
    /// SHA-256 over the canonical serialization of the complete request: instructions, allowed labels, output schema, items and serialization version.
    pub payload_digest: Digest,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisclosureSummaryItem {
    pub handle: TeacherHandle,
    pub name: Text<256>,
    pub extension: Text<32>,
    pub context: Text<512>,
    pub size_bucket: Int<0, 16>,
}

/// Built by Rust from the stored immutable preview, never from UI input, and rendered by the native confirmation dialog item by item. Not sent over the wire; defined here so the Rust and dialog implementations agree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisclosureSummary {
    pub preview_id: PreviewId,
    pub recipient: TeacherRecipient,
    pub model: TeacherPreviewModel,
    pub reasoning_effort: TeacherPreviewReasoningEffort,
    pub dataset_class: DatasetClass,
    pub fields: UniqueVec<TeacherField, 1, 4>,
    pub items: BoundedVec<DisclosureSummaryItem, 1, 25>,
    pub excluded_count: Int<0, 25>,
    pub payload_digest: Digest,
    pub runner_profile_digest: Digest,
    pub expires_at: Timestamp,
}

/// Personal dataset: the Tauri adapter shows a native confirmation dialog from Rust; the HTTP adapter refuses. Synthetic dataset: granted by policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisclosureGrantRequest {
    pub preview_id: PreviewId,
    pub payload_digest: Digest,
}

const_str!(
    pub DisclosureGrantKind = "teacher_disclosure"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisclosureGrantConfirmedVia {
    DesktopDialog,
    SyntheticPolicy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisclosureGrant {
    pub grant_id: GrantId,
    pub kind: DisclosureGrantKind,
    pub recipient: TeacherRecipient,
    pub dataset_class: DatasetClass,
    pub fields: UniqueVec<TeacherField, 1, 4>,
    pub item_count: Int<1, 25>,
    pub payload_digest: Digest,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub revoked_at: Option<Timestamp>,
    /// Single use: one grant authorises exactly one teacher request with exactly the previewed payload.
    pub used: bool,
    pub confirmed_via: DisclosureGrantConfirmedVia,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisclosureGrantResultOutcome {
    Granted,
    DeclinedByUser,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisclosureGrantResultRefusal {
    PersonalRequiresDesktopDialog,
    PreviewExpired,
    PreviewAlreadyUsed,
    DigestMismatch,
    TeacherUnavailable,
    RunnerProfileMismatch,
    ConfinementNotEnforced,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisclosureGrantResult {
    pub outcome: DisclosureGrantResultOutcome,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub grant: Option<DisclosureGrant>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub refusal: Option<DisclosureGrantResultRefusal>,
}

const_str!(
    pub RootGrantRecordKind = "metadata_root"
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootGrantRecord {
    pub grant_id: GrantId,
    pub kind: RootGrantRecordKind,
    pub root_id: RootId,
    pub granted_at: Timestamp,
    pub granted_via: RootGrantedVia,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub revoked_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GrantListGrant {
    RootGrantRecord(RootGrantRecord),
    DisclosureGrant(DisclosureGrant),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantList {
    pub grants: BoundedVec<GrantListGrant, 0, 256>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantRevokeRequest {
    pub grant_id: GrantId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantRevokeResult {
    pub grant_id: GrantId,
    pub revoked_at: Timestamp,
}

/// Sends exactly the payload bound to the grant's digest. No node list is accepted here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeacherRunRequest {
    pub grant_id: GrantId,
}

const_str!(
    pub TeacherLabelDataClass = "teacher"
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeacherLabel {
    pub node_id: NodeId,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub label: Option<Label>,
    pub abstain: bool,
    pub reason: Text<512>,
    pub evidence: UniqueVec<TeacherField, 0, 4>,
    pub data_class: TeacherLabelDataClass,
    pub weight: ConstWeight,
    pub requires_review: ConstTrue,
    pub autonomy_allowed: ConstFalse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeacherResults {
    pub job_id: JobId,
    pub state: JobState,
    pub recipient: TeacherRecipient,
    pub payload_digest: Digest,
    pub labels: BoundedVec<TeacherLabel, 0, 25>,
    pub rejected_output: bool,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub rejection_reason: Option<Text<512>>,
}
