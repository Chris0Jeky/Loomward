//! `loomward-service`: the typed view service both adapters call (docs/41 section 5, LW-103).
//! This first commit holds its policy primitives; the service itself follows.

#![allow(dead_code)]

mod cursors;
mod idem;
mod ids;
pub mod paths;
