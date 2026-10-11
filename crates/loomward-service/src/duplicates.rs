use crate::{db, fail, sql_error, with_reason, Inner, Out};
use loomward_engine::scan::CatalogRootIdentity;
use loomward_protocol::*;
use std::time::{Duration, Instant};

impl Inner {
    pub(crate) fn duplicates_inspect(&self, req: &RequestEnvelope) -> Result<Out, ErrorBody> {
        // Refuse before resolving a root, opening metadata or querying the catalogue.
        if self.dataset != DatasetClass::Synthetic {
            return Err(with_reason(
                fail(
                    ErrorCode::CapabilityUnavailable,
                    "Personal content inspection is gated; no content was opened.",
                ),
                "content_read_gated",
            ));
        }
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
        if root.origin != "lab_generated" {
            return Err(with_reason(
                fail(
                    ErrorCode::PermissionDenied,
                    "Synthetic inspection requires a registered lab root.",
                ),
                "synthetic_session_requires_lab_root",
            ));
        }
        let granted = self.engine.granted_root(
            request.root_id,
            root.grant,
            &CatalogRootIdentity {
                row,
                instance: self.instance.clone(),
            },
        )?;
        let mut reader = self.catalog.reader().map_err(crate::catalog_error)?;
        reader.set_deadline(
            Instant::now()
                + Duration::from_millis(req.deadline_ms.map_or(5000, |n| n.get() as u64)),
        );
        let candidates = reader
            .duplicate_candidates(
                row,
                request.budget.min_bytes.map_or(1, |n| n.get()),
                request.budget.max_candidates.get() as usize,
            )
            .map_err(crate::catalog_error)?;
        let job = self
            .engine
            .duplicates_start(granted, candidates, request.budget)?;
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
