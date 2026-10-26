//! Read-only observations by default, with opt-in lab effects. Identity is never an operation grant.
#[cfg(all(windows, feature = "diagnostics-export"))]
pub mod diagnostics;
#[cfg(any(feature = "content-read", test))]
pub mod duplicate_reader;
#[cfg(all(windows, feature = "copy-primitives"))]
pub mod effects;
pub mod enumerate;
pub mod identity;
pub mod jobs;
pub mod linkcount;
pub mod recycle_bin;
pub mod usn;
pub mod volumes;
pub mod watch;

#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;

#[cfg(all(windows, any(feature = "content-read", test)))]
mod noncached;

#[cfg(windows)]
mod win;

#[cfg(test)]
mod tests;
