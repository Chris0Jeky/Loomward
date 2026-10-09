//! Tier model and placement simulation. Mirrors `contracts/v3/view-service.schema.json`; the contract tests keep them equal.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TierVolumePressure {
    Ok,
    Watch,
    Pressure,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TierVolume {
    pub volume_id: VolumeId,
    pub display_name: Text<64>,
    pub tier: TierInfo,
    pub online: bool,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub writable: Option<bool>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub capacity_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub free_bytes: Option<Bytes>,
    pub reserve_bytes: Bytes,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub free_fraction: Option<Fraction>,
    pub pressure: TierVolumePressure,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TierModelPolicy {
    pub watch_free_fraction: Fraction,
    pub pressure_free_fraction: Fraction,
    pub reserve_note: Text<256>,
    pub note: Text<512>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TierModel {
    pub volumes: BoundedVec<TierVolume, 0, 64>,
    pub policy: TierModelPolicy,
}

/// mtime_proxy is a labelled what-if only; modification age is not access heat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeatBasis {
    Unknown,
    Assumed,
    MtimeProxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateBasis {
    RootChildren,
    LargestDirs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementCandidatesRequest {
    pub source_volume_id: VolumeId,
    pub basis: CandidateBasis,
    pub max_groups: Int<1, 200>,
    pub min_bytes: Bytes,
}

const_str!(
    pub CandidateGroupEstimateBasis = "source_allocated_destination_logical_transfer_logical"
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateGroup {
    pub group_id: GroupId,
    pub node_id: NodeId,
    pub root_id: RootId,
    pub name: Text<260>,
    pub source_bytes: Bytes,
    pub destination_bytes: Bytes,
    pub transfer_bytes: Bytes,
    pub estimate_basis: CandidateGroupEstimateBasis,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub heat: Option<Fraction>,
    pub heat_basis: HeatBasis,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub newest_modified_at: Option<Timestamp>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub pinned: Option<bool>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub active: Option<bool>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub protected: Option<bool>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub days_since_move: Option<Int<0, 36500>>,
    pub coverage: CoverageState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementCandidates {
    pub source_volume_id: VolumeId,
    pub root_generations: BoundedVec<RootGeneration, 0, 64>,
    pub groups: BoundedVec<CandidateGroup, 0, 200>,
    pub note: Text<512>,
}

/// Owner what-if assumptions, recorded as assumed in the plan. They never change capacities or volumes; those come from observations only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupOverride {
    pub group_id: GroupId,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub heat: Option<Option<Fraction>>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub pinned: Option<Option<bool>>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub active: Option<Option<bool>>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub protected: Option<Option<bool>>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub days_since_move: Option<Option<Int<0, 36500>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementSimulateRequestHeatPolicy {
    UnknownIsIneligible,
    MtimeProxyWhatif,
    AssumedOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementSimulateRequest {
    pub source_volume_id: VolumeId,
    pub target_free_bytes: Bytes,
    pub max_transfer_bytes: Bytes,
    pub heat_policy: PlacementSimulateRequestHeatPolicy,
    pub candidate_basis: CandidateBasis,
    pub max_groups: Int<1, 200>,
    pub overrides: BoundedVec<GroupOverride, 0, 200>,
    pub node_budget: Int<0, 200000>,
    pub save: bool,
}

const_str!(
    pub PlacementProposalReason = "bounded_capacity_budget_allocation"
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementProposal {
    pub group_id: GroupId,
    pub source_id: VolumeId,
    pub target_id: VolumeId,
    pub source_bytes_relieved: Bytes,
    pub destination_bytes_required: Bytes,
    pub transfer_bytes: Bytes,
    pub reason: PlacementProposalReason,
    pub requires_consent: ConstTrue,
    pub executable: ConstFalse,
}

const_str!(
    pub PlacementPlanMode = "simulation"
);

const_str!(
    pub PlacementPlanAlgorithm = "bounded_portfolio_search_v2"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementPlanSearchReason {
    Exhausted,
    NodeBudget,
    ProblemSizeLimit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementPlanSearch {
    pub complete: bool,
    pub nodes_visited: Count,
    pub node_budget: Count,
    pub reason: PlacementPlanSearchReason,
    pub eligible_groups: Count,
    pub eligible_targets: Count,
    pub lower_bound_shortfall_bytes: Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementPlanRejectedReason {
    NotOnSource,
    PinnedActiveProtectedOrUnspecified,
    HeatUnknown,
    NotCold,
    CooldownOrHistoryUnknown,
    SourceOfflineOrReadonly,
    NotSelectedByBoundedAllocator,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementPlanRejected {
    pub group_id: GroupId,
    pub reason: PlacementPlanRejectedReason,
}

/// Field-for-field the planner_v2 output (python/loomward/planner_v2.py) with bytes as decimal strings, plus heat_policy, assumptions, root_generations and proposal_id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementPlan {
    pub mode: PlacementPlanMode,
    pub algorithm: PlacementPlanAlgorithm,
    pub optimality_claim: bool,
    pub optimality_scope: Text<512>,
    pub shortfall_optimal: bool,
    pub search: PlacementPlanSearch,
    pub proposals: BoundedVec<PlacementProposal, 0, 200>,
    pub rejected: BoundedVec<PlacementPlanRejected, 0, 200>,
    pub projected_free_bytes: BoundedMap<VolumeId, Bytes, 64>,
    pub target_free_bytes: Bytes,
    pub shortfall_bytes: Bytes,
    pub satisfied: bool,
    pub transfer_bytes: Bytes,
    pub filesystem_changed: ConstFalse,
    pub baseline_shortfall_bytes: Bytes,
    pub shortfall_improvement_bytes: Bytes,
    pub assumption: Text<512>,
    pub heat_policy: PlacementSimulateRequestHeatPolicy,
    pub assumptions: BoundedVec<Text<256>, 0, 16>,
    pub root_generations: BoundedVec<RootGeneration, 0, 64>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub proposal_id: Option<ProposalId>,
}

const_str!(
    pub ProposalSummaryKind = "placement_simulation"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalSummaryStatus {
    SimulationOnly,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalSummary {
    pub proposal_id: ProposalId,
    pub kind: ProposalSummaryKind,
    pub status: ProposalSummaryStatus,
    pub created_at: Timestamp,
    pub source_volume_id: VolumeId,
    pub satisfied: bool,
    pub shortfall_bytes: Bytes,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalList {
    pub proposals: BoundedVec<ProposalSummary, 0, 100>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalRefRequest {
    pub proposal_id: ProposalId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalDetail {
    pub summary: ProposalSummary,
    pub inputs_digest: Digest,
    pub plan: PlacementPlan,
}
