//! LW-121: low-space and fast-growth alerts. Mirrors `contracts/v3/view-service.schema.json`; the
//! contract tests keep them equal. Alerts are advice computed from existing read-only facts, never
//! an action: unknown capacity, missing history and partial scans are statuses, never zero.

use super::*;

/// The owner's alert thresholds. Every field is required: `alerts.configure` replaces the whole
/// record. Documented defaults are in `contracts/v3/semantics.md` and in `AlertReport.defaults`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertSettings {
    /// Off until the owner enables it. Gates local OS notifications only; the in-app list is
    /// always available. No notification channel is wired yet (see `AlertReport.notifications_note`).
    pub notifications_enabled: bool,
    /// Low free space when free < max(capacity x this percent, `low_space_free_bytes`).
    pub low_space_free_percent: Int<1, 50>,
    pub low_space_free_bytes: Bytes,
    /// Fast growth: net logical growth of a granted root between the oldest and newest complete
    /// scans inside this many days reaches `growth_bytes`.
    pub growth_window_days: Int<1, 90>,
    pub growth_bytes: Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    LowSpace,
    FastGrowth,
}

/// `alert` is the only status that counts as an active alert. The others say why none was raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertStatus {
    Alert,
    Ok,
    /// Capacity, free space or the volume itself could not be observed: suppressed, not zero.
    Unknown,
    /// Fewer than two complete scans of the root inside the window.
    InsufficientHistory,
    /// The scans in the window do not cover the whole root, so their sizes are not comparable.
    IncompleteCoverage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertItem {
    pub kind: AlertKind,
    pub status: AlertStatus,
    pub label: Text<64>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub volume_id: Option<VolumeId>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub root_id: Option<RootId>,
    /// Free bytes (low_space) or net growth (fast_growth). null is unknown, never zero.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub measured_bytes: Option<Bytes>,
    /// The effective threshold the measurement was compared with. null when none applied.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub threshold_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub capacity_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub window_start: Option<Timestamp>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub window_end: Option<Timestamp>,
    /// What the measurement covers: `complete`, `partial` or `unknown`.
    pub coverage: CoverageState,
    pub note: Text<256>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertReport {
    pub evaluated_at: Timestamp,
    pub settings: AlertSettings,
    pub defaults: AlertSettings,
    /// Items whose status is `alert`.
    pub active_count: Count,
    pub items: BoundedVec<AlertItem, 0, 128>,
    pub notifications_note: Text<256>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertHistoryRequest {
    pub root_id: RootId,
}

/// One completed scan's recorded counters, not a fresh observation of the folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertSample {
    pub finished_at: Timestamp,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub logical_bytes: Option<Bytes>,
    pub coverage: CoverageState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertHistory {
    pub evaluated_at: Timestamp,
    pub growth_window_days: Int<1, 90>,
    pub check: AlertItem,
    /// Newest 64 completed records, displayed oldest first; unknown counters remain visible.
    pub samples: BoundedVec<AlertSample, 0, 64>,
    pub truncated: bool,
}
