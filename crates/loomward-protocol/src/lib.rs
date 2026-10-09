//! Loomward v0.3 view-service wire contract.
//!
//! The JSON Schema in `contracts/v3/` is the source of truth; this crate is its Rust
//! implementation: envelopes, the closed command and event lists, the service trait the
//! adapters call, and a DTO for every `$def`. Deserialising validates, so a handler only
//! ever sees in-bounds values. A schema match is never permission: the service
//! re-validates scope, grants and bounds.

pub mod command;
pub mod envelope;
pub mod error;
pub mod event;
pub mod service;
pub mod types;

pub use command::Command;
pub use envelope::{
    Payload, RequestEnvelope, ResponseEnvelope, ResponseError, ResponseMeta, ResponseOk,
};
pub use error::{ErrorBody, ErrorCode};
pub use event::{EventEnvelope, EventName};
pub use service::{CallContext, DisclosureSummary, EventSubscription, NativeDialogs, ViewService};
pub use types::*;

/// Protocol bounds of docs/41 section 5.1.
pub mod limits {
    pub const MAX_REQUEST_BYTES: usize = 65_536;
    pub const MAX_SLICE_NODES: usize = 6_000;
    pub const DEFAULT_SLICE_NODES: usize = 2_500;
    pub const MAX_PAGE_ITEMS: usize = 200;
    pub const MAX_SEARCH_ITEMS: usize = 100;
    pub const MAX_TEACHER_BATCH: usize = 25;
    pub const EVENT_QUEUE_CAPACITY: usize = super::service::EVENT_QUEUE_CAPACITY;
}
