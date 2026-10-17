use super::{decode, learned, Store};
use crate::{Engine, EngineError, EngineResult};
use loomward_protocol::*;
use serde_json::{json, Value};
use std::time::Instant;

/// Work budget includes catalogue feature extraction in the service, not just predictions.
pub const WORK_BUDGET: usize = 1000;

impl Engine {
    /// Read the stored candidate once. No training, promotion, feedback or object references.
    pub fn learning_meaning(
        &self,
        candidates: Vec<(NodeId, Option<Value>)>,
        state_rev: i64,
        deadline: Instant,
        budget_hit: bool,
    ) -> EngineResult<MeaningResult> {
        if candidates.len() > WORK_BUDGET {
            return Err(EngineError::ResourceBudget {
                message: "meaning work budget is 1000".into(),
            });
        }
        let store = self.learning_store()?;
        let db = store.db.lock().unwrap();
        Store::redact(&db)?;
        let state: i64 = db
            .query_row("SELECT state_rev FROM st.revision", [], |r| r.get(0))
            .map_err(super::internal)?;
        if state != state_rev {
            return Err(EngineError::StaleGeneration {
                message: "learning state changed".into(),
                detail: None,
            });
        }
        let (student, fitted) = Store::student(&db)?;
        drop(db);
        let model_id = fitted.map(|_| format!("md_{}", student.model_id()));
        let mut items = Vec::with_capacity(candidates.len());
        for (node_id, features) in candidates {
            if Instant::now() >= deadline {
                return Err(EngineError::DeadlineExceeded {
                    message: "meaning read deadline expired".into(),
                });
            }
            let mut meaning = json!({"state":"not_eligible","label":null,"share":null,"source":null,"collection_ids":[]});
            if let Some(features) = features {
                if model_id.is_some() {
                    let p = student.predict(&features).map_err(learned)?;
                    meaning["state"] = json!(if p.abstain { "abstained" } else { "suggested" });
                    meaning["source"] = json!("student");
                    if !p.abstain {
                        meaning["label"] = json!(p.suggestions[0].label);
                        meaning["share"] = json!(p.suggestions[0].score);
                    }
                    meaning["suggestion"] = json!({"node_id":node_id,"model_id":model_id,"ranked":p.to_value()["suggestions"],"abstain":p.abstain,"reasons":p.reasons,"feature_coverage":p.feature_coverage,"review_priority":p.review_priority,"calibrated":false,"autonomy_allowed":false,"teacher":null});
                } else {
                    meaning["state"] = json!("no_model");
                }
            }
            items.push(json!({"node_id":node_id,"meaning":meaning}));
        }
        decode(json!({"model_id":model_id,"items":items,"budget_hit":budget_hit}))
    }
}
