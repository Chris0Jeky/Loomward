# Implementation backlog

100 local, dependency-linked issue specifications: the original 64 plus 36 expansion tasks. These are **not remote GitHub issue numbers** and all remain planned production continuation. The runnable reference work is recorded separately in `docs/37-expansion-verification.md`.

The first eight manifest entries preserve the original native critical path. Publishing helpers remain dry-run by default, at most eight unless explicitly expanded. `M1I` covers integration foundations, `M2I` advanced evidence/workload integration, and `MX` optional experiments; none bypass the original native safety milestones.

| ID | Task | Milestone | Priority | Dependencies |
|---|---|---|---|---|
| [LW-001](issues/LW-001.md) | Verify the reference workbench on Windows | M1 | P0 | None |
| [LW-002](issues/LW-002.md) | Compile, format and lock the native Rust workspace | M1 | P0 | LW-001 |
| [LW-064](issues/LW-064.md) | Build a disposable Windows filesystem fixture laboratory | M1 | P0 | LW-001, LW-002 |
| [LW-003](issues/LW-003.md) | Implement handle-derived Windows object identity | M1 | P0 | LW-002, LW-064 |
| [LW-004](issues/LW-004.md) | Inventory volume identity and capabilities | M1 | P0 | LW-002, LW-064 |
| [LW-016](issues/LW-016.md) | Standardise the exact v2 wire protocol | M1 | P0 | LW-001, LW-002 |
| [LW-005](issues/LW-005.md) | Build the local native catalogue and migration harness | M1 | P0 | LW-003, LW-004, LW-016 |
| [LW-006](issues/LW-006.md) | Add bounded native scan jobs and cancellation | M1 | P0 | LW-005 |
| [LW-007](issues/LW-007.md) | Implement optional USN catch-up with gap recovery | M1 | P1 | LW-006 |
| [LW-008](issues/LW-008.md) | Implement watcher overflow reconciliation | M1 | P0 | LW-006 |
| [LW-009](issues/LW-009.md) | Separate logical, allocated and unique-object accounting | M1 | P0 | LW-003, LW-004 |
| [LW-010](issues/LW-010.md) | Detect project anchors and confirm atomic groups | M2 | P1 | LW-006 |
| [LW-011](issues/LW-011.md) | Implement paginated native queries and large explorer | M1 | P0 | LW-005, LW-016 |
| [LW-012](issues/LW-012.md) | Port duplicate inspection behind the native identity boundary | M1 | P0 | LW-003, LW-006, LW-009 |
| [LW-013](issues/LW-013.md) | Explain duplicate groups and reclaimability uncertainty | M2 | P1 | LW-012, LW-010 |
| [LW-014](issues/LW-014.md) | Build a sandboxed bounded extraction worker | M2 | P0 | LW-003, LW-017 |
| [LW-015](issues/LW-015.md) | Add safe previews without shell-handler loading | M2 | P1 | LW-014, LW-018 |
| [LW-017](issues/LW-017.md) | Implement authenticated per-user native IPC | M1 | P0 | LW-003, LW-016 |
| [LW-018](issues/LW-018.md) | Connect the Tauri shell to read-only native view models | M1 | P0 | LW-011, LW-017 |
| [LW-019](issues/LW-019.md) | Complete Windows accessibility and scaling QA | M1 | P1 | LW-018 |
| [LW-020](issues/LW-020.md) | Implement private state retention and export controls | M2 | P0 | LW-005, LW-016 |
| [LW-021](issues/LW-021.md) | Add feedback correction, retraction and bounded training windows | M2 | P0 | LW-020 |
| [LW-022](issues/LW-022.md) | Build a versioned user taxonomy editor | M2 | P1 | LW-021 |
| [LW-023](issues/LW-023.md) | Integrate the optional teacher into a review queue | M2 | P0 | LW-021, LW-022 |
| [LW-024](issues/LW-024.md) | Evaluate embeddings against the token student | M2 | P1 | LW-021, LW-025 |
| [LW-025](issues/LW-025.md) | Create grouped temporal ML evaluation and calibration reports | M2 | P0 | LW-021 |
| [LW-026](issues/LW-026.md) | Implement interruption budgets and active review selection | M2 | P1 | LW-025 |
| [LW-027](issues/LW-027.md) | Implement model promotion, drift and rollback | M2 | P0 | LW-023, LW-025 |
| [LW-028](issues/LW-028.md) | Add explicit usage observation with coverage semantics | M2 | P0 | LW-003, LW-018, LW-050 |
| [LW-029](issues/LW-029.md) | Train and evaluate a bounded access-heat model | M4 | P1 | LW-025, LW-028 |
| [LW-030](issues/LW-030.md) | Build the storage simulator and small exact oracle | M2 | P1 | LW-002, LW-009 |
| [LW-031](issues/LW-031.md) | Plan bidirectional tiering with hysteresis | M4 | P1 | LW-029, LW-030, LW-033 |
| [LW-032](issues/LW-032.md) | Schedule storage work under device and foreground budgets | M4 | P0 | LW-004, LW-030, LW-050 |
| [LW-033](issues/LW-033.md) | Define path continuity and provider relocation contracts | M3 | P0 | LW-003, LW-010, LW-016 |
| [LW-034](issues/LW-034.md) | Manage owned model leases and memory budgets | M6 | P1 | LW-023, LW-050 |
| [LW-035](issues/LW-035.md) | Implement a typed backup-provider adapter | M3 | P0 | LW-003, LW-004, LW-017 |
| [LW-036](issues/LW-036.md) | Verify backup restore and Windows metadata fidelity | M3 | P0 | LW-035, LW-064 |
| [LW-037](issues/LW-037.md) | Design explicit retention and source-retirement policies | M3 | P0 | LW-036 |
| [LW-038](issues/LW-038.md) | Bind consent to exact canonical plans | M3 | P0 | LW-003, LW-016, LW-020, LW-033 |
| [LW-039](issues/LW-039.md) | Implement the durable operation journal and recovery reader | M3 | P0 | LW-005, LW-038 |
| [LW-040](issues/LW-040.md) | Implement copy-only supported-file transactions | M3 | P0 | LW-036, LW-039 |
| [LW-041](issues/LW-041.md) | Run the Windows crash and fault-injection matrix | M3 | P0 | LW-040, LW-064 |
| [LW-042](issues/LW-042.md) | Gate the first narrowly supported approved move | M4 | P0 | LW-037, LW-041 |
| [LW-043](issues/LW-043.md) | Implement conflict-aware undo as a fresh plan | M4 | P0 | LW-042 |
| [LW-044](issues/LW-044.md) | Resolve known folders and desktop ownership | M5 | P1 | LW-003, LW-017 |
| [LW-045](issues/LW-045.md) | Inspect shortcut health without execution | M5 | P0 | LW-014, LW-044 |
| [LW-046](issues/LW-046.md) | Build virtual desktop workspaces and session views | M5 | P1 | LW-018, LW-045 |
| [LW-047](issues/LW-047.md) | Implement side-effect-free application inventory | M5 | P0 | LW-017 |
| [LW-048](issues/LW-048.md) | Offer usage-qualified application cleanup advice | M5 | P1 | LW-028, LW-047 |
| [LW-049](issues/LW-049.md) | Design a reviewed provider-specific uninstall handoff | M5 | P0 | LW-038, LW-047, LW-054 |
| [LW-050](issues/LW-050.md) | Implement native read-only process telemetry | M6 | P0 | LW-003, LW-017 |
| [LW-051](issues/LW-051.md) | Add resource trends and evidence-led explanations | M6 | P1 | LW-050 |
| [LW-052](issues/LW-052.md) | Budget Loomward-owned workers with Job Objects | M6 | P0 | LW-050 |
| [LW-053](issues/LW-053.md) | Add reversible process policies with expiry | M6 | P0 | LW-052, LW-054 |
| [LW-054](issues/LW-054.md) | Define process-control identity and protection gates | M6 | P0 | LW-038, LW-050 |
| [LW-055](issues/LW-055.md) | Create redacted diagnostic bundles | M1 | P0 | LW-020 |
| [LW-056](issues/LW-056.md) | Benchmark against current specialist tools fairly | M1 | P1 | LW-001, LW-006, LW-011, LW-050 |
| [LW-057](issues/LW-057.md) | Create reviewed Windows distributables and release provenance | M1 | P0 | LW-002, LW-018, LW-055, LW-063 |
| [LW-058](issues/LW-058.md) | Expose a read-only external-agent API | M5 | P1 | LW-016, LW-017, LW-020 |
| [LW-059](issues/LW-059.md) | Explore semantic and perceptual similarity safely | M2 | P2 | LW-014, LW-024, LW-025 |
| [LW-060](issues/LW-060.md) | Prototype capacity-growth and anomaly forecasts | M2 | P2 | LW-005, LW-025, LW-028 |
| [LW-061](issues/LW-061.md) | Add explicit developer-workspace providers | M5 | P1 | LW-010, LW-033, LW-047 |
| [LW-062](issues/LW-062.md) | Run an observation-only personal field trial | M2 | P0 | LW-018, LW-027, LW-056 |
| [LW-063](issues/LW-063.md) | Confirm name, licence and public repository governance | M1 | P0 | LW-001 |
| [LW-065](issues/LW-065.md) | Run a real-host dual-era MCP compatibility matrix | M1I | P0 | LW-058 |
| [LW-066](issues/LW-066.md) | Adopt a production MCP SDK behind the bounded ToolService | M1I | P1 | LW-065, LW-067 |
| [LW-067](issues/LW-067.md) | Persist and revoke per-client disclosure grants | M1I | P0 | LW-017, LW-020 |
| [LW-068](issues/LW-068.md) | Replace full-state explorer loads with paged reference views | M1I | P1 | LW-011 |
| [LW-069](issues/LW-069.md) | Port scoped catalogue projections to the native query layer | M1I | P0 | LW-005, LW-011, LW-067, LW-068 |
| [LW-070](issues/LW-070.md) | Represent group lineage and evidence in relational projections | M2I | P1 | LW-005, LW-010, LW-016 |
| [LW-071](issues/LW-071.md) | Build an opt-in provider registry and descriptor validator | M1I | P0 | LW-017, LW-067 |
| [LW-072](issues/LW-072.md) | Implement transactional provider inbox and outbox | M1I | P1 | LW-005, LW-016, LW-071 |
| [LW-073](issues/LW-073.md) | Create the provider replay and contract conformance kit | M1I | P1 | LW-071, LW-072 |
| [LW-074](issues/LW-074.md) | Add a read-only Estate Console host-summary adapter | M2I | P1 | LW-067, LW-073 |
| [LW-075](issues/LW-075.md) | Prepare Taskdeck review proposals without approval authority | M2I | P1 | LW-067, LW-073, LW-088 |
| [LW-076](issues/LW-076.md) | Import Agent Harness activity and worktree-lease evidence | M2I | P1 | LW-067, LW-073 |
| [LW-077](issues/LW-077.md) | Connect admission decisions to Loomward-owned worker launch | M2I | P1 | LW-032, LW-052, LW-078 |
| [LW-078](issues/LW-078.md) | Move resource leases into one authenticated native authority | M2I | P0 | LW-017, LW-050, LW-067 |
| [LW-079](issues/LW-079.md) | Measure demand estimates against actual owned-worker usage | M2I | P1 | LW-050, LW-077 |
| [LW-080](issues/LW-080.md) | Port the v2 allocation portfolio and oracle fixtures to Rust | M2I | P1 | LW-002, LW-030 |
| [LW-081](issues/LW-081.md) | Improve large-instance allocation under explicit search budgets | M2I | P1 | LW-080, LW-056 |
| [LW-082](issues/LW-082.md) | Add recall reservations and explicit active-work windows | M2I | P1 | LW-031, LW-078 |
| [LW-083](issues/LW-083.md) | Validate dependency groups before placement proposals | M2I | P0 | LW-010, LW-033, LW-070 |
| [LW-084](issues/LW-084.md) | Normalise backup evidence without equating success to recovery | M2I | P1 | LW-035, LW-036, LW-073 |
| [LW-085](issues/LW-085.md) | Add creative-source and derivative manifests | M2I | P1 | LW-061, LW-070, LW-073 |
| [LW-086](issues/LW-086.md) | Add a cooperative model-host lifecycle provider | M2I | P1 | LW-034, LW-073, LW-078 |
| [LW-087](issues/LW-087.md) | Estimate regeneration cost for explicit build-cache providers | M2I | P1 | LW-048, LW-061, LW-073 |
| [LW-088](issues/LW-088.md) | Persist decision notes with evidence-bound review history | M2I | P1 | LW-020, LW-038, LW-070 |
| [LW-089](issues/LW-089.md) | Expose bounded MCP proposal preparation without approve/apply | M2I | P0 | LW-066, LW-067, LW-088 |
| [LW-090](issues/LW-090.md) | Evaluate a read-only MCP Apps evidence view | MX | P1 | LW-065, LW-088 |
| [LW-091](issues/LW-091.md) | Threat-model optional remote MCP transport and authentication | MX | P1 | LW-066, LW-067, LW-095 |
| [LW-092](issues/LW-092.md) | Prototype conflict-aware preference sync only | MX | P1 | LW-021, LW-067, LW-070 |
| [LW-093](issues/LW-093.md) | Measure information-value question selection | M2I | P1 | LW-025, LW-026 |
| [LW-094](issues/LW-094.md) | Propagate feedback retraction through derived model state | M2I | P1 | LW-021, LW-027, LW-070 |
| [LW-095](issues/LW-095.md) | Exercise provider isolation and diagnostic privacy failures | M1I | P0 | LW-055, LW-067, LW-073 |
| [LW-096](issues/LW-096.md) | Verify all workbench states with keyboard, zoom and large data | M1I | P1 | LW-019, LW-068, LW-088 |
| [LW-097](issues/LW-097.md) | Benchmark end-to-end resource and energy effects | M2I | P1 | LW-056, LW-077, LW-079 |
| [LW-098](issues/LW-098.md) | Preserve disclosure suppression across views and exports | M1I | P0 | LW-020, LW-067, LW-069 |
| [LW-099](issues/LW-099.md) | Cache extraction results under bounded invalidation and budgets | M2I | P1 | LW-014, LW-072, LW-078, LW-094 |
| [LW-100](issues/LW-100.md) | Gate release claims against executable capability evidence | M2I | P0 | LW-057, LW-063, LW-065, LW-096, LW-097 |
