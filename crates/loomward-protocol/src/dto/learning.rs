//! Learning, feedback and collections. Mirrors `contracts/v3/view-service.schema.json`; the contract tests keep them equal.

use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Taxonomy {
    pub version: Int<1, 9007199254740991>,
    pub labels: UniqueVec<Label, 1, 32>,
}

/// label is null only when retract is true (service-enforced). Records a human label; never a grant.
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
    pub data_class: FeedbackResultDataClass,
    pub learning: LearningStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningQueueRequest {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub root_id: Option<RootId>,
    pub limit: Int<1, 100>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub cursor: Option<Cursor>,
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
    pub logical_bytes: Bytes,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionMembersUpdate {
    pub collection_id: CollectionId,
    pub add: UniqueVec<NodeId, 0, 500>,
    pub remove: UniqueVec<NodeId, 0, 500>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionMembersRequest {
    pub collection_id: CollectionId,
    pub limit: Int<1, 200>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub cursor: Option<Cursor>,
}
