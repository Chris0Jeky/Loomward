//! Learning integration: human feedback, the review queue and refit jobs.
//!
//! Owner: lane L14 (LW-021, LW-026, LW-110), over `loomward-learn`. Wave 3.
//!
//! Scope (docs/41 section 9.1): a withdrawn label on retraction, unique `(object_ref, revision)`
//! per source, durable `client_event_id` replay and conflict, labels only on durable references,
//! the refit job. A learned score is never an approval; data classes (human, teacher) stay
//! separate (invariant 3).
//!
//! #117 errata items this lane applies, each with its test before merge:
//! - Partial #15 state-backed cursors: `learning.queue` cursors also bind a durable
//!   `state_rev`, since feedback changes `state.db` while `catalog_rev` stays put.
//! - N4 recovery (owned by L1 and L8, relied on here): `feedback.record` is safe to re-send only
//!   through its durable `client_event_id`; an identical replay returns the original result with
//!   `idempotent_replay: true`, a different payload under the same key is `invalid_request`.
#![allow(unused_variables)]

use crate::{Component, Engine, EngineError, EngineResult};
use loomward_protocol::{
    FeedbackRequest, FeedbackResult, Job, LearningQueue, LearningQueueRequest,
};

impl Engine {
    /// Records one human label or retraction (`feedback.record`). Never a grant.
    pub fn learning_feedback(&self, request: &FeedbackRequest) -> EngineResult<FeedbackResult> {
        Err(EngineError::unavailable(Component::Learning))
    }

    /// The suggestion queue for review (`learning.queue`).
    pub fn learning_queue(&self, request: &LearningQueueRequest) -> EngineResult<LearningQueue> {
        Err(EngineError::unavailable(Component::Learning))
    }

    /// Starts a refit as a job (`learning.refit`).
    pub fn learning_refit(&self) -> EngineResult<Job> {
        Err(EngineError::unavailable(Component::Learning))
    }
}
