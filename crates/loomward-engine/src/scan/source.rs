//! Width-preserving source seam, shared with the native enumerator.
pub use loomward_windows::enumerate::{
    refused_attributes, DirSource, FileIdObs, Flow, IdBasis, IncompleteReason, ListOutcome,
    OpenedIdentity, RawEntry, SourceError, Strategy,
};

#[cfg(windows)]
pub(crate) fn native_cancel() -> loomward_windows::enumerate::IoCancellation {
    Default::default()
}
