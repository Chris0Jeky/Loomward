//! Durable queue preferences; no labels, feature snapshots, model state or file effects.
use crate::{db, fail, sql_error, with_reason, Inner, Out};
use loomward_catalog::NodeKey;
use loomward_protocol::*;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde_json::json;
use sha2::{Digest, Sha256};

// A committed pending publication replaces the old binding, including a retirement to NULL.
pub(super) const CURRENT_REFERENCES: &str = "WITH review_references AS (
 SELECT o.id reference_id,f.id node_row,f.born_run,rt.grant_id
 FROM st.object_ref o
 LEFT JOIN st.pending_reference p ON p.object_ref_id=o.id
   AND p.catalog_instance=o.catalog_instance
   AND p.publication_token=(SELECT value FROM main.meta WHERE key='reference_publication')
 JOIN main.file f ON f.id=CASE WHEN p.object_ref_id IS NULL THEN o.row_id ELSE p.row_id END
   AND f.born_run=CASE WHEN p.object_ref_id IS NULL THEN o.born_run ELSE p.born_run END
   AND f.file_id=o.file_id AND f.created_ft=o.creation_ft
 JOIN main.dir d ON d.id=f.dir_id
 JOIN main.root rt ON rt.id=d.root_id
 JOIN main.volume v ON v.id=rt.volume_id AND v.volume_key=o.volume_key
 JOIN st.root_grant g ON g.id=rt.grant_id AND g.state='active'
 WHERE o.kind='file' AND o.state='resolved' AND o.retired=0
   AND o.catalog_instance=(SELECT value FROM main.meta WHERE key='instance_id')
   AND f.flags=0 AND v.filesystem IN ('NTFS','ReFS')
   AND (v.filesystem!='ReFS' OR length(f.file_id)=16)
)";

impl Inner {
    pub(crate) fn learning_set_deferred(&self, req: &RequestEnvelope) -> Result<Out, ErrorBody> {
        let r: LearningDeferredRequest = req.decode_payload()?;
        let expected = req
            .expected_state_rev
            .ok_or_else(|| fail(ErrorCode::InvalidRequest, "expected_state_rev is required"))?;
        let digest = format!("{:x}", Sha256::digest(json!([r, expected]).to_string()));
        let _permit = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = self.db();
        // Like collection preferences, reference creation and the preference share one transaction.
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let revs = db::revisions(&tx).map_err(sql_error)?;
        let receipt: Option<(i64, String, i64, String, bool)> = tx
            .query_row(
                "SELECT r.object_ref_id,r.digest,r.expected_revision,r.result_json,g.state='active'
             FROM st.learning_deferred_receipt r JOIN st.root_grant g ON g.id=r.grant_id
             WHERE r.client_event_id=?1",
                [r.client_event_id.as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()
            .map_err(sql_error)?;
        if let Some((reference, saved_digest, revision, result, active)) = receipt {
            let mut result: LearningDeferredResult =
                serde_json::from_str(&result).map_err(|_| {
                    fail(
                        ErrorCode::InternalError,
                        "review preference receipt unavailable",
                    )
                })?;
            if saved_digest != digest {
                if result.deferred != r.deferred || revision as u64 != expected.get() || !active {
                    return Err(fail(
                        ErrorCode::InvalidRequest,
                        "client_event_id was used for different content",
                    ));
                }
                let (key, born) = self
                    .ids
                    .open(r.node_id.as_str(), &self.instance)
                    .map_err(|e| crate::id_refusal(e, "unknown node"))?;
                if !self.feedback_reference_matches(&tx, key, born, reference)? {
                    return Err(fail(
                        ErrorCode::InvalidRequest,
                        "client_event_id was used for different content",
                    ));
                }
            }
            result.idempotent_replay = true;
            return Ok(Out {
                result: json!(result),
                catalog_rev: Some(revs.0),
                state_rev: Some(revs.1),
            });
        }
        if expected.get() != revs.1 as u64 {
            return Err(with_reason(
                fail(
                    ErrorCode::StaleGeneration,
                    "Review changed; refresh before choosing again.",
                ),
                "state_rev_changed",
            ));
        }
        let (key, born) = self
            .ids
            .open(r.node_id.as_str(), &self.instance)
            .map_err(|e| crate::id_refusal(e, "unknown node"))?;
        let NodeKey::File(row) = key else {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "Only review files can be deferred.",
            ));
        };
        let identity: Option<(String, Vec<u8>, i64, i64)> = tx
            .query_row(
                "SELECT v.volume_key,f.file_id,f.created_ft,rt.grant_id
             FROM main.file f JOIN main.dir d ON d.id=f.dir_id JOIN main.root rt ON rt.id=d.root_id
             JOIN main.volume v ON v.id=rt.volume_id JOIN st.root_grant g ON g.id=rt.grant_id
             WHERE f.id=?1 AND f.born_run=?2 AND g.state='active' AND f.flags=0
               AND f.file_id IS NOT NULL AND f.created_ft IS NOT NULL
               AND v.filesystem IN ('NTFS','ReFS') AND length(f.file_id) IN (8,16)
               AND (v.filesystem!='ReFS' OR length(f.file_id)=16)",
                params![row, born],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(sql_error)?;
        let Some((volume, identity, creation, grant)) = identity else {
            return Err(fail(
                ErrorCode::NotFound,
                "No eligible durable review file.",
            ));
        };
        let existing = tx.query_row(
            &format!("{CURRENT_REFERENCES} SELECT reference_id FROM review_references WHERE node_row=?1 AND born_run=?2 ORDER BY reference_id LIMIT 1"),
            params![row,born], |r| r.get::<_, i64>(0)
        ).optional().map_err(sql_error)?;
        let reference = match existing {
            Some(reference) => reference,
            None => {
                tx.execute("INSERT INTO st.object_ref(volume_key,file_id,creation_ft,kind,state,last_resolved_ns,incarnation,catalog_instance,row_id,born_run)
                    VALUES(?1,?2,?3,'file','resolved',?4,(SELECT coalesce(max(incarnation),0)+1 FROM st.object_ref WHERE volume_key=?1 AND file_id=?2),?5,?6,?7)",
                    params![volume,identity,creation,crate::now_ns(),self.instance,row,born]).map_err(sql_error)?;
                tx.last_insert_rowid()
            }
        };
        if !self.feedback_reference_matches(&tx, key, born, reference)? {
            return Err(fail(
                ErrorCode::PermissionDenied,
                "This review reference is no longer visible.",
            ));
        }
        if r.deferred {
            tx.execute("INSERT INTO st.learning_deferred VALUES(?1,?2) ON CONFLICT(object_ref_id) DO UPDATE SET grant_id=excluded.grant_id", params![reference,grant]).map_err(sql_error)?;
        } else {
            tx.execute(
                "DELETE FROM st.learning_deferred WHERE object_ref_id=?1",
                [reference],
            )
            .map_err(sql_error)?;
        }
        tx.execute(
            "UPDATE st.revision SET state_rev=state_rev+1 WHERE id=1",
            [],
        )
        .map_err(sql_error)?;
        let revs = db::revisions(&tx).map_err(sql_error)?;
        let result =
            json!({"deferred":r.deferred,"revision":revs.1.to_string(),"idempotent_replay":false});
        tx.execute(
            "INSERT INTO st.learning_deferred_receipt VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                r.client_event_id.as_str(),
                reference,
                grant,
                digest,
                expected.get() as i64,
                result.to_string()
            ],
        )
        .map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        Ok(Out {
            result,
            catalog_rev: Some(revs.0),
            state_rev: Some(revs.1),
        })
    }
}
