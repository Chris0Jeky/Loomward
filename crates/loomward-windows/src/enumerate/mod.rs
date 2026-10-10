//! Directory enumeration strategies and handle-relative child opens.
//!
//! Owner: lane L7 (LW-006, LW-101). The engine side of the seam is
//! `loomward-engine::scan::source::DirSource` (docs/41 section 4.1); this module supplies its
//! Windows implementation: the strategy chain (extended IDs, then `FileIdBothDirectoryInfo`
//! with the ID width kept, then `FindFirstFileExW`), `NtCreateFile` child opens relative to the
//! parent handle, and post-open validation before any listing.
//!
//! #117 errata items for this module: N3 (ReFS needs 128-bit identity) and partial #2 (the
//! post-open ID policy and a 64-bit-compatible query); both are applied together with
//! `loomward-engine::scan`.
//!
//! Read-only: no write, delete, rename or content read. Stub for C2: no API yet; it compiles on
//! every platform and the Win32 code arrives behind `cfg(windows)` with `Unsupported` stubs.
