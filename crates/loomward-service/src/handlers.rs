//! One handler per wave-2 command. Each returns the result object and the revisions it was read
//! or committed at; `Inner::respond` validates the object against the command's result type.

use crate::cursors::{Position, Resume};
use crate::{
    catalog_error, db, fail, now, now_ns, paths, refusal_name, sql_error, ts_ns, with_reason,
    Inner, Out,
};
use loomward_catalog::{
    Basis as CBasis, ChildrenRequest, NodeKey, Reader, SliceRequest, Sort, WriteCommand,
};
use loomward_engine::EngineResult;
use loomward_protocol::*;
use serde::Serialize;
use serde_json::{json, Value};

type Handled = Result<Out, ErrorBody>;

fn out(result: Value, revs: Option<(i64, i64)>) -> Out {
    Out {
        result,
        catalog_rev: revs.map(|r| r.0),
        state_rev: revs.map(|r| r.1),
    }
}

/// Engine-only results carry no revision.
fn engine<T: Serialize>(r: EngineResult<T>) -> Handled {
    let value = r.map_err(ErrorBody::from)?;
    Ok(out(
        serde_json::to_value(value).map_err(|_| fail(ErrorCode::InternalError, "encode"))?,
        None,
    ))
}

fn unavailable(what: &str) -> Handled {
    Err(fail(ErrorCode::CapabilityUnavailable, what))
}

fn basis(b: Basis) -> CBasis {
    match b {
        Basis::Logical => CBasis::Logical,
        Basis::Allocated => CBasis::Allocated,
    }
}

fn grant_number(id: &str) -> Result<i64, ErrorBody> {
    id.strip_prefix("gr_")
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| fail(ErrorCode::NotFound, "unknown grant"))
}

fn encode<T: Serialize>(v: &T) -> Result<Value, ErrorBody> {
    serde_json::to_value(v).map_err(|_| fail(ErrorCode::InternalError, "encode"))
}

/// The request with its paging fields removed: the binding a cursor must match (section 5).
fn binding(request: &RequestEnvelope) -> String {
    let mut p = request.payload.clone();
    p.remove("cursor");
    p.remove("limit");
    Value::Object(p).to_string()
}

fn stale(message: &str, reason: &str) -> ErrorBody {
    with_reason(fail(ErrorCode::StaleGeneration, message), reason)
}

impl Inner {
    pub(crate) fn handle(
        &self,
        req: &RequestEnvelope,
        command: Command,
        ctx: &CallContext,
    ) -> Handled {
        #[cfg(test)]
        self.hooks.before(command);
        use Command::*;
        let handled = match command {
            SessionHello => self.hello(ctx),
            HealthGet => self.health(),
            RootsList => self.roots_list(),
            RootsRequestGrant => self.request_grant(ctx),
            RootsRevoke => {
                let r: RootRevokeRequest = req.decode_payload()?;
                let grant = self.grant_of_root(self.root_row(&r.root_id)?)?;
                let (at, revs) = self.revoke(grant)?;
                Ok(out(json!({"root_id": r.root_id, "revoked_at": at, "purged": false}), Some(revs)))
            }
            GrantsRevoke => {
                let r: GrantRevokeRequest = req.decode_payload()?;
                let (at, revs) = self.revoke(grant_number(&r.grant_id)?)?;
                Ok(out(json!({"grant_id": r.grant_id, "revoked_at": at}), Some(revs)))
            }
            GrantsList => self.grants_list(),
            VolumesList => {
                let revs = self.revisions()?;
                Ok(out(json!({"volumes": self.volumes(None)?}), Some(revs)))
            }
            VolumesDeclareTier => self.declare_tier(req.decode_payload()?, req.expected_state_rev),
            ScanStart => unavailable("scanning is not wired to the catalogue yet (catalogue scan sink and grant proof pending)"),
            ScanCancel => engine(self.engine.scan_cancel(&req.decode_payload::<JobRefRequest>()?.job_id).map(|job| JobResult { job })),
            JobsGet => engine(self.engine.job_status(&req.decode_payload::<JobRefRequest>()?.job_id).map(|job| JobResult { job })),
            JobsList => engine(self.engine.job_list(&req.decode_payload()?)),
            TreeSlice => self.tree_slice(req),
            TreeChildren => self.tree_children(req),
            TreePath => self.node_read(req, |reader, key| reader.path(key).map(|p| encode(&p))),
            NodeInspect => self.node_read(req, |reader, key| reader.inspect(key).map(|d| encode(&d))),
            SearchQuery => self.search(req),
            StatsBreakdown => self.breakdown(req),
            TiersModel | PlacementCandidates | PlacementSimulate | ProposalsList | ProposalsGet => {
                unavailable("placement is not wired to the catalogue yet")
            }
            TelemetrySubscribe => engine(self.engine.telemetry_lease(&req.decode_payload()?)),
            TelemetryUnsubscribe => engine(self.engine.telemetry_release(&req.decode_payload()?)),
            TelemetrySnapshot => engine(self.engine.telemetry_snapshot(&req.decode_payload()?)),
            ProcessesList => engine(self.engine.processes_list(&req.decode_payload()?)),
            ProcessesExplain => engine(self.engine.processes_explain(&req.decode_payload()?)),
            BudgetsGet => engine(self.engine.budgets_get()),
            BudgetsSet => engine(self.engine.budgets_set(&req.decode_payload()?)),
            _ => unavailable("this command arrives with wave 3 (lane L20)"),
        };
        #[cfg(test)]
        self.hooks.gate(command, "after");
        handled
    }

    fn hello(&self, ctx: &CallContext) -> Handled {
        let personal = self.dataset == DatasetClass::Personal;
        let f = false;
        Ok(out(
            json!({
                "protocol": "loomward/3",
                "engine_version": concat!("loomward-service/", env!("CARGO_PKG_VERSION")),
                "adapter": ctx.adapter,
                "dataset_class": self.dataset,
                "session_started_at": self.started_at,
                "enumeration_strategy": "file_id_extd_directory_info",
                "capabilities": {
                    "observation": {
                        // Honest: scans are not wired yet; telemetry observes personal hosts only;
                        // the teacher stays disabled (confinement_not_enforced).
                        "metadata_scan": f,
                        "process_observation": personal,
                        "gpu_observation": f,
                        "disk_io_observation": personal,
                        "teacher_disclosure": f,
                    },
                    "effects": {
                        "file_move": f, "file_delete": f, "file_rename": f, "file_write": f,
                        "content_read": f, "process_kill": f, "process_suspend": f,
                        "process_priority": f, "memory_trim": f, "uninstall": f, "elevation": f,
                    },
                },
                "features": {
                    "grant_picker": personal && ctx.dialogs.is_some(),
                    "disclosure_dialog": f,
                    "teacher_available": f,
                    "telemetry_available": personal,
                    "gpu_available": f,
                },
                "limits": {
                    "max_request_bytes": limits::MAX_REQUEST_BYTES,
                    "max_slice_nodes": limits::MAX_SLICE_NODES,
                    "max_page_items": limits::MAX_PAGE_ITEMS,
                },
            }),
            None,
        ))
    }

    /// `Health.engine` is read now from the process itself (#163), never a retained telemetry
    /// sample under a fresh `observed_at`. Unknown stays `null`.
    fn health(&self) -> Handled {
        let own = match loomward_telemetry::own_usage() {
            loomward_telemetry::Observation::Observed { value } => Some(value),
            _ => None,
        };
        let size = |name: &str| {
            std::fs::metadata(self.state_dir.join(name))
                .ok()
                .map(|m| m.len().to_string())
        };
        let (files, dirs) = db::counts(&self.db()).map_err(sql_error)?;
        let running = self
            .engine
            .job_list(&decode_exact(json!({"limit": 50}))?)
            .map(|l| {
                l.jobs
                    .iter()
                    .filter(|j| {
                        matches!(
                            j.state,
                            JobState::Queued | JobState::Running | JobState::CancelRequested
                        )
                    })
                    .count()
            })
            .unwrap_or(0);
        let revs = self.revisions()?;
        Ok(out(
            json!({
                "observed_at": now(),
                "engine": {
                    "private_commit_bytes": own.as_ref().map(|o| o.private_commit_bytes.to_string()),
                    "working_set_bytes": null,
                    "cpu_seconds": own.as_ref().map(|o| o.cpu_seconds),
                    "threads": null,
                },
                "catalog": {
                    "schema_version": loomward_catalog::SCHEMA_VERSION,
                    "db_bytes": size("catalog.db"),
                    "wal_bytes": size("catalog.db-wal"),
                    "files": files,
                    "dirs": dirs,
                    // The writer queue is bounded in bytes; depth and capacity are both bytes.
                    "writer_queue_depth": self.catalog.writer().queued_bytes(),
                    "writer_queue_capacity": loomward_catalog::QUEUE_BYTES,
                },
                "jobs_running": running,
                "last_error": null,
                "warnings": [],
            }),
            Some(revs),
        ))
    }

    fn root_json(&self, r: &db::RootRow, reader: Option<&mut Reader>) -> Result<Value, ErrorBody> {
        let (state, coverage) = match r.state.as_str() {
            "never_scanned" => ("never_scanned", "unscanned"),
            "scanning" => ("scanning", "partial"),
            "complete" => ("complete", "complete"),
            "partial" | "repairing" => ("partial", "partial"),
            "stale" => ("stale", "stale"),
            _ => ("failed", "unknown"),
        };
        let active = r.grant_state == "active";
        let totals = match (reader, r.dir, active && r.state != "never_scanned") {
            (Some(reader), Some(dir), true) => reader
                .inspect(NodeKey::Dir(dir))
                .ok()
                .and_then(|d| d.subtree)
                .map(|s| encode(&s))
                .transpose()?,
            _ => None,
        };
        let display: String = r.display_path.chars().take(1024).collect();
        Ok(json!({
            "root_id": self.root_id(r.root),
            "display_path": {"text": display, "truncated": r.display_path.chars().count() > 1024},
            "origin": r.origin,
            "dataset_class": self.dataset,
            "volume_id": r.volume.map(|v| self.volume_id(v)),
            "granted_at": ts_ns(r.granted_at_ns),
            "granted_via": r.granted_via,
            "grant_state": r.grant_state,
            "scan": {
                "state": if active { state } else { "stale" },
                "last_job_id": null,
                "generation": r.generation.map(|g| g.to_string()),
                "finished_at": r.finished_at_ns.map(ts_ns),
                "coverage": if active { coverage } else { "stale" },
            },
            "totals": totals,
        }))
    }

    fn root_list(&self) -> Result<Value, ErrorBody> {
        let rows = db::roots(&self.db(), None).map_err(sql_error)?;
        let mut reader = self.catalog.reader().map_err(catalog_error)?;
        let roots = rows
            .iter()
            .map(|r| self.root_json(r, Some(&mut reader)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({"roots": roots}))
    }

    fn roots_list(&self) -> Handled {
        let revs = self.revisions()?;
        Ok(out(self.root_list()?, Some(revs)))
    }

    fn grant_of_root(&self, root: i64) -> Result<i64, ErrorBody> {
        db::roots(&self.db(), Some(root))
            .map_err(sql_error)?
            .first()
            .map(|r| r.grant)
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown root"))
    }

    /// Revocation (docs/41 section 12): commit in `state.db` at FULL first (the catalogue writer
    /// commits the grant row before it cancels the root's run and refuses any later message for
    /// the grant), then cancel the engine's jobs for the root, then fence the engine's writer, and
    /// only then publish `roots.changed`. Monotone: a revoked grant returns its original time.
    pub(crate) fn revoke(&self, grant: i64) -> Result<(Timestamp, (i64, i64)), ErrorBody> {
        let _m = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        let (state, revoked_at): (String, Option<i64>) = self
            .db()
            .query_row(
                "SELECT state,revoked_at_ns FROM st.root_grant WHERE id=?1",
                [grant],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|_| fail(ErrorCode::NotFound, "unknown grant"))?;
        if state != "active" {
            let at = revoked_at.ok_or_else(|| {
                fail(
                    ErrorCode::PermissionDenied,
                    "the grant's root identity changed",
                )
            })?;
            return Ok((ts_ns(at), self.revisions()?));
        }
        let at = now_ns();
        self.catalog
            .writer()
            .call(WriteCommand::RevokeGrant {
                grant_id: grant,
                revoked_at_ns: at,
            })
            .map_err(catalog_error)?;
        if let Some(root) = db::root_for_grant(&self.db(), grant).map_err(sql_error)? {
            let id = RootId::new(self.root_id(root)).expect("rt_ id");
            // The revocation is durable whatever happens next; a missing scan writer or job only
            // means there is nothing to cancel or fence.
            let _ = self.engine.scan_cancel_root(&id);
            let _ = self.engine.scan_writer_fence(&id);
        }
        let revs = self.revisions()?;
        if let Ok(list) = self.root_list() {
            self.publish(EventName::RootsChanged, list, Some(revs.0), Some(revs.1));
        }
        Ok((ts_ns(at), revs))
    }

    fn grants_list(&self) -> Handled {
        let revs = self.revisions()?;
        let rows = db::grant_rows(&self.db()).map_err(sql_error)?;
        let grants: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({
                    "grant_id": format!("gr_{}", r.grant),
                    "kind": "metadata_root",
                    "root_id": self.root_id(r.root),
                    "granted_at": ts_ns(r.granted_at_ns),
                    "granted_via": r.granted_via,
                    "revoked_at": r.revoked_at_ns.map(ts_ns),
                })
            })
            .collect();
        Ok(out(json!({"grants": grants}), Some(revs)))
    }

    /// Desktop only. Personal sessions only (a synthetic session refuses before any dialog). The
    /// folder comes from the native picker, is checked exactly like a grant root (#167), and is
    /// granted under the same provenance rules. One dialog at a time.
    fn request_grant(&self, ctx: &CallContext) -> Handled {
        let refused = |r: RootGrantResultRefusal| {
            Ok(out(
                json!({"outcome": "refused", "root": null, "refusal": refusal_name(r)}),
                None,
            ))
        };
        if self.dataset == DatasetClass::Synthetic {
            return refused(RootGrantResultRefusal::SyntheticSessionRequiresLabRoot);
        }
        let dialogs = ctx.dialogs.as_ref().ok_or_else(|| {
            fail(
                ErrorCode::CapabilityUnavailable,
                "no native dialogs on this adapter",
            )
        })?;
        let Ok(_dialog) = self.dialog.try_lock() else {
            return Err(fail(ErrorCode::Busy, "a native dialog is already open"));
        };
        let Some(picked) = dialogs.pick_folder() else {
            return Ok(out(
                json!({"outcome": "cancelled_by_user", "root": null, "refusal": null}),
                None,
            ));
        };
        // Nested or overlapping roots are refused like a repeated grant.
        let existing: Vec<std::path::PathBuf> = db::roots(&self.db(), None)
            .map_err(sql_error)?
            .iter()
            .filter(|r| r.grant_state == "active")
            .filter_map(|r| std::path::Path::new(&r.display_path).canonicalize().ok())
            .collect();
        let path = match paths::check_root(&picked, Some(&self.state_dir), &existing, true) {
            Ok(p) => p,
            Err(r) => return refused(r),
        };
        let (root, fresh) = match self.grant(&path, "desktop_picker") {
            Ok(g) => g,
            Err(crate::Refused::Rule(r)) => return refused(r),
            Err(crate::Refused::RootLimit) => {
                return Err(with_reason(
                    fail(
                        ErrorCode::ResourceBudget,
                        "64 roots are already granted; revoke one before granting another",
                    ),
                    "root_limit_reached",
                ))
            }
        };
        if !fresh {
            return refused(RootGrantResultRefusal::AlreadyGranted);
        }
        let revs = self.revisions()?;
        let rows = db::roots(&self.db(), Some(root)).map_err(sql_error)?;
        let row = rows
            .first()
            .ok_or_else(|| fail(ErrorCode::InternalError, "grant vanished"))?;
        let root_json = self.root_json(row, None)?;
        if let Ok(list) = self.root_list() {
            self.publish(EventName::RootsChanged, list, Some(revs.0), Some(revs.1));
        }
        Ok(out(
            json!({"outcome": "granted", "root": root_json, "refusal": null}),
            Some(revs),
        ))
    }

    fn volumes(&self, only: Option<i64>) -> Result<Vec<Value>, ErrorBody> {
        let rows = db::volumes(&self.db(), only).map_err(sql_error)?;
        Ok(rows
            .iter()
            .map(|v| {
                let declared = v.declared.flatten();
                json!({
                    "volume_id": self.volume_id(v.id),
                    "display_name": v.display_name.chars().take(64).collect::<String>(),
                    "mount_points": [],
                    "filesystem": v.filesystem.as_ref().map(|f| f.chars().take(32).collect::<String>()),
                    "label": null,
                    "online": v.online,
                    "read_only": null,
                    "removable": null,
                    "capacity_bytes": null,
                    "free_bytes": null,
                    "device": {"bus_type": "unknown", "seek_penalty": null, "multiple_disks": null, "basis": "unavailable"},
                    "tier": {
                        "tier": declared,
                        "basis": if declared.is_some() { "declared" } else { "unknown" },
                        "declared_tier": declared,
                        "hint_tier": null,
                        "note": if declared.is_some() { "Declared by the owner." } else { "No declaration and no device hint yet." },
                    },
                    "features": {"file_ids_128": null, "hard_links": null, "sparse_files": null, "compression": null, "reparse_points": null, "usn_journal": null},
                    "observed_at": ts_ns(v.observed_at_ns),
                })
            })
            .collect())
    }

    /// Append-only owner preference under the optional `expected_state_rev` precondition
    /// (section 3 step 3): a mismatch writes nothing and is `stale_generation` /
    /// `state_rev_changed`.
    fn declare_tier(&self, r: DeclareTierRequest, expected: Option<Generation>) -> Handled {
        let volume = self.volume_row(&r.volume_id)?;
        let _m = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(test)]
        self.hooks.gate(Command::VolumesDeclareTier, "locked");
        let key = db::volumes(&self.db(), Some(volume))
            .map_err(sql_error)?
            .first()
            .map(|v| v.key.clone())
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown volume"))?;
        let at = now_ns();
        let written = db::declare_tier(
            &mut self.db(),
            &key,
            r.tier.map(|t| t.get()),
            expected.map(|g| g.get() as i64),
            at,
        )
        .map_err(sql_error)?;
        let state_rev = written.map_err(|_| {
            stale(
                "state.db changed since your read; re-read and ask the owner again",
                "state_rev_changed",
            )
        })?;
        let catalog_rev = self.revisions()?.0;
        let volume_json = self
            .volumes(Some(volume))?
            .pop()
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown volume"))?;
        let all = self.volumes(None)?;
        self.publish(
            EventName::VolumesChanged,
            json!({"volumes": all}),
            None,
            Some(state_rev),
        );
        Ok(Out {
            result: json!({"volume": volume_json, "preference_recorded_at": ts_ns(at)}),
            catalog_rev: Some(catalog_rev),
            state_rev: Some(state_rev),
        })
    }

    /// `expected_generation` on a root-anchored read (section 5): the anchor's root generation
    /// must still be the one the caller holds.
    fn check_generation(
        &self,
        expected: Option<Generation>,
        key: NodeKey,
    ) -> Result<(), ErrorBody> {
        let Some(expected) = expected else {
            return Ok(());
        };
        let (dir, file) = match key {
            NodeKey::Dir(d) => (Some(d), None),
            NodeKey::File(f) => (None, Some(f)),
            _ => {
                return Err(fail(
                    ErrorCode::InvalidRequest,
                    "expected_generation needs an anchor inside one root",
                ))
            }
        };
        let conn = self.db();
        let root = db::root_of(&conn, dir, file)
            .map_err(sql_error)?
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown node"))?;
        if db::root_generation(&conn, root).map_err(sql_error)? != Some(expected.get() as i64) {
            return Err(stale(
                "the root has a newer generation",
                "generation_changed",
            ));
        }
        Ok(())
    }

    fn sealed(&self, value: Value, revs: (i64, i64)) -> Handled {
        let mut value = value;
        self.seal(&mut value)?;
        Ok(out(value, Some(revs)))
    }

    fn node_read(
        &self,
        req: &RequestEnvelope,
        read: impl FnOnce(&mut Reader, NodeKey) -> loomward_catalog::Result<Result<Value, ErrorBody>>,
    ) -> Handled {
        let r: NodeRefRequest = req.decode_payload()?;
        let revs = self.revisions()?;
        let mut reader = self.catalog.reader().map_err(catalog_error)?;
        let key = self.node(&r.node_id, &mut reader)?;
        if !matches!(key, NodeKey::Dir(_) | NodeKey::File(_)) {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "this command needs a directory or file node",
            ));
        }
        self.check_generation(req.expected_generation, key)?;
        let value = read(&mut reader, key).map_err(catalog_error)??;
        self.sealed(value, revs)
    }

    fn tree_slice(&self, req: &RequestEnvelope) -> Handled {
        let r: TreeSliceRequest = req.decode_payload()?;
        let revs = self.revisions()?;
        let mut reader = self.catalog.reader().map_err(catalog_error)?;
        let anchor = match &r.anchor {
            Anchor::Atlas {} => NodeKey::Atlas,
            Anchor::Root { root_id } => {
                let dir = db::active_root_dir(&self.db(), self.root_row(root_id)?)
                    .map_err(sql_error)?
                    .ok_or_else(|| fail(ErrorCode::NotFound, "unknown root"))?;
                NodeKey::Dir(dir)
            }
            Anchor::Node { node_id } => self.node(node_id, &mut reader)?,
        };
        if !matches!(r.anchor, Anchor::Atlas {}) {
            self.check_generation(req.expected_generation, anchor)?;
        }
        let slice = reader
            .slice(&SliceRequest {
                anchor,
                depth: r.depth.get() as usize,
                max_nodes: r.max_nodes.get() as usize,
                min_share: r.min_share.get(),
                basis: basis(r.basis),
                include_files: r.include_files,
            })
            .map_err(catalog_error)?;
        self.sealed(encode(&slice)?, revs)
    }

    fn tree_children(&self, req: &RequestEnvelope) -> Handled {
        let r: TreeChildrenRequest = req.decode_payload()?;
        let revs = self.revisions()?;
        let mut reader = self.catalog.reader().map_err(catalog_error)?;
        let key = self.node(&r.node_id, &mut reader)?;
        let NodeKey::Dir(dir) = key else {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "tree.children needs a directory node",
            ));
        };
        self.check_generation(req.expected_generation, key)?;
        let bind = binding(req);
        let cursor = match &r.cursor {
            None => None,
            Some(handle) => match self
                .cursors
                .resume(handle, Command::TreeChildren, &bind, None)
            {
                Ok(Position::Children(c)) => Some(c),
                Ok(_) | Err(Resume::Mismatch) => {
                    return Err(fail(
                        ErrorCode::InvalidRequest,
                        "the cursor belongs to another request",
                    ))
                }
                Err(_) => {
                    return Err(stale(
                        "the cursor expired; restart from the first page",
                        "cursor_expired",
                    ))
                }
            },
        };
        let page = reader
            .children(&ChildrenRequest {
                dir_id: dir,
                sort: match r.sort {
                    TreeChildrenRequestSort::SizeDesc => Sort::SizeDesc,
                    TreeChildrenRequestSort::NameAsc => Sort::NameAsc,
                    TreeChildrenRequestSort::ModifiedDesc => Sort::ModifiedDesc,
                },
                basis: basis(r.basis),
                limit: r.limit.get() as usize,
                cursor,
            })
            .map_err(catalog_error)?;
        let next = page.next_cursor.map(|c| {
            self.cursors
                .issue(Command::TreeChildren, bind, Position::Children(c), None)
        });
        let value = json!({
            "anchor": page.anchor,
            "generation": page.generation,
            "items": encode(&page.items)?,
            "next_cursor": next,
            "total": page.total,
            "budget_hit": page.budget_hit,
        });
        self.sealed(value, revs)
    }

    fn search(&self, req: &RequestEnvelope) -> Handled {
        let r: SearchRequest = req.decode_payload()?;
        let revs = self.revisions()?;
        let root = r.root_id.as_ref().map(|id| self.root_row(id)).transpose()?;
        let bind = binding(req);
        let cursor = match &r.cursor {
            None => None,
            Some(handle) => match self
                .cursors
                .resume(handle, Command::SearchQuery, &bind, None)
            {
                Ok(Position::Search(c)) => Some(c),
                Ok(_) | Err(Resume::Mismatch) => {
                    return Err(fail(
                        ErrorCode::InvalidRequest,
                        "the cursor belongs to another request",
                    ))
                }
                Err(_) => {
                    return Err(stale(
                        "the cursor expired; restart from the first page",
                        "cursor_expired",
                    ))
                }
            },
        };
        let mut reader = self.catalog.reader().map_err(catalog_error)?;
        let page = reader
            .search(&loomward_catalog::SearchRequest {
                root_id: root,
                text: r.text.to_string(),
                extension: r.extension.as_ref().map(|e| e.to_string()),
                min_bytes: r.min_bytes.map(|b| b.get()),
                kind: match r.kind {
                    SearchRequestKind::Any => "any",
                    SearchRequestKind::File => "file",
                    SearchRequestKind::Dir => "dir",
                }
                .into(),
                limit: r.limit.get() as usize,
                cursor,
                // SQLite VM steps; the deadline still bounds the caller's wait.
                work_budget: 50_000_000,
            })
            .map_err(catalog_error)?;
        let next = page.next_cursor.map(|c| {
            self.cursors
                .issue(Command::SearchQuery, bind, Position::Search(c), None)
        });
        let value = json!({
            "anchor": null,
            "generation": page.generation,
            "items": encode(&page.items)?,
            "next_cursor": next,
            "total": page.total,
            "budget_hit": page.budget_hit,
        });
        self.sealed(value, revs)
    }

    fn breakdown(&self, req: &RequestEnvelope) -> Handled {
        let r: BreakdownRequest = req.decode_payload()?;
        let revs = self.revisions()?;
        let mut reader = self.catalog.reader().map_err(catalog_error)?;
        let key = self.node(&r.node_id, &mut reader)?;
        let NodeKey::Dir(dir) = key else {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "stats.breakdown needs a directory node",
            ));
        };
        self.check_generation(req.expected_generation, key)?;
        let by = match r.by {
            BreakdownRequestBy::ExtFamily => "ext_family",
            BreakdownRequestBy::Extension => "extension",
            BreakdownRequestBy::AgeBand => "age_band",
        };
        let b = reader
            .breakdown(dir, by, basis(r.basis), r.limit.get() as usize, 50_000_000)
            .map_err(catalog_error)?;
        self.sealed(encode(&b)?, revs)
    }
}
