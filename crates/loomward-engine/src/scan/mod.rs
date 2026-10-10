//! Scan pipeline: enumerate a granted root, stage, commit and roll up (docs/41 sections 7-8).
//!
//! Owner: lane L7 (LW-006, LW-101). `scan/watch.rs` (one `mod watch;` line here) passes to
//! L16 (LW-008, LW-007 spike) in wave 3. The enumeration seam is [`source`]; the Win32 side is
//! `loomward-windows::enumerate`.
//!
//! #117 errata items this lane applies, each with its test before merge (the lane PR links #117):
//! - N1 same-run obsolete finals: every `DirFinal` carries its input revision; accept it only
//!   while that equals `dirty_rev`, and set `agg_valid_rev` to the input revision.
//! - N2 targeted runs sweep unrelated tombstones: only a complete root traversal sweeps globally;
//!   a targeted run keeps unresolved tombstones unless its recorded scope proves absence.
//! - N3 ReFS 64-bit IDs treated as unique: ReFS needs 128-bit identity for uniqueness,
//!   reparenting and durable references; a 64-bit fallback there is downgraded.
//! - Partial #2 fallback identity width: after the open, query a 64-bit-compatible ID when the
//!   listing gave 64 bits, and define the policy when the listing had an ID but the post-open
//!   query has none (see [`source::IdBasis`]).
//! - Partial #8 reparent invalidation: reparenting invalidates both the old and the new
//!   ancestor chains.
//! - N6 M-tier fixture: M stays at 1M entries; the 2.5M-entry oversized directory is a separate
//!   stress fixture with a partial oracle.
//!
//! L2 (catalogue) owns the SQL side of N1, N2, N3 and #8; this module owns the pipeline side.
//! Revocation (L8) must reach [`Engine::scan_cancel`] before the writer is fenced.
#![allow(unused_variables)]

pub mod source;

use crate::{Component, Engine, EngineError, EngineResult};
use loomward_protocol::{DatasetClass, Job, JobId, RootId, ScanBudget};
use std::path::PathBuf;

/// A root the owner granted, resolved by the service from `state.db`. This is the only way a
/// path enters the engine: callers of the view service never supply one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GrantedRoot {
    /// Opaque root identifier from the catalogue.
    pub root_id: RootId,
    /// The granted directory (a disposable root for synthetic sessions).
    pub path: PathBuf,
    /// Class of the session that holds the grant.
    pub dataset_class: DatasetClass,
}

impl GrantedRoot {
    /// A granted root. Only the service calls this, from a live grant row.
    pub fn new(root_id: RootId, path: PathBuf, dataset_class: DatasetClass) -> Self {
        Self {
            root_id,
            path,
            dataset_class,
        }
    }
}

impl Engine {
    /// Starts a full scan of `root` as a job (`scan.start`, mode `full`). Non-elevated and
    /// read-only; a real-disk root needs an explicit grant.
    pub fn scan_start(&self, root: &GrantedRoot, budget: Option<ScanBudget>) -> EngineResult<Job> {
        Err(EngineError::unavailable(Component::Scan))
    }

    /// Starts a refresh of `root` (`scan.start`, mode `refresh`): relists dirty directories and
    /// keeps unresolved tombstones unless the run is a complete root traversal (N2).
    pub fn scan_refresh(
        &self,
        root: &GrantedRoot,
        budget: Option<ScanBudget>,
    ) -> EngineResult<Job> {
        Err(EngineError::unavailable(Component::Scan))
    }

    /// Cancels a scan job (`scan.cancel`). Acknowledges as `cancel_requested`; handles close
    /// only after the blocked call returns.
    pub fn scan_cancel(&self, job_id: &JobId) -> EngineResult<Job> {
        Err(EngineError::unavailable(Component::Scan))
    }
}
