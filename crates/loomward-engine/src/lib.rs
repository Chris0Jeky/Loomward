//! Loomward v0.3 engine (docs/41 section 4): jobs, scan pipeline, event bus, telemetry
//! scheduler, placement scenario builder, learning jobs, teacher runner and own-pool budgets.
//!
//! This is the C2 skeleton (docs/43). Every entry point exists with its final shape and answers
//! [`EngineError::Unavailable`] until the lane that owns its module fills the body. The
//! `smoke` test in `tests/smoke.rs` lists every entry point, so a lane that lands one must flip
//! its assertion.
//!
//! The engine exposes no generic command and accepts no path from a caller: the service resolves
//! a granted root from `state.db` and passes a [`scan::GrantedRoot`]. Nothing here performs a
//! file or process effect (AGENTS.md invariant 1).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod budgets;
pub mod error;
pub mod events;
pub mod jobs;
pub mod learning;
pub mod placement;
pub mod scan;
pub mod teacher;
pub mod telemetry;

pub use error::{Component, EngineError, EngineResult};

use loomward_protocol::DatasetClass;
use std::path::PathBuf;

/// How an [`Engine`] is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    /// Directory holding `state.db` and the per-class catalogue file. Loomward's own files only.
    pub state_dir: PathBuf,
    /// The one dataset class this engine serves (ADR-V3-15); a session never mixes classes.
    pub dataset_class: DatasetClass,
}

/// The engine handle. Cheap to share behind an `Arc`; lanes add their state as private fields.
#[derive(Debug)]
pub struct Engine {
    config: EngineConfig,
}

impl Engine {
    /// Opens the engine for `config`. The skeleton opens nothing and touches no file; the lanes
    /// that own the stores open them here (L7: catalogue and state files, restart recovery).
    pub fn open(config: EngineConfig) -> EngineResult<Engine> {
        Ok(Engine { config })
    }

    /// The configuration the engine was opened with.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }
}
