//! Event payloads. Mirrors `contracts/v3/view-service.schema.json`; the contract tests keep them equal.

use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamHello {
    pub session_started_at: Timestamp,
    pub last_seq: Count,
    pub dataset_class: DatasetClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamLaggedResync {
    Roots,
    Volumes,
    Jobs,
    Tree,
    Learning,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamLagged {
    pub dropped: Count,
    pub resync: UniqueVec<StreamLaggedResync, 0, 5>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanProgressEvent {
    pub job_id: JobId,
    pub root_id: RootId,
    pub progress: ScanProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeInvalidatedScope {
    All,
    Nodes,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeInvalidated {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub root_id: Option<RootId>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub generation: Option<Generation>,
    pub scope: TreeInvalidatedScope,
    pub node_ids: BoundedVec<NodeId, 0, 256>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetrySampleEvent {
    pub subscription_id: SubscriptionId,
    pub sample: TelemetrySample,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningUpdated {
    pub status: LearningStatus,
}
