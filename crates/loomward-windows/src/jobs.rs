//! Owned-child Job Objects for the teacher process.
//!
//! Owner: lane L15 (LW-052, owned teacher child only; LW-023, LW-109).
//!
//! Scope: launch a pinned native executable with `CreateProcessW` and `STARTUPINFOEX` (job list
//! with a non-inherited job handle, stdio-only handle list, no breakaway, fail closed). It never
//! acts on a process Loomward did not spawn (invariant 1).
//!
//! #117 errata items for this module: none assigned. Stub for C2: no API yet.
