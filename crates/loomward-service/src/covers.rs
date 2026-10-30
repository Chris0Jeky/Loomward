//! #1106: `coverage.start` and `coverage.result`, the owner's "is everything in folder A already in
//! folder B, by content, whatever the paths" check (docs/48 section 4.5, the app side of
//! `loomward-native covers`). Read-only on both folders: the engine's `observe_coverage` and the
//! operations crate's `containment` do the work unchanged; this module admits the two roots like a
//! duplicates inspection (q-10), runs them as one cancellable job and keeps a bounded report.
//! Observation-time evidence only: nothing here says a file is safe to delete.
use crate::{db, fail, sql_error, ts_ns, with_reason, Inner, Out};
use loomward_catalog::NodeKey;
use loomward_engine::{
    jobs::{JobContext, JobRunner, JobSpec},
    scan::{CatalogRootIdentity, GrantedRoot, TreeBudget},
    EngineError, EngineResult,
};
use loomward_operations::{
    containment::{containment, Coverage, Item, Report},
    tree::Side,
};
use loomward_protocol::*;
use serde_json::json;
use std::{
    cmp::Ordering,
    path::{Component, Path},
    sync::{mpsc, Arc, Mutex, Weak},
    time::{Duration, Instant},
};

/// The verifier's entry cap (`docs/48` 4.5); a larger tree reads as an `entry_budget` unknown.
const MAX_ENTRIES: usize = 200_000;
const DEFAULT_BYTES: u64 = 1 << 30;
const DEFAULT_SECS: u64 = 60;
/// Each list carries this many entries; the totals always count everything.
const LIST_CAP: usize = 200;
/// Reports are kept as long as the job record (ten minutes), newest few only.
const KEPT: usize = 8;
const KEPT_FOR: Duration = Duration::from_secs(600);
const NOTE: &str = "Covered by content when read: every covered file of A had a file in B of equal size and \
equal content digest. Observation-time evidence, not a receipt: it is not metadata fidelity \
(timestamps, attributes, ACLs, streams), not an independent backup (B may share a failing disk) and \
not permission to delete anything. Either folder may change afterwards. Unknown bytes are a lower \
bound when size_unknown is above zero.";

struct Stored {
    at: Instant,
    grants: [i64; 2],
    report: CoverageReport,
}

/// The newest few finished reports. A report names files of two granted folders, so revoking,
/// forgetting or losing either grant deletes it (`forget_grant`).
#[derive(Default)]
pub(crate) struct Results(Mutex<Vec<Stored>>);

impl Results {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Stored>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub(crate) fn get(&self, job: &JobId) -> Option<CoverageReport> {
        let mut kept = self.lock();
        kept.retain(|s| s.at.elapsed() < KEPT_FOR);
        kept.iter()
            .find(|s| &s.report.job_id == job)
            .map(|s| s.report.clone())
    }
    pub(crate) fn forget_grant(&self, grant: i64) {
        self.lock().retain(|s| !s.grants.contains(&grant));
    }
}

/// Everything the job needs, resolved and admitted before the job is submitted.
struct Plan {
    a: GrantedRoot,
    b: GrantedRoot,
    a_id: RootId,
    b_id: RootId,
    /// Active grants of A and B, rechecked when the report is stored.
    grants: [i64; 2],
    /// A's catalogue root directory, to find the catalogue rows of the files reported.
    a_dir: Option<i64>,
    budget_bytes: u64,
    budget_secs: u64,
}

impl Inner {
    fn coverage_root(&self, id: &RootId) -> Result<db::RootRow, ErrorBody> {
        let row = self.root_row(id.as_str())?;
        let root = db::roots(&self.db(), Some(row))
            .map_err(sql_error)?
            .into_iter()
            .next()
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown root"))?;
        if root.grant_state != "active" {
            return Err(fail(ErrorCode::PermissionDenied, "grant_not_active"));
        }
        // The same admission as a duplicates inspection (q-10): the session's own dataset class,
        // liveness, and for a personal session today's path fences.
        self.session_root_gate(&root)
            .map_err(|e| self.root_admission_error(&root, e))?;
        Ok(root)
    }

    fn coverage_granted(&self, root: &db::RootRow, id: &RootId) -> Result<GrantedRoot, ErrorBody> {
        let granted = self.engine.granted_root(
            id.clone(),
            root.grant,
            &CatalogRootIdentity {
                row: root.root,
                instance: self.instance.clone(),
            },
        );
        // A replaced folder ends its grant here exactly as at scan start (#378).
        if matches!(&granted, Err(EngineError::PermissionDenied { message })
            if message == "root_identity_changed")
        {
            self.identity_changed(root.grant, id, true);
        }
        Ok(granted?)
    }

    pub(crate) fn coverage_start(&self, req: &RequestEnvelope) -> Result<Out, ErrorBody> {
        let request: CoverageStartRequest = req.decode_payload()?;
        if request.a_root_id == request.b_root_id {
            return Err(with_reason(
                fail(
                    ErrorCode::InvalidRequest,
                    "choose two different folders to compare",
                ),
                "roots_must_differ",
            ));
        }
        let a = self.coverage_root(&request.a_root_id)?;
        let b = self.coverage_root(&request.b_root_id)?;
        // Overlapping grants are refused at grant time, so this is a defence: a folder inside the
        // other would make "everything in A is in B" trivially about itself.
        if crate::paths::overlaps_stored(Path::new(&a.display_path), Path::new(&b.display_path)) {
            return Err(with_reason(
                fail(
                    ErrorCode::InvalidRequest,
                    "one folder lies inside the other; choose two separate folders",
                ),
                "roots_overlap",
            ));
        }
        let plan = Plan {
            a: self.coverage_granted(&a, &request.a_root_id)?,
            b: self.coverage_granted(&b, &request.b_root_id)?,
            a_id: request.a_root_id.clone(),
            b_id: request.b_root_id.clone(),
            grants: [a.grant, b.grant],
            a_dir: a.dir,
            budget_bytes: request.budget_bytes.map_or(DEFAULT_BYTES, |b| b.get()),
            budget_secs: request.budget_secs.map_or(DEFAULT_SECS, |s| s.get() as u64),
        };
        let weak = self.weak.clone();
        let spec = JobSpec::new(JobKind::Coverage, Some(request.a_root_id));
        let runner: JobRunner = Arc::new(move |ctx, _| run(&weak, &plan, &ctx));
        let job = self.engine.job_submit_runner(spec, runner)?;
        Ok(Out {
            result: json!({ "job": job }),
            catalog_rev: None,
            state_rev: None,
        })
    }

    pub(crate) fn coverage_result(&self, request: JobRefRequest) -> Result<Out, ErrorBody> {
        let report = self.covers.get(&request.job_id).ok_or_else(|| {
            fail(
                ErrorCode::NotFound,
                "no coverage result for this job: it is still running, was cancelled or failed, or expired",
            )
        })?;
        Ok(Out {
            result: serde_json::to_value(report)
                .map_err(|_| fail(ErrorCode::InternalError, "unreadable coverage report"))?,
            catalog_rev: None,
            state_rev: None,
        })
    }

    /// Keeps a finished report unless a grant it names has ended meanwhile. The check and the
    /// insert share the results lock that `forget_grant` takes after a revocation commits, so a
    /// report is either refused here or deleted there.
    fn store_coverage(&self, grants: [i64; 2], report: CoverageReport) -> EngineResult<()> {
        let mut kept = self.covers.lock();
        let active: i64 = self
            .db()
            .query_row(
                "SELECT count(*) FROM st.root_grant WHERE state='active' AND id IN (?1,?2)",
                grants,
                |r| r.get(0),
            )
            .map_err(|_| internal("grant store unavailable"))?;
        if active != 2 {
            return Err(EngineError::PermissionDenied {
                message: "cancelled".into(),
            });
        }
        kept.retain(|s| s.at.elapsed() < KEPT_FOR);
        kept.push(Stored {
            at: Instant::now(),
            grants,
            report,
        });
        let surplus = kept.len().saturating_sub(KEPT);
        kept.drain(..surplus);
        Ok(())
    }

    /// The catalogue row of `relative` below A's root directory, sealed; `None` when the catalogue
    /// does not hold exactly one row under that exact name (never a guess).
    fn coverage_node(
        &self,
        conn: &rusqlite::Connection,
        dir: i64,
        relative: &Path,
    ) -> Option<NodeId> {
        let names: Vec<&str> = relative
            .components()
            .map(|c| match c {
                Component::Normal(name) => name.to_str(),
                _ => None,
            })
            .collect::<Option<_>>()?;
        let (key, born) = catalogue_row(conn, dir, &names)?;
        NodeId::new(self.ids.seal(key, born, &self.instance)).ok()
    }
}

fn internal(message: &str) -> EngineError {
    EngineError::Internal {
        message: format!("coverage check: {message}"),
        detail: None,
    }
}

/// The one live row named by `names` below directory row `root_dir` (a folder or a file), with its
/// incarnation. Zero rows, or two spellings of the same name, are `None`.
fn catalogue_row(
    conn: &rusqlite::Connection,
    root_dir: i64,
    names: &[&str],
) -> Option<(NodeKey, i64)> {
    const DIRS: &str = "SELECT id,born_run FROM main.dir WHERE parent_id=?1 AND name=?2 AND listing_state!='absent_pending' LIMIT 2";
    const FILES: &str = "SELECT id,born_run FROM main.file WHERE dir_id=?1 AND name=?2 LIMIT 2";
    let rows = |sql: &str, parent: i64, name: &str| -> Option<Vec<(i64, i64)>> {
        let mut stmt = conn.prepare_cached(sql).ok()?;
        let rows = stmt
            .query_map(rusqlite::params![parent, name], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .ok()?;
        rows.collect::<rusqlite::Result<Vec<_>>>().ok()
    };
    let Some((last, parents)) = names.split_last() else {
        // The root folder itself.
        let born = conn
            .query_row(
                "SELECT born_run FROM main.dir WHERE id=?1",
                [root_dir],
                |r| r.get(0),
            )
            .ok()?;
        return Some((NodeKey::Dir(root_dir), born));
    };
    let mut dir = root_dir;
    for name in parents {
        match rows(DIRS, dir, name)?.as_slice() {
            [(id, _)] => dir = *id,
            _ => return None,
        }
    }
    match (
        rows(DIRS, dir, last)?.as_slice(),
        rows(FILES, dir, last)?.as_slice(),
    ) {
        ([(id, born)], []) => Some((NodeKey::Dir(*id), *born)),
        ([], [(id, born)]) => Some((NodeKey::File(*id), *born)),
        _ => None,
    }
}

fn gone() -> EngineError {
    internal("service stopped")
}

/// The job body. It waits for the read on a helper thread so that `jobs.cancel` is answered at
/// once: `Engine::observe_coverage` takes no per-job cancel flag, so a cancelled read runs on to
/// its time budget (at most `budget_secs`) with its result discarded, and reads nothing it does
/// not already have a handle on.
// ponytail: detached read after a cancel; give observe_coverage a job cancel flag (#1037's owner)
// to stop it early.
fn run(weak: &Weak<Inner>, plan: &Plan, ctx: &JobContext) -> EngineResult<CoverageState> {
    let engine = weak.upgrade().ok_or_else(gone)?.engine.clone();
    let (tx, rx) = mpsc::channel();
    let (a, b) = (plan.a.clone(), plan.b.clone());
    let budget = TreeBudget {
        bytes: plan.budget_bytes,
        time: Duration::from_secs(plan.budget_secs),
    };
    std::thread::Builder::new()
        .name("loomward-coverage-read".into())
        .spawn(move || {
            let _ = tx.send(engine.observe_coverage(&a, &b, budget, MAX_ENTRIES));
        })
        .map_err(|_| internal("read thread unavailable"))?;
    #[cfg(test)]
    weak.upgrade()
        .ok_or_else(gone)?
        .hooks
        .gate(Command::CoverageStart, "reading");
    let (a_tree, b_tree, bytes_read) = loop {
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(done) => break done.map_err(refusal)?,
            Err(mpsc::RecvTimeoutError::Timeout) if ctx.is_cancelled() => {
                return Ok(CoverageState::Cancelled)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(internal("read thread lost")),
        }
    };
    if ctx.is_cancelled() {
        return Ok(CoverageState::Cancelled);
    }
    let inner = weak.upgrade().ok_or_else(gone)?;
    let observed = containment(&a_tree, &b_tree);
    let report = {
        let conn = inner.db();
        let node = |path: &Path| {
            plan.a_dir
                .and_then(|dir| inner.coverage_node(&conn, dir, path))
        };
        summarise(
            &ctx.job_id,
            (&plan.a_id, &plan.b_id),
            &observed,
            bytes_read,
            &node,
        )
    }
    .map_err(|_| internal("report bounds"))?;
    let partial = report.outcome == CoverageOutcome::Partial;
    inner.store_coverage(plan.grants, report)?;
    Ok(if partial {
        CoverageState::Partial
    } else {
        CoverageState::Complete
    })
}

/// A refused read names its reason, never a path: the local CLI's text carries the root's display
/// path, and the job error also reaches HTTP clients.
fn refusal(error: EngineError) -> EngineError {
    match error {
        EngineError::PermissionDenied { message } => EngineError::PermissionDenied {
            message: message.split(':').next().unwrap_or("refused").to_owned(),
        },
        other => other,
    }
}

fn display(path: &Path) -> BoundedStr<1, 32768> {
    let text = path.to_string_lossy();
    BoundedStr::truncated(if text.is_empty() { "." } else { &text })
}

fn stop_of(reason: &str) -> CoverageStop {
    match reason {
        "byte_budget" | "time_budget" | "entry_budget" => CoverageStop::Budget,
        "changed_during_compare" | "changed_during_read" => CoverageStop::ChangedDuringRead,
        _ => CoverageStop::Refused,
    }
}

/// Largest first, an unknown size after every known one, then by path.
fn largest_first(a: &Item, b: &Item) -> Ordering {
    b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path))
}

/// The wire report of one `containment` run. Covered files are counted, never listed.
pub(crate) fn summarise(
    job_id: &JobId,
    roots: (&RootId, &RootId),
    report: &Report,
    bytes_read: u64,
    node: &dyn Fn(&Path) -> Option<NodeId>,
) -> Result<CoverageReport, serde_json::Error> {
    let totals = report.totals();
    let total = |t: loomward_operations::containment::Total| json!({"files": t.files, "bytes": t.bytes.to_string(), "size_unknown": t.size_unknown});
    let mut not_covered: Vec<&Item> = Vec::new();
    let mut unknown: Vec<&Item> = Vec::new();
    let mut stops = [false; 3];
    for item in &report.items {
        match &item.coverage {
            Coverage::Covered { .. } => {}
            Coverage::NotCovered => not_covered.push(item),
            Coverage::Unknown { reason, .. } => {
                stops[stop_of(reason) as usize] = true;
                unknown.push(item);
            }
        }
    }
    not_covered.sort_by(|a, b| largest_first(a, b));
    unknown.sort_by(|a, b| largest_first(a, b));
    let bytes = |item: &Item| item.size.map(|n| n.to_string());
    let reasons: Vec<_> = [
        CoverageStop::Budget,
        CoverageStop::Refused,
        CoverageStop::ChangedDuringRead,
    ]
    .into_iter()
    .filter(|&s| stops[s as usize])
    .collect();
    serde_json::from_value(json!({
        "job_id": job_id,
        "a_root_id": roots.0,
        "b_root_id": roots.1,
        "outcome": if totals.unknown.files == 0 { "complete" } else { "partial" },
        "reasons": reasons,
        "bytes_read": bytes_read.to_string(),
        "totals": {
            "covered": total(totals.covered),
            "not_covered": total(totals.not_covered),
            "unknown": total(totals.unknown),
        },
        "not_covered": not_covered.iter().take(LIST_CAP).map(|i| json!({
            "path": display(&i.path), "node_id": node(&i.path), "bytes": bytes(i),
        })).collect::<Vec<_>>(),
        "not_covered_truncated": not_covered.len() > LIST_CAP,
        "unknown": unknown.iter().take(LIST_CAP).map(|i| {
            let Coverage::Unknown { side, reason } = &i.coverage else { unreachable!() };
            json!({
                "path": display(&i.path), "node_id": node(&i.path), "bytes": bytes(i),
                "side": if *side == Side::Source { "a" } else { "b" }, "reason": reason,
            })
        }).collect::<Vec<_>>(),
        "unknown_truncated": unknown.len() > LIST_CAP,
        "observed_at": ts_ns(crate::now_ns()),
        "note": NOTE,
    }))
}
