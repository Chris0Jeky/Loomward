//! Placement: tier model, ancestry-antichain candidates and bounded simulation (docs/41 section 10).
//!
//! Owner: lane L12 (LW-108, LW-030 wiring). The budgeted link-count pass is
//! `loomward-windows::linkcount`; the planner is `loomward-core` (v1, and the v2 port of #107).
//!
//! Scope: three byte quantities (entry, allocated, relief), `relief_basis` and `estimate_basis`,
//! cluster-rounded destination estimate, `pre_rejected`, `excluded_volumes`; unknown flags and
//! heat are omitted from the planner scenario, never `null`. Simulation only: nothing moves.
//!
//! #117 errata items this lane applies, each with its test before merge:
//! - Partial #4 relief mapping: planner `source_bytes` = `estimated_relief_bytes` under the
//!   verified policy. Verified relief requires known allocation and identity-matched link
//!   observations; passing entry bytes directly reproduces the "allocations exceed used
//!   capacity" rejection.
//! - N5 slow placement: `placement.simulate` stays synchronous and bounded by `node_budget`,
//!   returning `deadline_exceeded` (or `resource_budget`). There is no job variant in v0.3.
#![allow(unused_variables)]

use crate::{Component, Engine, EngineError, EngineResult};
use loomward_protocol::{
    PlacementCandidates, PlacementCandidatesRequest, PlacementPlan, PlacementSimulateRequest,
    ProposalDetail, ProposalList, ProposalRefRequest, TierModel,
};

impl Engine {
    /// The tier model: volumes, declared tiers and pressure (`tiers.model`).
    pub fn placement_model(&self) -> EngineResult<TierModel> {
        Err(EngineError::unavailable(Component::Placement))
    }

    /// Candidate groups forming an ancestry antichain (`placement.candidates`).
    pub fn placement_candidates(
        &self,
        request: &PlacementCandidatesRequest,
    ) -> EngineResult<PlacementCandidates> {
        Err(EngineError::unavailable(Component::Placement))
    }

    /// Synchronous, bounded simulation; `DeadlineExceeded` or `ResourceBudget` on overrun (N5).
    /// With `save` the plan is committed and listed, or not saved at all.
    pub fn placement_simulate(
        &self,
        request: &PlacementSimulateRequest,
    ) -> EngineResult<PlacementPlan> {
        Err(EngineError::unavailable(Component::Placement))
    }

    /// Saved proposals (`proposals.list`).
    pub fn proposals_list(&self) -> EngineResult<ProposalList> {
        Err(EngineError::unavailable(Component::Placement))
    }

    /// One saved proposal (`proposals.get`).
    pub fn proposals_get(&self, request: &ProposalRefRequest) -> EngineResult<ProposalDetail> {
        Err(EngineError::unavailable(Component::Placement))
    }
}
