//! Budgeted link-count pass for placement relief.
//!
//! Owner: lane L12 (LW-108, LW-030 wiring); consumed by `loomward-engine::placement`.
//!
//! Scope: metadata-only opens (no content read), placeholders skipped, 200k files per group;
//! reports link counts keyed by identity so hard-linked objects are counted once.
//!
//! #117 errata items for this module: partial #4 (verified relief requires known allocation and
//! identity-matched link observations). Stub for C2: no API yet.
