//! Loomward v0.3 engine (docs/41 section 4): jobs, scan pipeline, event bus, telemetry
//! scheduler, placement scenario builder, learning jobs, teacher runner and own-pool budgets.
//!
//! L7 implements read-only scans through an installed catalogue sink, kind-dispatched jobs,
//! own-pool budgets and event replay; L11b adds the read-only telemetry scheduler. Other lane
//! entry points remain explicitly unavailable. No catalogue durability is implied by the
//! aggregate-only in-memory fixture sink.
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
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

/// How an [`Engine`] is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EngineConfig {
    /// Directory holding `state.db` and the per-class catalogue file. Loomward's own files only.
    pub state_dir: PathBuf,
    /// The one dataset class this engine serves (ADR-V3-15); a session never mixes classes.
    pub dataset_class: DatasetClass,
}

impl EngineConfig {
    /// Configure one dataset-class engine without opening or mutating any user path.
    pub fn new(state_dir: PathBuf, dataset_class: DatasetClass) -> Self {
        Self {
            state_dir,
            dataset_class,
        }
    }
}

/// The engine handle. Cheap to share behind an `Arc`; lanes add their state as private fields.
pub struct Engine {
    config: EngineConfig,
    budgets: budgets::Budgets,
    events: Arc<events::Bus>,
    jobs: Arc<jobs::Registry>,
    scan_sink: Mutex<Option<Arc<dyn scan::ScanSink>>>,
    #[cfg(windows)]
    scan_watches:
        Mutex<std::collections::HashMap<loomward_protocol::RootId, Arc<scan::watch::RootWatch>>>,
    telemetry: telemetry::Telemetry,
}

impl Engine {
    /// Creates process-local managers. L2 installs its recovered catalogue sink before scans
    /// can start; this constructor itself opens no user or state file.
    pub fn open(config: EngineConfig) -> EngineResult<Engine> {
        let events = Arc::new(events::Bus::new(
            config.dataset_class,
            Duration::from_secs(15),
        ));
        Ok(Engine {
            config,
            budgets: Default::default(),
            events,
            jobs: Default::default(),
            scan_sink: Mutex::new(None),
            #[cfg(windows)]
            scan_watches: Mutex::new(Default::default()),
            telemetry: telemetry::Telemetry::default(),
        })
    }

    /// The configuration the engine was opened with.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }
}
