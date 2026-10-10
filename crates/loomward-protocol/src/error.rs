//! Error vocabulary of the service (docs/41 section 5.1).

use crate::types::{Detail, Text};
use serde::{Deserialize, Serialize};

/// Why a call failed. Unknown request fields fail closed as `invalid_request`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    UnsupportedProtocol,
    UnknownCommand,
    NotFound,
    PermissionDenied,
    CapabilityUnavailable,
    StaleGeneration,
    DeviceOffline,
    PartialCoverage,
    ResourceBudget,
    Busy,
    Cancelled,
    DeadlineExceeded,
    InternalError,
}

/// The body of a failed response, also embedded in `Job.error` and `Health.last_error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: Text<1000>,
    pub retryable: bool,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub detail: Option<Detail>,
}

impl ErrorBody {
    /// A body without detail. Over-long messages are cut to the wire limit rather than lost.
    pub fn new(code: ErrorCode, message: &str, retryable: bool) -> Self {
        Self {
            code,
            message: Text::truncated(message),
            retryable,
            detail: None,
        }
    }

    pub fn invalid_request(message: &str) -> Self {
        Self::new(ErrorCode::InvalidRequest, message, false)
    }
}
