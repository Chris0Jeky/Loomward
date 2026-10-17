//! Read-only observations by default, with opt-in lab effects. Identity is never an operation grant.
#[cfg(any(feature = "content-read", test))]
pub mod duplicate_reader;
#[cfg(all(windows, feature = "copy-primitives"))]
pub mod effects;
pub mod enumerate;
pub mod identity;
pub mod jobs;
pub mod linkcount;
pub mod usn;
pub mod volumes;
pub mod watch;

#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;

#[cfg(windows)]
mod win;

#[cfg(test)]
mod tests;
