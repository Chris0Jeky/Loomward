//! Collections are local meaning preferences; only state.db is written, never a file or grant.
use crate::cursors::{Position, Resume};
use crate::{catalog_error, db, fail, now_ns, sql_error, ts_ns, with_reason, Inner, Out, ReadWork};
use loomward_catalog::NodeKey;
use loomward_protocol::*;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};

// Pending publication supersedes the old binding; a retirement intent resolves to no row.
const REFERENCES: &str = "WITH refs AS (
 SELECT o.id,o.kind,o.volume_key,o.file_id,o.creation_ft,
 CASE WHEN p.object_ref_id IS NOT NULL THEN p.row_id ELSE o.row_id END row_id,
 CASE WHEN p.object_ref_id IS NOT NULL THEN p.born_run ELSE o.born_run END born_run,
 o.state='resolved' AND o.retired=0 AND o.catalog_instance=(SELECT value FROM main.meta WHERE key='instance_id') eligible
 FROM st.object_ref o LEFT JOIN st.pending_reference p ON p.object_ref_id=o.id
 AND p.catalog_instance=o.catalog_instance AND p.publication_token=(SELECT value FROM main.meta WHERE key='reference_publication')
), resolved AS (
 SELECT o.id,o.kind,o.row_id,o.born_run,g.state grant_state,
 CASE o.kind WHEN 'file' THEN f.logical ELSE d.sub_logical END logical,
 CASE o.kind WHEN 'file' THEN 1 ELSE d.sub_complete AND d.agg_valid_rev>=d.dirty_rev END complete
 FROM refs o LEFT JOIN main.file f ON o.kind='file' AND f.id=o.row_id AND f.born_run=o.born_run AND f.file_id=o.file_id AND f.created_ft=o.creation_ft
 LEFT JOIN main.dir d ON (o.kind='dir' AND d.id=o.row_id AND d.born_run=o.born_run AND d.file_id=o.file_id AND d.created_ft=o.creation_ft) OR (o.kind='file' AND d.id=f.dir_id)
 LEFT JOIN main.root rt ON rt.id=d.root_id LEFT JOIN st.root_grant g ON g.id=rt.grant_id
 WHERE o.eligible AND rt.volume_id=(SELECT id FROM main.volume WHERE volume_key=o.volume_key)
 AND NOT EXISTS(WITH RECURSIVE a(id,parent_id,listing_state) AS (SELECT id,parent_id,listing_state FROM main.dir WHERE id=d.id UNION ALL SELECT x.id,x.parent_id,x.listing_state FROM main.dir x JOIN a ON x.id=a.parent_id) SELECT 1 FROM a WHERE listing_state='absent_pending')
)";

type Handled = Result<Out, ErrorBody>;
type Reference = (String, i64, i64, String, Vec<u8>, i64);
fn out(result: Value, revs: (i64, i64)) -> Out {
    Out {
        result,
        catalog_rev: Some(revs.0),
        state_rev: Some(revs.1),
    }
}
fn collection_row(id: &str) -> Result<i64, ErrorBody> {
    id.strip_prefix("co_")
        .and_then(|n| n.parse::<i64>().ok())
        .filter(|n| *n > 0 && format!("co_{n}") == id)
        .ok_or_else(|| fail(ErrorCode::NotFound, "unknown collection"))
}
fn collection(conn: &Connection, id: i64) -> Result<Value, ErrorBody> {
    let sql=format!("{REFERENCES} SELECT c.id,c.name,c.kind,c.created_at_ns,count(m.object_ref_id),
 CASE WHEN count(m.object_ref_id)=count(CASE WHEN r.grant_state='active' AND r.complete THEN r.logical END)
 THEN coalesce(sum(CASE WHEN r.grant_state='active' AND r.complete THEN r.logical END),0) END
 FROM st.collection c LEFT JOIN st.collection_member m ON m.collection_id=c.id LEFT JOIN resolved r ON r.id=m.object_ref_id
 WHERE c.id=?1 GROUP BY c.id");
    conn.query_row(&sql,[id],|r| Ok(json!({"collection_id":format!("co_{}",r.get::<_,i64>(0)?),"name":r.get::<_,String>(1)?,"kind":r.get::<_,String>(2)?,"created_at":ts_ns(r.get(3)?),"member_count":r.get::<_,i64>(4)?,"logical_bytes":r.get::<_,Option<i64>>(5)?.map(|n|n.to_string())})))
        .optional().map_err(sql_error)?.ok_or_else(||fail(ErrorCode::NotFound,"unknown collection"))
}

impl Inner {
    /// Whether `deadline` has passed. Tests may run the clock ahead (`Hooks::skew`) to expire a
    /// page deterministically at a gate instead of racing a wall-clock budget.
    fn past(&self, deadline: std::time::Instant) -> bool {
        #[cfg(test)]
        let ahead = *self.hooks.skew.lock().unwrap();
        #[cfg(not(test))]
        let ahead = std::time::Duration::ZERO;
        std::time::Instant::now() + ahead >= deadline
    }
    fn collection_read(&self, work: Option<&ReadWork>) -> Result<Connection, ErrorBody> {
        let conn = db::open(&self.state_dir).map_err(sql_error)?;
        conn.execute_batch("PRAGMA query_only=ON; BEGIN")
            .map_err(sql_error)?;
        if let Some(work) = work {
            let mut handle = work.interrupt.lock().unwrap_or_else(|e| e.into_inner());
            if std::time::Instant::now() >= work.deadline {
                return Err(fail(
                    ErrorCode::DeadlineExceeded,
                    "the read deadline expired",
                ));
            }
            let deadline = work.deadline;
            conn.progress_handler(1000, Some(move || std::time::Instant::now() >= deadline))
                .map_err(sql_error)?;
            *handle = Some(conn.get_interrupt_handle());
        }
        Ok(conn)
    }
    pub(crate) fn collections_list(&self, work: Option<&ReadWork>) -> Handled {
        let conn = self.collection_read(work)?;
        let revs = db::revisions(&conn).map_err(sql_error)?;
        let ids = conn
            .prepare("SELECT id FROM st.collection ORDER BY id LIMIT 257")
            .map_err(sql_error)?
            .query_map([], |r| r.get::<_, i64>(0))
            .map_err(sql_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql_error)?;
        if ids.len() > 256 {
            return Err(fail(
                ErrorCode::ResourceBudget,
                "more than 256 collections; list cannot be represented",
            ));
        }
        let collections = ids
            .into_iter()
            .map(|id| collection(&conn, id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(out(json!({"collections":collections}), revs))
    }
    pub(crate) fn collections_create(&self, req: &RequestEnvelope) -> Handled {
        let r: CollectionCreateRequest = req.decode_payload()?;
        let _permit = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = self.db();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        if let Some(id) = tx
            .query_row(
                "SELECT id FROM st.collection WHERE name=?1",
                [r.name.as_ref()],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map_err(sql_error)?
        {
            let mut error = fail(
                ErrorCode::InvalidRequest,
                "a collection with this name already exists",
            );
            error.detail = Some(
                serde_json::from_value(json!({"existing_id":format!("co_{id}")})).expect("detail"),
            );
            return Err(error);
        }
        let count: i64 = tx
            .query_row("SELECT count(*) FROM st.collection", [], |r| r.get(0))
            .map_err(sql_error)?;
        if count >= 256 {
            return Err(fail(
                ErrorCode::ResourceBudget,
                "the 256 collection limit was reached",
            ));
        }
        tx.execute(
            "INSERT INTO st.collection(name,kind,created_at_ns) VALUES(?1,'human',?2)",
            params![r.name.as_ref(), now_ns()],
        )
        .map_err(sql_error)?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "UPDATE st.revision SET state_rev=state_rev+1 WHERE id=1",
            [],
        )
        .map_err(sql_error)?;
        let result = collection(&tx, id)?;
        let revs = db::revisions(&tx).map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        Ok(out(result, revs))
    }
    /// Resolve sealed current nodes inside the state transaction before writing any references.
    fn collection_reference(&self, conn: &Connection, id: &str) -> Result<Reference, ErrorBody> {
        let (key, born) = self
            .ids
            .open(id, &self.instance)
            .map_err(|e| crate::id_refusal(e, "unknown node"))?;
        let (kind, row) = match key {
            NodeKey::File(n) => ("file", n),
            NodeKey::Dir(n) => ("dir", n),
            _ => {
                return Err(fail(
                    ErrorCode::CapabilityUnavailable,
                    "durable identity unavailable",
                ))
            }
        };
        let from = if kind == "file" {
            "main.file n JOIN main.dir d ON d.id=n.dir_id"
        } else {
            "main.dir n JOIN main.dir d ON d.id=n.id"
        };
        let sql=format!("SELECT v.volume_key,n.file_id,n.created_ft,v.filesystem FROM {from} JOIN main.root rt ON rt.id=d.root_id JOIN main.volume v ON v.id=rt.volume_id JOIN st.root_grant g ON g.id=rt.grant_id WHERE n.id=?1 AND n.born_run=?2 AND g.state='active' AND NOT EXISTS(WITH RECURSIVE a(id,parent_id,listing_state) AS (SELECT id,parent_id,listing_state FROM main.dir WHERE id=d.id UNION ALL SELECT x.id,x.parent_id,x.listing_state FROM main.dir x JOIN a ON x.id=a.parent_id) SELECT 1 FROM a WHERE listing_state='absent_pending')");
        let value = conn
            .query_row(&sql, params![row, born], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<Vec<u8>>>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .optional()
            .map_err(sql_error)?
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown node"))?;
        let (volume, identity, creation, fs) = value;
        let identity = identity
            .filter(|id| matches!(id.len(), 8 | 16))
            .ok_or_else(|| {
                fail(
                    ErrorCode::CapabilityUnavailable,
                    "durable identity unavailable",
                )
            })?;
        if !matches!(fs.as_deref(), Some("NTFS" | "ReFS"))
            || (fs.as_deref() == Some("ReFS") && identity.len() != 16)
        {
            return Err(fail(
                ErrorCode::CapabilityUnavailable,
                "durable identity unavailable",
            ));
        }
        let creation = creation.ok_or_else(|| {
            fail(
                ErrorCode::CapabilityUnavailable,
                "durable creation time unavailable",
            )
        })?;
        Ok((kind.into(), row, born, volume, identity, creation))
    }
    pub(crate) fn collections_update(&self, req: &RequestEnvelope) -> Handled {
        let r: CollectionMembersUpdate = req.decode_payload()?;
        let expected = req
            .expected_state_rev
            .ok_or_else(|| fail(ErrorCode::InvalidRequest, "expected_state_rev is required"))?;
        if r.add.iter().any(|id| r.remove.contains(id)) {
            return Err(fail(ErrorCode::InvalidRequest, "adds and removes overlap"));
        }
        let id = collection_row(&r.collection_id)?;
        let remove_members = r
            .remove_members
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|id| {
                id.strip_prefix("om_")
                    .and_then(|n| n.parse::<i64>().ok())
                    .filter(|n| *n > 0 && format!("om_{n}") == id.as_ref())
                    .ok_or_else(|| fail(ErrorCode::InvalidRequest, "invalid member ID"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let _permit = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = self.db();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let revs = db::revisions(&tx).map_err(sql_error)?;
        if expected.get() != revs.1 as u64 {
            return Err(with_reason(
                fail(
                    ErrorCode::StaleGeneration,
                    "state.db changed; read members again",
                ),
                "state_rev_changed",
            ));
        }
        let c = collection(&tx, id)?;
        if c["kind"] != "human" {
            return Err(fail(
                ErrorCode::CapabilityUnavailable,
                "label views cannot be edited",
            ));
        }
        let nodes = r
            .add
            .iter()
            .chain(r.remove.iter())
            .map(|id| self.collection_reference(&tx, id))
            .collect::<Result<Vec<_>, _>>()?;
        let mut changed = 0;
        for (i, (kind, row, born, volume, identity, creation)) in nodes.into_iter().enumerate() {
            let sql=format!("{REFERENCES} SELECT r.id FROM resolved r JOIN st.object_ref o ON o.id=r.id WHERE r.kind=?1 AND r.row_id=?2 AND r.born_run=?3 AND o.volume_key=?4 AND o.file_id=?5 AND o.creation_ft=?6");
            let existing = tx
                .query_row(
                    &sql,
                    params![kind, row, born, volume, identity, creation],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .map_err(sql_error)?;
            if i < r.add.len() {
                let reference = match existing {
                    Some(n) => n,
                    None => {
                        tx.execute("INSERT INTO st.object_ref(volume_key,file_id,creation_ft,kind,state,last_resolved_ns,incarnation,catalog_instance,row_id,born_run) VALUES(?1,?2,?3,?4,'resolved',?5,(SELECT coalesce(max(incarnation),0)+1 FROM st.object_ref WHERE volume_key=?1 AND file_id=?2),?6,?7,?8)",params![volume,identity,creation,kind,now_ns(),self.instance,row,born]).map_err(sql_error)?;
                        tx.last_insert_rowid()
                    }
                };
                if remove_members.contains(&reference) {
                    return Err(fail(ErrorCode::InvalidRequest, "adds and removes overlap"));
                }
                changed += tx
                    .execute(
                        "INSERT OR IGNORE INTO st.collection_member VALUES(?1,?2,?3)",
                        params![id, reference, now_ns()],
                    )
                    .map_err(sql_error)?;
            } else if let Some(reference) = existing {
                changed+=tx.execute("DELETE FROM st.collection_member WHERE collection_id=?1 AND object_ref_id=?2",params![id,reference]).map_err(sql_error)?;
            }
        }
        for reference in remove_members {
            changed += tx
                .execute(
                    "DELETE FROM st.collection_member WHERE collection_id=?1 AND object_ref_id=?2",
                    params![id, reference],
                )
                .map_err(sql_error)?;
        }
        if changed > 0 {
            tx.execute(
                "UPDATE st.revision SET state_rev=state_rev+1 WHERE id=1",
                [],
            )
            .map_err(sql_error)?;
        }
        let result = collection(&tx, id)?;
        let revs = db::revisions(&tx).map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        Ok(out(result, revs))
    }
    pub(crate) fn collections_members(
        &self,
        req: &RequestEnvelope,
        work: Option<&ReadWork>,
    ) -> Handled {
        let r: CollectionMembersRequest = req.decode_payload()?;
        let id = collection_row(&r.collection_id)?;
        let conn = self.collection_read(work)?;
        let revs = db::revisions(&conn).map_err(sql_error)?;
        let c = collection(&conn, id)?;
        let bind = json!({"collection_id":r.collection_id}).to_string();
        let after = match &r.cursor {
            None => 0,
            Some(cursor) => {
                match self
                    .cursors
                    .resume(cursor, Command::CollectionsMembers, &bind, Some(revs.1))
                {
                    Ok(Position::Members { after, catalog_rev }) if catalog_rev == revs.0 => after,
                    Ok(Position::Members { .. }) => {
                        return Err(with_reason(
                            fail(
                                ErrorCode::StaleGeneration,
                                "catalogue changed; restart paging",
                            ),
                            "catalog_rev_changed",
                        ))
                    }
                    Err(Resume::StateChanged) => {
                        return Err(with_reason(
                            fail(ErrorCode::StaleGeneration, "state changed; restart paging"),
                            "state_rev_changed",
                        ))
                    }
                    Err(Resume::Expired) => {
                        return Err(with_reason(
                            fail(ErrorCode::StaleGeneration, "cursor expired"),
                            "cursor_expired",
                        ))
                    }
                    _ => {
                        return Err(fail(
                            ErrorCode::InvalidRequest,
                            "cursor belongs to another request",
                        ))
                    }
                }
            }
        };
        let sql=format!("{REFERENCES} SELECT m.object_ref_id,r.kind,r.row_id,r.born_run,r.grant_state FROM st.collection_member m LEFT JOIN resolved r ON r.id=m.object_ref_id WHERE m.collection_id=?1 AND m.object_ref_id>?2 ORDER BY m.object_ref_id LIMIT ?3");
        let rows = conn
            .prepare(&sql)
            .map_err(sql_error)?
            .query_map(params![id, after, r.limit.get() + 1], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(sql_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql_error)?;
        let mut reader = match work {
            Some(work) => work.reader(&self.catalog)?,
            None => self.catalog.reader().map_err(catalog_error)?,
        };
        #[cfg(test)]
        self.hooks.gate(Command::CollectionsMembers, "hydrate");
        let mut items = Vec::new();
        for (reference, kind, row, born, grant) in rows.iter().take(r.limit.get() as usize) {
            if work.is_some_and(|w| self.past(w.deadline)) {
                return Err(fail(
                    ErrorCode::DeadlineExceeded,
                    "the read deadline expired",
                ));
            }
            let mut status = match grant.as_deref() {
                Some("revoked") => "root_revoked",
                Some(g) if g != "active" => "root_unavailable",
                _ => "unresolved",
            };
            let mut entry = Value::Null;
            if grant.as_deref() == Some("active") {
                if let (Some(kind), Some(row), Some(born)) = (kind, row, born) {
                    let key = if kind == "file" {
                        NodeKey::File(*row)
                    } else {
                        NodeKey::Dir(*row)
                    };
                    match reader.inspect(key) {
                        Ok(detail) => {
                            entry = serde_json::to_value(detail.row)
                                .map_err(|_| fail(ErrorCode::InternalError, "encode"))?;
                            entry["node_id"] = json!(self.ids.seal(key, *born, &self.instance));
                            entry["location_hint"] = serde_json::to_value(detail.display_path)
                                .map_err(|_| fail(ErrorCode::InternalError, "encode"))?;
                            status = "resolved";
                        }
                        Err(loomward_catalog::Error::NotFound) => {}
                        Err(e) => return Err(catalog_error(e)),
                    }
                }
            }
            items
                .push(json!({"member_id":format!("om_{reference}"),"status":status,"entry":entry}));
        }
        #[cfg(test)]
        self.hooks.gate(Command::CollectionsMembers, "hydrated");
        // The row projection uses the catalogue reader: reject a concurrent publication rather than mix snapshots.
        if self.revisions()? != revs {
            return Err(with_reason(
                fail(
                    ErrorCode::StaleGeneration,
                    "collection changed while reading",
                ),
                "revision_changed",
            ));
        }
        let next = if rows.len() > r.limit.get() as usize {
            Some(self.cursors.issue(
                Command::CollectionsMembers,
                bind,
                Position::Members {
                    after: rows[r.limit.get() as usize - 1].0,
                    catalog_rev: revs.0,
                },
                Some(revs.1),
            ))
        } else {
            None
        };
        Ok(out(
            json!({"collection_id":r.collection_id,"items":items,"next_cursor":next,"total":c["member_count"],"budget_hit":false}),
            revs,
        ))
    }
}
