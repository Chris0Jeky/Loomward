use crate::{Error, Result};
use serde::{Deserialize, Serialize};

pub const HARDLINK_SUSPECTED: u32 = 1 << 2;
/// `cloud_placeholder`, `reparse_not_followed` and `excluded_state_dir`: an entry the scan saw
/// but deliberately did not follow. It stays visible (listing state `excluded` for a directory)
/// and is never counted in totals.
pub const EXCLUDED_FLAGS: u32 = (1 << 3) | (1 << 4) | (1 << 5);
/// `identity_unavailable`: the row carries no usable native file ID.
pub const IDENTITY_UNAVAILABLE: u32 = 1 << 8;
/// `identity_unverified` (#219): listed on a FAT, exFAT or unknown filesystem. Metadata only:
/// never a placement candidate, duplicate content-read input or durable reference.
pub const IDENTITY_UNVERIFIED: u32 = 1 << 10;
pub const FLAG_NAMES: [&str; 11] = [
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
    "identity_unverified",
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
    /// Handle-derived GUID as 32 lowercase hexadecimal digits; unknown for non-native fixtures.
    pub volume_guid: Option<String>,
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
            || self.flags >> FLAG_NAMES.len() != 0
            // #219: an identity-unverified row never stores a (recyclable) file ID.
            || (self.flags & IDENTITY_UNVERIFIED != 0 && self.file_id.is_some())
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
impl ListingOutcome {
    pub(crate) fn coverage(&self) -> Result<&str> {
        match self {
            Self::Complete => Ok("complete"),
            Self::Incomplete(reason)
                if reason != "complete" && STATES.contains(&reason.as_str()) =>
            {
                Ok(reason)
            }
            _ => Err(Error::Invalid("incomplete listing reason")),
        }
    }
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
    /// Forget a revoked grant (#288): state.db first (labels, references, the display path), then
    /// the catalogue rows. Idempotent. `expected_state_rev` is the owner-state precondition.
    ForgetGrant {
        grant_id: i64,
        forgotten_at_ns: i64,
        expected_state_rev: Option<i64>,
    },
    /// The granted root no longer has the grant's native identity (docs/41 section 7 step 1):
    /// the grant ends `identity_changed` and its root's run is cancelled, as a revocation does.
    GrantIdentityChanged {
        grant_id: i64,
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
/// Keyset position of `search.query` (q-34): the match pass and the last examined index key. It
/// binds the catalogue instance and the filters, not `catalog_rev`, so live watcher commits do not
/// invalidate a paging user; a row changed between pages may be skipped or seen twice.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchCursor {
    pub catalog_instance: String,
    pub filters: String,
    pub pass: u8,
    pub last_key: i64,
    /// Extension-only listings run largest first: the size of the last examined file.
    pub last_size: Option<i64>,
    /// Ranked search only: the `catalog_rev` its pool was ranked at. A ranked cursor is refused
    /// once the catalogue has moved (`StaleGeneration`); the keyset cursor above never is.
    #[serde(default)]
    pub revision: Option<i64>,
}
#[derive(Clone, Debug, Default)]
pub struct SearchRequest {
    pub root_id: Option<i64>,
    pub text: String,
    pub extension: Option<String>,
    pub min_bytes: Option<u64>,
    pub max_bytes: Option<u64>,
    /// FILETIME bounds on the modified time, inclusive. An entry with no modified time never matches one.
    pub modified_after: Option<i64>,
    pub modified_before: Option<i64>,
    pub kind: String,
    pub limit: usize,
    pub cursor: Option<SearchCursor>,
    /// A client's disclosure scope (LW-069): root ids it may see. `None` is the owner's view; with
    /// `Some`, rows, root states, the generation and the cursor binding all stay inside it, and a
    /// `root_id` outside it is `NotFound`, as an unknown root is.
    pub scope: Option<Vec<i64>>,
    pub work_budget: u64,
    /// Wall-clock work per page: past it the page returns what it has, `budget_hit` and a cursor.
    pub time_budget: Option<std::time::Duration>,
}
#[derive(Clone, Debug, Serialize)]
pub struct SearchHit {
    pub row: EntryRow,
    /// The catalogue root row the hit lies under.
    pub root_id: i64,
    /// Root-relative path of the hit itself, `\`-separated, display only.
    pub path: DisplayPath,
    pub match_kind: String,
    /// Half-open UTF-16 offsets into `row.name` of the matched text; best effort, may be empty.
    pub highlights: Vec<[u32; 2]>,
}
#[derive(Clone, Debug, Serialize)]
pub struct SearchRootState {
    pub root_id: i64,
    pub scan_state: String,
    pub coverage: String,
}
#[derive(Clone, Debug)]
pub struct SearchPage {
    pub generation: Option<String>,
    pub hits: Vec<SearchHit>,
    pub next_cursor: Option<SearchCursor>,
    pub budget_hit: bool,
    pub index_state: String,
    pub partial: bool,
    pub substring_searched: bool,
    pub roots: Vec<SearchRootState>,
}

// The following projections mirror contracts/v3. L8 replaces the internal references and cursors.
#[derive(Clone, Debug, Serialize)]
pub struct EntryRow {
    pub node_id: String,
    pub kind: String,
    pub name: String,
    pub extension: Option<String>,
    pub ext_family: Option<String>,
    pub logical_bytes: Option<String>,
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
/// The FILETIME of an RFC 3339 timestamp, the inverse of `timestamp`; None when unparseable or
/// outside the FILETIME range.
pub fn filetime(rfc3339: &str) -> Option<i64> {
    let t = time::OffsetDateTime::parse(rfc3339, &time::format_description::well_known::Rfc3339)
        .ok()?;
    i64::try_from(t.unix_timestamp_nanos() / 100 + 116_444_736_000_000_000)
        .ok()
        .filter(|ft| *ft >= 0)
}
pub(crate) fn flags(bits: u32) -> Vec<String> {
    FLAG_NAMES
        .iter()
        .enumerate()
        .filter(|(i, _)| bits & (1 << i) != 0)
        .map(|(_, s)| s.to_string())
        .collect()
}
