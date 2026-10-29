use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectsRequest {
    pub root_id: RootId,
    pub limit: Int<1, 200>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub cursor: Option<Cursor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMarker {
    pub name: Text<260>,
    pub provider: DeveloperClutterProvider,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectCandidate {
    pub node_id: NodeId,
    pub name: Text<260>,
    pub path: DisplayPath,
    pub coverage: CoverageState,
    pub markers: BoundedVec<ProjectMarker, 1, 16>,
    pub markers_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectsPage {
    pub root_id: RootId,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub generation: Option<Generation>,
    pub coverage: CoverageState,
    pub partial: bool,
    pub budget_hit: bool,
    pub candidates: BoundedVec<ProjectCandidate, 0, 200>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub next_cursor: Option<Cursor>,
}
