use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuplicateBudget {
    pub max_candidates: Int<2, 200>,
    pub max_bytes: Bytes,
    pub max_ms: Int<1, 60000>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub min_bytes: Option<Bytes>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuplicatesInspectRequest {
    pub root_id: RootId,
    pub budget: DuplicateBudget,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuplicateEntry {
    pub relative_path: BoundedStr<1, 32768>,
    pub logical_bytes: Bytes,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuplicateSkip {
    pub entry: Int<0, 199>,
    pub reason: BoundedStr<1, 64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuplicateReport {
    pub entries: BoundedVec<DuplicateEntry, 0, 200>,
    pub groups: BoundedVec<UniqueVec<Int<0, 199>, 2, 200>, 0, 100>,
    pub links: BoundedVec<UniqueVec<Int<0, 199>, 1, 200>, 0, 200>,
    pub links_not_compared: Count,
    pub skipped: BoundedVec<DuplicateSkip, 0, 200>,
    pub bytes_read: Bytes,
    pub partial: bool,
    pub truncated: bool,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub stop: Option<BoundedStr<1, 64>>,
}
