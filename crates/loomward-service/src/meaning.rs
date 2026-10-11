use crate::{catalog_error, fail, learning::features, Inner, Out, ReadWork};
use loomward_catalog::NodeKey;
use loomward_engine::learning::MEANING_WORK_BUDGET;
use loomward_protocol::*;
use std::time::{Duration, Instant};

impl Inner {
    pub(crate) fn meaning(
        &self,
        r: MeaningRequest,
        work: Option<&ReadWork>,
    ) -> Result<Out, ErrorBody> {
        self.learning_gate()?;
        let revs = self.revisions()?;
        let deadline = work.map_or_else(|| Instant::now() + Duration::from_secs(2), |w| w.deadline);
        let mut reader = match work {
            Some(w) => w.reader(&self.catalog)?,
            None => self.catalog.reader().map_err(catalog_error)?,
        };
        let mut candidates = Vec::new();
        for id in r.node_ids.iter().take(MEANING_WORK_BUDGET) {
            if Instant::now() >= deadline {
                return Err(fail(
                    ErrorCode::DeadlineExceeded,
                    "meaning read deadline expired",
                ));
            }
            // Folded slice nodes name no object, but remain explicit in the meaning projection.
            let (key, _) = self
                .ids
                .open(id, &self.instance)
                .map_err(|e| crate::id_refusal(e, "unknown node"))?;
            let feature = if matches!(key, NodeKey::Other(_)) {
                None
            } else {
                let key = self.node(id, &mut reader)?;
                if matches!(key, NodeKey::File(_)) {
                    let detail = reader.inspect(key).map_err(catalog_error)?;
                    if detail.identity.durable_reference == "available"
                        && detail.exclusions.is_empty()
                    {
                        Some(features(&mut reader, key)?.1)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            candidates.push((id.clone(), feature));
        }
        drop(reader);
        let result = self
            .engine
            .learning_meaning(
                candidates,
                revs.1,
                deadline,
                r.node_ids.len() > MEANING_WORK_BUDGET,
            )
            .map_err(ErrorBody::from)?;
        if self.revisions()? != revs {
            return Err(fail(
                ErrorCode::StaleGeneration,
                "catalogue or learning changed during meaning read",
            ));
        }
        Ok(Out {
            result: serde_json::to_value(result)
                .map_err(|_| fail(ErrorCode::InternalError, "meaning result"))?,
            catalog_rev: Some(revs.0),
            state_rev: Some(revs.1),
        })
    }
}
