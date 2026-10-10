//! One Rust type for every `$def` in `contracts/v3/view-service.schema.json`.
//!
//! Structs are `deny_unknown_fields`; byte quantities are [`Bytes`]; every bounded array,
//! string and number carries its bound in its type, so deserialising validates.

use crate::error::ErrorBody;
use crate::types::*;
use serde::{Deserialize, Serialize};

mod common;
mod events;
mod jobs;
mod learning;
mod placement;
mod roots;
mod session;
mod teacher;
mod telemetry;
mod tree;
mod volumes;

pub use common::*;
pub use events::*;
pub use jobs::*;
pub use learning::*;
pub use placement::*;
pub use roots::*;
pub use session::*;
pub use teacher::*;
pub use telemetry::*;
pub use tree::*;
pub use volumes::*;
