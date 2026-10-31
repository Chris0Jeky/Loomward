//! Learning, feedback and collections. Mirrors `contracts/v3/view-service.schema.json`; the contract tests keep them equal.

use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Taxonomy {
    pub version: Int<1, 9007199254740991>,
    pub labels: UniqueVec<Label, 1, 32>,
}

/// label is null only when retract is true (service-enforced); the service stores the withdrawn label internally (docs/41 section 9.1). client_event_id is a durable idempotency key: identical replay returns the original result, reuse with different content is invalid_request. Records a human label; never a grant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackRequest {
    pub node_id: NodeId,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub label: Option<Label>,
    pub retract: bool,
    pub client_event_id: ClientEventId,
}

const_str!(
    pub LearningStatusAlgorithm = "weighted_multinomial_nb_v1"
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningStatus {
    /// Named personal-learning refusal, or null for synthetic sessions.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub gate: Option<BoundedStr<1, 64>>,
    /// Stored human events excluded because their durable references are unresolved.
    pub unresolved_human_events: Count,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub model_id: Option<ModelId>,
    pub algorithm: LearningStatusAlgorithm,
    pub taxonomy_version: Int<1, 9007199254740991>,
    pub training_count: Count,
    pub human_events: Count,
    pub teacher_labels: Count,
    pub human_support: BoundedMap<String, Count, 32>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub fitted_at: Option<Timestamp>,
    pub dataset_class: DatasetClass,
    pub calibrated: ConstFalse,
    pub autonomy_allowed: ConstFalse,
}

const_str!(
    pub FeedbackResultDataClass = "human"
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackResult {
    pub event_id: EventId,
    pub revision: Int<1, 9007199254740991>,
    /// True when client_event_id matched an already committed identical event; nothing new was written.
    pub idempotent_replay: bool,
    pub data_class: FeedbackResultDataClass,
    pub learning: LearningStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackHistoryRequest {
    pub limit: Int<1, 100>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub cursor: Option<Cursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackReferenceStatus {
    Resolved,
    Unresolved,
    Redacted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackHistoryItem {
    pub event_id: EventId,
    pub revision: Int<1, 9007199254740991>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub label: Option<Label>,
    pub retracted: bool,
    /// Latest event for this durable object, including a withdrawal; not training eligibility.
    pub is_current: bool,
    pub taxonomy_version: Int<1, 9007199254740991>,
    pub recorded_at: Timestamp,
    pub reference_status: FeedbackReferenceStatus,
    /// Recorded name, never a current path. Hidden unless provenance and reference are visible.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub name: Option<BoundedStr<0, 256>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackHistory {
    pub items: BoundedVec<FeedbackHistoryItem, 0, 100>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub next_cursor: Option<Cursor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningQueueRequest {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub root_id: Option<RootId>,
    pub limit: Int<1, 100>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub cursor: Option<Cursor>,
    /// Omitted means ordinary review, for older callers.
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub mode: Option<LearningQueueMode>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningQueueMode {
    #[default]
    Review,
    Deferred,
}

/// An owner preference, never a human label or permission. Requires expected_state_rev.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningDeferredRequest {
    pub node_id: NodeId,
    pub deferred: bool,
    pub client_event_id: ClientEventId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningDeferredResult {
    pub deferred: bool,
    pub revision: Generation,
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningQueueItem {
    pub row: EntryRow,
    pub suggestion: Suggestion,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningQueue {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub model_id: Option<ModelId>,
    pub items: BoundedVec<LearningQueueItem, 0, 100>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub next_cursor: Option<Cursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionKind {
    Human,
    LabelView,
}

/// Virtual membership in Loomward's own database. Never moves, links or renames a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Collection {
    pub collection_id: CollectionId,
    pub name: BoundedStr<1, 64>,
    pub kind: CollectionKind,
    pub member_count: Count,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub logical_bytes: Option<Bytes>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionList {
    pub collections: BoundedVec<Collection, 0, 256>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionCreateRequest {
    pub name: CollectionName,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CollectionMembersUpdate {
    pub collection_id: CollectionId,
    pub add: UniqueVec<NodeId, 0, 500>,
    pub remove: UniqueVec<NodeId, 0, 500>,
    /// Stable local references can be removed without a live node or an active root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_members: Option<UniqueVec<BoundedStr<1, 64>, 0, 500>>,
}

impl<'de> Deserialize<'de> for CollectionMembersUpdate {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            collection_id: CollectionId,
            add: UniqueVec<NodeId, 0, 500>,
            remove: UniqueVec<NodeId, 0, 500>,
            #[serde(default, deserialize_with = "crate::types::optional_present")]
            remove_members: Option<UniqueVec<BoundedStr<1, 64>, 0, 500>>,
        }
        let fields = Fields::deserialize(deserializer)?;
        // semantics.md (Collections): adding and removing the same reference is refused, since
        // the result would depend on the order the two sets were applied in.
        if fields.add.iter().any(|id| fields.remove.contains(id)) {
            return Err(serde::de::Error::custom("adds and removes overlap"));
        }
        Ok(Self {
            collection_id: fields.collection_id,
            add: fields.add,
            remove: fields.remove,
            remove_members: fields.remove_members,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionMembersRequest {
    pub collection_id: CollectionId,
    pub limit: Int<1, 200>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub cursor: Option<Cursor>,
}

/// A durable member remains visible when its catalogue object cannot be resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionMemberStatus {
    Resolved,
    Unresolved,
    RootRevoked,
    RootUnavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CollectionMember {
    pub member_id: BoundedStr<1, 64>,
    pub status: CollectionMemberStatus,
    pub entry: Option<super::tree::EntryRow>,
}

impl<'de> Deserialize<'de> for CollectionMember {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            member_id: BoundedStr<1, 64>,
            status: CollectionMemberStatus,
            #[serde(deserialize_with = "crate::types::required_nullable")]
            entry: Option<super::tree::EntryRow>,
        }
        let fields = Fields::deserialize(deserializer)?;
        if (fields.status == CollectionMemberStatus::Resolved) != fields.entry.is_some() {
            return Err(serde::de::Error::custom(
                "entry must be present exactly when resolved",
            ));
        }
        Ok(Self {
            member_id: fields.member_id,
            status: fields.status,
            entry: fields.entry,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionMemberPage {
    pub collection_id: CollectionId,
    pub items: BoundedVec<CollectionMember, 0, 200>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub next_cursor: Option<Cursor>,
    pub total: Count,
    pub budget_hit: bool,
}
