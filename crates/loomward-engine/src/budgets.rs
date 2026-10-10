//! Caps on the engine's own worker pools.
//!
//! Owner: lane L7 (LW-006, LW-101); `budgets.rs` is in L7's file list in docs/43. L11 reads the
//! telemetry pool and L14 the learning pool.
//!
//! Scope: `scan_enumerate`, `learning` and `telemetry` pool sizes and the teacher child's
//! timeout and memory enforcement note (`OwnBudgets`). Caps Loomward's own in-process pools
//! only: no OS priority, affinity or memory call is ever made on anything else (invariant 1).
//!
//! #117 errata items for this module: none assigned.
#![allow(unused_variables)]

use crate::{Component, Engine, EngineError, EngineResult};
use loomward_protocol::{BudgetSetRequest, OwnBudgets};

impl Engine {
    /// Current caps and the teacher child's limits (`budgets.get`).
    pub fn budgets_get(&self) -> EngineResult<OwnBudgets> {
        Err(EngineError::unavailable(Component::Budgets))
    }

    /// Sets one own-pool worker cap and returns the new budgets (`budgets.set`).
    pub fn budgets_set(&self, request: &BudgetSetRequest) -> EngineResult<OwnBudgets> {
        Err(EngineError::unavailable(Component::Budgets))
    }
}
