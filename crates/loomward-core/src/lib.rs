//! Portable native core: scanner, policy, transaction simulation and the v1/v2 planners. Compiled and
//! tested on Windows and Linux (CI) since 2026-10-09.
//! No module in this crate moves/deletes files, spawns commands, or controls processes.
#![forbid(unsafe_code)]
pub mod inventory;
pub mod planner;
pub mod planner_v2;
pub mod policy;
pub mod transaction;
