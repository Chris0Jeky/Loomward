//! The engine's error type and its mapping onto the wire vocabulary (docs/41 section 5.1).

use loomward_protocol::{Detail, ErrorBody, ErrorCode};
use std::fmt;

/// An engine component a lane owns. Names the part that is not built yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Component {
    /// `jobs/`: the job manager (lane L7).
    Jobs,
    /// `scan/`: the scan pipeline (lane L7, watching L16).
    Scan,
    /// `events.rs`: the event bus and replay buffer (lane L7).
    Events,
    /// `telemetry/`: the lease-driven sampler (lane L11).
    Telemetry,
    /// `placement/`: tier model, candidates and simulation (lane L12).
    Placement,
    /// `learning/`: feedback, queue and refit (lane L14).
    Learning,
    /// `teacher/`: preview and the synthetic-only runner (lane L15).
    Teacher,
    /// `budgets.rs`: caps on the engine's own worker pools (lane L7).
    Budgets,
}

impl Component {
    /// Stable lower-case name, e.g. `scan`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Component::Jobs => "jobs",
            Component::Scan => "scan",
            Component::Events => "events",
            Component::Telemetry => "telemetry",
            Component::Placement => "placement",
            Component::Learning => "learning",
            Component::Teacher => "teacher",
            Component::Budgets => "budgets",
        }
    }
}

/// Why an engine call failed. One variant per protocol [`ErrorCode`] the engine can produce;
/// `unsupported_protocol` and `unknown_command` belong to the service and have no variant here.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineError {
    /// The component is not implemented yet (skeleton). Maps to `capability_unavailable`.
    Unavailable {
        /// The part that is missing.
        component: Component,
    },
    /// The request was well-formed on the wire but wrong for the engine.
    InvalidRequest {
        /// Names the bound or rule, never a caller value.
        message: String,
    },
    /// The job, root, grant or object does not exist (or is not visible to the caller).
    NotFound {
        /// What was not found.
        message: String,
    },
    /// A grant, dataset class or confinement rule refuses the call.
    PermissionDenied {
        /// The refusal reason, e.g. `confinement_not_enforced`.
        message: String,
    },
    /// A generation, revision or epoch the caller holds is out of date.
    StaleGeneration {
        /// What moved.
        message: String,
        /// Optional structured wire reason or resource identifier.
        detail: Option<Detail>,
    },
    /// The volume or device is offline.
    DeviceOffline {
        /// Which device, without paths.
        message: String,
    },
    /// The data needed is only partly covered.
    PartialCoverage {
        /// What is missing.
        message: String,
    },
    /// An engine budget (workers, bytes, nodes) forbids or cut the work.
    ResourceBudget {
        /// Which budget.
        message: String,
    },
    /// Another job holds the resource.
    Busy {
        /// What is busy.
        message: String,
        /// Optional structured wire reason or resource identifier.
        detail: Option<Detail>,
    },
    /// The work was cancelled.
    Cancelled {
        /// What was cancelled.
        message: String,
    },
    /// The work did not finish inside its deadline.
    DeadlineExceeded {
        /// What overran.
        message: String,
    },
    /// An engine bug or an I/O failure that is no caller's fault.
    Internal {
        /// What failed, without paths or user data.
        message: String,
        /// Optional structured wire reason or resource identifier.
        detail: Option<Detail>,
    },
}

impl EngineError {
    /// The skeleton answer for `component`.
    pub const fn unavailable(component: Component) -> Self {
        EngineError::Unavailable { component }
    }

    /// The wire code this error maps to.
    pub const fn code(&self) -> ErrorCode {
        match self {
            EngineError::Unavailable { .. } => ErrorCode::CapabilityUnavailable,
            EngineError::InvalidRequest { .. } => ErrorCode::InvalidRequest,
            EngineError::NotFound { .. } => ErrorCode::NotFound,
            EngineError::PermissionDenied { .. } => ErrorCode::PermissionDenied,
            EngineError::StaleGeneration { .. } => ErrorCode::StaleGeneration,
            EngineError::DeviceOffline { .. } => ErrorCode::DeviceOffline,
            EngineError::PartialCoverage { .. } => ErrorCode::PartialCoverage,
            EngineError::ResourceBudget { .. } => ErrorCode::ResourceBudget,
            EngineError::Busy { .. } => ErrorCode::Busy,
            EngineError::Cancelled { .. } => ErrorCode::Cancelled,
            EngineError::DeadlineExceeded { .. } => ErrorCode::DeadlineExceeded,
            EngineError::Internal { .. } => ErrorCode::InternalError,
        }
    }

    /// `retryable` per the table in `semantics.md` section 8. The engine cannot tell a read from
    /// a mutation, so the service clears the flag for a `deadline_exceeded` mutation (section 3).
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            EngineError::StaleGeneration { .. }
                | EngineError::DeviceOffline { .. }
                | EngineError::PartialCoverage { .. }
                | EngineError::ResourceBudget { .. }
                | EngineError::Busy { .. }
                | EngineError::DeadlineExceeded { .. }
        )
    }

    fn message(&self) -> String {
        match self {
            EngineError::Unavailable { component } => {
                format!("engine component not available: {}", component.as_str())
            }
            EngineError::InvalidRequest { message }
            | EngineError::NotFound { message }
            | EngineError::PermissionDenied { message }
            | EngineError::StaleGeneration { message, .. }
            | EngineError::DeviceOffline { message }
            | EngineError::PartialCoverage { message }
            | EngineError::ResourceBudget { message }
            | EngineError::Busy { message, .. }
            | EngineError::Cancelled { message }
            | EngineError::DeadlineExceeded { message }
            | EngineError::Internal { message, .. } => message.clone(),
        }
    }

    /// The wire body, preserving optional structured detail.
    pub fn to_body(&self) -> ErrorBody {
        let mut body = ErrorBody::new(self.code(), &self.message(), self.retryable());
        body.detail = match self {
            Self::Busy { detail, .. }
            | Self::StaleGeneration { detail, .. }
            | Self::Internal { detail, .. } => detail.clone(),
            _ => None,
        };
        body
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code(), self.message())
    }
}

impl std::error::Error for EngineError {}

impl From<EngineError> for ErrorBody {
    fn from(e: EngineError) -> Self {
        e.to_body()
    }
}

/// Result of every engine entry point.
pub type EngineResult<T> = Result<T, EngineError>;
