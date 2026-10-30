//! `coverage.start` and `coverage.result` (#1106): is every file of granted folder A present in
//! granted folder B by content, whatever the paths. Observation-time evidence, never a receipt and
//! never permission to delete (docs/48 section 4.5).
use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageStartRequest {
    pub a_root_id: RootId,
    pub b_root_id: RootId,
    /// Default-stream bytes read across all passes of both folders (default 1 GiB).
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub budget_bytes: Option<Bytes>,
    /// Wall-clock seconds shared by all passes (default 60).
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub budget_secs: Option<Int<1, 3600>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageOutcome {
    Complete,
    Partial,
}

/// Why a partial result is partial. `refused` covers every other unreadable or unprovable case. A
/// cancelled job produces no result at all, so cancellation is a job state, not a stop here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStop {
    Budget,
    Refused,
    ChangedDuringRead,
}

/// Which folder an unknown verdict is about: `a` could not be read, or `b` could not vouch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageSide {
    A,
    B,
}

/// Files and bytes in one verdict class. `bytes` is a lower bound while `size_unknown` is above zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageTotal {
    pub files: Count,
    pub bytes: Bytes,
    pub size_unknown: Count,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageTotals {
    pub covered: CoverageTotal,
    pub not_covered: CoverageTotal,
    pub unknown: CoverageTotal,
}

/// An A file with no B file of equal size and content, B's listing having been complete.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageNotCovered {
    /// Display path below A (`.` for A itself); lossy for a non-Unicode name.
    pub path: BoundedStr<1, 32768>,
    /// The catalogue row of this path, null when the catalogue does not hold it.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub node_id: Option<NodeId>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub bytes: Option<Bytes>,
}

/// An A file (or folder listing) whose verdict could not be reached.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageUnknown {
    pub path: BoundedStr<1, 32768>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub node_id: Option<NodeId>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub bytes: Option<Bytes>,
    pub side: CoverageSide,
    pub reason: BoundedStr<1, 64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageReport {
    pub job_id: JobId,
    pub a_root_id: RootId,
    pub b_root_id: RootId,
    pub outcome: CoverageOutcome,
    /// Empty exactly when `outcome` is `complete`.
    pub reasons: UniqueVec<CoverageStop, 0, 3>,
    pub bytes_read: Bytes,
    pub totals: CoverageTotals,
    /// Largest first, unknown sizes last; at most 200.
    pub not_covered: BoundedVec<CoverageNotCovered, 0, 200>,
    pub not_covered_truncated: bool,
    pub unknown: BoundedVec<CoverageUnknown, 0, 200>,
    pub unknown_truncated: bool,
    pub observed_at: Timestamp,
    pub note: BoundedStr<1, 1024>,
}
