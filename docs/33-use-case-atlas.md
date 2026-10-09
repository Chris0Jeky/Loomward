# Use-case atlas

All scenarios below are proposed production uses unless they explicitly name a reference simulation. No listed provider is connected by this document. Each case has a trigger, an intended decision, a boundary and an outcome measure.

## UC-01: A developer opens a project

**Trigger.** Select a workspace profile containing a repository, docs and its build outputs.

**Useful response.** Use explicit project selection and an active work lease to pin the repository and admit a build before optional hashing. Show which requested workers fit and which were deferred.

**Boundary.** A worktree path in a chat is not permission to scan or remove it. Dirty/untracked/ignored state remains a keep signal.

**Dependencies.** Project provider + native catalogue + cooperative build adapter.

**Measure.** Time to first successful build; unexpected relocations; background I/O during the build.

## UC-02: An agent swarm starts too much work

**Trigger.** Several coding or analysis workers request RAM and CPU at once.

**Useful response.** Use a shared budget and idempotent leases so simultaneous callers cannot each consume the same apparent headroom. Give the owner an explanation of queue order.

**Boundary.** A lease is not authority to kill another agent. Self-declared identities need authentication before cross-process use.

**Dependencies.** Resource coordinator + runtime adapters.

**Measure.** Foreground latency, admitted useful work and queue age under identical arrivals.

## UC-03: A model is large but periodically needed

**Trigger.** A local model is idle now but used by a scheduled research job.

**Useful response.** Combine explicit schedule/lease evidence with expected recall cost. Propose cold residency only outside the planned-use window and reserve scratch/loading capacity for recall.

**Boundary.** A quiet process or old modification time does not prove the model is unused. Never unload an unrelated host model.

**Dependencies.** Model-host read adapter + tier planner.

**Measure.** Avoided low-space intervals versus model warm-up delay and failed recalls.

## UC-04: Build caches consume a drive

**Trigger.** A project has large generated outputs and dependency caches.

**Useful response.** Distinguish cheaply reproducible outputs from irreplaceable artefacts. Suggest an app-specific cleanup recipe with estimated regeneration cost and exact scope.

**Boundary.** Do not assume every bin, target or cache-named directory is disposable. Reproduction inputs may be missing or private.

**Dependencies.** Build-system provider + recovery evidence.

**Measure.** Verified space relief minus rebuild cost; zero loss of unique outputs.

## UC-05: A Git worktree appears abandoned

**Trigger.** An old worktree is large and has no recent obvious activity.

**Useful response.** Read the harness activity/lease evidence and expose uncertainty. Create a review item for closeout using the owning tool rather than duplicate its removal implementation.

**Boundary.** Missing lease is not safe-to-remove evidence. Native identity and Git state checks remain mandatory.

**Dependencies.** Agent Harness read-only adapter.

**Measure.** Correct keep decisions; avoided false abandonment recommendations.

## UC-06: Creative project footage and proxies differ

**Trigger.** A video project contains masters, proxies, exports and application sidecars.

**Useful response.** Group dependencies and distinguish regeneration cost. Prefer moving a validated whole group or rebuilding proxies over splitting path-sensitive assets.

**Boundary.** A final export does not replace its original sources. Application manifests need authorised content reads.

**Dependencies.** Media-project provider + group model.

**Measure.** Project reopen success; missing media; time to usable edit session.

## UC-07: An avatar asset pipeline accumulates versions

**Trigger.** A character project has source art, rigging files, texture atlases and exported animations.

**Useful response.** Link editable sources to generated derivatives and preserve a known-good release set. Offer virtual version collections and archive candidates with lineage.

**Boundary.** Near-duplicate frames are not redundant source assets. Never replace a rig with a rendered GIF because it looks similar.

**Dependencies.** Asset-manifest provider + virtual collections.

**Measure.** Time to locate editable source and successfully reproduce a release.

## UC-08: A photo library contains apparent duplicates

**Trigger.** Similar images exist as originals, edited versions and smaller exports.

**Useful response.** Separate byte equality from perceptual similarity and preserve editing lineage. Present side-by-side evidence and storage estimates rather than a delete-all action.

**Boundary.** A perceptual match is not identity. Metadata, edits and original resolution may be valuable.

**Dependencies.** Content-read grant + photo metadata/extractor.

**Measure.** Correct duplicate grouping and retained originals; review time per useful decision.

## UC-09: Invoices belong to projects and Finance

**Trigger.** The owner wants accounting retrieval without disrupting project folders.

**Useful response.** Learn a virtual Finance membership from a small number of corrections. Keep physical paths unchanged and explain which rule or examples support the membership.

**Boundary.** Category membership is not tax advice or a reason to move/delete a record. Sensitive content remains out of teacher context unless granted.

**Dependencies.** Student model + virtual collections.

**Measure.** Retrieval time and correction burden, with zero unnecessary physical moves.

## UC-10: Downloads become a staging inbox

**Trigger.** New downloads mix installers, receipts, archives and temporary exports.

**Useful response.** Offer a bounded review queue with project/context suggestions and a keep-current choice. Batch related items and ask only high-value questions.

**Boundary.** The Downloads location alone is not deletion permission. Do not execute installers or inspect archives without a grant.

**Dependencies.** Native observer + personal learning.

**Measure.** Useful dispositions per review minute; repeated unwanted prompts.

## UC-11: A desktop is visually cluttered

**Trigger.** Shortcuts and documents obscure frequently used work.

**Useful response.** Preview a workspace profile, virtual grouping and shortcut repairs. Let the user approve a layout/shortcut manifest separately from document organisation.

**Boundary.** Do not silently rewrite target arguments or move a shortcut target. Offline is different from missing.

**Dependencies.** Shell/shortcut provider, later native capability.

**Measure.** Time to launch intended work; broken target rate; ease of reverting layout.

## UC-12: An external archive drive is disconnected

**Trigger.** A candidate target is unavailable during planning.

**Useful response.** Mark placement blocked and show the reason. Preserve the last observation as stale rather than assuming its capacity is current. Replan when the exact volume returns.

**Boundary.** A reused drive letter must not identify the old target. Do not mount or wake devices merely for a cosmetic refresh.

**Dependencies.** Volume identity provider + planner.

**Measure.** Correct offline handling; no wrong-volume proposals after reconnect.

## UC-13: A NAS has limited throughput

**Trigger.** Large cold groups could reside on a network share but recalls may be slow.

**Useful response.** Model network availability, destination capability and transfer/recall cost separately. Start with an explicit copy/recovery assessment rather than transparent relocation.

**Boundary.** A network share is not automatically a suitable live SQLite location or an independent backup.

**Dependencies.** Network-storage adapter + recovery evidence.

**Measure.** Measured recall latency and interruption rate under a declared network condition.

## UC-14: Cloud placeholders appear huge

**Trigger.** A synced folder contains hydrated files and placeholders.

**Useful response.** Display logical size, local allocation and provider state separately. Plan analysis that does not hydrate every placeholder during indexing or hashing.

**Boundary.** Do not infer local reclaimable bytes from cloud logical size. Provider eviction is an app-specific operation.

**Dependencies.** Cloud-files provider, explicit hydration capability.

**Measure.** Avoided unintended downloads; accurate local-allocation coverage.

## UC-15: A VM or WSL image dominates storage

**Trigger.** A large virtual disk contains dynamically allocated content.

**Useful response.** Identify its owning application and distinguish host-file size from guest free space. Suggest a documented owner-tool workflow with preconditions and recovery.

**Boundary.** Never truncate or casually move an active virtual disk. Do not promise compaction from a host-size observation alone.

**Dependencies.** VM/WSL provider, later explicit supported workflow.

**Measure.** Successful guest reopen and measured host relief after approved maintenance.

## UC-16: A game or mod project occupies fast storage

**Trigger.** A game installation and user-made mods share directories.

**Useful response.** Separate reproducible installation files from saves, configs and unique mods. Prefer the owning launcher’s relocation mechanism where supported.

**Boundary.** Installed files are not uniformly replaceable. Do not use a generic directory move in place of a tested application workflow.

**Dependencies.** Application/launcher provider.

**Measure.** Successful launch, preserved saves/mods, restore time and actual relief.

## UC-17: Backups ran but recovery is untested

**Trigger.** A backup job says success, but no recent restore has been checked.

**Useful response.** Show inventory evidence, coverage gaps and a proposed restore drill. Keep backup history, integrity checking and restore testing as separate evidence types.

**Boundary.** Job success is not proof of restorability. No automatic pruning or source retirement follows from a green status.

**Dependencies.** Read-only backup provider + explicit restore-test workflow.

**Measure.** Verified restore coverage and age; failed drills surfaced promptly.

## UC-18: The only recovery copy is on the same disk

**Trigger.** A cleanup proposal relies on source-side quarantine.

**Useful response.** Explain that source bytes remain allocated. Compare retaining the original with copying to an independent verified recovery target before retirement.

**Boundary.** Do not count quarantined bytes as free. Do not imply independence just because the path differs.

**Dependencies.** Volume topology + recovery policy.

**Measure.** Honest relief accounting; independently tested recovery.

## UC-19: A researcher has reproducible datasets

**Trigger.** Raw data, notebooks, intermediate tables and experiment outputs accumulate.

**Useful response.** Use explicit lineage and experiment manifests to distinguish source data from reproducible intermediates. Offer a virtual experiment view and cold-group planning.

**Boundary.** A notebook without its environment and external inputs may not reproduce anything. Licence/privacy constraints remain attached.

**Dependencies.** Research manifest provider + scoped extraction.

**Measure.** Successful regeneration, data provenance retention and retrieval time.

## UC-20: A laptop switches to battery

**Trigger.** Background indexing and inference compete with mobility constraints.

**Useful response.** Select a conservative owned-worker policy, defer optional GPU work and batch updates. Preserve interactive search from the current catalogue.

**Boundary.** Battery mode does not justify suspending arbitrary applications. Energy benefit must be measured rather than assumed.

**Dependencies.** Power observation + cooperative scheduler.

**Measure.** Foreground responsiveness, work completion and measured energy under a controlled task.

## UC-21: A full disk blocks Loomward itself

**Trigger.** The source volume runs out of capacity while the app has pending metadata work.

**Useful response.** Prioritise preserving decision/recovery records; pause optional ingestion and explain the failure. Use bounded logs and an explicit state-reserve policy.

**Boundary.** Do not launch emergency deletion without approval. A partially written catalogue is not a successful scan.

**Dependencies.** State-store health + native observer.

**Measure.** Recovery after full-disk fault injection; no loss of operation-state evidence.

## UC-22: An agent needs one answer, not the whole drive

**Trigger.** A host asks which project group could be reviewed for archival.

**Useful response.** Expose a scoped summary and bounded evidence references through MCP. Return the minimum fields needed, with unknowns and no execution tool.

**Boundary.** Local transport does not ensure local model processing. Names need an explicit disclosure grant.

**Dependencies.** Implemented snapshot MCP reference; live provider later.

**Measure.** Answer usefulness per disclosed field and payload byte; cross-scope rejection.

## UC-23: A reviewer needs to understand a refusal

**Trigger.** The planner excludes a seemingly obvious large folder.

**Useful response.** Show the exact constraint: active lease, unknown heat, insufficient target reserve, missing recovery or unsupported identity. Link to the evidence and a bounded next step.

**Boundary.** Do not replace a refusal with an unsafe fallback just to keep the workflow moving.

**Dependencies.** Decision desk + policy explanation.

**Measure.** Correct user understanding and reduced repeated attempts to bypass a guard.

## UC-24: Two devices hold different preferences

**Trigger.** The owner organises a project on both laptop and desktop.

**Useful response.** Synchronise selected preference events with provenance and explicit conflicts; keep each machine authoritative for its own live file effects.

**Boundary.** Do not sync the live SQLite file or treat a remote stale approval as a local native grant.

**Dependencies.** Future preference sync, not in v0.2.

**Measure.** Convergent non-destructive preferences; no cross-device action escalation.

## UC-25: A small business wants an understandable audit

**Trigger.** Several routines produce storage and process recommendations.

**Useful response.** Provide local receipts explaining source, proposed effect, review and actual outcome. Export a redacted report rather than raw paths and command lines.

**Boundary.** A tamper-evident local chain is not an independently authenticated compliance audit.

**Dependencies.** Decision/evidence store + export projection.

**Measure.** Traceability of decisions and measured redaction correctness.

## UC-26: A user prefers explicit rules over inference

**Trigger.** The owner wants predictable project profiles and only occasional suggestions.

**Useful response.** Let explicit pins, naming conventions and collection rules coexist with learning. The model can propose exceptions without replacing the user’s declared policy.

**Boundary.** Do not force a chat/ML workflow for basic organisation or convert ignored suggestions into approval.

**Dependencies.** Policy profile + optional student.

**Measure.** Task completion without model availability; reduced manual effort without surprise.
