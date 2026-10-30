//! Copy-and-verify operation journal (doc 47 lane C3, LW-039, ADR-V3-25) and the pure read-only tree
//! comparison of lane C1 (`tree`, no operation or effect authority), and lane C4's manifest, preflight
//! refusal classifier, digest and single-use approval (`manifest`, LW-038).
//!
//! The state machine of doc 47 section 5, the write-ahead journal records, their `state.db` storage and the
//! recovery reader. This crate performs NO file effect: it never opens, creates, writes, renames or deletes a
//! file. A recovery decision is a proposal the executor (lane C5) must re-check against native identities;
//! it is never authority on its own, and it only ever names identities and staging-relative names that the
//! journal recorded.
//!
//! The per-operation hash chain detects edited, duplicated, reordered or missing records. Store also
//! checks a separately stored tail anchor, committed atomically with each append, to detect tail loss.
//! Pure row-only replay cannot detect tail truncation; durable recovery must go through Store.
#![forbid(unsafe_code)]

pub mod containment;
pub mod custody;
pub mod journal;
pub mod manifest;
pub mod receipt;
pub mod recovery;
pub mod state;
pub mod store;
pub mod tree;

pub use custody::{manifest_root, CustodyEntry, EntryKind};
pub use journal::{
    Entry, FenceCause, Invalid, ObjectId, Record, Refusal, Replay, Row, StagedItem,
    StagingDisposition, StopCause, StopKind, MAX_BATCH, MAX_ITEMS,
};
pub use manifest::{
    approve, preview, Admitted, Approval, Code, Context, Destination, DestinationFacts, Identity,
    Manifest, ManifestItem, Preview, Refused, SourceEntry,
};
pub use receipt::{CopyReceipt, ReceiptLabel, ReceiptState};
pub use recovery::{recover, Attention, Cleanup, Decision, Disk, Observed, Recovery, Resume};
pub use state::{IllegalTransition, ItemState, OpState};
pub use store::{AppendError, Store, SCHEMA_SQL};
