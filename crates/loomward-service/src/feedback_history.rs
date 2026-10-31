//! Bounded, read-only owner history over the existing human ledger.
use crate::cursors::{Position, Resume};
use crate::{db, fail, sql_error, ts_ns, with_reason, Inner, Out, ReadWork};
use loomward_catalog::NodeKey;
use loomward_protocol::*;
use rusqlite::params;
use serde_json::{json, Value};

impl Inner {
    pub(crate) fn feedback_history(
        &self,
        r: FeedbackHistoryRequest,
        work: Option<&ReadWork>,
    ) -> Result<Out, ErrorBody> {
        let conn = self.collection_read(work)?;
        let revs = db::revisions(&conn).map_err(sql_error)?;
        let before = match &r.cursor {
            None => i64::MAX,
            Some(cursor) => {
                match self
                    .cursors
                    .resume(cursor, Command::FeedbackHistory, "{}", Some(revs.1))
                {
                    Ok(Position::Feedback {
                        before,
                        catalog_rev,
                    }) if catalog_rev == revs.0 => before,
                    Ok(Position::Feedback { .. }) => {
                        return Err(with_reason(
                            fail(
                                ErrorCode::StaleGeneration,
                                "Catalogue changed; restart history.",
                            ),
                            "catalog_rev_changed",
                        ))
                    }
                    Err(Resume::StateChanged) => {
                        return Err(with_reason(
                            fail(
                                ErrorCode::StaleGeneration,
                                "Feedback or access changed; restart history.",
                            ),
                            "state_rev_changed",
                        ))
                    }
                    Err(Resume::Expired) => {
                        return Err(with_reason(
                            fail(ErrorCode::StaleGeneration, "History cursor expired."),
                            "cursor_expired",
                        ))
                    }
                    _ => {
                        return Err(fail(
                            ErrorCode::InvalidRequest,
                            "Cursor belongs to another request.",
                        ))
                    }
                }
            }
        };
        let mut query = conn.prepare(
            "SELECT h.id,h.event_id,h.revision,h.label,h.retracted,h.taxonomy_version,h.recorded_at_ns,
             NOT EXISTS(SELECT 1 FROM st.human_feedback newer WHERE newer.object_ref_id=h.object_ref_id AND newer.revision>h.revision),
             h.object_ref_id,o.kind,
             CASE WHEN p.object_ref_id IS NULL THEN o.row_id ELSE p.row_id END,
             CASE WHEN p.object_ref_id IS NULL THEN o.born_run ELSE p.born_run END,
             EXISTS(SELECT 1 FROM st.learning_feature_grant fg JOIN st.root_grant g ON g.id=fg.grant_id
                    WHERE fg.event_id=h.event_id AND g.state='active')
               AND NOT EXISTS(SELECT 1 FROM st.learning_redacted x WHERE x.event_id=h.event_id),
             h.features_json
             FROM st.human_feedback h JOIN st.object_ref o ON o.id=h.object_ref_id
             LEFT JOIN st.pending_reference p ON p.object_ref_id=o.id AND p.catalog_instance=?1
               AND p.publication_token=(SELECT value FROM main.meta WHERE key='reference_publication')
             WHERE h.id<?2 ORDER BY h.id DESC LIMIT ?3"
        ).map_err(sql_error)?;
        let mut rows = query
            .query(params![self.instance, before, r.limit.get() + 1])
            .map_err(sql_error)?;
        let mut items = Vec::new();
        let mut last = before;
        let mut more = false;
        while let Some(row) = rows.next().map_err(sql_error)? {
            if items.len() == r.limit.get() as usize {
                more = true;
                break;
            }
            if work.is_some_and(|w| self.past(w.deadline)) {
                return Err(fail(
                    ErrorCode::DeadlineExceeded,
                    "The history read deadline expired.",
                ));
            }
            let reference = row.get::<_, i64>(8).map_err(sql_error)?;
            let kind = row.get::<_, String>(9).map_err(sql_error)?;
            let id = row.get::<_, Option<i64>>(10).map_err(sql_error)?;
            let born = row.get::<_, Option<i64>>(11).map_err(sql_error)?;
            let provenance = row.get::<_, bool>(12).map_err(sql_error)?;
            let key = id.and_then(|id| match kind.as_str() {
                "file" => Some(NodeKey::File(id)),
                "dir" => Some(NodeKey::Dir(id)),
                _ => None,
            });
            let visible = if let (true, Some(key), Some(born)) = (provenance, key, born) {
                self.feedback_reference_matches(&conn, key, born, reference)?
            } else {
                false
            };
            let name = if visible {
                let features: Value = serde_json::from_str(
                    &row.get::<_, String>(13).map_err(sql_error)?,
                )
                .map_err(|_| {
                    fail(
                        ErrorCode::InternalError,
                        "Invalid recorded learning features.",
                    )
                })?;
                features
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|s| s.chars().take(256).collect::<String>())
            } else {
                None
            };
            last = row.get(0).map_err(sql_error)?;
            items.push(json!({
                "event_id": row.get::<_, String>(1).map_err(sql_error)?,
                "revision": row.get::<_, i64>(2).map_err(sql_error)?,
                "label": row.get::<_, Option<String>>(3).map_err(sql_error)?,
                "retracted": row.get::<_, bool>(4).map_err(sql_error)?,
                "taxonomy_version": row.get::<_, i64>(5).map_err(sql_error)?,
                "recorded_at": ts_ns(row.get(6).map_err(sql_error)?),
                "is_current": row.get::<_, bool>(7).map_err(sql_error)?,
                "reference_status": if !provenance { "redacted" } else if visible { "resolved" } else { "unresolved" },
                "name": name,
            }));
        }
        #[cfg(test)]
        self.hooks.gate(Command::FeedbackHistory, "history_read");
        let next_cursor = more.then(|| {
            self.cursors.issue(
                Command::FeedbackHistory,
                "{}".into(),
                Position::Feedback {
                    before: last,
                    catalog_rev: revs.0,
                },
                Some(revs.1),
            )
        });
        Ok(Out {
            result: json!({"items":items,"next_cursor":next_cursor}),
            catalog_rev: Some(revs.0),
            state_rev: Some(revs.1),
        })
    }
}
