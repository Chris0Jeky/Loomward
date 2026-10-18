//! Learning integration: human feedback, the review queue and refit jobs.
//!
//! Owner: lane L14 (LW-021, LW-026, LW-110), over `loomward-learn`. Wave 3.
//!
//! Scope (docs/41 section 9.1): a withdrawn label on retraction, unique `(object_ref, revision)`
//! per source, durable `client_event_id` replay and conflict, labels only on durable references,
//! the refit job. A learned score is never an approval; data classes (human, teacher) stay
//! separate (invariant 3).
//!
//! Personal sessions learn locally from the owner's human labels (HUMAN_TODO q-11): the store is
//! the session's own `state.db`, whose recorded dataset class must match the engine's, so personal
//! and synthetic evidence never mix. Revoking a root redacts its labels' features and withdraws
//! them from training; the teacher stays synthetic-only (`teacher_personal_gated`).
//!
//! #117 errata items this lane applies, each with its test before merge:
//! - Partial #15 state-backed cursors: `learning.queue` cursors also bind a durable
//!   `state_rev`, since feedback changes `state.db` while `catalog_rev` stays put.
//! - N4 recovery (owned by L1 and L8, relied on here): `feedback.record` is safe to re-send only
//!   through its durable `client_event_id`; an identical replay returns the original result with
//!   `idempotent_replay: true`, a different payload under the same key is `invalid_request`.
use crate::{jobs::JobSpec, Component, Engine, EngineError, EngineResult};
mod meaning;
use loomward_learn::student::{Student, DEFAULT_LABELS};
use loomward_protocol::*;
pub use meaning::WORK_BUDGET as MEANING_WORK_BUDGET;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

/// Durable human-only learning store installed by the service after catalogue recovery.
pub struct Store {
    db: Mutex<Connection>,
    cursors: Mutex<HashMap<String, QueuePosition>>,
    dataset: String,
}

/// Feature snapshot left on a human event once its root's grant is no longer active.
const REDACTED: &str = r#"{"name":"","extension":"","context":"","size_bytes":0}"#;
type QueuePosition = (Option<RootId>, (i64, i64), usize);

fn internal(_: impl std::fmt::Display) -> EngineError {
    EngineError::Internal {
        message: "learning store unavailable".into(),
        detail: None,
    }
}
fn invalid(message: &str) -> EngineError {
    EngineError::InvalidRequest {
        message: message.into(),
    }
}
fn learned(e: loomward_learn::LearnError) -> EngineError {
    EngineError::ResourceBudget {
        message: e.to_string(),
    }
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> EngineResult<T> {
    serde_json::from_value(value).map_err(internal)
}
fn fingerprint(request: &FeedbackRequest) -> String {
    format!(
        "{:x}",
        Sha256::digest(loomward_learn::student::canonical_json(&json!(request)).as_bytes())
    )
}

impl Store {
    /// Opens only Loomward's recovered stores; no user file is opened.
    pub fn open(dir: &Path) -> EngineResult<Arc<Self>> {
        let dir = &loomward_catalog::sqlite_dir(dir);
        let db = Connection::open(dir.join("catalog.db")).map_err(internal)?;
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(internal)?;
        db.execute(
            "ATTACH DATABASE ?1 AS st",
            [dir.join("state.db").to_string_lossy().as_ref()],
        )
        .map_err(internal)?;
        db.execute_batch("PRAGMA foreign_keys=ON;PRAGMA st.synchronous=FULL;PRAGMA st.secure_delete=ON;
            CREATE TABLE IF NOT EXISTS st.learning_receipt(client_event_id TEXT PRIMARY KEY, result_json TEXT NOT NULL) STRICT;
            CREATE TABLE IF NOT EXISTS st.learning_node(node_id TEXT PRIMARY KEY, object_ref_id INTEGER NOT NULL REFERENCES object_ref(id)) STRICT;
            CREATE TABLE IF NOT EXISTS st.learning_redacted(event_id TEXT PRIMARY KEY) STRICT;
            CREATE TABLE IF NOT EXISTS st.learning_feature_grant(event_id TEXT PRIMARY KEY REFERENCES human_feedback(event_id) ON DELETE CASCADE, grant_id INTEGER NOT NULL REFERENCES root_grant(id)) STRICT;
            CREATE TEMP VIEW learning_resolved AS SELECT o.id FROM st.object_ref o
            LEFT JOIN st.pending_reference p ON p.object_ref_id=o.id
              AND p.catalog_instance=(SELECT value FROM main.meta WHERE key='instance_id')
              AND p.publication_token=(SELECT value FROM main.meta WHERE key='reference_publication')
            WHERE o.state='resolved' AND o.retired=0 AND (p.object_ref_id IS NULL OR p.row_id IS NOT NULL);").map_err(internal)?;
        db.execute(
            "INSERT OR IGNORE INTO st.taxonomy VALUES(1,?1,0)",
            [json!(DEFAULT_LABELS).to_string()],
        )
        .map_err(internal)?;
        let dataset: String = db
            .query_row(
                "SELECT value FROM st.meta WHERE key='dataset_class'",
                [],
                |r| r.get(0),
            )
            .map_err(internal)?;
        Self::redact(&db)?;
        Ok(Arc::new(Self {
            db: Mutex::new(db),
            cursors: Mutex::new(HashMap::new()),
            dataset,
        }))
    }

    /// Redacts the features of every human event captured under a grant that is no
    /// longer active, withdraws those events from training and deletes every stored model (a
    /// model's state carries name tokens). Idempotent; true when anything changed.
    fn redact(db: &Connection) -> EngineResult<bool> {
        let tx = db.unchecked_transaction().map_err(internal)?;
        let marked = tx.execute("INSERT OR IGNORE INTO st.learning_redacted SELECT p.event_id FROM st.learning_feature_grant p JOIN st.root_grant g ON g.id=p.grant_id WHERE g.state!='active'", []).map_err(internal)?;
        // Legacy snapshots lack original provenance. Keep their best-effort sweep pending the
        // owner's migration decision; a current binding cannot reconstruct a snapshot's origin.
        let legacy = tx.execute("INSERT OR IGNORE INTO st.learning_redacted SELECT h.event_id FROM st.human_feedback h JOIN st.object_ref o ON o.id=h.object_ref_id
            WHERE NOT EXISTS(SELECT 1 FROM st.learning_feature_grant p WHERE p.event_id=h.event_id) AND o.catalog_instance=(SELECT value FROM main.meta WHERE key='instance_id') AND EXISTS(
              SELECT 1 FROM main.dir d JOIN main.root rt ON rt.id=d.root_id JOIN st.root_grant g ON g.id=rt.grant_id
              WHERE g.state!='active' AND ((o.kind='dir' AND d.id=o.row_id) OR (o.kind='file' AND d.id=(SELECT dir_id FROM main.file WHERE id=o.row_id))))", []).map_err(internal)?;
        let changed = tx
            .execute(
                "UPDATE st.human_feedback SET features_json=?1 WHERE features_json!=?1 AND event_id IN (SELECT event_id FROM st.learning_redacted)",
                [REDACTED],
            )
            .map_err(internal)?;
        if changed + marked + legacy > 0 {
            tx.execute("DELETE FROM st.student_model", [])
                .map_err(internal)?;
            tx.execute("UPDATE st.revision SET state_rev=state_rev+1", [])
                .map_err(internal)?;
        }
        tx.commit().map_err(internal)?;
        Ok(changed + marked + legacy > 0)
    }

    fn student(db: &Connection) -> EngineResult<(Student, Option<String>)> {
        // New fits bind a monotonic feedback ID; legacy cache rows retain their timestamp guard.
        let saved: Option<(String,i64)> = db.query_row("SELECT state_json,fitted_at_ns FROM st.student_model WHERE status='candidate' AND training_count>0 AND CASE WHEN json_type(state_json,'$.human_feedback_id')='integer' THEN json_extract(state_json,'$.human_feedback_id')=coalesce((SELECT max(id) FROM st.human_feedback),0) ELSE fitted_at_ns>=coalesce((SELECT max(recorded_at_ns) FROM st.human_feedback),0) END ORDER BY fitted_at_ns DESC,id DESC LIMIT 1", [], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(internal)?;
        match saved {
            Some((s, at)) => Ok((
                Student::from_value(&serde_json::from_str(&s).map_err(internal)?)
                    .map_err(learned)?,
                Some(crate::events::timestamp_ns(at).to_string()),
            )),
            None => Ok((Student::new(&Value::Null).map_err(learned)?, None)),
        }
    }

    fn status(db: &Connection, dataset: DatasetClass) -> EngineResult<LearningStatus> {
        let (student, fitted) = Self::student(db)?;
        let human: i64 = db
            .query_row("SELECT count(*) FROM st.human_feedback", [], |r| r.get(0))
            .map_err(internal)?;
        let teacher: i64 = db
            .query_row("SELECT count(*) FROM st.teacher_label", [], |r| r.get(0))
            .map_err(internal)?;
        let unresolved: i64 = db.query_row("SELECT count(*) FROM st.human_feedback WHERE object_ref_id NOT IN (SELECT id FROM learning_resolved)", [], |r|r.get(0)).map_err(internal)?;
        decode(
            json!({"model_id":fitted.as_ref().map(|_|format!("md_{}",student.model_id())),"algorithm":"weighted_multinomial_nb_v1","taxonomy_version":1,"training_count":student.training_count(),"human_events":human,"teacher_labels":teacher,"human_support":student.labels().iter().zip(student.human_support()).collect::<std::collections::BTreeMap<_,_>>(),"fitted_at":fitted,"dataset_class":dataset,"calibrated":false,"autonomy_allowed":false,"gate":null,"unresolved_human_events":unresolved}),
        )
    }

    fn replay(db: &Connection, r: &FeedbackRequest) -> EngineResult<Option<FeedbackResult>> {
        let saved: Option<(String,String)> = db.query_row("SELECT h.content_digest,r.result_json FROM st.human_feedback h JOIN st.learning_receipt r USING(client_event_id) WHERE h.client_event_id=?1", [r.client_event_id.as_str()], |row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(internal)?;
        match saved {
            Some((content, result)) => {
                if content != fingerprint(r) {
                    return Err(invalid("client_event_id was used for different content"));
                }
                let mut result: FeedbackResult = serde_json::from_str(&result).map_err(internal)?;
                result.idempotent_replay = true;
                Ok(Some(result))
            }
            None => Ok(None),
        }
    }
}

impl Engine {
    /// Installs the recovered store once. Does not open any user path.
    pub fn install_learning(&self, store: Arc<Store>) {
        *self.learning.lock().unwrap() = Some(store);
    }

    /// The installed store, refused unless its recorded dataset class is this engine's: a
    /// personal session never trains on synthetic labels, nor a synthetic one on personal labels.
    fn learning_store(&self) -> EngineResult<Arc<Store>> {
        let store = self
            .learning
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| EngineError::unavailable(Component::Learning))?;
        let expected = match self.config.dataset_class {
            DatasetClass::Synthetic => "synthetic",
            DatasetClass::Personal => "personal",
        };
        if store.dataset != expected {
            return Err(EngineError::PermissionDenied {
                message: "dataset_class_mismatch".into(),
            });
        }
        Ok(store)
    }
    /// Revocation sweep (see [`Store::redact`]); the service calls it after a grant stops being active.
    pub fn learning_redact_revoked(&self) -> EngineResult<()> {
        let store = self.learning_store()?;
        let changed = Store::redact(&store.db.lock().unwrap())?;
        if changed {
            self.learning_updated()?;
        }
        Ok(())
    }
    /// Durable replay precedes session-local node resolution, including after restart.
    pub fn learning_replay(&self, r: &FeedbackRequest) -> EngineResult<Option<FeedbackResult>> {
        Store::replay(&self.learning_store()?.db.lock().unwrap(), r)
    }

    /// Retrieve the prior human event without resolving or reading the current node.
    pub fn learning_prior(&self, node: &NodeId) -> EngineResult<Option<(i64, Value)>> {
        let store = self.learning_store()?;
        let db = store.db.lock().unwrap();
        Store::redact(&db)?;
        let prior: Option<(i64, String)> = db.query_row("SELECT h.object_ref_id,h.features_json FROM st.learning_node n JOIN st.human_feedback h ON h.object_ref_id=n.object_ref_id WHERE n.node_id=?1 ORDER BY h.revision DESC LIMIT 1", [node.as_str()], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(internal)?;
        prior
            .map(|(reference, features)| {
                Ok((
                    reference,
                    serde_json::from_str(&features).map_err(internal)?,
                ))
            })
            .transpose()
    }

    /// Validate label semantics before the service creates a durable reference.
    pub fn learning_validate_feedback(&self, r: &FeedbackRequest) -> EngineResult<()> {
        if r.retract != r.label.is_none() {
            return Err(invalid("label must be null exactly when retract is true"));
        }
        if r.label
            .as_ref()
            .is_some_and(|label| !DEFAULT_LABELS.contains(&label.as_str()))
        {
            return Err(invalid("label is outside the taxonomy"));
        }
        Ok(())
    }

    /// Appends one human event against a resolved durable object and a bounded feature snapshot.
    pub fn learning_feedback(
        &self,
        r: &FeedbackRequest,
        object_ref: i64,
        features: &Value,
        feature_grant: Option<i64>,
    ) -> EngineResult<FeedbackResult> {
        let store = self.learning_store()?;
        let mut db = store.db.lock().unwrap();
        Store::redact(&db)?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(internal)?;
        if let Some(replay) = Store::replay(&tx, r)? {
            return Ok(replay);
        }
        self.learning_validate_feedback(r)?;
        loomward_learn::features::validate_features(features)
            .map_err(|_| invalid("invalid learning features"))?;
        let resolved: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM learning_resolved WHERE id=?1)",
                [object_ref],
                |row| row.get(0),
            )
            .map_err(internal)?;
        if !r.retract && !resolved {
            return Err(invalid("labels require a resolved durable reference"));
        }
        let prior: Option<(i64,String,String,Option<i64>)> = tx.query_row("SELECT h.revision,h.label,h.features_json,p.grant_id FROM st.human_feedback h LEFT JOIN st.learning_feature_grant p ON p.event_id=h.event_id WHERE h.object_ref_id=?1 ORDER BY h.revision DESC LIMIT 1",[object_ref],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional().map_err(internal)?;
        let label = if r.retract {
            prior
                .as_ref()
                .map(|p| p.1.clone())
                .ok_or_else(|| invalid("no prior human label to retract"))?
        } else {
            r.label.as_ref().unwrap().as_str().to_owned()
        };
        if !DEFAULT_LABELS.contains(&label.as_str()) {
            return Err(invalid("label is outside the taxonomy"));
        }
        let grant: Option<i64> = if r.retract {
            prior.as_ref().and_then(|p| p.3)
        } else {
            let active: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM st.root_grant WHERE id=?1 AND state='active')",
                    [feature_grant],
                    |row| row.get(0),
                )
                .map_err(internal)?;
            if !active {
                return Err(invalid("labels require an active feature grant"));
            }
            feature_grant
        };
        let features_json = if r.retract {
            prior.as_ref().unwrap().2.clone()
        } else {
            features.to_string()
        };
        let revision = prior.as_ref().map_or(1, |p| p.0 + 1);
        if revision > 1_000_000_000 {
            return Err(invalid("feedback revision exhausted"));
        }
        let event = crate::events::random_id("fe_");
        tx.execute("INSERT INTO st.human_feedback(event_id,client_event_id,content_digest,object_ref_id,revision,label,retracted,taxonomy_version,features_json,recorded_at_ns) VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8,?9)",params![event,r.client_event_id.as_str(),fingerprint(r),object_ref,revision,label,r.retract,features_json,now_ns()]).map_err(internal)?;
        if let Some(grant) = grant {
            tx.execute(
                "INSERT INTO st.learning_feature_grant VALUES(?1,?2)",
                params![event, grant],
            )
            .map_err(internal)?;
        }
        tx.execute("INSERT INTO st.learning_node VALUES(?1,?2) ON CONFLICT(node_id) DO UPDATE SET object_ref_id=excluded.object_ref_id", params![r.node_id.as_str(), object_ref]).map_err(internal)?;
        tx.execute(
            "UPDATE st.student_model SET status='retired' WHERE status='candidate'",
            [],
        )
        .map_err(internal)?;
        tx.execute("UPDATE st.revision SET state_rev=state_rev+1", [])
            .map_err(internal)?;
        let result: FeedbackResult = decode(
            json!({"event_id":event,"revision":revision,"idempotent_replay":false,"data_class":"human","learning":Store::status(&tx,self.config.dataset_class)?}),
        )?;
        tx.execute(
            "INSERT INTO st.learning_receipt VALUES(?1,?2)",
            params![r.client_event_id.as_str(), json!(result).to_string()],
        )
        .map_err(internal)?;
        tx.commit().map_err(internal)?;
        drop(db);
        self.learning_updated()?;
        Ok(result)
    }
    /// Human/teacher counts remain distinct; a candidate is advice, never a promoted model.
    pub fn learning_status(&self) -> EngineResult<LearningStatus> {
        let store = self.learning_store()?;
        let db = store.db.lock().unwrap();
        Store::redact(&db)?;
        Store::status(&db, self.config.dataset_class)
    }
    fn learning_updated(&self) -> EngineResult<()> {
        self.publish_event(
            EventName::LearningUpdated,
            json!({"status":self.learning_status()?})
                .as_object()
                .unwrap()
                .clone(),
            None,
            None,
        )?;
        Ok(())
    }

    /// Bounded active-review queue. The service supplies catalogue rows and revisions from its read.
    pub fn learning_queue(
        &self,
        r: &LearningQueueRequest,
        candidates: Vec<(EntryRow, Value)>,
        revisions: (i64, i64),
    ) -> EngineResult<LearningQueue> {
        let store = self.learning_store()?;
        let db = store.db.lock().unwrap();
        Store::redact(&db)?;
        let (student, fitted) = Store::student(&db)?;
        let model_id = fitted.map(|_| format!("md_{}", student.model_id()));
        let state: i64 = db
            .query_row("SELECT state_rev FROM st.revision", [], |row| row.get(0))
            .map_err(internal)?;
        if state != revisions.1 {
            return Err(EngineError::StaleGeneration {
                message: "learning state changed".into(),
                detail: None,
            });
        }
        let mut cursors = store.cursors.lock().unwrap();
        let offset = match &r.cursor {
            Some(c) => {
                let (root, revs, offset) =
                    cursors
                        .get(c.as_str())
                        .ok_or_else(|| EngineError::StaleGeneration {
                            message: "learning cursor expired".into(),
                            detail: None,
                        })?;
                if root != &r.root_id {
                    return Err(invalid("learning cursor filters changed"));
                }
                if *revs != revisions {
                    return Err(EngineError::StaleGeneration {
                        message: "learning cursor revisions changed".into(),
                        detail: None,
                    });
                }
                *offset
            }
            None => 0,
        };
        let mut items = Vec::new();
        for (row, features) in candidates {
            let prediction = student.predict(&features).map_err(learned)?;
            let suggestion = decode(
                json!({"node_id":row.node_id,"model_id":model_id,"ranked":prediction.to_value()["suggestions"],"abstain":prediction.abstain,"reasons":prediction.reasons,"feature_coverage":prediction.feature_coverage,"review_priority":prediction.review_priority,"calibrated":false,"autonomy_allowed":false,"teacher":null}),
            )?;
            items.push(LearningQueueItem { row, suggestion });
        }
        items.sort_by(|a, b| {
            b.suggestion
                .review_priority
                .get()
                .total_cmp(&a.suggestion.review_priority.get())
                .then_with(|| a.row.node_id.as_str().cmp(b.row.node_id.as_str()))
        });
        let end = (offset + r.limit.get() as usize).min(items.len());
        let next = if end < items.len() {
            // shortcut: bounded session cursors; restart paging after eviction.
            if cursors.len() >= 4096 {
                cursors.clear();
            }
            let c = crate::events::random_id("cu_");
            cursors.insert(c.clone(), (r.root_id.clone(), revisions, end));
            Some(Cursor::new(c).map_err(internal)?)
        } else {
            None
        };
        decode(
            json!({"model_id":model_id,"items":items.into_iter().skip(offset).take(end.saturating_sub(offset)).collect::<Vec<_>>(),"next_cursor":next}),
        )
    }

    /// Starts a cancellable human-only refit job; the registry excludes concurrent refits.
    pub fn learning_refit(&self) -> EngineResult<Job> {
        let store = self.learning_store()?;
        let events = self.events.clone();
        let dataset = self.config.dataset_class;
        let runner = Arc::new(move |ctx: crate::jobs::JobContext, _: JobSpec| {
            if ctx.is_cancelled() {
                return Ok(CoverageState::Cancelled);
            }
            let (events_data, rev) = {
                let db = store.db.lock().unwrap();
                Store::redact(&db)?;
                let tx = db.unchecked_transaction().map_err(internal)?;
                let rev: i64 = tx
                    .query_row("SELECT state_rev FROM st.revision", [], |r| r.get(0))
                    .map_err(internal)?;
                let mut stmt=tx.prepare("SELECT h.event_id,h.object_ref_id,h.revision,h.label,h.retracted,h.features_json FROM st.human_feedback h WHERE h.object_ref_id IN (SELECT id FROM learning_resolved) AND h.retracted=0 AND h.event_id NOT IN (SELECT event_id FROM st.learning_redacted) AND NOT EXISTS(SELECT 1 FROM st.human_feedback newer WHERE newer.object_ref_id=h.object_ref_id AND newer.revision>h.revision) ORDER BY h.id DESC LIMIT 10000").map_err(internal)?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, bool>(4)?,
                            r.get::<_, String>(5)?,
                        ))
                    })
                    .map_err(internal)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(internal)?;
                let mut data = Vec::new();
                for (event, item, revision, label, retracted, features) in rows {
                    if ctx.is_cancelled() {
                        return Ok(CoverageState::Cancelled);
                    }
                    data.push(json!({"event_id":event,"item_id":item.to_string(),"revision":revision,"label":label,"retracted":retracted,"source":"human","features":serde_json::from_str::<Value>(&features).map_err(internal)?}));
                }
                (Value::Array(data), rev)
            };
            #[cfg(test)]
            if let Some(hook) = REFIT_HOOK.lock().unwrap().clone() {
                hook(&ctx, &store.db.lock().unwrap(), RefitPoint::Snapshot);
            }
            let mut student = Student::new(&Value::Null).map_err(learned)?;
            student.fit(&events_data).map_err(learned)?;
            let mut db = store.db.lock().unwrap();
            let tx = db
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(internal)?;
            if ctx.is_cancelled() {
                return Ok(CoverageState::Cancelled);
            }
            let current: i64 = tx
                .query_row("SELECT state_rev FROM st.revision", [], |r| r.get(0))
                .map_err(internal)?;
            if current != rev {
                return Err(EngineError::StaleGeneration {
                    message: "feedback changed during refit; retry".into(),
                    detail: None,
                });
            }
            tx.execute(
                "UPDATE st.student_model SET status='retired' WHERE status='candidate'",
                [],
            )
            .map_err(internal)?;
            if student.training_count() > 0 {
                let feedback_id: i64 = tx
                    .query_row(
                        "SELECT coalesce(max(id),0) FROM st.human_feedback",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(internal)?;
                let mut model = student.to_value();
                model["human_feedback_id"] = json!(feedback_id);
                tx.execute("INSERT INTO st.student_model VALUES(?1,'weighted_multinomial_nb_v1',1,?2,?3,?4,'candidate',?5) ON CONFLICT(id) DO UPDATE SET fitted_at_ns=excluded.fitted_at_ns,state_json=excluded.state_json,status='candidate'",params![format!("md_{}",student.model_id()),student.model_id(),student.training_count() as i64,model.to_string(),now_ns()]).map_err(internal)?;
            }
            tx.execute("UPDATE st.revision SET state_rev=state_rev+1", [])
                .map_err(internal)?;
            // Capture the announcement in the commit's snapshot, before another read can fail.
            let status = Store::status(&tx, dataset);
            #[cfg(test)]
            if let Some(hook) = REFIT_HOOK.lock().unwrap().clone() {
                hook(&ctx, &tx, RefitPoint::Commit);
            }
            if !ctx.commit_if_not_cancelled(|| tx.commit().map_err(internal))? {
                return Ok(CoverageState::Cancelled);
            }
            if let Ok(status) = status {
                if events
                    .publish(
                        EventName::LearningUpdated,
                        json!({"status":status}).as_object().unwrap().clone(),
                        None,
                        None,
                    )
                    .is_err()
                {
                    eprintln!("learning refit committed; learning.updated publication failed");
                }
            } else {
                eprintln!("learning refit committed; status unavailable for learning.updated");
            }
            Ok(CoverageState::Complete)
        });
        self.jobs.submit(
            JobSpec::new(JobKind::Refit, None),
            runner,
            self.events.clone(),
            None,
        )
    }
}
fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
type RefitHook = Arc<dyn Fn(&crate::jobs::JobContext, &Connection, RefitPoint) + Send + Sync>;
#[cfg(test)]
#[derive(PartialEq)]
enum RefitPoint {
    Snapshot,
    Commit,
}
#[cfg(test)]
static REFIT_HOOK: Mutex<Option<RefitHook>> = Mutex::new(None);
#[cfg(test)]
static REFIT_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        let catalog = loomward_catalog::Catalog::open(dir.path(), "synthetic").unwrap();
        catalog
            .writer()
            .call(loomward_catalog::WriteCommand::RegisterRoot(
                loomward_catalog::RootObservation {
                    volume_guid: None,
                    volume_key: "fixture".into(),
                    display_name: "Synthetic".into(),
                    display_path: "Synthetic root".into(),
                    root_file_id: Some(vec![1; 16]),
                    filesystem: Some("NTFS".into()),
                    origin: "fixture".into(),
                    granted_via: "fixture".into(),
                    observed_at_ns: 0,
                },
            ))
            .unwrap();
        let engine = Engine::open(crate::EngineConfig::new(
            dir.path().into(),
            DatasetClass::Synthetic,
        ))
        .unwrap();
        let store = Store::open(dir.path()).unwrap();
        store.db.lock().unwrap().execute_batch("INSERT INTO st.object_ref(volume_key,file_id,creation_ft,kind,state,retired,catalog_instance,row_id,born_run) SELECT 'fixture',cast('1111111111111111' AS BLOB),1,'dir','resolved',0,value,1,1 FROM main.meta WHERE key='instance_id';
            INSERT INTO st.human_feedback(event_id,client_event_id,content_digest,object_ref_id,revision,label,retracted,taxonomy_version,features_json,recorded_at_ns) VALUES('fe_fixture','ce_fixture','fixture',1,1,'Documents',0,1,'{\"name\":\"fixture.txt\",\"extension\":\"txt\",\"context\":\"fixture\",\"size_bytes\":1}',0)").unwrap();
        engine.install_learning(store);
        (dir, engine)
    }
    #[test]
    fn learning_queue_model_ids_match_status_before_and_after_fit_and_feedback() {
        let _serial = REFIT_TEST_LOCK.lock().unwrap();
        let (_dir, engine) = fixture();
        let example: Value = serde_json::from_str(include_str!(
            "../../../../contracts/v3/examples/commands/learning.queue.result.json"
        ))
        .unwrap();
        let row: EntryRow = serde_json::from_value(example["items"][0]["row"].clone()).unwrap();
        let features =
            json!({"name":"fixture.txt","extension":"txt","context":"fixture","size_bytes":1});
        let queue = || {
            let store = engine.learning_store().unwrap();
            let state = store
                .db
                .lock()
                .unwrap()
                .query_row("SELECT state_rev FROM st.revision", [], |r| r.get(0))
                .unwrap();
            json!(engine
                .learning_queue(
                    &decode(json!({"root_id":null,"limit":20,"cursor":null})).unwrap(),
                    vec![(row.clone(), features.clone())],
                    (1, state),
                )
                .unwrap())
        };
        let assert_cold = || {
            assert!(engine.learning_status().unwrap().model_id.is_none());
            let queue = queue();
            assert_eq!(queue["model_id"], Value::Null);
            let suggestion = &queue["items"][0]["suggestion"];
            assert_eq!(suggestion["model_id"], Value::Null);
            assert_eq!(suggestion["abstain"], true);
            assert!(suggestion["reasons"]
                .as_array()
                .unwrap()
                .contains(&json!("no_training_examples")));
        };
        assert_cold();

        let job = engine.learning_refit().unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let done = engine.job_status(&job.job_id).unwrap();
            if done.finished_at.is_some() {
                assert_eq!(done.state, JobState::Completed);
                break;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let status = json!(engine.learning_status().unwrap());
        assert!(status["model_id"].is_string());
        let fitted_queue = queue();
        assert_eq!(fitted_queue["model_id"], status["model_id"]);
        assert_eq!(
            fitted_queue["items"][0]["suggestion"]["model_id"],
            status["model_id"]
        );

        engine.learning_feedback(
            &decode(json!({"node_id":row.node_id,"label":"Finance","retract":false,"client_event_id":"ce_changed"})).unwrap(),
            1, &features, Some(1),
        ).unwrap();
        assert_cold();
    }

    /// #390: the store opens SQLite by the plain drive path, never `\\?\` (see `sqlite_dir`).
    #[test]
    fn learning_store_opens_by_plain_path() {
        let (dir, _engine) = fixture();
        let store = Store::open(&dir.path().canonicalize().unwrap()).unwrap();
        let db = store.db.lock().unwrap();
        let files: Vec<String> = db
            .prepare("SELECT file FROM pragma_database_list WHERE name IN ('main','st')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| !f.starts_with(r"\\")), "{files:?}");
    }
    /// #391: revocation redaction rewrites and deletes rows in `state.db`, so this connection must
    /// zero the cells it frees, as the catalogue's own connections do.
    #[test]
    fn learning_store_connection_zeroes_freed_state_cells() {
        let (dir, _engine) = fixture();
        let store = Store::open(&dir.path().canonicalize().unwrap()).unwrap();
        let secure: i64 = store
            .db
            .lock()
            .unwrap()
            .query_row("PRAGMA st.secure_delete", [], |r| r.get(0))
            .unwrap();
        assert_eq!(secure, 1, "the learning store must set st.secure_delete");
    }

    #[test]
    fn learning_refit_status_failure_announces_committed_candidate() {
        let _serial = REFIT_TEST_LOCK.lock().unwrap();
        let (_dir, engine) = fixture();
        *REFIT_HOOK.lock().unwrap() = Some(Arc::new(|_, tx, point| {
            if point == RefitPoint::Commit {
                tx.execute_batch("DROP TABLE st.teacher_label").unwrap();
            }
        }));
        let mut stream = engine.subscribe_events(None).unwrap();
        let job = engine.learning_refit().unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let done = loop {
            let done = engine.job_status(&job.job_id).unwrap();
            if done.finished_at.is_some() {
                break done;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        *REFIT_HOOK.lock().unwrap() = None;
        assert_eq!(done.state, JobState::Completed);
        assert!(engine.learning_status().is_err());
        assert_eq!(engine.learning_store().unwrap().db.lock().unwrap().query_row("SELECT count(*) FROM st.student_model WHERE status='candidate' AND training_count=1", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        let mut updates = Vec::new();
        while let RecvOutcome::Event(event) = stream.recv_timeout(std::time::Duration::ZERO) {
            if event.event == EventName::LearningUpdated {
                updates.push(event);
            }
        }
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].data["status"]["training_count"], 1);
        updates[0].validate().unwrap();
    }
    #[test]
    fn learning_legacy_candidate_stays_stale_until_counter_refit() {
        let _serial = REFIT_TEST_LOCK.lock().unwrap();
        let (_dir, engine) = fixture();
        let refit = || {
            let job = engine.learning_refit().unwrap();
            let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
            loop {
                let done = engine.job_status(&job.job_id).unwrap();
                if done.finished_at.is_some() {
                    assert_eq!(done.state, JobState::Completed);
                    break;
                }
                assert!(std::time::Instant::now() < until);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        };
        refit();
        let before = json!(engine.learning_status().unwrap());
        assert_eq!(before["training_count"], 1);
        // Pre-#351 stores can retain a candidate after later feedback. An unresolved event also
        // leaves the selected training data/model ID unchanged when the owner explicitly refits.
        engine.learning_store().unwrap().db.lock().unwrap().execute_batch(
            "UPDATE st.student_model SET state_json=json_remove(state_json,'$.human_feedback_id');
             INSERT INTO st.object_ref(volume_key,file_id,creation_ft,kind,state,retired) VALUES('fixture',cast('2222222222222222' AS BLOB),1,'file','unresolved',0);
             INSERT INTO st.human_feedback(event_id,client_event_id,content_digest,object_ref_id,revision,label,retracted,taxonomy_version,features_json,recorded_at_ns)
             SELECT 'fe_legacy_later','ce_legacy_later','fixture',2,1,'Finance',0,1,features_json,9223372036854775807 FROM st.human_feedback WHERE event_id='fe_fixture';
             UPDATE st.revision SET state_rev=state_rev+1;"
        ).unwrap();
        assert!(engine.learning_status().unwrap().model_id.is_none());
        refit();
        let after = json!(engine.learning_status().unwrap());
        assert_eq!(after["model_id"], before["model_id"]);
        assert_eq!(after["training_count"], 1);
        assert_eq!(after["unresolved_human_events"], 1);
    }
    #[test]
    fn learning_revocation_mid_refit_rejects_the_snapshot() {
        let _serial = REFIT_TEST_LOCK.lock().unwrap();
        let (_dir, engine) = fixture();
        *REFIT_HOOK.lock().unwrap() = Some(Arc::new(|_, db, point| {
            if point == RefitPoint::Snapshot {
                db.execute_batch("UPDATE st.root_grant SET state='revoked'; UPDATE st.revision SET state_rev=state_rev+1").unwrap();
                Store::redact(db).unwrap();
            }
        }));
        let job = engine.learning_refit().unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let done = loop {
            let done = engine.job_status(&job.job_id).unwrap();
            if done.finished_at.is_some() {
                break done;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        *REFIT_HOOK.lock().unwrap() = None;
        assert_eq!(done.state, JobState::Failed);
        assert_eq!(done.error.unwrap().code, ErrorCode::StaleGeneration);
        let store = engine.learning_store().unwrap();
        assert_eq!(
            store
                .db
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM st.student_model", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn learning_refits_cancel_at_commit_without_candidate_or_update() {
        let _serial = REFIT_TEST_LOCK.lock().unwrap();
        let (_dir, engine) = fixture();
        let (tx, rx) = std::sync::mpsc::channel();
        *REFIT_HOOK.lock().unwrap() = Some(Arc::new(move |ctx, _, point| {
            if point == RefitPoint::Commit {
                tx.send(()).unwrap();
                while !ctx.is_cancelled() {
                    std::thread::yield_now();
                }
            }
        }));
        let mut stream = engine.subscribe_events(None).unwrap();
        let job = engine.learning_refit().unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        let error = engine.learning_refit().unwrap_err();
        assert_eq!(error.code(), ErrorCode::Busy);
        assert_eq!(
            error.to_body().detail.unwrap().get("job_id"),
            Some(&json!(job.job_id))
        );
        engine.job_cancel(&job.job_id).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while engine.job_status(&job.job_id).unwrap().state != JobState::Cancelled {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        *REFIT_HOOK.lock().unwrap() = None;
        assert!(engine.learning_status().unwrap().model_id.is_none());
        let db = engine.learning_store().unwrap();
        assert_eq!(
            db.db
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM st.student_model", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        while let RecvOutcome::Event(event) = stream.recv_timeout(std::time::Duration::ZERO) {
            assert_ne!(event.event, EventName::LearningUpdated);
        }
        assert!(engine.job_status(&job.job_id).unwrap().rev.get() >= 2);
    }
}
