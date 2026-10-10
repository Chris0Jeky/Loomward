//! Read-only native observations. Identity is never an operation grant.
#[cfg(any(feature = "content-read", test))]
pub mod duplicate_reader;
pub mod enumerate;
pub mod identity;
pub mod jobs;
pub mod linkcount;
pub mod volumes;
pub mod watch;

#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;

#[cfg(windows)]
mod win;

#[cfg(test)]
mod tests;
