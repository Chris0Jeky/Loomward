//! `changes.new_since` (#1092): what first appeared in one active root since a baseline scan run, read
//! from the existing `born_run` columns (no schema change). `born_run` is the run that first listed a
//! row and survives rescans for unchanged, changed-in-place and same-folder-renamed entries (#1091), so
//! "new since run B" is `born_run > B`: never `= the last run`, because watcher runs stamp it too.
//! A file moved across folders or deleted and recreated gets a new row and reads as new; anything an
//! earlier run could not list is born in the first run that lists it. Those are said, not fixed.
//! Read-only: no aggregate repair, no live path, no effect.
use crate::{query, DisplayPath, Error, NodeKey, Reader, Result, EXCLUDED_FLAGS};
use rusqlite::{params, Connection, OptionalExtension};
use std::{cmp::Reverse, collections::BinaryHeap, time::Instant};

/// Rows of the root's visible tree, exactly as `stats.breakdown` walks it (excluded and
/// absent-pending directories, and what is under them, are never counted).
const TREE: &str = "WITH RECURSIVE tree(id) AS (
    SELECT id FROM dir WHERE root_id=?1 AND parent_id IS NULL AND listing_state!='excluded'
    UNION ALL
    SELECT d.id FROM dir d JOIN tree t ON d.parent_id=t.id
    WHERE d.listing_state NOT IN ('absent_pending','excluded'))";
/// SQLite VM steps for the whole answer; also stopped at the request deadline.
const WORK_BUDGET: u64 = 32_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanRunInfo {
    /// The `scan_run` id: for a full-root run, also the root generation it published.
    pub run: i64,
    pub mode: String,
    pub finished_at_ns: Option<i64>,
    /// Completed and the root aggregate was a full, clean traversal at its end. A run recorded
    /// without that counter (an older row) is not known to be complete.
    pub complete: bool,
}

#[derive(Clone, Debug)]
pub struct NewFile {
    pub node: NodeKey,
    pub name: String,
    pub path: DisplayPath,
    pub logical_bytes: u64,
    pub allocated_bytes: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct NewFolder {
    pub node: NodeKey,
    pub name: String,
    pub path: DisplayPath,
    /// The folder's whole recorded size now, including anything moved into it. Null is unknown.
    pub logical_bytes: Option<u64>,
    pub allocated_bytes: Option<u64>,
    pub coverage: String,
}

#[derive(Clone, Debug, Default)]
pub struct NewSinceTotals {
    pub new_files: u64,
    pub new_dirs: u64,
    pub logical_bytes: u64,
    /// None when any new file's allocation is unknown; never a sum of the known ones.
    pub allocated_bytes: Option<u64>,
    pub allocation_unknown_files: u64,
}

#[derive(Clone, Debug, Default)]
pub struct NewSince {
    /// None: there is no earlier completed scan to compare with.
    pub baseline: Option<ScanRunInfo>,
    /// The newest completed run of any mode.
    pub latest: Option<ScanRunInfo>,
    pub totals: Option<NewSinceTotals>,
    pub largest_files: Vec<NewFile>,
    pub largest_folders: Vec<NewFolder>,
    /// Why the figures are not a clean answer: `baseline_not_completed`, `baseline_incomplete`,
    /// `coverage_partial`, `scan_running`, `catalogue_repairing`.
    pub partial_reasons: Vec<&'static str>,
}

fn run_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(ScanRunInfo, String)> {
    let state: String = r.get(2)?;
    Ok((
        ScanRunInfo {
            run: r.get(0)?,
            mode: r.get(1)?,
            finished_at_ns: r.get(3)?,
            complete: state == "completed" && r.get::<_, bool>(4)?,
        },
        state,
    ))
}
const RUN: &str = "SELECT id,mode,state,finished_at_ns,coalesce(json_extract(counters_json,'$.complete'),0)=1 FROM scan_run";

fn u(v: i64) -> Result<u64> {
    u64::try_from(v).map_err(|_| Error::Invalid("negative size"))
}

impl Reader {
    /// Entries of `root` born after `baseline` (a `scan_run` id of this root), or after the full-root
    /// run completed just before the newest completed one when `baseline` is None. `cap` bounds each
    /// ranked list. A bounded, index-free pass over the root's files: a budget hit is `ResourceBudget`,
    /// never a short count.
    pub fn new_since(&mut self, root: i64, baseline: Option<i64>, cap: usize) -> Result<NewSince> {
        self.new_since_within(root, baseline, cap, WORK_BUDGET)
    }

    /// `new_since` with an explicit SQLite step budget: a test seam for the budget-hit path.
    #[doc(hidden)]
    pub fn new_since_within(
        &mut self,
        root: i64,
        baseline: Option<i64>,
        cap: usize,
        work_budget: u64,
    ) -> Result<NewSince> {
        if !(1..=50).contains(&cap) || baseline.is_some_and(|b| b < 1) {
            return Err(Error::Invalid("new since request"));
        }
        let deadline = self.deadline;
        if deadline.is_some_and(|at| Instant::now() >= at) {
            return Err(Error::Cancelled);
        }
        let result = self.read_observations(|conn| {
            let _guard = query::WorkGuard::new(conn, work_budget, deadline)?;
            read(conn, root, baseline, cap)
        });
        match result {
            Err(e) if query::is_budget_interrupt(&e) => {
                Err(if deadline.is_some_and(|at| Instant::now() >= at) {
                    Error::Cancelled
                } else {
                    Error::ResourceBudget
                })
            }
            result => result,
        }
    }
}

fn read(conn: &Connection, root: i64, baseline: Option<i64>, cap: usize) -> Result<NewSince> {
    query::revisions(conn, Some(root))?; // an unknown or revoked root is NotFound
    let latest = conn
        .query_row(
            &format!("{RUN} WHERE root_id=?1 AND state='completed' ORDER BY id DESC LIMIT 1"),
            [root],
            run_row,
        )
        .optional()?
        .map(|r| r.0);
    let mut out = NewSince {
        latest,
        ..NewSince::default()
    };
    let baseline_state = match baseline {
        Some(id) => {
            let (run, state) = conn
                .query_row(
                    &format!("{RUN} WHERE id=?1 AND root_id=?2"),
                    params![id, root],
                    run_row,
                )
                .optional()?
                .ok_or(Error::NotFound)?;
            if state == "running" {
                return Err(Error::Invalid("the baseline run is still running"));
            }
            out.baseline = Some(run);
            state
        }
        None => {
            // The full-root run before the newest completed full-root run: "the previous scan".
            let mut stmt = conn.prepare(&format!(
                "{RUN} WHERE root_id=?1 AND state='completed' AND mode IN ('full','refresh') ORDER BY id DESC LIMIT 2"
            ))?;
            let runs = stmt
                .query_map([root], run_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            match runs.into_iter().nth(1) {
                Some((run, state)) => {
                    out.baseline = Some(run);
                    state
                }
                None => return Ok(out), // no earlier completed scan: unknown, never zero
            }
        }
    };
    let base = out.baseline.as_ref().expect("set above").clone();
    if baseline_state != "completed" {
        out.partial_reasons.push("baseline_not_completed");
    } else if !base.complete {
        out.partial_reasons.push("baseline_incomplete");
    }
    let root_dir: i64 = conn.query_row(
        "SELECT id FROM dir WHERE root_id=?1 AND parent_id IS NULL",
        [root],
        |r| r.get(0),
    )?;
    let observed = query::get(conn, NodeKey::Dir(root_dir))?;
    let repairing: bool = conn.query_row(
        "SELECT state='repairing' FROM root WHERE id=?1",
        [root],
        |r| r.get(0),
    )?;
    if repairing || !observed.valid {
        out.partial_reasons.push("catalogue_repairing");
    } else if observed.coverage() != "complete" {
        out.partial_reasons.push("coverage_partial");
    }
    if query::live(conn, root)? {
        out.partial_reasons.push("scan_running");
    }

    // One pass over the new files: count, sums and the cap largest (largest logical size first, row id
    // breaking ties).
    let mut totals = NewSinceTotals::default();
    let mut allocated = 0_u64;
    let mut best: BinaryHeap<Reverse<(i64, Reverse<i64>)>> = BinaryHeap::new();
    {
        let mut stmt = conn.prepare(&format!(
            "{TREE} SELECT f.id,f.logical,f.allocated FROM file f JOIN tree t ON t.id=f.dir_id
             WHERE f.born_run>?2 AND f.flags & ?3=0"
        ))?;
        let mut rows = stmt.query(params![root, base.run, EXCLUDED_FLAGS])?;
        while let Some(r) = rows.next()? {
            let (id, logical, alloc): (i64, i64, Option<i64>) = (r.get(0)?, r.get(1)?, r.get(2)?);
            totals.new_files += 1;
            totals.logical_bytes += u(logical)?;
            match alloc {
                Some(a) => allocated += u(a)?,
                None => totals.allocation_unknown_files += 1,
            }
            best.push(Reverse((logical, Reverse(id))));
            if best.len() > cap {
                best.pop();
            }
        }
    }
    totals.allocated_bytes = (totals.allocation_unknown_files == 0).then_some(allocated);
    totals.new_dirs = u(conn.query_row(
        &format!("{TREE} SELECT count(*) FROM dir d JOIN tree t ON t.id=d.id WHERE d.born_run>?2"),
        params![root, base.run],
        |r| r.get(0),
    )?)?;
    out.totals = Some(totals);

    let mut ids: Vec<_> = best.into_iter().map(|Reverse(k)| k).collect();
    ids.sort_by(|a, b| b.cmp(a));
    for (_, Reverse(id)) in ids {
        let key = NodeKey::File(id);
        let row = query::get(conn, key)?;
        out.largest_files.push(NewFile {
            node: key,
            path: path(conn, key)?,
            name: row.name,
            logical_bytes: row.totals.logical,
            allocated_bytes: (row.totals.allocation_unknown == 0).then_some(row.totals.allocated),
        });
    }

    // New top-level folders: new directories whose parent is not new, largest subtree first.
    let tops = conn
        .prepare(&format!(
            "{TREE} SELECT d.id FROM dir d JOIN tree t ON t.id=d.id JOIN dir p ON p.id=d.parent_id
             WHERE d.born_run>?2 AND p.born_run<=?2 AND d.flags & ?3=0
             ORDER BY d.sub_logical DESC,d.id LIMIT ?4"
        ))?
        .query_map(params![root, base.run, EXCLUDED_FLAGS, cap as i64], |r| {
            r.get::<_, i64>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for id in tops {
        let key = NodeKey::Dir(id);
        let row = query::get(conn, key)?;
        let known = row.state != "unknown";
        out.largest_folders.push(NewFolder {
            node: key,
            path: path(conn, key)?,
            coverage: row.coverage(),
            logical_bytes: known.then_some(row.totals.logical),
            allocated_bytes: (known && row.totals.allocation_unknown == 0)
                .then_some(row.totals.allocated),
            name: row.name,
        });
    }
    Ok(out)
}

fn path(conn: &Connection, key: NodeKey) -> Result<DisplayPath> {
    let p = query::node_path(conn, key)?;
    Ok(DisplayPath::bounded(
        p.ancestors
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(" / "),
        p.truncated,
    ))
}
