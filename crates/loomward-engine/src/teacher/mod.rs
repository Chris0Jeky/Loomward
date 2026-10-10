//! Teacher: immutable disclosure preview and the synthetic-only runner (docs/41 section 9.2).
//!
//! Owner: lane L15 (LW-023, LW-052 owned teacher child only, LW-109), with
//! `loomward-windows::jobs` for the owned-child Job Object; L14 supplies the payload builder and
//! strict validator in `loomward-learn`. Confinement for personal data is L15b (LW-111).
//!
//! Scope: preview builds the complete stored request and digests and sends nothing; run consumes
//! the grant and writes the request row before spawn, never retries `interrupted`, times out at
//! 180 s and validates output strictly. The only egress is `teacher.run`, from a pinned native
//! `codex.exe` launched directly (no shell).
//!
//! Gate: [`Engine::teacher_run`] already refuses a personal-class engine with
//! `PermissionDenied("confinement_not_enforced")`; that line stays until LW-111 demonstrates
//! enforcement. Synthetic runs only from fixture or registered lab roots.
//!
//! #117 errata items for this module: none assigned (finding 1, teacher confinement, is closed
//! by keeping personal use disabled).
#![allow(unused_variables)]

use crate::{Component, Engine, EngineError, EngineResult};
use loomward_protocol::{
    DatasetClass, Job, TeacherPreview, TeacherPreviewRequest, TeacherRunRequest,
};

/// The refusal text shared with `semantics.md` section 11.
pub const CONFINEMENT_NOT_ENFORCED: &str = "confinement_not_enforced";

impl Engine {
    /// Builds the immutable preview of what a teacher would receive (`teacher.preview`).
    /// Sends nothing.
    pub fn teacher_preview(&self, request: &TeacherPreviewRequest) -> EngineResult<TeacherPreview> {
        Err(EngineError::unavailable(Component::Teacher))
    }

    /// Runs the teacher for one disclosure grant (`teacher.run`) as a job. Synthetic-only: a
    /// personal-class engine is refused before anything else.
    pub fn teacher_run(&self, request: &TeacherRunRequest) -> EngineResult<Job> {
        if self.config().dataset_class == DatasetClass::Personal {
            return Err(EngineError::PermissionDenied {
                message: CONFINEMENT_NOT_ENFORCED.to_string(),
            });
        }
        Err(EngineError::unavailable(Component::Teacher))
    }
}
