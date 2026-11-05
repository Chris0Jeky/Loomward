//! #1092: `changes.new_since`, what first appeared in one granted root since a baseline scan run,
//! read from `born_run` (no schema change). A count of rows first listed after the baseline, never a
//! claim that a file is a new file on disk: a file moved across folders or recreated reads as new, and
//! anything an earlier run could not list is born in the first run that lists it. Unknown stays null
//! (invariant 4): no earlier scan is `no_baseline`, never zero. Read-only; no effect.
use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangesNewSinceRequest {
    pub root_id: RootId,
    /// A scan run of this root (the generation a full scan published); null is the scan before the
    /// newest completed full-root scan ("since the previous scan").
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub baseline_run: Option<Generation>,
    pub limit: Int<1, 50>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewSinceOutcome {
    Ok,
    /// No earlier completed scan of this root exists to compare with. Every figure is null.
    NoBaseline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewSinceRunMode {
    Full,
    Refresh,
    Targeted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewSinceRun {
    pub run: Generation,
    pub mode: NewSinceRunMode,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub finished_at: Option<Timestamp>,
    /// Completed and the root aggregate was a full, clean traversal at its end. A run recorded
    /// without that counter is not known to be complete.
    pub complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewSinceReason {
    /// The requested baseline was cancelled or failed: no comparison is available.
    BaselineNotCompleted,
    /// The baseline run finished but was not a full, clean traversal (or has no recorded counter).
    BaselineIncomplete,
    /// The root's coverage is not complete now.
    CoveragePartial,
    /// A scan of this root is running: rows it has listed so far are included.
    ScanRunning,
    /// The root's aggregates are being repaired: folder sizes are not current.
    CatalogueRepairing,
    /// The watcher saw changes since the last scan: the figures describe an earlier state.
    ChangesSinceScan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewSinceCaveatTopic {
    MovedFilesCountAsNew,
    RecreatedFilesCountAsNew,
    FirstListingCountsAsNew,
    ExistingFilesNotMeasured,
}

/// A fixed limit of the method, said with the result so a client cannot show the figure without it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewSinceCaveat {
    pub topic: NewSinceCaveatTopic,
    pub note: Text<256>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewSinceFile {
    pub node_id: NodeId,
    pub name: Text<260>,
    pub path: DisplayPath,
    pub logical_bytes: Bytes,
    /// Null when the allocation was not observed, never zero.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub allocated_bytes: Option<Bytes>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewSinceFolder {
    pub node_id: NodeId,
    pub name: Text<260>,
    pub path: DisplayPath,
    /// The folder's whole recorded size now (including anything moved into it); null is unknown.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub logical_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub allocated_bytes: Option<Bytes>,
    pub coverage: CoverageState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, try_from = "ChangesNewSinceWire")]
pub struct ChangesNewSince {
    pub root_id: RootId,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub generation: Option<Generation>,
    pub outcome: NewSinceOutcome,
    /// The run compared against; null exactly when `outcome` is `no_baseline`.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub baseline: Option<NewSinceRun>,
    /// The newest completed run of any mode (watcher and targeted runs included); null when none.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub latest: Option<NewSinceRun>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub new_files: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub new_dirs: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub logical_bytes: Option<Bytes>,
    /// Null when any new file's allocation is unknown (see `allocation_unknown_files`), never a partial sum.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub allocated_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub allocation_unknown_files: Option<Count>,
    /// Largest first by logical size, row order breaking ties; at most `limit`.
    pub largest_files: BoundedVec<NewSinceFile, 0, 50>,
    /// New folders whose parent is not new, largest subtree first; at most `limit`.
    pub largest_folders: BoundedVec<NewSinceFolder, 0, 50>,
    /// True when the figures are not a clean answer; `partial_reasons` say why.
    /// Also explains an unusable explicit baseline.
    pub partial: bool,
    pub partial_reasons: BoundedVec<NewSinceReason, 0, 8>,
    pub caveats: BoundedVec<NewSinceCaveat, 0, 8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangesNewSinceWire {
    root_id: RootId,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    generation: Option<Generation>,
    outcome: NewSinceOutcome,
    /// The run compared against; null exactly when `outcome` is `no_baseline`.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    baseline: Option<NewSinceRun>,
    /// The newest completed run of any mode (watcher and targeted runs included); null when none.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    latest: Option<NewSinceRun>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    new_files: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    new_dirs: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    logical_bytes: Option<Bytes>,
    /// Null when any new file's allocation is unknown (see `allocation_unknown_files`), never a partial sum.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    allocated_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    allocation_unknown_files: Option<Count>,
    /// Largest first by logical size, row order breaking ties; at most `limit`.
    largest_files: BoundedVec<NewSinceFile, 0, 50>,
    /// New folders whose parent is not new, largest subtree first; at most `limit`.
    largest_folders: BoundedVec<NewSinceFolder, 0, 50>,
    /// True when the figures are not a clean answer; `partial_reasons` say why.
    /// Also explains an unusable explicit baseline.
    partial: bool,
    partial_reasons: BoundedVec<NewSinceReason, 0, 8>,
    caveats: BoundedVec<NewSinceCaveat, 0, 8>,
}

impl TryFrom<ChangesNewSinceWire> for ChangesNewSince {
    type Error = &'static str;
    fn try_from(v: ChangesNewSinceWire) -> Result<Self, Self::Error> {
        let absent = v.outcome == NewSinceOutcome::NoBaseline;
        let figures = [
            v.new_files.is_some(),
            v.new_dirs.is_some(),
            v.logical_bytes.is_some(),
            v.allocation_unknown_files.is_some(),
        ];
        if v.baseline.is_none() != absent
            || (absent
                && (figures.iter().any(|present| *present)
                    || v.allocated_bytes.is_some()
                    || !v.largest_files.is_empty()
                    || !v.largest_folders.is_empty()))
            || (!absent && figures.iter().any(|present| !present))
        {
            return Err("new-since outcome disagrees with baseline, figures or lists");
        }
        Ok(Self {
            root_id: v.root_id,
            generation: v.generation,
            outcome: v.outcome,
            baseline: v.baseline,
            latest: v.latest,
            new_files: v.new_files,
            new_dirs: v.new_dirs,
            logical_bytes: v.logical_bytes,
            allocated_bytes: v.allocated_bytes,
            allocation_unknown_files: v.allocation_unknown_files,
            largest_files: v.largest_files,
            largest_folders: v.largest_folders,
            partial: v.partial,
            partial_reasons: v.partial_reasons,
            caveats: v.caveats,
        })
    }
}
