//! Native reply admission after the blocking-worker handoff. The shell uses this exact function;
//! it is independent of Tauri so the await ordering can be exercised without a native UI runtime.

use loomward_protocol::{PendingResponse, ResponseEnvelope};
use std::future::Future;

pub(crate) async fn admit_after_work<E>(
    work: impl Future<Output = Result<Result<PendingResponse, String>, E>>,
) -> Result<ResponseEnvelope, String> {
    let pending = work.await.map_err(|_| "the service failed".to_string())??;
    // No await or service lock after admission. A later forget cannot recall an admitted reply.
    Ok(pending.admit())
}
