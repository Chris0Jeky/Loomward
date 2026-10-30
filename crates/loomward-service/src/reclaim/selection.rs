//! One bounded selected set. Only its aggregate and catalogue bindings survive the job.
use super::*;
use loomward_engine::placement::ReliefTally;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Binding {
    kind: String,
    id: i64,
    root: i64,
    grant: i64,
    generation: i64,
    incarnation: i64,
    subtree_rev: i64,
}

impl Binding {
    pub(super) fn node(&self) -> NodeKey {
        if self.kind == "dir" {
            NodeKey::Dir(self.id)
        } else {
            NodeKey::File(self.id)
        }
    }
}

pub(super) fn prepare(
    inner: &Inner,
    gathered: &Gathered<'_>,
    ids: &BoundedVec<NodeId, 1, 12>,
) -> Result<Vec<Binding>, ErrorBody> {
    let mut reader = inner.read_reader(None)?.without_repairs();
    let mut nodes = HashSet::new();
    for id in ids.iter() {
        if !nodes.insert(inner.node(id.as_str(), &mut reader)?) {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "Select each candidate only once.",
            ));
        }
    }
    // Every gathered detector candidate, before the plan ranks and truncates per group: ranking
    // depends on stored relief, so a displayed candidate need not be in an unranked top 200 (#1003).
    let planner = &gathered.planner;
    let candidates: HashSet<NodeKey> = planner
        .developer
        .iter()
        .flat_map(|r| r.candidates.iter().map(|c| c.directory))
        .chain(
            planner
                .machine
                .iter()
                .flat_map(|r| r.candidates.iter().map(|c| c.node)),
        )
        .collect();
    if nodes.iter().any(|n| !candidates.contains(n)) {
        return Err(fail(
            ErrorCode::InvalidRequest,
            "Select current candidates on this volume.",
        ));
    }
    let conn = &gathered.input.conn;
    let mut members = Vec::new();
    for node in &nodes {
        let (kind, id) = node_parts(*node);
        let (dir, incarnation): (i64, i64) = conn
            .query_row(
                if kind == "dir" {
                    "SELECT id,born_run FROM main.dir WHERE id=?1"
                } else {
                    "SELECT dir_id,born_run FROM main.file WHERE id=?1"
                },
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(sql_error)?;
        let chain = conn.prepare_cached(
            "WITH RECURSIVE a(id,parent_id,depth) AS (SELECT id,parent_id,0 FROM main.dir WHERE id=?1
             UNION ALL SELECT d.id,d.parent_id,a.depth+1 FROM main.dir d JOIN a ON d.id=a.parent_id WHERE a.depth<511)
             SELECT id,parent_id FROM a",
        ).map_err(sql_error)?.query_map([dir], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)))
            .map_err(sql_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(sql_error)?;
        if !chain.iter().any(|(_, parent)| parent.is_none()) {
            return Err(fail(
                ErrorCode::ResourceBudget,
                "Candidate ancestry is incomplete.",
            ));
        }
        if chain.iter().any(|(ancestor, _)| {
            NodeKey::Dir(*ancestor) != *node && nodes.contains(&NodeKey::Dir(*ancestor))
        }) {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "Select a candidate or its descendant, not both.",
            ));
        }
        let member = conn.query_row(
            "SELECT r.id,r.grant_id,coalesce(r.generation,-1),d.subtree_rev FROM main.dir d JOIN main.root r ON r.id=d.root_id WHERE d.id=?1 AND r.volume_id=?2",
            params![dir, gathered.volume_row], |r| Ok(Binding { kind: kind.into(), id, root: r.get(0)?, grant: r.get(1)?, generation: r.get(2)?, incarnation, subtree_rev: r.get(3)? }),
        ).map_err(sql_error)?;
        members.push(member);
    }
    members.sort_by_key(|m| (m.kind.clone(), m.id));
    Ok(members)
}

// The entire binding check is one SQLite snapshot, also used inside the publication statement.
const CURRENT: &str = "SELECT count(*)=json_array_length(?2) FROM json_each(?2) m
 JOIN main.root r ON r.id=json_extract(m.value,'$.root')
 JOIN st.root_grant g ON g.id=r.grant_id
 LEFT JOIN main.file f ON json_extract(m.value,'$.kind')='file' AND f.id=json_extract(m.value,'$.id')
 JOIN main.dir d ON d.id=CASE json_extract(m.value,'$.kind') WHEN 'dir' THEN json_extract(m.value,'$.id') ELSE f.dir_id END AND d.root_id=r.id
 WHERE r.volume_id=?1 AND g.id=json_extract(m.value,'$.grant') AND g.state='active'
 AND r.generation=json_extract(m.value,'$.generation') AND r.state='complete' AND r.active_run IS NULL
 AND d.subtree_rev=json_extract(m.value,'$.subtree_rev')
 AND d.sub_complete=1 AND d.listing_state='complete'
 AND EXISTS(SELECT 1 FROM main.dir rd WHERE rd.root_id=r.id AND rd.parent_id IS NULL AND rd.sub_complete=1 AND rd.listing_state='complete')
 AND CASE json_extract(m.value,'$.kind') WHEN 'dir' THEN d.born_run ELSE f.born_run END=json_extract(m.value,'$.incarnation')
 AND (SELECT value FROM main.meta WHERE key='instance_id')=?3";

fn current(
    inner: &Inner,
    conn: &Connection,
    volume: i64,
    members: &[Binding],
) -> EngineResult<bool> {
    if members.iter().any(|m| inner.freshness.stale(m.root)) {
        return Ok(false);
    }
    conn.query_row(
        CURRENT,
        params![
            volume,
            serde_json::to_string(members).map_err(internal)?,
            inner.instance
        ],
        |r| r.get(0),
    )
    .map_err(internal)
}

pub(super) fn read(
    inner: &Inner,
    gathered: &Gathered<'_>,
    ids: &BoundedVec<NodeId, 1, 12>,
) -> Result<Value, ErrorBody> {
    let members = prepare(inner, gathered, ids)?;
    let mut result = json!({"members": ids, "status": "unobserved", "observed_at": null,
        "reclaimable_bytes": null, "reason": "This selection has not been observed."});
    let fresh =
        gathered.online && current(inner, &gathered.input.conn, gathered.volume_row, &members)?;
    if !fresh {
        result["reason"] = json!("Selected coverage is incomplete, stale or unavailable. Refresh the granted roots before observing again.");
    }
    let stored = gathered.input.conn.query_row(
        "SELECT catalog_instance,members,observed_at_ns,unique_allocated_bytes,reason FROM st.reclaim_selection WHERE volume_id=?1",
        [gathered.volume_row], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?)),
    ).optional().map_err(sql_error)?;
    if let Some((instance, bindings, at, bytes, reason)) = stored {
        let saved: Vec<Binding> = serde_json::from_str(&bindings)
            .map_err(|_| fail(ErrorCode::InternalError, "Invalid selection record."))?;
        if saved
            .iter()
            .map(Binding::node)
            .eq(members.iter().map(Binding::node))
        {
            result["observed_at"] = json!(crate::ts_ns(at));
            if instance == inner.instance && saved == members && fresh {
                result["status"] = json!("recorded");
                result["reclaimable_bytes"] = json!(bytes);
                result["reason"] = json!(reason);
            } else {
                result["status"] = json!("expired");
                result["reason"] = json!("A selected member changed or its coverage is stale. Observe the current selection again.");
            }
        }
    }
    Ok(result)
}

fn store(
    inner: &Inner,
    job: &Observation,
    members: &[Binding],
    bytes: Option<u64>,
    reason: Option<&str>,
) -> EngineResult<()> {
    if members.iter().any(|m| inner.freshness.stale(m.root)) {
        return Ok(());
    }
    inner.db().execute(&format!(
        "INSERT INTO st.reclaim_selection(volume_id,catalog_instance,members,observed_at_ns,unique_allocated_bytes,reason)
         SELECT ?1,?3,?2,?4,?5,?6 WHERE ({CURRENT})
         ON CONFLICT(volume_id) DO UPDATE SET catalog_instance=excluded.catalog_instance,members=excluded.members,
           observed_at_ns=excluded.observed_at_ns,unique_allocated_bytes=excluded.unique_allocated_bytes,reason=excluded.reason"
    ), params![job.volume_row, serde_json::to_string(members).map_err(internal)?, inner.instance, crate::now_ns(), bytes.map(|b| b.to_string()), reason]).map_err(internal)?;
    Ok(())
}

pub(super) fn observe(
    weak: &Weak<Inner>,
    job: &Observation,
    members: &[Binding],
    ctx: &JobContext,
) -> EngineResult<CoverageState> {
    let inner = weak.upgrade().ok_or_else(gone)?;
    let conn = db::open(&inner.state_dir).map_err(internal)?;
    conn.execute_batch("PRAGMA query_only=ON")
        .map_err(internal)?;
    let deadline = Instant::now() + job.budget;
    let cancellation = ctx.clone();
    conn.progress_handler(
        1000,
        Some(move || Instant::now() >= deadline || cancellation.is_cancelled()),
    )
    .map_err(internal)?;
    let mut tally = ReliefTally::default();
    let limit = MAX_FILES_PER_GROUP;
    #[cfg(test)]
    let limit = {
        let test_limit = inner
            .hooks
            .reclaim_selection_limit
            .load(std::sync::atomic::Ordering::Acquire);
        if test_limit == 0 {
            limit
        } else {
            test_limit.min(limit)
        }
    };
    let mut seen_names = HashSet::new();
    let mut seen_dirs = HashSet::new();
    let (mut done, mut checked) = (0, 0);
    let progress = |done: u64, checked: u64| {
        ctx.set_reclaim_progress(ReclaimObserveProgress {
            candidates_total: Count::new(members.len() as u64).map_err(internal)?,
            candidates_done: Count::new(done).map_err(internal)?,
            names_checked: Count::new(checked).map_err(internal)?,
        })
    };
    progress(0, 0)?;
    store(
        &inner,
        job,
        members,
        None,
        Some("Observation has not completed."),
    )?;
    let run = || -> EngineResult<Option<&'static str>> {
        for task in &job.tasks {
            #[cfg(test)]
            inner.hooks.gate(Command::ReclaimObserve, "candidate");
            if ctx.is_cancelled() {
                return Ok(Some("Observation was cancelled."));
            }
            if Instant::now() >= deadline {
                return Ok(Some("The selection time budget was reached."));
            }
            if matches!(task.class, "virtual_disk" | "system_managed")
                || task.logical.is_none()
                || task.allocated.is_none()
            {
                return Ok(Some(
                    "A selected candidate has unknown or managed allocation.",
                ));
            }
            let Some(root) = &job.roots[task.root] else {
                return Ok(Some("A selected root could not be admitted."));
            };
            if !current(&inner, &conn, job.volume_row, members)? {
                return Ok(Some("A selected member changed or lost its grant."));
            }
            let Some(listing) = list_bounded(
                &conn,
                root,
                task,
                limit - seen_names.len(),
                limit - seen_dirs.len(),
                &seen_dirs,
            )?
            else {
                return Ok(Some("A selected candidate has incomplete coverage."));
            };
            let binding = members
                .iter()
                .find(|m| m.node() == task.node)
                .expect("selected task");
            if binding.incarnation != listing.incarnation
                || binding.subtree_rev != listing.subtree_rev
            {
                return Ok(Some("A selected member changed during observation."));
            }
            let dirs: Vec<_> = listing
                .dirs
                .into_iter()
                .filter(|(p, ..)| seen_dirs.insert(p.clone()))
                .collect();
            if listing.directory_overflow || listing.names.len() > limit - seen_names.len() {
                return Ok(Some("The selection name or directory budget was reached."));
            }
            for directories in dirs.chunks(OBSERVE_CHUNK) {
                if !current(&inner, &conn, job.volume_row, members)?
                    || !dirs_unchanged(&listing.volume_key, directories, deadline, ctx)
                {
                    return Ok(Some("A directory identity or active grant could not be verified, or observation stopped."));
                }
            }
            let (mut logical, mut allocated) = (Some(0u64), Some(0u64));
            for chunk in listing.names.chunks(OBSERVE_CHUNK) {
                #[cfg(test)]
                inner.hooks.gate(Command::ReclaimObserve, "chunk");
                if ctx.is_cancelled() {
                    return Ok(Some("Observation was cancelled."));
                }
                if Instant::now() >= deadline {
                    return Ok(Some("The selection time budget was reached."));
                }
                if !current(&inner, &conn, job.volume_row, members)? {
                    return Ok(Some("A selected member changed or lost its grant."));
                }
                let mut entries = Vec::with_capacity(chunk.len());
                for (id, rel, size, allocation, file_id, created, reparse, attrs) in chunk {
                    if !seen_names.insert(*id) {
                        return Ok(Some("A name was listed more than once."));
                    }
                    logical = logical.and_then(|n| n.checked_add(*size));
                    allocated = allocated.and_then(|n| allocation.and_then(|a| n.checked_add(a)));
                    entries.push(linkcount::Entry {
                        path: if rel.is_empty() {
                            listing.base.clone()
                        } else {
                            listing.base.join(rel)
                        },
                        expected: crate::placement::catalogue_object(
                            &listing.volume_key,
                            file_id.clone(),
                            *created,
                            *reparse,
                        ),
                        attributes: *attrs,
                    });
                }
                #[cfg(test)]
                inner
                    .hooks
                    .reclaim_names_opened
                    .fetch_add(entries.len(), std::sync::atomic::Ordering::AcqRel);
                let report = linkcount::observe(&entries, deadline);
                checked += report.outcomes.len() as u64;
                progress(done, checked)?;
                if report.stop.is_some() || report.outcomes.len() != chunk.len() {
                    return Ok(Some("The selection time or name budget was reached."));
                }
                for ((id, _, size, allocation, file_id, created, reparse, _), outcome) in
                    chunk.iter().zip(report.outcomes)
                {
                    let file = FileEntry {
                        node_id: NodeId::new(format!("nd_file_{id}")).expect("file row"),
                        logical_bytes: *size,
                        allocated_bytes: *allocation,
                        object: crate::placement::catalogue_object(
                            &listing.volume_key,
                            file_id.clone(),
                            *created,
                            *reparse,
                        ),
                        link_observation: match outcome {
                            Outcome::Matched(o) => Some(*o),
                            _ => None,
                        },
                    };
                    tally.add(&file, job.volume.volume_serial.as_deref(), job.volume.ntfs);
                }
            }
            if logical != task.logical || allocated != task.allocated {
                return Ok(Some(
                    "Listed names do not cover a selected candidate's allocation.",
                ));
            }
            done += 1;
            progress(done, checked)?;
        }
        if done != members.len() as u64 {
            return Ok(Some("The selection was not fully observed."));
        }
        Ok(None)
    };
    let mut run = run;
    let mut reason = match run() {
        Ok(reason) => reason,
        Err(e) => {
            eprintln!("loomward-service: selection left unknown: {e}");
            Some("The selection could not be completely observed.")
        }
    };
    conn.progress_handler(0, None::<fn() -> bool>)
        .map_err(internal)?;
    if ctx.is_cancelled() {
        reason = Some("Observation was cancelled.");
    }
    if Instant::now() >= deadline {
        reason = Some("The selection time budget was reached.");
    }
    let complete = reason.is_none();
    let bytes = if complete {
        tally.relief().ok().flatten()
    } else {
        None
    };
    if bytes.is_none() && reason.is_none() {
        reason = Some("An identity, allocation or complete hard-link set could not be verified.");
    }
    store(&inner, job, members, bytes, reason)?;
    Ok(if ctx.is_cancelled() {
        CoverageState::Cancelled
    } else if complete {
        CoverageState::Complete
    } else {
        CoverageState::Partial
    })
}
