//! A candidate inside another candidate is listed once, under the outer one (#1076). The engine
//! plan dedupes only identical nodes, so a machine store holding a project's `node_modules` would
//! otherwise count the same bytes twice in any sum over a group.
use super::*;

/// Ancestry rows one plan may read in total. Real chains are a few dozen rows; the cap only bounds
/// a catalogue that is far deeper than any walk the detectors accept.
const ROW_BUDGET: usize = 100_000;

/// One directory and its ancestors, nearest first.
pub(super) struct Chain {
    pub(super) ids: Vec<i64>,
    /// The walk reached the root directory. Cut by the 511 depth cap or a missing parent row, the
    /// chain proves nothing about the directories above it.
    pub(super) complete: bool,
}

/// The one bounded ancestry pass, shared by selection and the plan's dedupe.
pub(super) fn ancestry(conn: &Connection, dir: i64) -> Result<Chain, ErrorBody> {
    let rows = conn
        .prepare_cached(
            "WITH RECURSIVE a(id,parent_id,depth) AS (SELECT id,parent_id,0 FROM main.dir WHERE id=?1
             UNION ALL SELECT d.id,d.parent_id,a.depth+1 FROM main.dir d JOIN a ON d.id=a.parent_id WHERE a.depth<511)
             SELECT id,parent_id FROM a ORDER BY depth",
        )
        .map_err(sql_error)?
        .query_map([dir], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
        })
        .map_err(sql_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(sql_error)?;
    Ok(Chain {
        complete: rows.iter().any(|(_, parent)| parent.is_none()),
        ids: rows.into_iter().map(|(id, _)| id).collect(),
    })
}

/// Wire reference to catalogue node for every detector candidate, read before the plan consumes them.
pub(super) fn references(planner: &ReclaimInput) -> HashMap<String, NodeKey> {
    let developer = planner
        .developer
        .iter()
        .flat_map(|r| r.candidates.iter().map(|c| c.directory));
    let machine = planner
        .machine
        .iter()
        .flat_map(|r| r.candidates.iter().map(|c| c.node));
    developer
        .chain(machine)
        .map(|node| (node.reference(), node))
        .collect()
}

/// Drop every candidate, in any group, with a strict ancestor directory that is itself a listed
/// candidate, and say on the outermost one how many it absorbed.
///
/// A candidate whose ancestry cannot be read to the root (depth cap, missing row, row budget) is
/// kept and the plan is marked partial: dropping it would hide bytes, keeping it unmarked would
/// claim they are not counted twice. Candidates without a catalogue node are untouched. `truncated`
/// is left as the engine set it: the cut candidates stay cut, so it remains true.
pub(crate) fn dedupe(
    conn: &Connection,
    plan: &mut reclaim::ReclaimPlan,
    nodes: &HashMap<String, NodeKey>,
) -> Result<(), ErrorBody> {
    let node =
        |c: &reclaim::ReclaimCandidate| c.node_id.as_ref().and_then(|id| nodes.get(id)).copied();
    let listed: HashSet<i64> = plan
        .groups
        .iter()
        .flat_map(|g| &g.candidates)
        .filter_map(|c| match node(c) {
            Some(NodeKey::Dir(id)) => Some(id),
            _ => None,
        })
        .collect();
    if listed.is_empty() {
        return Ok(());
    }
    let mut inside: HashMap<String, i64> = HashMap::new();
    let mut absorbed: HashMap<i64, usize> = HashMap::new();
    let mut unreadable = false;
    let mut budget = ROW_BUDGET;
    for c in plan.groups.iter().flat_map(|g| &g.candidates) {
        let (Some(wire), Some(key)) = (c.node_id.as_ref(), node(c)) else {
            continue;
        };
        let (start, strict_from) = match key {
            NodeKey::Dir(id) => (Some(id), 1),
            _ => {
                let (_, id) = node_parts(key);
                let dir = conn
                    .query_row("SELECT dir_id FROM main.file WHERE id=?1", [id], |r| {
                        r.get::<_, i64>(0)
                    })
                    .optional()
                    .map_err(sql_error)?;
                (dir, 0)
            }
        };
        let chain = match start {
            Some(dir) if budget > 0 => ancestry(conn, dir)?,
            _ => Chain {
                ids: Vec::new(),
                complete: false,
            },
        };
        budget = budget.saturating_sub(chain.ids.len());
        if !chain.complete {
            unreadable = true;
            continue;
        }
        // Nearest first, so the last listed ancestor is the outermost and is itself never nested.
        if let Some(outer) = chain.ids[strict_from..]
            .iter()
            .rev()
            .find(|id| listed.contains(id))
        {
            inside.insert(wire.clone(), *outer);
            *absorbed.entry(*outer).or_default() += 1;
        }
    }
    plan.partial |= unreadable;
    if inside.is_empty() {
        return Ok(());
    }
    for candidates in plan.groups.iter_mut().map(|g| &mut g.candidates) {
        candidates.retain(|c| !c.node_id.as_ref().is_some_and(|id| inside.contains_key(id)));
        for c in candidates.iter_mut() {
            let Some(NodeKey::Dir(id)) = node(c) else {
                continue;
            };
            if let Some(n) = absorbed.get(&id) {
                c.evidence.push(format!(
                    "Contains {n} other recognised candidate{}, counted once here.",
                    if *n == 1 { "" } else { "s" }
                ));
            }
        }
    }
    Ok(())
}
