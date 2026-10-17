//! Copy-and-verify operation journal (doc 47 lane C3, LW-039, ADR-V3-25).
//!
//! The state machine of doc 47 section 5, the write-ahead journal records, their `state.db` storage and the
//! recovery reader. This crate performs NO file effect: it never opens, creates, writes, renames or deletes a
//! file. A recovery decision is a proposal the executor (lane C5) must re-check against native identities;
//! it is never authority on its own, and it only ever names identities and staging-relative names that the
//! journal recorded.
//!
//! The per-operation hash chain detects damage in the middle of a journal (an edited, truncated, duplicated,
//! reordered or missing record). It does not detect tail truncation: losing the last records looks exactly
//! like a crash before they were written, and recovery treats it so.
#![forbid(unsafe_code)]

pub mod journal;
pub mod recovery;
pub mod state;
pub mod store;

pub use journal::{
    Entry, FenceCause, Invalid, ObjectId, Record, Refusal, Replay, Row, StagedItem, MAX_BATCH,
    MAX_ITEMS,
};
pub use recovery::{recover, Attention, Cleanup, Decision, Disk, Observed, Recovery, Resume};
pub use state::{IllegalTransition, ItemState, OpState};
pub use store::{AppendError, Store, SCHEMA_SQL};
