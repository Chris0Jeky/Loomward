//! Job manager: submit, cancel, status and list for scans, refits and teacher runs.
//!
//! Owner: lane L7 (LW-006, LW-101). Later lanes register their runners here: L14 (refit) and
//! L15 (teacher).
//!
//! Scope: queued, running, cancel-requested and terminal states; cancel acknowledged at once as
//! `cancel_requested` with the 250 ms ack target; restart recovery of interrupted jobs.
//!
//! #117 errata items for this module: none assigned directly. N4 (recovery never blindly
//! resends a mutation) is owned by L1 and L8, but it relies on this module: keep terminal job
//! records at least as long as the service's 10-minute idempotency window (`semantics.md`
//! section 3), and never retry an `interrupted` teacher job.
#![allow(unused_variables)]

use crate::{Component, Engine, EngineError, EngineResult};
use loomward_protocol::{Job, JobId, JobKind, JobList, JobListRequest, RootId};

/// What to run. The engine never accepts a path or a command here: the kind selects a runner
/// the engine owns, and `root_id` names a root the service already resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct JobSpec {
    /// Which runner.
    pub kind: JobKind,
    /// The root the job works on; `None` for a refit.
    pub root_id: Option<RootId>,
}

impl JobSpec {
    /// A spec for `kind` on `root_id`.
    pub fn new(kind: JobKind, root_id: Option<RootId>) -> Self {
        Self { kind, root_id }
    }
}

impl Engine {
    /// Queues a job and returns it in its first state.
    pub fn job_submit(&self, spec: JobSpec) -> EngineResult<Job> {
        Err(EngineError::unavailable(Component::Jobs))
    }

    /// Requests cancellation and returns the job as `cancel_requested` (or already terminal).
    pub fn job_cancel(&self, job_id: &JobId) -> EngineResult<Job> {
        Err(EngineError::unavailable(Component::Jobs))
    }

    /// The current state of one job (`jobs.get`).
    pub fn job_status(&self, job_id: &JobId) -> EngineResult<Job> {
        Err(EngineError::unavailable(Component::Jobs))
    }

    /// Recent jobs, newest first, optionally filtered by kind (`jobs.list`).
    pub fn job_list(&self, request: &JobListRequest) -> EngineResult<JobList> {
        Err(EngineError::unavailable(Component::Jobs))
    }
}
