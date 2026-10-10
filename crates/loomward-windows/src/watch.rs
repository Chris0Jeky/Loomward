//! Change watching for granted roots.
//!
//! Owner: lane L16 (LW-008, with the LW-007 USN spike). Wave 3; it follows L7. The engine side
//! is `loomward-engine::scan::watch`.
//!
//! Scope: the watch is established before the scan; both overflow forms
//! (`ERROR_NOTIFY_ENUM_DIR` and a successful zero-byte completion) dirty the root.
//!
//! #117 errata items for this module: partial #8 (reparent invalidation) on the engine side;
//! renames reconcile both parents or leave a tombstone (N2 keeps unresolved tombstones).
//! Stub for C2: no API yet.
