use crate::{db, fail, sql_error, Inner, Out};
use loomward_engine::scan::CatalogRootIdentity;
use loomward_protocol::*;
use std::time::{Duration, Instant};

impl Inner {
    pub(crate) fn duplicates_inspect(
        &self,
        req: &RequestEnvelope,
        work: Option<&crate::ReadWork>,
    ) -> Result<Out, ErrorBody> {
        // Personal content reads are open for owner grants (q-10); the session's root gate (origin,
        // today's fences) runs before any engine call, metadata open or catalogue query.
        let request: DuplicatesInspectRequest = req.decode_payload()?;
        let row = self.root_row(&request.root_id)?;
        let root = db::roots(&self.db(), Some(row))
            .map_err(sql_error)?
            .into_iter()
            .next()
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown root"))?;
        if root.grant_state != "active" {
            return Err(fail(ErrorCode::PermissionDenied, "grant_not_active"));
        }
        self.session_root_gate(&root)?;
        let granted = self.engine.granted_root(
            request.root_id.clone(),
            root.grant,
            &CatalogRootIdentity {
                row,
                instance: self.instance.clone(),
            },
        );
        // A replaced folder ends its grant here exactly as at scan start (#378).
        if matches!(&granted, Err(loomward_engine::EngineError::PermissionDenied { message })
            if message == "root_identity_changed")
        {
            self.identity_changed(root.grant, &request.root_id, true);
        }
        let granted = granted?;
        let deadline = work.map_or_else(|| Instant::now() + Duration::from_secs(5), |w| w.deadline);
        // #568: fence automatic starts through candidate selection and submission. Drain the
        // old refresh first: it can publish candidates and holds the shared scan-worker budget.
        let claim = self.freshness.explicit(row);
        if let Some(auto) = &claim.auto {
            let _ = self.engine.scan_cancel(auto);
            while self.engine.job_status(auto).is_ok_and(|j| {
                matches!(
                    j.state,
                    JobState::Queued | JobState::Running | JobState::CancelRequested
                )
            }) {
                if Instant::now() >= deadline {
                    return Err(fail(ErrorCode::Busy, "automatic refresh is still stopping"));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let mut reader = self.catalog.reader().map_err(crate::catalog_error)?;
        reader.set_deadline(deadline);
        let candidates = reader
            .duplicate_candidates(
                row,
                request.budget.min_bytes.map_or(1, |n| n.get()),
                request.budget.max_candidates.get() as usize,
            )
            .map_err(crate::catalog_error)?;
        #[cfg(test)]
        self.hooks
            .gate(Command::DuplicatesInspect, "candidates_ready");
        let start = || {
            self.engine.duplicates_start(
                granted.clone(),
                candidates.clone(),
                request.budget.clone(),
            )
        };
        let mut job = start();
        // As at scan.start, wait out only the brief start/revoke fence; an owner job stays busy.
        while matches!(
            &job,
            Err(loomward_engine::EngineError::Busy { detail: None, .. })
        ) && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
            job = start();
        }
        let job = job?;
        drop(claim);
        Ok(Out {
            result: serde_json::json!({"job":job}),
            catalog_rev: None,
            state_rev: None,
        })
    }
    pub(crate) fn duplicates_cancel(&self, request: JobRefRequest) -> Result<Out, ErrorBody> {
        if self.engine.job_status(&request.job_id)?.kind != JobKind::Duplicates {
            return Err(fail(ErrorCode::InvalidRequest, "not a duplicates job"));
        }
        let mut job = self.engine.job_cancel(&request.job_id)?;
        // Cancellation receipts outlive a root grant; reports belong only to live job reads.
        job.duplicates = None;
        Ok(Out {
            result: serde_json::json!({"job":job}),
            catalog_rev: None,
            state_rev: None,
        })
    }
}
