//! LW-119: developer-clutter advice for one granted root. Mirrors `contracts/v3/view-service.schema.json`;
//! the contract tests keep them equal. Advice from recorded names only: never a delete action, never a
//! claim that anything is safe to remove, never a size where one was not observed (invariants 1 and 4).

use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeveloperClutterRequest {
    pub root_id: RootId,
    pub limit: Int<1, 200>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperClutterProvider {
    Cargo,
    Npm,
    Python,
    Dotnet,
    Gradle,
}

/// What the conventional directory name is. A reading of the name, not of the contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperClutterKind {
    BuildOutput,
    DependencyInstall,
    Environment,
    BytecodeCache,
    BuildCache,
}

/// `unknown` is the only value today: regeneration cost is measured by LW-087, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegenerationCost {
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeveloperClutterCandidate {
    /// The conventional output directory.
    pub node_id: NodeId,
    pub name: Text<260>,
    /// The recorded manifest beside it that made the name a candidate.
    pub anchor_node_id: NodeId,
    pub anchor_name: Text<260>,
    pub provider: DeveloperClutterProvider,
    pub kind: DeveloperClutterKind,
    /// Observed logical bytes under the directory; null is unknown (partial, stale or unscanned), never zero.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub logical_bytes: Option<Bytes>,
    /// null when any allocation under it is unknown.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub allocated_bytes: Option<Bytes>,
    pub coverage: CoverageState,
    pub regeneration_cost: RegenerationCost,
    pub regeneration_hint: Text<256>,
    pub reason: Text<256>,
    pub uncertainty: Text<512>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClutterTopic {
    RegenerationCost,
    ModelHostLifecycle,
    StaleWorktrees,
}

/// A part of LW-119 that no provider supplies yet: said so, never filled with a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClutterNotAssessed {
    pub topic: ClutterTopic,
    pub note: Text<256>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeveloperClutterReport {
    pub root_id: RootId,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub generation: Option<Generation>,
    /// Version of the conventional-name rules, not an executable provider contract.
    pub rules_version: Int<1, 1000>,
    /// The root is mid-scan, stale or not fully covered, or a candidate's size is not fully known:
    /// projects may be missing and sizes may be lower bounds.
    pub partial: bool,
    /// More candidates exist than `limit` returned.
    pub truncated: bool,
    /// The detector reached its work budget: `candidates` is then empty and says nothing about the root.
    pub budget_hit: bool,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub budget_note: Option<Text<256>>,
    /// Largest known logical size first, then unknown sizes.
    pub candidates: BoundedVec<DeveloperClutterCandidate, 0, 200>,
    pub not_assessed: BoundedVec<ClutterNotAssessed, 0, 8>,
}
