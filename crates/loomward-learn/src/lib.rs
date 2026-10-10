//! Weighted multinomial Naive Bayes student, ported from `python/loomward/learning.py` (ADR-V3-09).
//! Python stays the oracle: fixtures in `fixtures/learning-v3/` pin tokens, abstention and model IDs.
//! Scores are relative and uncalibrated; nothing here authorises a file or process effect.

use std::fmt;

pub mod features;
pub mod student;

/// A `ValueError`-style rejection. The message is the Python reference's message, byte for byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnError {
    message: String,
}

impl LearnError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for LearnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LearnError {}
