//! LW-138 (#877): `insight.folder`, an owner-only rule inference over one folder's catalogue facts.
//! Never a decision, a probability, an approval or an effect: labels come from the labelling rules,
//! sentences from `loomward_learn::explain`, and every sentence names the packet fields it rests on.
use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsightFolderRequest {
    pub root_id: RootId,
    pub node_id: NodeId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsightAxisName {
    Meaning,
    Lifecycle,
    Recreatability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsightOutcome {
    Label,
    Abstain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsightAbstainReason {
    NoVote,
    Conflict,
    Coverage,
}

/// One labelling rule's vote on an axis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsightVote {
    pub function: Text<64>,
    pub label: Text<64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsightAxis {
    pub axis: InsightAxisName,
    pub outcome: InsightOutcome,
    /// Set exactly when the rules agree; an abstention carries `abstain_reason` instead.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub label: Option<Text<64>>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub abstain_reason: Option<InsightAbstainReason>,
    /// The agreeing votes for a label, the disagreeing ones for a conflict, none otherwise.
    pub votes: BoundedVec<InsightVote, 0, 32>,
}

/// A fact-only sentence and the evidence-packet fields (JSON pointers) it was read from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsightLine {
    pub text: Text<1024>,
    pub trace: BoundedVec<Text<64>, 0, 16>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsightFolder {
    pub root_id: RootId,
    pub node_id: NodeId,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub generation: Option<Generation>,
    /// When the facts were read; the lines' coldness months are relative to it.
    pub observed_at: Timestamp,
    pub coverage: CoverageState,
    /// True unless coverage is complete: counts and dates may be incomplete, and the axes abstain.
    pub partial: bool,
    /// Always the three axes, in this order: meaning, lifecycle, recreatability.
    pub axes: BoundedVec<InsightAxis, 3, 3>,
    pub lines: BoundedVec<InsightLine, 1, 32>,
}
