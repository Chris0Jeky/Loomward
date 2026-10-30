use crate::{
    catalog_error,
    cursors::{Position, Resume},
    db, fail, sql_error, Inner, Out, ReadWork,
};
use loomward_catalog::NodeKey;
use loomward_protocol::*;
use serde_json::json;

impl Inner {
    pub(crate) fn projects_find(
        &self,
        req: &RequestEnvelope,
        work: Option<&ReadWork>,
    ) -> Result<Out, ErrorBody> {
        let r: ProjectsRequest = req.decode_payload()?;
        let root = self.root_row(&r.root_id)?;
        let revs = self.revisions()?;
        let (dir, generation) = {
            let conn = self.db();
            (
                db::active_root_dir(&conn, root)
                    .map_err(sql_error)?
                    .ok_or_else(|| fail(ErrorCode::NotFound, "unknown root"))?,
                db::root_generation(&conn, root).map_err(sql_error)?,
            )
        };
        let bind = json!({"root_id":r.root_id}).to_string();
        let after = match r.cursor.as_ref().map(|c| {
            self.cursors
                .resume(c, Command::ProjectsFind, &bind, Some(revs.1))
        }) {
            None => 0,
            Some(Ok(Position::Projects {
                after,
                catalog_rev,
                generation: g,
            })) if catalog_rev == revs.0 && g == generation => after,
            Some(Err(Resume::Mismatch)) => {
                return Err(fail(
                    ErrorCode::InvalidRequest,
                    "cursor belongs to another request",
                ))
            }
            _ => {
                return Err(fail(
                    ErrorCode::StaleGeneration,
                    "project results changed; find projects again",
                ))
            }
        };
        let mut reader = self.read_reader(work)?;
        if let Some(g) = req.expected_generation {
            reader.expect_generation(NodeKey::Dir(dir), g.get());
        }
        let stale = self.freshness.stale(root);
        let mut value = json!({"root_id":r.root_id,"generation":generation.map(|g|g.to_string()),
            "coverage":"unknown","partial":true,"budget_hit":false,"candidates":[],"next_cursor":null});
        match reader.projects(root, r.limit.get() as usize, after) {
            Ok(page) => {
                if page.catalog_rev != revs.0 || page.generation != generation {
                    return Err(fail(
                        ErrorCode::StaleGeneration,
                        "catalogue changed; find projects again",
                    ));
                }
                value["coverage"] = (if stale {
                    "stale"
                } else if page.partial && page.coverage == "complete" {
                    "partial"
                } else {
                    crate::clutter::coverage(&page.coverage)
                })
                .into();
                value["partial"] = (page.partial || stale).into();
                value["candidates"] = page.candidates.into_iter().map(|c| json!({
                    "node_id":self.ids.seal(c.directory,c.born_run,&self.instance),"name":c.name,"path":c.path,
                    "coverage":if stale { "stale" } else { crate::clutter::coverage(&c.coverage) },
                    "markers":c.markers.into_iter().map(|(name,provider)|json!({"name":name,"provider":provider})).collect::<Vec<_>>(),
                    "markers_truncated":c.markers_truncated,
                })).collect();
                if let Some(after) = page.next_after {
                    value["next_cursor"] = self
                        .cursors
                        .issue(
                            Command::ProjectsFind,
                            bind,
                            Position::Projects {
                                after,
                                catalog_rev: revs.0,
                                generation,
                            },
                            Some(revs.1),
                        )
                        .into();
                }
            }
            Err(loomward_catalog::Error::ResourceBudget) => {
                value["budget_hit"] = true.into();
            }
            Err(e) => return Err(catalog_error(e)),
        }
        if self.revisions()? != revs || self.freshness.stale(root) != stale {
            return Err(fail(
                ErrorCode::StaleGeneration,
                "project observations changed; find projects again",
            ));
        }
        Ok(Out {
            result: value,
            catalog_rev: Some(revs.0),
            state_rev: Some(revs.1),
        })
    }
}
