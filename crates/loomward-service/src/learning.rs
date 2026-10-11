//! L14 handlers; the engine owns human events and refit, the service resolves session identities.
use crate::{catalog_error, fail, sql_error, with_reason, Inner, Out};
use loomward_catalog::{NodeKey, Reader, WriteCommand, WriteReply};
use loomward_protocol::*;
use serde_json::{json, Value};

pub(super) fn features(reader: &mut Reader, key: NodeKey) -> Result<(EntryRow, Value), ErrorBody> {
    let detail = reader.inspect(key).map_err(catalog_error)?;
    let path = reader.path(key).map_err(catalog_error)?;
    let context = path
        .ancestors
        .iter()
        .rev()
        .skip(1)
        .take(2)
        .map(|p| p.name.as_str())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" ");
    let row: EntryRow = serde_json::from_value(json!(detail.row))
        .map_err(|_| fail(ErrorCode::InternalError, "learning row"))?;
    let features = json!({"name":row.name.as_str().chars().take(256).collect::<String>(),"extension":row.extension.as_ref().map_or("",|e|e.as_str()),"context":context.chars().take(512).collect::<String>(),"size_bytes":row.logical_bytes.get().min((1<<53)-1)});
    Ok((row, features))
}

impl Inner {
    pub(super) fn learning_gate(&self) -> Result<(), ErrorBody> {
        if self.dataset == DatasetClass::Personal {
            return Err(with_reason(
                fail(
                    ErrorCode::CapabilityUnavailable,
                    "Personal learning awaits a separate owner decision.",
                ),
                "learning_personal_gated",
            ));
        }
        Ok(())
    }
    fn learning_out<T: serde::Serialize>(&self, result: T) -> Result<Out, ErrorBody> {
        let rev = self.revisions()?;
        let result = serde_json::to_value(result)
            .map_err(|_| fail(ErrorCode::InternalError, "learning result"))?;
        Ok(Out {
            result,
            catalog_rev: Some(rev.0),
            state_rev: Some(rev.1),
        })
    }
    pub(crate) fn learning_status(&self) -> Result<Out, ErrorBody> {
        self.learning_out(self.engine.learning_status().map_err(ErrorBody::from)?)
    }
    pub(crate) fn feedback_record(&self, r: FeedbackRequest) -> Result<Out, ErrorBody> {
        self.learning_gate()?;
        let _permit = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(replay) = self.engine.learning_replay(&r).map_err(ErrorBody::from)? {
            return self.learning_out(replay);
        }
        self.engine
            .learning_validate_feedback(&r)
            .map_err(ErrorBody::from)?;
        if r.retract {
            let prior = self
                .engine
                .learning_prior(&r.node_id)
                .map_err(ErrorBody::from)?;
            // Older labels predate the node lookup; authenticate the old incarnation without resolving it.
            let (reference, features) = match prior {
                Some(prior) => prior,
                None => {
                    use rusqlite::OptionalExtension;
                    let (key, born) = self
                        .ids
                        .open(r.node_id.as_str(), &self.instance)
                        .map_err(|e| crate::id_refusal(e, "unknown node"))?;
                    let (kind, row) = match key {
                        NodeKey::File(id) => ("file", id),
                        NodeKey::Dir(id) => ("dir", id),
                        _ => {
                            return Err(fail(
                                ErrorCode::InvalidRequest,
                                "no prior human label to retract",
                            ))
                        }
                    };
                    let prior: Option<(i64, String)> = self.db().query_row("SELECT h.object_ref_id,h.features_json FROM st.human_feedback h JOIN st.object_ref o ON o.id=h.object_ref_id WHERE o.catalog_instance=?1 AND o.kind=?2 AND o.row_id=?3 AND o.born_run=?4 ORDER BY (o.state='resolved' AND o.retired=0) DESC,h.revision DESC,h.id DESC LIMIT 1", rusqlite::params![self.instance, kind, row, born], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(sql_error)?;
                    let (reference, features) = prior.ok_or_else(|| {
                        fail(ErrorCode::InvalidRequest, "no prior human label to retract")
                    })?;
                    (
                        reference,
                        serde_json::from_str(&features)
                            .map_err(|_| fail(ErrorCode::InternalError, "learning features"))?,
                    )
                }
            };
            return self.learning_out(
                self.engine
                    .learning_feedback(&r, reference, &features)
                    .map_err(ErrorBody::from)?,
            );
        }
        let mut reader = self.catalog.reader().map_err(catalog_error)?;
        let key = self.node(&r.node_id, &mut reader)?;
        let detail = reader.inspect(key).map_err(catalog_error)?;
        if detail.identity.durable_reference != "available" {
            return Err(with_reason(
                fail(
                    ErrorCode::CapabilityUnavailable,
                    "Feedback requires a durable native reference.",
                ),
                "durable_reference_unavailable",
            ));
        }
        if !detail.exclusions.is_empty() {
            return Err(fail(
                ErrorCode::PermissionDenied,
                "Excluded metadata cannot be labelled.",
            ));
        }
        let (_, features) = features(&mut reader, key)?;
        drop(reader);
        let reference = self
            .catalog
            .writer()
            .call(WriteCommand::ObjectReference {
                node: key,
                observed_at_ns: crate::now_ns(),
            })
            .map_err(catalog_error)?;
        let WriteReply::Reference(reference) = reference else {
            return Err(fail(ErrorCode::InternalError, "durable reference"));
        };
        let result = self
            .engine
            .learning_feedback(&r, reference, &features)
            .map_err(ErrorBody::from)?;
        self.learning_out(result)
    }
    pub(crate) fn learning_refit(&self) -> Result<Out, ErrorBody> {
        self.learning_gate()?;
        let _permit = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        self.learning_out(JobResult {
            job: self.engine.learning_refit().map_err(ErrorBody::from)?,
        })
    }
    pub(crate) fn learning_queue(
        &self,
        r: LearningQueueRequest,
        work: Option<&crate::ReadWork>,
    ) -> Result<Out, ErrorBody> {
        if self.dataset == DatasetClass::Personal {
            return self.learning_out(LearningQueue {
                model_id: None,
                items: BoundedVec::new(vec![]).unwrap(),
                next_cursor: None,
            });
        }
        let root = r.root_id.as_ref().map(|id| self.root_row(id)).transpose()?;
        let revs = self.revisions()?;
        let ids = {
            let db = self.db();
            let ids=db.prepare("SELECT f.id FROM main.file f JOIN main.dir d ON d.id=f.dir_id JOIN main.root rt ON rt.id=d.root_id JOIN st.root_grant g ON g.id=rt.grant_id JOIN main.volume v ON v.id=rt.volume_id WHERE g.state='active' AND (?1 IS NULL OR rt.id=?1) AND f.flags=0 AND d.listing_state NOT IN ('excluded','absent_pending') AND v.filesystem IN ('NTFS','ReFS') AND f.created_ft IS NOT NULL AND f.file_id IS NOT NULL AND (v.filesystem!='ReFS' OR length(f.file_id)=16) ORDER BY f.id LIMIT 10001").map_err(sql_error)?.query_map([root],|row|row.get::<_,i64>(0)).map_err(sql_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(sql_error)?;
            ids
        };
        if ids.len() > 10000 {
            return Err(fail(
                ErrorCode::ResourceBudget,
                "Review queue supports at most 10000 candidates.",
            ));
        }
        let mut reader = match work {
            Some(w) => w.reader(&self.catalog)?,
            None => self.catalog.reader().map_err(catalog_error)?,
        };
        let mut candidates = Vec::new();
        for id in ids {
            let key = NodeKey::File(id);
            let detail = reader.inspect(key).map_err(catalog_error)?;
            if detail.identity.durable_reference == "available" && detail.exclusions.is_empty() {
                candidates.push(features(&mut reader, key)?);
            }
        }
        drop(reader);
        if self.revisions()? != revs {
            return Err(fail(
                ErrorCode::StaleGeneration,
                "Catalogue or feedback changed during review.",
            ));
        }
        let result = self
            .engine
            .learning_queue(&r, candidates, revs)
            .map_err(ErrorBody::from)?;
        let mut result = json!(result);
        self.seal(&mut result)?;
        Ok(Out {
            result,
            catalog_rev: Some(revs.0),
            state_rev: Some(revs.1),
        })
    }
}
