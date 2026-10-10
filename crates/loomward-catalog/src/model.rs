use crate::{Error, Result};
use serde::{Deserialize, Serialize};

pub const HARDLINK_SUSPECTED: u32 = 1 << 2;
pub const FLAG_NAMES: [&str; 10] = [
    "sensitive_name",
    "protected_context",
    "hardlink_suspected",
    "cloud_placeholder",
    "reparse_not_followed",
    "excluded_state_dir",
    "access_denied",
    "enumeration_error",
    "identity_unavailable",
    "name_lossy",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeKey {
    Atlas,
    Volume(i64),
    Dir(i64),
    File(i64),
    Other(i64),
}
impl NodeKey {
    pub fn reference(self) -> String {
        match self {
            Self::Atlas => "nd_atlas".into(),
            Self::Volume(id) => format!("nd_volume_{id}"),
            Self::Dir(id) => format!("nd_dir_{id}"),
            Self::File(id) => format!("nd_file_{id}"),
            Self::Other(id) => format!("nd_other_{id}"),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    Logical,
    Allocated,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    SizeDesc,
    NameAsc,
    ModifiedDesc,
}

/// Observed names/metadata only. No library method enumerates or opens these display paths.
#[derive(Clone, Debug)]
pub struct RootObservation {
    pub volume_key: String,
    pub display_name: String,
    pub display_path: String,
    pub root_file_id: Option<Vec<u8>>,
    pub filesystem: Option<String>,
    pub origin: String,
    pub granted_via: String,
    pub observed_at_ns: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub name: String,
    pub name_utf16: Option<Vec<u8>>,
    pub file_id: Option<Vec<u8>>,
    pub id_basis: String,
    pub logical: u64,
    pub allocated: Option<u64>,
    pub extension: Option<String>,
    pub family: String,
    pub created_ft: Option<i64>,
    pub modified_ft: Option<i64>,
    pub changed_ft: Option<i64>,
    pub accessed_ft: Option<i64>,
    pub attrs: u32,
    pub reparse_tag: Option<u32>,
    pub flags: u32,
}
impl Observation {
    pub fn file(name: impl Into<String>, logical: u64, allocated: Option<u64>) -> Self {
        Self {
            name: name.into(),
            name_utf16: None,
            file_id: None,
            id_basis: "listed".into(),
            logical,
            allocated,
            extension: None,
            family: "none".into(),
            created_ft: None,
            modified_ft: None,
            changed_ft: None,
            accessed_ft: None,
            attrs: 0,
            reparse_tag: None,
            flags: 0,
        }
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if self
            .file_id
            .as_ref()
            .is_some_and(|id| !matches!(id.len(), 8 | 16))
            || !matches!(self.id_basis.as_str(), "listed" | "post_open" | "none")
            || self.name.is_empty()
            || self.name.chars().count() > 260
            || self.name.contains(['\0', '/', '\\'])
            || matches!(self.name.as_str(), "." | "..")
            || self.logical > i64::MAX as u64
            || self.allocated.is_some_and(|n| n > i64::MAX as u64)
            || self
                .extension
                .as_ref()
                .is_some_and(|s| s.chars().count() > 32)
            || !FAMILIES.contains(&self.family.as_str())
            || self
                .name_utf16
                .as_ref()
                .is_some_and(|s| s.len() > 520 || s.len() % 2 != 0)
            || self.flags >> 10 != 0
        {
            return Err(Error::Invalid("invalid observation"));
        }
        Ok(())
    }
}
pub const FAMILIES: [&str; 15] = [
    "document",
    "spreadsheet",
    "presentation",
    "image",
    "video",
    "audio",
    "archive",
    "code",
    "data",
    "model",
    "executable",
    "system",
    "font",
    "other",
    "none",
];
pub const STATES: [&str; 7] = [
    "complete",
    "partial",
    "denied",
    "excluded",
    "unscanned",
    "stale",
    "cancelled",
];

#[derive(Clone, Debug)]
pub struct DirListing {
    pub run_id: i64,
    pub dir_id: i64,
    pub files: Vec<Observation>,
    pub dirs: Vec<Observation>,
    pub state: String,
    pub skipped: u64,
    pub errors: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListingOutcome {
    Complete,
    Incomplete(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub files: u64,
    pub dirs: u64,
    pub logical: u64,
    pub allocated: u64,
    pub allocation_unknown: u64,
    pub skipped: u64,
    pub errors: u64,
    pub newest_ft: Option<i64>,
    pub complete: bool,
}
#[derive(Clone, Debug)]
pub enum WriteCommand {
    RegisterRoot(RootObservation),
    BeginRun {
        grant_id: i64,
        mode: String,
        strategy: String,
        started_at_ns: i64,
    },
    DirListing(DirListing),
    StageChunk {
        run_id: i64,
        dir_id: i64,
        seq: i64,
        files: Vec<Observation>,
        dirs: Vec<Observation>,
    },
    ListingDone {
        run_id: i64,
        dir_id: i64,
        outcome: ListingOutcome,
        skipped: u64,
        errors: u64,
    },
    RevokeGrant {
        grant_id: i64,
        revoked_at_ns: i64,
    },
    Repair {
        root_id: i64,
    },
    ObjectReference {
        node: NodeKey,
        observed_at_ns: i64,
    },
    ReconcileReference {
        reference_id: i64,
        node: NodeKey,
        observed_at_ns: i64,
    },
    DirFinal {
        run_id: i64,
        dir_id: i64,
        totals: Totals,
        input_revision: i64,
    },
    EndRun {
        run_id: i64,
        state: String,
        finished_at_ns: i64,
    },
    Barrier,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteReply {
    Root {
        grant_id: i64,
        root_id: i64,
        dir_id: i64,
    },
    Run(i64),
    Listing {
        revision: i64,
    },
    Reference(i64),
    Done,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChildrenCursor {
    pub dir_id: i64,
    pub subtree_rev: i64,
    pub catalog_instance: String,
    pub sort: Sort,
    pub basis: Basis,
    pub value: CursorValue,
    pub kind: u8,
    pub row_id: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CursorValue {
    Unknown,
    Number(i64),
    Text(String),
}
#[derive(Clone, Debug)]
pub struct ChildrenRequest {
    pub dir_id: i64,
    pub sort: Sort,
    pub basis: Basis,
    pub limit: usize,
    pub cursor: Option<ChildrenCursor>,
}
#[derive(Clone, Debug)]
pub struct SliceRequest {
    pub anchor: NodeKey,
    pub depth: usize,
    pub max_nodes: usize,
    pub min_share: f64,
    pub basis: Basis,
    pub include_files: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchCursor {
    pub catalog_rev: i64,
    pub catalog_instance: String,
    pub text: String,
    pub extension: Option<String>,
    pub min_bytes: Option<u64>,
    pub kind: String,
    pub root_id: Option<i64>,
    pub last_kind: u8,
    pub last_id: i64,
}
#[derive(Clone, Debug)]
pub struct SearchRequest {
    pub root_id: Option<i64>,
    pub text: String,
    pub extension: Option<String>,
    pub min_bytes: Option<u64>,
    pub kind: String,
    pub limit: usize,
    pub cursor: Option<SearchCursor>,
    pub work_budget: u64,
}

// The following projections mirror contracts/v3. L8 replaces the internal references and cursors.
#[derive(Clone, Debug, Serialize)]
pub struct EntryRow {
    pub node_id: String,
    pub kind: String,
    pub name: String,
    pub extension: Option<String>,
    pub ext_family: Option<String>,
    pub logical_bytes: String,
    pub allocated_bytes: Option<String>,
    pub files: Option<u64>,
    pub dirs: Option<u64>,
    pub modified_at: Option<String>,
    pub attributes: Vec<String>,
    pub flags: Vec<String>,
    pub coverage: String,
    pub location_hint: Option<DisplayPath>,
}
#[derive(Clone, Debug, Serialize)]
pub struct DisplayPath {
    pub text: String,
    pub truncated: bool,
}
impl DisplayPath {
    pub(crate) fn bounded(text: String, partial: bool) -> Self {
        let truncated = partial || text.chars().count() > 1024;
        Self {
            text: text.chars().take(1024).collect(),
            truncated,
        }
    }
}
#[derive(Clone, Debug)]
pub struct EntryPage<C> {
    pub anchor: Option<String>,
    pub generation: Option<String>,
    pub items: Vec<EntryRow>,
    pub next_cursor: Option<C>,
    pub total: Option<u64>,
    pub budget_hit: bool,
}
pub type ChildrenPage = EntryPage<ChildrenCursor>;
pub type SearchPage = EntryPage<SearchCursor>;
#[derive(Clone, Debug, Serialize)]
pub struct MeaningThread {
    pub state: String,
    pub label: Option<String>,
    pub share: Option<f64>,
    pub source: Option<String>,
    pub collection_ids: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ResidencyThread {
    pub volume_id: Option<String>,
    pub tier: Option<u8>,
    pub tier_basis: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct PermissionThread {
    pub state: String,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Threads {
    pub meaning: MeaningThread,
    pub residency: ResidencyThread,
    pub permission: PermissionThread,
}
#[derive(Clone, Debug, Serialize)]
pub struct SliceNode {
    pub node_id: String,
    pub parent: Option<usize>,
    pub kind: String,
    pub name: String,
    pub depth: usize,
    pub size_bytes: String,
    pub size_unknown_files: u64,
    pub logical_bytes: String,
    pub allocated_bytes: Option<String>,
    pub files: u64,
    pub dirs: u64,
    pub child_count: Option<u64>,
    pub folded_count: Option<u64>,
    pub coverage: String,
    pub live: bool,
    pub ext_family: Option<String>,
    pub modified_at: Option<String>,
    pub threads: Threads,
}
#[derive(Clone, Debug, Serialize)]
pub struct RootGeneration {
    pub root_id: String,
    pub generation: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct TreeSlice {
    pub anchor_node_id: String,
    pub basis: Basis,
    pub root_generations: Vec<RootGeneration>,
    pub complete: bool,
    pub live: bool,
    pub ordering: String,
    pub aggregate_state: String,
    pub truncated: bool,
    pub nodes: Vec<SliceNode>,
}
#[derive(Clone, Debug, Serialize)]
pub struct PathPart {
    pub node_id: String,
    pub kind: String,
    pub name: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct NodePath {
    pub root_id: Option<String>,
    pub ancestors: Vec<PathPart>,
    pub truncated: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct SubtreeTotals {
    pub files: u64,
    pub dirs: u64,
    pub logical_bytes: String,
    pub allocated_bytes: Option<String>,
    pub allocation_unknown_files: u64,
    pub stream_coverage: String,
    pub unique_objects: Option<u64>,
    pub unique_allocated_bytes: Option<String>,
    pub multi_link_entries: u64,
    pub skipped: u64,
    pub failed: u64,
    pub complete: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct IdentityObservation {
    pub quality: String,
    pub volume_key: Option<String>,
    pub file_id_hex: Option<String>,
    pub observed_generation: Option<String>,
    pub authorises_effects: bool,
    pub durable_reference: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Timestamps {
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
    pub changed_at: Option<String>,
    pub accessed_at: Option<String>,
    pub accessed_note: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Exclusion {
    pub code: String,
    pub explanation: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct DeniedCapability {
    pub capability: String,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct CollectionRef {
    pub collection_id: String,
    pub name: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct NodeDetail {
    pub row: EntryRow,
    pub root_id: String,
    pub volume_id: Option<String>,
    pub display_path: DisplayPath,
    pub identity: IdentityObservation,
    pub timestamps: Timestamps,
    pub subtree: Option<SubtreeTotals>,
    pub threads: Threads,
    pub exclusions: Vec<Exclusion>,
    pub denied_capabilities: Vec<DeniedCapability>,
    pub memberships: Vec<CollectionRef>,
    pub suggestion: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Serialize)]
pub struct BytesAndFiles {
    pub bytes: String,
    pub files: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Bucket {
    pub key: String,
    pub label: String,
    pub bytes: String,
    pub files: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Breakdown {
    pub node_id: String,
    pub by: String,
    pub basis: Basis,
    pub generation: Option<String>,
    pub complete: bool,
    pub buckets: Vec<Bucket>,
    pub other: BytesAndFiles,
    pub unknown: BytesAndFiles,
}

pub(crate) fn timestamp(ft: Option<i64>) -> Option<String> {
    ft.and_then(|v| {
        time::OffsetDateTime::from_unix_timestamp_nanos((v as i128 - 116_444_736_000_000_000) * 100)
            .ok()
    })
    .and_then(|t| {
        t.format(&time::format_description::well_known::Rfc3339)
            .ok()
    })
}
pub(crate) fn flags(bits: u32) -> Vec<String> {
    FLAG_NAMES
        .iter()
        .enumerate()
        .filter(|(i, _)| bits & (1 << i) != 0)
        .map(|(_, s)| s.to_string())
        .collect()
}
