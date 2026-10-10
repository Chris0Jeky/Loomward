//! Telemetry, processes and budgets. Mirrors `contracts/v3/view-service.schema.json`; the contract tests keep them equal.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryChannel {
    System,
    Processes,
    Gpu,
    Disks,
    Engine,
}

/// Sampling interval in milliseconds: exactly 1, 2, 5 or 10 seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TelemetryInterval {
    OneSecond,
    TwoSeconds,
    FiveSeconds,
    TenSeconds,
}

impl TelemetryInterval {
    pub const fn millis(self) -> u32 {
        match self {
            Self::OneSecond => 1000,
            Self::TwoSeconds => 2000,
            Self::FiveSeconds => 5000,
            Self::TenSeconds => 10000,
        }
    }
}

impl Serialize for TelemetryInterval {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(self.millis())
    }
}

impl<'de> Deserialize<'de> for TelemetryInterval {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match u32::deserialize(d)? {
            1000 => Ok(Self::OneSecond),
            2000 => Ok(Self::TwoSeconds),
            5000 => Ok(Self::FiveSeconds),
            10000 => Ok(Self::TenSeconds),
            _ => Err(serde::de::Error::custom(
                "interval_ms must be 1000, 2000, 5000 or 10000",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetrySubscribeRequest {
    /// null creates; an existing id renews and may change channels/interval.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub subscription_id: Option<SubscriptionId>,
    pub channels: UniqueVec<TelemetryChannel, 1, 5>,
    pub interval_ms: TelemetryInterval,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetrySubscription {
    pub subscription_id: SubscriptionId,
    pub channels: UniqueVec<TelemetryChannel, 1, 5>,
    pub interval_ms: TelemetryInterval,
    /// 60 s lease; the sampler stops when no live subscription remains.
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionRefRequest {
    pub subscription_id: SubscriptionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryUnsubscribed {
    pub subscription_id: SubscriptionId,
    pub stopped: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetrySnapshotRequest {
    pub channels: UniqueVec<TelemetryChannel, 1, 5>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemSampleMemory {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub total_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub available_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub commit_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub commit_limit_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub load_fraction: Option<Fraction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemSampleCpu {
    pub logical_cpus: Count,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub busy_fraction: Option<Fraction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemSample {
    pub memory: SystemSampleMemory,
    pub cpu: SystemSampleCpu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuSampleState {
    Observed,
    Unavailable,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuSampleBasis {
    PdhGpuCounters,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpuSampleAdapter {
    pub adapter_id: Text<64>,
    pub name: Text<128>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub dedicated_total_bytes: Option<Bytes>,
    /// Global, from the GPU Adapter Memory counter; never a sum of per-process values.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub dedicated_used_bytes: Option<Bytes>,
    /// Global, from the GPU Adapter Memory counter.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub shared_used_bytes: Option<Bytes>,
    /// Maximum over engine types of the per-type utilisation summed across that adapter's engine instances, clamped to 1 (the Task Manager method).
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub engine_busy_fraction: Option<Fraction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpuSample {
    pub state: GpuSampleState,
    pub basis: GpuSampleBasis,
    pub adapters: BoundedVec<GpuSampleAdapter, 0, 8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskSampleState {
    Observed,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskSampleDisk {
    pub disk_label: Text<64>,
    pub volume_ids: BoundedVec<VolumeId, 0, 16>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub read_bytes_per_s: Option<Rate>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub write_bytes_per_s: Option<Rate>,
    /// 1 - (PhysicalDisk % Idle Time / 100), clamped to [0, 1].
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub busy_fraction: Option<Fraction>,
    /// PhysicalDisk Avg. Disk Queue Length; a separate quantity from busy_fraction.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub queue_length: Option<Rate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskSample {
    pub state: DiskSampleState,
    pub disks: BoundedVec<DiskSampleDisk, 0, 16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolName {
    ScanEnumerate,
    CatalogWrite,
    Learning,
    Teacher,
    Telemetry,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolUse {
    pub pool: PoolName,
    pub max_workers: Int<0, 64>,
    pub busy_workers: Int<0, 64>,
    pub queue_depth: Count,
    pub queue_capacity: Count,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineSample {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub private_commit_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub working_set_bytes: Option<Bytes>,
    /// Machine-normalised: CPU time over the interval divided by (interval x logical CPUs).
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub cpu_fraction: Option<Fraction>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub threads: Option<Count>,
    pub pools: BoundedVec<PoolUse, 0, 8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessRowAccess {
    Full,
    Limited,
    Denied,
}

/// No command line, no environment, no window titles (privacy).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessRow {
    pub process_ref: ProcessRef,
    pub pid: Int<0, 4294967295>,
    pub name: Text<260>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub started_at: Option<Timestamp>,
    /// PROCESS_MEMORY_COUNTERS_EX2.PrivateUsage (commit charge), not resident memory.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub private_commit_bytes: Option<Bytes>,
    /// PROCESS_MEMORY_COUNTERS_EX2.PrivateWorkingSetSize where supported, else null.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub private_working_set_bytes: Option<Bytes>,
    /// Total working set, including shared pages.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub working_set_bytes: Option<Bytes>,
    /// Machine-normalised over the sample interval; null on the first sample.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub cpu_fraction: Option<Fraction>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub io_read_bytes_per_s: Option<Rate>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub io_write_bytes_per_s: Option<Rate>,
    /// GPU Process Memory dedicated usage for this process; may include shared allocations, so rows are never summed into an adapter total.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub gpu_dedicated_bytes: Option<Bytes>,
    pub access: ProcessRowAccess,
    pub loomward_owned: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessSummary {
    pub observed_count: Count,
    pub denied_count: Count,
    pub top: BoundedVec<ProcessRow, 0, 20>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetrySample {
    pub sample_seq: Count,
    pub observed_at: Timestamp,
    /// Monotonic time since the previous sample; null on the first sample.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub elapsed_ms: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub system: Option<SystemSample>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub gpu: Option<GpuSample>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub disks: Option<DiskSample>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub engine: Option<EngineSample>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub processes: Option<ProcessSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessListRequestSort {
    PrivateDesc,
    WorkingSetDesc,
    CpuDesc,
    IoDesc,
    GpuDesc,
    NameAsc,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessListRequest {
    pub sort: ProcessListRequestSort,
    pub limit: Int<1, 200>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessList {
    pub sample_seq: Count,
    pub observed_at: Timestamp,
    pub rows: BoundedVec<ProcessRow, 0, 200>,
    pub observed_count: Count,
    pub denied_count: Count,
    pub truncated: bool,
    pub note: Text<512>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessRefRequest {
    pub process_ref: ProcessRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessExplanationFact {
    pub code: FactCode,
    pub text: Text<256>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessExplanation {
    pub process_ref: ProcessRef,
    pub observed_at: Timestamp,
    pub facts: BoundedVec<ProcessExplanationFact, 0, 24>,
    pub summary: Text<1024>,
    pub caveats: BoundedVec<Text<256>, 0, 8>,
    /// Always empty in v0.3 (invariant 1).
    pub available_actions: BoundedVec<Never, 0, 0>,
}

const_str!(
    pub PoolBudgetScope = "loomward_own_threads"
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolBudget {
    pub pool: PoolName,
    pub max_workers: Int<1, 64>,
    pub default_workers: Int<1, 64>,
    pub current_workers: Int<0, 64>,
    pub queue_capacity: Count,
    pub scope: PoolBudgetScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnBudgetsTeacherEnforcement {
    JobObject,
    NotAvailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnBudgetsTeacher {
    pub max_in_flight: ConstOne,
    pub timeout_s: Int<10, 900>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub memory_limit_bytes: Option<Bytes>,
    pub enforcement: OwnBudgetsTeacherEnforcement,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnBudgets {
    pub pools: BoundedVec<PoolBudget, 0, 8>,
    pub teacher: OwnBudgetsTeacher,
    pub enforcement_note: Text<512>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetSetRequestPool {
    ScanEnumerate,
    Learning,
    Telemetry,
}

/// Caps Loomward's own in-process pools only. No OS priority, affinity or memory call is made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetSetRequest {
    pub pool: BudgetSetRequestPool,
    pub max_workers: Int<1, 32>,
}
