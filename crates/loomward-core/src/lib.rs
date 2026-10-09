//! Native source foundation. Compilation and Windows validation are outstanding.
//! No module in this crate moves/deletes files, spawns commands, or controls processes.
#![forbid(unsafe_code)]
pub mod inventory;
pub mod planner;
pub mod policy;
pub mod transaction;
