//! LW-067 disclosure projections; owner DTOs remain complete. Absent means withheld, null means unknown.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientField {
    Path,
    Size,
    Modified,
    Attributes,
    Volumes,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientGrantRequest {
    pub client: McpClient,
    pub roots: UniqueVec<RootId, 1, 64>,
    pub fields: UniqueVec<ClientField, 0, 5>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub expires_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientGrantResult {
    pub grant_id: BoundedStr<1, 64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientGrantRecord {
    pub grant_id: BoundedStr<1, 64>,
    pub client: McpClient,
    pub roots: BoundedVec<RootId, 0, 64>,
    pub fields: UniqueVec<ClientField, 0, 5>,
    pub created_at: Timestamp,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub expires_at: Option<Timestamp>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub revoked_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientGrantList {
    pub grants: Vec<ClientGrantRecord>,
    pub available_fields: UniqueVec<ClientField, 0, 5>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientRevokeRequest {
    pub client: McpClient,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientRevokeResult {
    pub client: McpClient,
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientEntryRow {
    pub node_id: NodeId,
    pub kind: EntryRowKind,
    pub name: Text<260>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub extension: Option<Text<32>>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub ext_family: Option<ExtFamily>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub logical_bytes: Option<Option<Bytes>>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub allocated_bytes: Option<Option<Bytes>>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub files: Option<Option<Count>>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub dirs: Option<Option<Count>>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub modified_at: Option<Option<Timestamp>>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub attributes: Option<UniqueVec<FileAttribute, 0, 16>>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub flags: Option<UniqueVec<EntryFlag, 0, 13>>,
    pub coverage: CoverageState,
    /// Search and collection results only: the parent's display path.
    #[serde(
        default,
        deserialize_with = "crate::types::optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub location_hint: Option<Option<DisplayPath>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientSearchHit {
    pub row: ClientEntryRow,
    pub root_id: RootId,
    /// Root-relative path of the hit itself, display only.
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub path: Option<DisplayPath>,
    pub match_kind: SearchMatchKind,
    /// Federated extension; absent means `own`.
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub source: Option<SearchSource>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub freshness: Option<SearchFreshness>,
    /// A relative rank signal, never a probability; not comparable across sources.
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub score: Option<Fraction>,
    /// Half-open UTF-16 offsets into row.name; best effort and may be empty.
    pub highlights: BoundedVec<BoundedVec<Int<0, 1024>, 2, 2>, 0, 8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientSearchPage {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub generation: Option<Generation>,
    pub hits: BoundedVec<ClientSearchHit, 0, 100>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub next_cursor: Option<Cursor>,
    pub budget_hit: bool,
    /// building: a searched root is being scanned; stale: a searched root is stale or failed.
    pub index_state: SearchPageIndexState,
    /// A searched root does not have complete coverage: no hit is not proof of absence.
    pub partial: bool,
    /// False when the text was too short for a trigram: inner substrings were not searched.
    pub substring_searched: bool,
    pub roots: BoundedVec<SearchRootCoverage, 0, 64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientNodeDetail {
    pub row: ClientEntryRow,
    pub root_id: RootId,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub display_path: Option<DisplayPath>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientRootScanState {
    pub state: RootScanStateState,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub generation: Option<Generation>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub finished_at: Option<Timestamp>,
    pub coverage: CoverageState,
}

/// A granted root's coverage and freshness; no owner grant metadata or subtree totals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientRoot {
    pub root_id: RootId,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub display_path: Option<DisplayPath>,
    pub scan: ClientRootScanState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientRootList {
    pub roots: BoundedVec<ClientRoot, 0, 64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientEntryPage {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub anchor: Option<EntryPageAnchor>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub generation: Option<Generation>,
    pub items: BoundedVec<ClientEntryRow, 0, 200>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub next_cursor: Option<Cursor>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub total: Option<Count>,
    pub budget_hit: bool,
}
