//! L14 handlers; the engine owns human events and refit, the service resolves session identities.
//! Personal sessions learn locally from owner labels on active grants (q-11); the teacher stays gated.
use crate::{catalog_error, fail, sql_error, with_reason, Inner, Out};
use loomward_catalog::{NodeDetail, NodeKey, Reader, WriteCommand, WriteReply};
use loomward_engine::learning::FeedbackReplay;
use loomward_protocol::*;
use serde_json::{json, Value};

#[path = "review_deferral.rs"]
mod deferral;

pub(super) fn features(reader: &mut Reader, key: NodeKey) -> Result<(EntryRow, Value), ErrorBody> {
    let detail = reader.inspect(key).map_err(catalog_error)?;
    features_of(reader, key, detail)
}

/// `features` for a caller that already inspected the node.
pub(super) fn features_of(
    reader: &mut Reader,
    key: NodeKey,
    detail: NodeDetail,
) -> Result<(EntryRow, Value), ErrorBody> {
    let path = reader.path(key).map_err(catalog_error)?;
    if path.root_id.as_deref() != Some(detail.root_id.as_str()) {
        return Err(fail(
            ErrorCode::StaleGeneration,
            "Learning root changed while reading features; retry.",
        ));
    }
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
    let size = row
        .logical_bytes
        .ok_or_else(|| fail(ErrorCode::PartialCoverage, "unsettled folder size"))?;
    let features = json!({"name":row.name.as_str().chars().take(256).collect::<String>(),"extension":row.extension.as_ref().map_or("",|e|e.as_str()),"context":context.chars().take(512).collect::<String>(),"size_bytes":size.get().min((1<<53)-1)});
    Ok((row, features))
}

impl Inner {
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
        let _permit = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        match self.engine.learning_replay(&r).map_err(ErrorBody::from)? {
            FeedbackReplay::Exact(result) => return self.learning_out(result),
            FeedbackReplay::Resolve { object_ref, result } => {
                self.feedback_replay_reference(&r.node_id, object_ref)?;
                return self.learning_out(result);
            }
            FeedbackReplay::Missing => {}
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
                    .learning_feedback(&r, reference, &features, None)
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
        let root = crate::bare_row(&detail.root_id, "rt_")
            .ok_or_else(|| fail(ErrorCode::InternalError, "learning root"))?;
        let grant: i64 = self
            .db()
            .query_row(
                "SELECT grant_id FROM main.root WHERE id=?1",
                [root],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        let (_, features) = features_of(&mut reader, key, detail)?;
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
            .learning_feedback(&r, reference, &features, Some(grant))
            .map_err(ErrorBody::from)?;
        self.learning_out(result)
    }

    /// Authenticate a replacement session ID without creating or reconciling a reference.
    /// The saved receipt is an observation of an earlier action, not another label mutation.
    fn feedback_replay_reference(&self, node: &NodeId, reference: i64) -> Result<(), ErrorBody> {
        let (key, born) = self
            .ids
            .open(node.as_str(), &self.instance)
            .map_err(|e| crate::id_refusal(e, "unknown node"))?;
        if !self.feedback_reference_matches(&self.db(), key, born, reference)? {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "client_event_id was used for different content",
            ));
        }
        Ok(())
    }

    pub(crate) fn feedback_reference_matches(
        &self,
        conn: &rusqlite::Connection,
        key: NodeKey,
        born: i64,
        reference: i64,
    ) -> Result<bool, ErrorBody> {
        let (kind, row, table, directory) = match key {
            NodeKey::File(row) => ("file", row, "file", "n.dir_id"),
            NodeKey::Dir(row) => ("dir", row, "dir", "n.id"),
            _ => {
                return Err(fail(
                    ErrorCode::InvalidRequest,
                    "client_event_id was used for different content",
                ))
            }
        };
        // Ordinary Reader methods can repair committed dirty aggregates. A receipt replay needs
        // no aggregate values, so prove visibility, exclusions and identity in one SELECT instead.
        // UNION also bounds malformed ancestor cycles; a same-root path must reach its root.
        // A committed publication can overlay row/birth before its state-side fold. Check the
        // effective binding and native continuity tuple together; paths and row IDs alone do
        // not establish identity, and an unresolved or retired reference is never reattached.
        conn
            .query_row(
                &format!(
                    "WITH RECURSIVE ancestry(id,parent_id,root_id,listing_state) AS (
                       SELECT d.id,d.parent_id,d.root_id,d.listing_state
                       FROM main.{table} n JOIN main.dir d ON d.id={directory} WHERE n.id=?1
                       UNION
                       SELECT d.id,d.parent_id,d.root_id,d.listing_state
                       FROM main.dir d JOIN ancestry a ON d.id=a.parent_id AND d.root_id=a.root_id
                     ) SELECT EXISTS(
                     SELECT 1 FROM st.object_ref o
                     LEFT JOIN st.pending_reference p ON p.object_ref_id=o.id
                       AND p.catalog_instance=?4
                       AND p.publication_token=(SELECT value FROM main.meta WHERE key='reference_publication')
                     JOIN main.{table} n ON n.id=?1
                     JOIN main.dir d ON d.id={directory}
                     JOIN main.root rt ON rt.id=d.root_id
                     JOIN main.volume v ON v.id=rt.volume_id
                     JOIN st.root_grant g ON g.id=rt.grant_id AND g.state='active'
                     WHERE o.id=?5 AND o.kind=?2 AND o.state='resolved' AND o.retired=0
                       AND o.catalog_instance=?4
                       AND ?4=(SELECT value FROM main.meta WHERE key='instance_id')
                       AND CASE WHEN p.object_ref_id IS NULL THEN o.row_id ELSE p.row_id END=?1
                       AND CASE WHEN p.object_ref_id IS NULL THEN o.born_run ELSE p.born_run END=?3
                       AND n.born_run=?3 AND o.volume_key=v.volume_key
                       AND o.file_id=n.file_id AND o.creation_ft=n.created_ft
                       AND v.filesystem IN ('NTFS','ReFS')
                       AND (v.filesystem!='ReFS' OR length(n.file_id)=16)
                       AND n.flags & ~?6 = 0
                       AND EXISTS(SELECT 1 FROM ancestry a WHERE a.parent_id IS NULL AND a.root_id=rt.id)
                       AND NOT EXISTS(SELECT 1 FROM ancestry
                                      WHERE listing_state IN ('absent_pending','excluded')))"
                ),
                rusqlite::params![
                    row,
                    kind,
                    born,
                    self.instance,
                    reference,
                    loomward_catalog::HARDLINK_SUSPECTED
                ],
                |r| r.get(0),
            )
            .map_err(sql_error)
    }
    pub(crate) fn learning_refit(&self) -> Result<Out, ErrorBody> {
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
        let root = r.root_id.as_ref().map(|id| self.root_row(id)).transpose()?;
        let revs = self.revisions()?;
        let ids = {
            let db = self.db();
            let sql = format!("{} SELECT f.id FROM main.file f JOIN main.dir d ON d.id=f.dir_id JOIN main.root rt ON rt.id=d.root_id JOIN st.root_grant g ON g.id=rt.grant_id JOIN main.volume v ON v.id=rt.volume_id WHERE g.state='active' AND (?1 IS NULL OR rt.id=?1) AND f.flags=0 AND d.listing_state NOT IN ('excluded','absent_pending') AND v.filesystem IN ('NTFS','ReFS') AND f.created_ft IS NOT NULL AND f.file_id IS NOT NULL AND (v.filesystem!='ReFS' OR length(f.file_id)=16)
                AND ?2 = (f.id IN (SELECT r.node_row FROM review_references r JOIN st.learning_deferred p ON p.object_ref_id=r.reference_id AND p.grant_id=r.grant_id))
                ORDER BY f.id LIMIT 10001", deferral::CURRENT_REFERENCES);
            let ids = db
                .prepare(&sql)
                .map_err(sql_error)?
                .query_map(
                    rusqlite::params![root, r.mode == Some(LearningQueueMode::Deferred)],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(sql_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sql_error)?;
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
