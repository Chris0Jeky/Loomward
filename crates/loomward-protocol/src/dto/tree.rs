//! Tree, nodes, search and breakdown. Mirrors `contracts/v3/view-service.schema.json`; the contract tests keep them equal.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeaningThreadState {
    Labelled,
    Suggested,
    Mixed,
    None,
    Pending,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeaningThreadSource {
    Human,
    Student,
    Teacher,
}

/// What the owner or a model thinks this is. A suggestion is not an approval (invariant 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeaningThread {
    pub state: MeaningThreadState,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub label: Option<Label>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub share: Option<Fraction>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub source: Option<MeaningThreadSource>,
    pub collection_ids: BoundedVec<CollectionId, 0, 8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidencyThreadTierBasis {
    Declared,
    DeviceHint,
    Unknown,
}

/// Where the bytes physically are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResidencyThread {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub volume_id: Option<VolumeId>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub tier: Option<Tier>,
    pub tier_basis: ResidencyThreadTierBasis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionThreadState {
    Granted,
    Partial,
    Excluded,
    Denied,
    Revoked,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionThreadReason {
    MetadataGrantActive,
    SensitiveName,
    ProtectedContext,
    CloudPlaceholder,
    ReparseNotFollowed,
    AccessDenied,
    GrantRevoked,
    NotScanned,
    MixedChildren,
}

/// What Loomward may observe. In v0.3 no thread state ever implies an effect permission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionThread {
    pub state: PermissionThreadState,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub reason: Option<PermissionThreadReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Threads {
    pub meaning: MeaningThread,
    pub residency: ResidencyThread,
    pub permission: PermissionThread,
}

/// What a slice is rooted at. Never a path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Anchor {
    /// Every granted root together.
    Atlas {},
    Root {
        root_id: RootId,
    },
    Node {
        node_id: NodeId,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeSliceRequest {
    pub anchor: Anchor,
    pub depth: Int<1, 8>,
    pub max_nodes: Int<16, 6000>,
    pub min_share: MinShare,
    pub basis: Basis,
    pub include_files: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceNodeKind {
    Atlas,
    Volume,
    Root,
    Dir,
    File,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceNode {
    pub node_id: NodeId,
    /// Index into TreeSlice.nodes; null only for the anchor at index 0. Parents always precede children.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub parent: Option<Int<0, 5999>>,
    pub kind: SliceNodeKind,
    pub name: Text<260>,
    pub depth: Int<0, 64>,
    /// Size under the requested basis. Under allocated basis, entries with unknown allocation contribute 0 here and are counted in size_unknown_files.
    pub size_bytes: Bytes,
    pub size_unknown_files: Count,
    pub logical_bytes: Bytes,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub allocated_bytes: Option<Bytes>,
    pub files: Count,
    pub dirs: Count,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub child_count: Option<Count>,
    /// Only for kind=other: how many siblings were folded into this node.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub folded_count: Option<Count>,
    pub coverage: CoverageState,
    /// True while a scan is still adding to this subtree.
    pub live: bool,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub ext_family: Option<ExtFamily>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub modified_at: Option<Timestamp>,
    pub threads: Threads,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootGeneration {
    pub root_id: RootId,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub generation: Option<Generation>,
}

/// consistent: every directory sum was published at a revision covering its committed listings. provisional_live: sums come from the running scan's arena snapshot; 'other' nodes are clamped at zero. Inconsistent committed sums are repaired before serving, never sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeSliceAggregateState {
    Consistent,
    ProvisionalLive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeSliceOrdering {
    Exact,
    ApproximateLive,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeSlice {
    pub anchor_node_id: NodeId,
    pub basis: Basis,
    pub root_generations: BoundedVec<RootGeneration, 0, 64>,
    pub complete: bool,
    pub live: bool,
    /// consistent: every directory sum was published at a revision covering its committed listings. provisional_live: sums come from the running scan's arena snapshot; 'other' nodes are clamped at zero. Inconsistent committed sums are repaired before serving, never sent.
    pub aggregate_state: TreeSliceAggregateState,
    pub ordering: TreeSliceOrdering,
    pub truncated: bool,
    pub nodes: BoundedVec<SliceNode, 1, 6000>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeaningRequest {
    pub node_ids: UniqueVec<NodeId, 1, 6000>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeaningResultItem {
    pub node_id: NodeId,
    pub meaning: MeaningThread,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeaningResult {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub model_id: Option<ModelId>,
    pub items: BoundedVec<MeaningResultItem, 0, 6000>,
    pub budget_hit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryRowKind {
    Dir,
    File,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryRow {
    pub node_id: NodeId,
    pub kind: EntryRowKind,
    pub name: Text<260>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub extension: Option<Text<32>>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub ext_family: Option<ExtFamily>,
    pub logical_bytes: Bytes,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub allocated_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub files: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub dirs: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub modified_at: Option<Timestamp>,
    pub attributes: UniqueVec<FileAttribute, 0, 16>,
    pub flags: UniqueVec<EntryFlag, 0, 12>,
    pub coverage: CoverageState,
    /// Search and collection results only: the parent's display path.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub location_hint: Option<DisplayPath>,
}

/// size_desc is index-backed for both bases (ties broken by row id). name_asc and modified_desc are served only for directories with at most 10,000 direct children; larger ones return resource_budget with detail sort_requires_index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeChildrenRequestSort {
    SizeDesc,
    NameAsc,
    ModifiedDesc,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeChildrenRequest {
    pub node_id: NodeId,
    /// size_desc is index-backed for both bases (ties broken by row id). name_asc and modified_desc are served only for directories with at most 10,000 direct children; larger ones return resource_budget with detail sort_requires_index.
    pub sort: TreeChildrenRequestSort,
    pub basis: Basis,
    pub limit: Int<1, 200>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub cursor: Option<Cursor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EntryPageAnchor {
    NodeId(NodeId),
    CollectionId(CollectionId),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryPage {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub anchor: Option<EntryPageAnchor>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub generation: Option<Generation>,
    pub items: BoundedVec<EntryRow, 0, 200>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub next_cursor: Option<Cursor>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub total: Option<Count>,
    pub budget_hit: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeRefRequest {
    pub node_id: NodeId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodePathAncestorKind {
    Atlas,
    Volume,
    Root,
    Dir,
    File,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodePathAncestor {
    pub node_id: NodeId,
    pub kind: NodePathAncestorKind,
    pub name: Text<260>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodePath {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub root_id: Option<RootId>,
    pub ancestors: BoundedVec<NodePathAncestor, 0, 512>,
    pub truncated: bool,
}

/// Width is preserved: a 64-bit ID is never padded and reported as 128-bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityObservationQuality {
    #[serde(rename = "native_file_id_128")]
    NativeFileId128,
    #[serde(rename = "native_file_id_64")]
    NativeFileId64,
    PathObservation,
    Unavailable,
}

/// Whether labels and collection membership may bind to this object (docs/41 section 6.3). Without it, feedback.record returns capability_unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityObservationDurableReference {
    Available,
    UnavailableIdentityQuality,
    UnavailableFilesystem,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityObservation {
    /// Width is preserved: a 64-bit ID is never padded and reported as 128-bit.
    pub quality: IdentityObservationQuality,
    /// Whether labels and collection membership may bind to this object (docs/41 section 6.3). Without it, feedback.record returns capability_unavailable.
    pub durable_reference: IdentityObservationDurableReference,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub volume_key: Option<Text<80>>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub file_id_hex: Option<FileIdHex>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub observed_generation: Option<Generation>,
    pub authorises_effects: ConstFalse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestionRanked {
    pub label: Label,
    pub score: Fraction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionReason {
    NoTrainingExamples,
    InsufficientHumanSupport,
    UnfamiliarFeatures,
    AmbiguousModelScores,
    SensitiveExcluded,
}

/// Scores are uncalibrated relative scores, not probabilities (invariant 4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suggestion {
    pub node_id: NodeId,
    pub model_id: ModelId,
    pub ranked: BoundedVec<SuggestionRanked, 0, 32>,
    pub abstain: bool,
    pub reasons: UniqueVec<SuggestionReason, 0, 5>,
    pub feature_coverage: Fraction,
    pub review_priority: Fraction,
    pub calibrated: ConstFalse,
    pub autonomy_allowed: ConstFalse,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub teacher: Option<TeacherLabel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionRef {
    pub collection_id: CollectionId,
    pub name: Text<64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDetailTimestamps {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub created_at: Option<Timestamp>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub modified_at: Option<Timestamp>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub changed_at: Option<Timestamp>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub accessed_at: Option<Timestamp>,
    pub accessed_note: Text<256>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDetailExclusion {
    pub code: EntryFlag,
    pub explanation: Text<512>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeDetailDeniedCapabilityCapability {
    FileMove,
    FileDelete,
    FileRename,
    ContentRead,
    TeacherDisclosure,
    Training,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeDetailDeniedCapabilityReason {
    NotImplementedInV03,
    SensitiveName,
    Excluded,
    NoGrant,
    CloudPlaceholder,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDetailDeniedCapability {
    pub capability: NodeDetailDeniedCapabilityCapability,
    pub reason: NodeDetailDeniedCapabilityReason,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDetail {
    pub row: EntryRow,
    pub root_id: RootId,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub volume_id: Option<VolumeId>,
    pub display_path: DisplayPath,
    pub identity: IdentityObservation,
    pub timestamps: NodeDetailTimestamps,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub subtree: Option<SubtreeTotals>,
    pub threads: Threads,
    pub exclusions: BoundedVec<NodeDetailExclusion, 0, 16>,
    pub denied_capabilities: BoundedVec<NodeDetailDeniedCapability, 0, 16>,
    pub memberships: BoundedVec<CollectionRef, 0, 32>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub suggestion: Option<Suggestion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchRequestKind {
    Any,
    File,
    Dir,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub root_id: Option<RootId>,
    pub text: Text<256>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub extension: Option<Text<32>>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub min_bytes: Option<Bytes>,
    pub kind: SearchRequestKind,
    pub limit: Int<1, 100>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub cursor: Option<Cursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakdownRequestBy {
    ExtFamily,
    Extension,
    AgeBand,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakdownRequest {
    pub node_id: NodeId,
    pub by: BreakdownRequestBy,
    pub basis: Basis,
    pub limit: Int<1, 64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BytesAndFiles {
    pub bytes: Bytes,
    pub files: Count,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakdownBucket {
    pub key: Text<64>,
    pub label: Text<64>,
    pub bytes: Bytes,
    pub files: Count,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Breakdown {
    pub node_id: NodeId,
    pub by: BreakdownRequestBy,
    pub basis: Basis,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub generation: Option<Generation>,
    pub complete: bool,
    pub buckets: BoundedVec<BreakdownBucket, 0, 64>,
    pub other: BytesAndFiles,
    pub unknown: BytesAndFiles,
}
