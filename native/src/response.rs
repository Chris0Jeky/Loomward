//! Native reply admission after the blocking-worker handoff. The shell uses this exact function;
//! it is independent of Tauri so the await ordering can be exercised without a native UI runtime.

use loomward_protocol::{PendingResponse, ResponseEnvelope};
use std::future::Future;

pub(crate) async fn admit_after_work<E>(
    work: impl Future<Output = Result<Result<PendingResponse, String>, E>>,
) -> Result<ResponseEnvelope, String> {
    let pending = work.await.map_err(|_| "the service failed".to_string())??;
    // A response pause yields to the runtime until the durable outcome is classified. Once
    // admitted there is no further await or service lock before returning to Tauri.
    Ok(pending.admit_async().await)
}
