//! Read-only native observations. Identity is never an operation grant.
pub mod identity;
pub mod volumes;

#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;

#[cfg(windows)]
mod win;

#[cfg(test)]
mod tests;
