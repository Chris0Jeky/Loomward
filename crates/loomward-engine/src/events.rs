//! Event bus and bounded subscriptions.
//!
//! Owner: lane L7 (LW-006, LW-101); `events.rs` is in L7's file list in docs/43.
//!
//! Scope: epochs, the replay buffer, `stream.lagged`, the 15 s `stream.hello` heartbeat and the
//! per-subscriber queue of [`loomward_protocol::service::EVENT_QUEUE_CAPACITY`] (1,024) per
//! docs/41 sections 5.3 and 7 and `semantics.md` section 7. A slow reader gets `stream.lagged`,
//! never an unbounded backlog, and a publisher is never blocked by a subscriber.
//!
//! #117 errata items for this module: none assigned. Events are emitted after commit, carrying
//! the `catalog_rev` and `state_rev` of that commit.
#![allow(unused_variables)]

use crate::{Component, Engine, EngineError, EngineResult};
use loomward_protocol::EventStream;

/// Where a reconnecting client left off: the epoch it last saw and the last `seq` it applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventResume {
    /// Stream epoch (random per service start); `seq` only compares within one epoch.
    pub epoch: String,
    /// Last applied sequence number in that epoch.
    pub last_seq: u64,
}

impl Engine {
    /// Opens a bounded subscription. The first item is `stream.hello`; replay or `stream.lagged`
    /// follows per `semantics.md` section 7. The service wraps this in `ViewService::subscribe`.
    pub fn subscribe_events(
        &self,
        resume: Option<EventResume>,
    ) -> EngineResult<Box<dyn EventStream>> {
        Err(EngineError::unavailable(Component::Events))
    }
}
