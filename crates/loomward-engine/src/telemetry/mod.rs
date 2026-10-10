//! Telemetry scheduler: leases, the sampler and snapshots over `loomward-telemetry`.
//!
//! Owner: lane L11 (LW-050, LW-107). Field definitions are docs/41 section 11; the sampler
//! itself lives in the `loomward-telemetry` crate (this module owns scheduling and leases).
//!
//! Scope: 60 s subscription leases (`telemetry.subscribe` renews), the sampler stops when no live
//! lease remains, read-only memory/process/disk/GPU samples, `telemetry.sample` events at the
//! leased interval, and the `processes.list` / `processes.explain` facts. No process is ever
//! mutated; per-process GPU memory is never summed into an adapter total.
//!
//! #117 errata items for this module: none assigned (finding 21, telemetry definitions, is
//! closed; apply the doc 41 section 11 definitions and the schema renames).
#![allow(unused_variables)]

use crate::{Component, Engine, EngineError, EngineResult};
use loomward_protocol::{
    ProcessExplanation, ProcessList, ProcessListRequest, ProcessRefRequest, SubscriptionRefRequest,
    TelemetrySample, TelemetrySnapshotRequest, TelemetrySubscribeRequest, TelemetrySubscription,
    TelemetryUnsubscribed,
};

impl Engine {
    /// Creates or renews a 60 s lease and starts the sampler if it is idle
    /// (`telemetry.subscribe`).
    pub fn telemetry_lease(
        &self,
        request: &TelemetrySubscribeRequest,
    ) -> EngineResult<TelemetrySubscription> {
        Err(EngineError::unavailable(Component::Telemetry))
    }

    /// Releases a lease early (`telemetry.unsubscribe`).
    pub fn telemetry_release(
        &self,
        request: &SubscriptionRefRequest,
    ) -> EngineResult<TelemetryUnsubscribed> {
        Err(EngineError::unavailable(Component::Telemetry))
    }

    /// One on-demand sample of the requested channels (`telemetry.snapshot`).
    pub fn telemetry_snapshot(
        &self,
        request: &TelemetrySnapshotRequest,
    ) -> EngineResult<TelemetrySample> {
        Err(EngineError::unavailable(Component::Telemetry))
    }

    /// The process table, read-only (`processes.list`).
    pub fn processes_list(&self, request: &ProcessListRequest) -> EngineResult<ProcessList> {
        Err(EngineError::unavailable(Component::Telemetry))
    }

    /// Facts explaining one process, never an instruction to act on it (`processes.explain`).
    pub fn processes_explain(
        &self,
        request: &ProcessRefRequest,
    ) -> EngineResult<ProcessExplanation> {
        Err(EngineError::unavailable(Component::Telemetry))
    }
}
