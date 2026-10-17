//! docs/47 lane C5 (LW-040 #46): copy-and-verify served for SYNTHETIC sessions on registered lab roots.
//! Personal sessions never reach this module: `Service::call` answers them `copy_effect_gated` before
//! dispatch (AGENTS.md invariant 1; the C8 gate stays closed until the G5 power-cut and G9 restore runs).
//! A synthetic session's grants exist only for registered lab roots (ADR-V3-15), and the session root
//! gate is re-run here for every source and the destination. MCP has no copy command.
use crate::{catalog_error, db, fail, now_ns, ts_ns, with_reason, Inner, Out};
use loomward_catalog::NodeKey;
use loomward_protocol::*;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc, Mutex},
};

/// docs/47 Q1, OPEN (agent-hq#461): may cancel and abandon delete Loomward's own unpublished staging?
/// `false` (default, option b): they stop and LEAVE the staging folder, reported in `staging_folder`.
/// `true`: identity-bound removal of Loomward's own journaled staging only. The owner's answer is this
/// one-line flip.
pub(crate) const LOOMWARD_COPY_STAGING_REMOVAL: bool = false;

struct Pending {
    sources: Vec<String>,
    destination: String,
    preview: loomward_operations::Preview,
    approval: Option<loomward_operations::Approval>,
}

pub(crate) struct Copies {
    /// The journal in `state.db`; `None` for personal sessions and off Windows.
    #[cfg(windows)]
    store: Option<Arc<Mutex<loomward_operations::Store>>>,
    pending: Mutex<HashMap<String, Pending>>,
    /// Wire ID to journal operation.
    ops: Mutex<HashMap<String, u64>>,
    #[cfg(windows)]
    running: Mutex<HashMap<u64, Run>>,
    /// The Q1 setting for this service (tests flip it; production reads the constant).
    pub(crate) removal: AtomicBool,
}

#[cfg(windows)]
struct Run {
    control: Arc<loomward_windows::effects::exec::Control>,
    thread: std::thread::JoinHandle<()>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn digest_text(d: [u8; 32]) -> String {
    format!("sha256:{}", hex(&d))
}

fn parse_digest(d: &str) -> Result<[u8; 32], ErrorBody> {
    let bad = || fail(ErrorCode::InvalidRequest, "malformed digest");
    let hexed = d.strip_prefix("sha256:").ok_or_else(bad)?;
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(hexed.get(2 * i..2 * i + 2).ok_or_else(bad)?, 16)
            .map_err(|_| bad())?;
    }
    Ok(out)
}

fn code_name<T: serde::Serialize>(code: T) -> Value {
    serde_json::to_value(code).unwrap_or(Value::Null)
}

fn not_open() -> ErrorBody {
    with_reason(
        fail(
            ErrorCode::CapabilityUnavailable,
            "copy-and-verify is not open: its effect gate (docs/47 lane C8) is closed",
        ),
        "copy_effect_gated",
    )
}

fn unknown() -> ErrorBody {
    fail(ErrorCode::NotFound, "unknown copy operation")
}

impl Copies {
    /// Open the journal for a synthetic session and run recovery (docs/47 section 5): C3's reader
    /// decides, and only its decisions run, through the C2 primitives. Nothing resumes by itself.
    pub(crate) fn open(state_dir: &Path, dataset: DatasetClass) -> Copies {
        let removal = AtomicBool::new(LOOMWARD_COPY_STAGING_REMOVAL);
        #[cfg(windows)]
        {
            let store = (dataset == DatasetClass::Synthetic)
                .then(|| {
                    loomward_operations::Store::open(
                        &loomward_catalog::sqlite_dir(state_dir).join("state.db"),
                    )
                    .ok()
                })
                .flatten()
                .map(|s| Arc::new(Mutex::new(s)));
            let mut ops = HashMap::new();
            if let Some(store) = &store {
                use loomward_windows::effects::exec;
                let listed = store.lock().unwrap().operations().unwrap_or_default();
                for op in listed {
                    let plan = store.lock().unwrap().plan(op).ok().flatten();
                    if let Some(plan) =
                        plan.and_then(|p| serde_json::from_str::<exec::Plan>(&p).ok())
                    {
                        ops.insert(plan.operation_id, op);
                    }
                    let control =
                        exec::Control::new(removal.load(std::sync::atomic::Ordering::SeqCst));
                    let _ = exec::recover(store, op, &control);
                }
            }
            Copies {
                store,
                pending: Mutex::default(),
                ops: Mutex::new(ops),
                running: Mutex::default(),
                removal,
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (state_dir, dataset);
            Copies {
                pending: Mutex::default(),
                ops: Mutex::default(),
                removal,
            }
        }
    }
}

/// Shutdown never detaches a copy: every running worker is cancelled (it stops at its next item or
/// 1 MiB chunk, before publish) and joined, so no effect or journal write outlives the service and a
/// reopened service never recovers alongside an old executor (review of #522).
#[cfg(windows)]
impl Drop for Copies {
    fn drop(&mut self) {
        let runs: Vec<Run> = self
            .running
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .map(|(_, r)| r)
            .collect();
        for run in &runs {
            run.control.cancel();
        }
        for run in runs {
            let _ = run.thread.join();
        }
    }
}

/// One resolved node: its root row, the granted root's verbatim path, and its path below it.
struct Resolved {
    root: db::RootRow,
    root_path: PathBuf,
    /// The grant's verified root identity: (volume serial, file ID).
    root_id: (u64, [u8; 16]),
    relative: PathBuf,
    is_dir: bool,
}

impl Inner {
    fn copy_node(
        &self,
        id: &str,
        reader: &mut loomward_catalog::Reader,
    ) -> Result<Resolved, ErrorBody> {
        let key = self.node(id, reader)?;
        let is_dir = match key {
            NodeKey::Dir(_) => true,
            NodeKey::File(_) => false,
            _ => {
                return Err(fail(
                    ErrorCode::InvalidRequest,
                    "copy selects files and folders only",
                ))
            }
        };
        let path = reader.path(key).map_err(catalog_error)?;
        if path.truncated {
            return Err(fail(ErrorCode::InvalidRequest, "the folder is too deep"));
        }
        let row: i64 = path
            .root_id
            .as_deref()
            .and_then(|r| r.strip_prefix("rt_"))
            .and_then(|r| r.parse().ok())
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown node"))?;
        let root = db::roots(&self.db(), Some(row))
            .map_err(crate::sql_error)?
            .into_iter()
            .next()
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown root"))?;
        if root.grant_state != "active" {
            return Err(fail(ErrorCode::PermissionDenied, "grant_not_active"));
        }
        // Synthetic sessions: lab roots only (the same gate the synthetic scan uses).
        self.session_root_gate(&root)?;
        let granted = self.engine.granted_root(
            RootId::new(self.root_id(row))
                .map_err(|_| fail(ErrorCode::NotFound, "unknown root"))?,
            root.grant,
            &loomward_engine::scan::CatalogRootIdentity {
                row,
                instance: self.instance.clone(),
            },
        )?;
        let root_id = match granted.expected().and_then(|e| e.id.zip(e.volume_serial)) {
            Some((loomward_windows::enumerate::FileIdObs::Id128(id), volume)) => (volume, id),
            _ => {
                return Err(with_reason(
                    fail(
                        ErrorCode::PermissionDenied,
                        "the root's identity is not verified",
                    ),
                    "identity_unverified",
                ))
            }
        };
        // Verbatim, so lab paths past 260 units stay reachable; bound to the grant's identity below.
        let root_path = std::fs::canonicalize(granted.path()).map_err(|_| {
            fail(
                ErrorCode::PermissionDenied,
                "the granted root is unavailable",
            )
        })?;
        Ok(Resolved {
            root,
            root_path,
            root_id,
            relative: path.ancestors.iter().skip(1).map(|p| &p.name).collect(),
            is_dir,
        })
    }

    /// Fresh native observations of the selection and destination (C4 inputs).
    #[cfg(windows)]
    fn copy_prepare(
        &self,
        sources: &[String],
        destination: &str,
    ) -> Result<
        (
            loomward_operations::Preview,
            loomward_windows::effects::exec::Plan,
        ),
        ErrorBody,
    > {
        use loomward_windows::effects::exec;
        let mut reader = self.catalog.reader().map_err(catalog_error)?;
        let mut resolved = Vec::with_capacity(sources.len());
        for s in sources {
            resolved.push(self.copy_node(s, &mut reader)?);
        }
        let dest = self.copy_node(destination, &mut reader)?;
        if !dest.is_dir {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "the destination must be a folder",
            ));
        }
        let first = &resolved[0];
        if resolved.iter().any(|r| r.root.root != first.root.root) {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "every source must come from one granted root",
            ));
        }
        let selected: Vec<PathBuf> = resolved.iter().map(|r| r.relative.clone()).collect();
        let dest_path = dest.root_path.join(&dest.relative);
        let state =
            std::fs::canonicalize(&self.state_dir).unwrap_or_else(|_| self.state_dir.clone());
        let date: String = serde_json::to_value(ts_ns(now_ns()))
            .ok()
            .and_then(|v| v.as_str().map(|s| s[..10].to_string()))
            .unwrap_or_default();
        let (preview, mut plan) = exec::prepare(&exec::Request {
            source_root: &first.root_path,
            selected: &selected,
            destination: &dest_path,
            destination_grant: dest.root.grant as u64,
            grants_active: true,
            protected: dest_path.starts_with(&state),
            date: &date,
            // The pinned destination must sit under its own grant's verified root (review of #522).
            destination_root: dest.root_id,
        })
        .map_err(|e| {
            with_reason(
                fail(
                    ErrorCode::PermissionDenied,
                    "the selection could not be observed",
                ),
                &e.to_string()
                    .chars()
                    .filter(|c| c.is_ascii_lowercase() || *c == '_')
                    .take(64)
                    .collect::<String>(),
            )
        })?;
        // The root opened for observation is the granted object, never a substitute at that path.
        if (plan.source_root_volume, plan.source_root_id) != first.root_id {
            return Err(with_reason(
                fail(ErrorCode::PermissionDenied, "the granted root changed"),
                "root_identity_changed",
            ));
        }
        plan.operation_id.clear();
        Ok((preview, plan))
    }

    fn copies_store(&self) -> Result<&Arc<Mutex<loomward_operations::Store>>, ErrorBody> {
        #[cfg(windows)]
        {
            self.copies.store.as_ref().ok_or_else(not_open)
        }
        #[cfg(not(windows))]
        {
            Err(not_open())
        }
    }

    pub(crate) fn copy_preview(&self, req: &RequestEnvelope) -> Result<Out, ErrorBody> {
        let request: CopyPreviewRequest = req.decode_payload()?;
        self.copies_store()?;
        #[cfg(windows)]
        {
            let sources: Vec<String> = request.sources.iter().map(|s| s.to_string()).collect();
            let destination = request.destination_node_id.to_string();
            let (preview, _) = self.copy_prepare(&sources, &destination)?;
            let mut id = [0u8; 9];
            getrandom::fill(&mut id)
                .map_err(|_| fail(ErrorCode::InternalError, "no randomness"))?;
            let operation_id = format!("cp_{}", hex(&id));
            let refusals: Vec<Value> = preview
                .refusals
                .iter()
                .take(256)
                .map(|r| json!({"code": code_name(r.code), "node_id": null}))
                .collect();
            let result = json!({
                "operation_id": operation_id,
                "state": if preview.refusals.is_empty() { "draft" } else { "refused" },
                "digest": digest_text(preview.manifest.digest()),
                "folder_name": preview.manifest.destination.folder_name,
                "files": preview.files,
                "folders": preview.folders,
                "bytes": preview.bytes.to_string(),
                "space_needed": preview.space_needed.to_string(),
                "space_reserve": preview.space_reserve.to_string(),
                "same_volume": preview.same_volume,
                "explicit_acl_items": preview.explicit_acl_items,
                "hard_link_names": preview.hard_link_names,
                "refusals": refusals,
                "refusal_count": preview.refusals.len(),
            });
            self.copies.pending.lock().unwrap().insert(
                operation_id,
                Pending {
                    sources,
                    destination,
                    preview,
                    approval: None,
                },
            );
            Ok(Out {
                result,
                catalog_rev: None,
                state_rev: None,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = request;
            Err(not_open())
        }
    }

    /// Synthetic sessions only (personal approvals over HTTP are refused before dispatch, G6).
    pub(crate) fn copy_approve(&self, req: &RequestEnvelope) -> Result<Out, ErrorBody> {
        let request: CopyConsentRequest = req.decode_payload()?;
        self.copies_store()?;
        let shown = parse_digest(request.digest.as_str())?;
        let mut pending = self.copies.pending.lock().unwrap();
        let p = pending
            .get_mut(request.operation_id.as_str())
            .ok_or_else(unknown)?;
        let now_ms = now_ns() / 1_000_000;
        let approval =
            loomward_operations::approve(&p.preview, shown, 0, now_ms).map_err(|code| {
                with_reason(
                    fail(ErrorCode::PermissionDenied, "the copy cannot be approved"),
                    code_name(code).as_str().unwrap_or("refused"),
                )
            })?;
        let result = json!({
            "operation_id": request.operation_id,
            "digest": request.digest,
            "approved_at": ts_ns(approval.approved_at_ms() * 1_000_000),
            "expires_at": ts_ns(approval.expires_at_ms() * 1_000_000),
        });
        p.approval = Some(approval);
        Ok(Out {
            result,
            catalog_rev: None,
            state_rev: None,
        })
    }

    pub(crate) fn copy_start(&self, req: &RequestEnvelope) -> Result<Out, ErrorBody> {
        let request: CopyConsentRequest = req.decode_payload()?;
        let store = self.copies_store()?.clone();
        #[cfg(windows)]
        {
            use loomward_windows::effects::exec;
            let id = request.operation_id.to_string();
            if self.copies.ops.lock().unwrap().contains_key(&id) {
                return Err(with_reason(
                    fail(
                        ErrorCode::InvalidRequest,
                        "this operation has started; abandon it to end it",
                    ),
                    "copy_already_started",
                ));
            }
            let shown = parse_digest(request.digest.as_str())?;
            // The approval is consumed here whatever follows: single use.
            let (sources, destination, approval) = {
                let mut pending = self.copies.pending.lock().unwrap();
                let p = pending.get_mut(&id).ok_or_else(unknown)?;
                let approval = p.approval.take().ok_or_else(|| {
                    with_reason(
                        fail(
                            ErrorCode::PermissionDenied,
                            "the copy has no unused approval",
                        ),
                        "approval_required",
                    )
                })?;
                (p.sources.clone(), p.destination.clone(), approval)
            };
            if approval.digest() != shown {
                return Err(with_reason(
                    fail(
                        ErrorCode::PermissionDenied,
                        "the digest is not the one approved",
                    ),
                    "digest_mismatch",
                ));
            }
            let (fresh, mut plan) = self.copy_prepare(&sources, &destination)?;
            let admitted = approval
                .admit(&fresh, 0, now_ns() / 1_000_000)
                .map_err(|code| {
                    with_reason(
                        fail(ErrorCode::PermissionDenied, "the copy cannot start"),
                        code_name(code).as_str().unwrap_or("refused"),
                    )
                })?;
            plan.operation_id = id.clone();
            let op = {
                let mut s = store.lock().unwrap();
                let op = s
                    .start(0, admitted)
                    .map_err(|_| fail(ErrorCode::InternalError, "the journal refused the start"))?;
                s.set_plan(op, &serde_json::to_string(&plan).unwrap_or_default())
                    .map_err(|_| fail(ErrorCode::InternalError, "the journal refused the plan"))?;
                op
            };
            self.copies.pending.lock().unwrap().remove(&id);
            self.copies.ops.lock().unwrap().insert(id.clone(), op);
            let control = Arc::new(exec::Control::new(
                self.copies
                    .removal
                    .load(std::sync::atomic::Ordering::SeqCst),
            ));
            let (worker_store, worker_control) = (store.clone(), control.clone());
            let thread = std::thread::Builder::new()
                .name("lw-copy".into())
                .spawn(move || {
                    let _ = exec::run(&worker_store, op, &worker_control);
                })
                .map_err(|_| {
                    // Nothing was created: settle the started operation as cancelled, never stuck.
                    let _ = exec::cancel(&store, op, &plan, &control);
                    fail(ErrorCode::ResourceBudget, "no worker")
                })?;
            self.copies
                .running
                .lock()
                .unwrap()
                .insert(op, Run { control, thread });
            self.copy_status_of(&id, op)
        }
        #[cfg(not(windows))]
        {
            let _ = (request, store);
            Err(not_open())
        }
    }

    fn copy_op(&self, id: &str) -> Result<u64, ErrorBody> {
        self.copies
            .ops
            .lock()
            .unwrap()
            .get(id)
            .copied()
            .ok_or_else(unknown)
    }

    /// Cancel (before publish) and abandon (blocked, needs_attention, interrupted) share one path: stop
    /// the run, journal `cancelling`, then the Q1 policy decides what happens to the staging.
    pub(crate) fn copy_cancel(&self, req: &RequestEnvelope) -> Result<Out, ErrorBody> {
        let request: CopyOperationRequest = req.decode_payload()?;
        let store = self.copies_store()?.clone();
        let id = request.operation_id.to_string();
        let op = self.copy_op(&id)?;
        #[cfg(windows)]
        {
            use loomward_windows::effects::exec;
            let running = self.copies.running.lock().unwrap().remove(&op);
            if let Some(run) = running {
                run.control.cancel();
                let _ = run.thread.join();
            }
            let plan: exec::Plan = store
                .lock()
                .unwrap()
                .plan(op)
                .ok()
                .flatten()
                .and_then(|p| serde_json::from_str(&p).ok())
                .ok_or_else(unknown)?;
            let control = exec::Control::new(
                self.copies
                    .removal
                    .load(std::sync::atomic::Ordering::SeqCst),
            );
            exec::cancel(&store, op, &plan, &control)
                .map_err(|_| fail(ErrorCode::InternalError, "the journal refused the cancel"))?;
        }
        #[cfg(not(windows))]
        let _ = store;
        self.copy_status_of(&id, op)
    }

    pub(crate) fn copy_status(&self, req: &RequestEnvelope) -> Result<Out, ErrorBody> {
        let request: CopyOperationRequest = req.decode_payload()?;
        self.copies_store()?;
        let id = request.operation_id.to_string();
        let op = self.copy_op(&id)?;
        self.copy_status_of(&id, op)
    }

    fn copy_status_of(&self, id: &str, op: u64) -> Result<Out, ErrorBody> {
        #[cfg(windows)]
        {
            let store = self.copies_store()?;
            let outcome = loomward_windows::effects::exec::status(store, op)
                .map_err(|_| fail(ErrorCode::InternalError, "the journal does not replay"))?;
            let digest = store
                .lock()
                .unwrap()
                .replay(op)
                .ok()
                .and_then(|r| r.ok())
                .map(|r| r.digest)
                .unwrap_or_default();
            Ok(Out {
                result: json!({
                    "operation_id": id,
                    "state": code_name(outcome.state),
                    "digest": digest_text(digest),
                    "items_total": outcome.items_total,
                    "items_verified": outcome.items_verified,
                    "staging_folder": outcome.staging_left,
                    "leftover_staging_folders": outcome.leftover_staging.iter().take(64).collect::<Vec<_>>(),
                    // Item refusals name a manifest entry, not a catalogue node: node_id stays null.
                    "refusal": outcome.refusal.map(|(code, _)| json!({"code": code_name(code), "node_id": null})),
                }),
                catalog_rev: None,
                state_rev: None,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = (id, op);
            Err(not_open())
        }
    }
}
