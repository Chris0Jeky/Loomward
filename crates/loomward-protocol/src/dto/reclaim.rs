//! LW-131 R1d: read-only advice. Owner steps and relocation settings are display text only.
use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReclaimPlanRequest {
    pub volume_id: VolumeId,
    pub limit: Int<1, 200>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReclaimVolume {
    pub volume_id: VolumeId,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub label: Option<Text<64>>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub bus_type: Option<Text<64>>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub capacity_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub free_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub reserve_bytes: Option<Bytes>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoveredRoot {
    pub root_id: RootId,
    pub coverage: CoverageState,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub allocated_bytes: Option<Bytes>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    ToolOwnedCleanup,
    Relocate,
    ArchiveCandidates,
    Informational,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relocation {
    pub variable_or_setting: Text<256>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub suggested_target_volume: Option<Text<128>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReclaimCandidate {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub node_id: Option<NodeId>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub name: Option<Text<260>>,
    pub class: Text<64>,
    pub provider: Text<64>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub kind: Option<Text<64>>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub logical_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub allocated_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub reclaimable_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub reclaimable_reason: Option<Text<256>>,
    pub coverage: CoverageState,
    pub owner_step: Text<1024>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub relocation: Option<Relocation>,
    pub evidence: BoundedVec<Text<1024>, 0, 16>,
    pub uncertainty: Text<1024>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReclaimGroup {
    pub kind: GroupKind,
    pub candidates: BoundedVec<ReclaimCandidate, 0, 200>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReclaimPlan {
    pub volume: ReclaimVolume,
    pub covered_roots: BoundedVec<CoveredRoot, 0, 64>,
    pub groups: BoundedVec<ReclaimGroup, 4, 4>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub unaccounted_bytes: Option<Bytes>,
    pub rules_version: Int<1, 1000>,
    pub partial: bool,
    pub truncated: bool,
    pub budget_hit: bool,
}
