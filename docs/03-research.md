# Research and decision ledger

Research date: 9 October 2026. These are primary documentation/research sources and design implications, not a claim that the resulting Windows application has been benchmarked or certified. Software documentation can change; re-check API contracts and resolve dependency versions during the first native build.

## Findings that change the design

WinDirStat is already open source [R01]. Replacing it requires a better measured experience, not a different licence description. Everything [R04], Czkawka [R02] and System Informer [R03] are distinct comparison points: search/indexing, duplicate inspection and process observability. A combined interface should not pretend one small prototype already matches each mature tool. No explanation for the user's WinDirStat failure has been established because no crash report, build identifier or Windows trace was supplied.

Windows change notifications and NTFS journals can improve incremental indexing, but coverage loss must be visible [R05–R07]. File identity requires more than a path string [R08–R10]. Cloud files need separate treatment to avoid unwanted hydration [R11]. An access timestamp is not reliable proof of disuse [R12]. These are architecture constraints, not optional polish.

An LLM can produce structured advisory labels [R28]. Weak supervision [R30] and calibrated predictive evaluation [R31] are relevant, but neither converts an LLM's answer into ground truth or a permission to alter a filesystem. The reference implements a small weighted student instead of prematurely fine-tuning a large language model.

Memory residency, committed memory and background scheduling are different concerns [R17–R20]. Prefer cooperative workload control and model lifecycle management [R29] over periodic working-set flushing. Backup completion, repository integrity and demonstrated restore ability are also separate dimensions [R16,R26,R27].

## Technology selection

**Rust engine, selected.** Appropriate for bounded native enumeration, durable storage logic and carefully isolated Win32 bindings. Memory safety does not solve filesystem identity races, mistaken authorisation or Windows metadata semantics. Keep unsafe native bindings in small adapter modules with targeted Windows tests, not throughout the planner or learner.

**Python learning/workbench, selected initially.** Standard-library experiments are easy to inspect and run. Avoid putting Python in the permanent high-volume indexing path merely because it is convenient for ML. Once useful learning behavior is demonstrated, compare a small native inference implementation, ONNX export and a bounded Python worker rather than assume a heavyweight model server is necessary.

**Tauri shell, selected target, not certified.** This pairs a web UI with Rust capabilities [R23–R25]. It still needs native integration, platform testing, accessible controls, strict CSP and an explicit IPC authority model. C# plus WinUI 3 is a credible Windows-first alternative if native shell/accessibility integration proves substantially easier. Electron is an alternative with a different runtime footprint; no measured comparison is claimed here. egui/Slint remain options if a native-rendered interface becomes a higher priority than the current web UI reuse. Do not rewrite the backend solely because the shell choice changes.

**SQLite catalogue, selected.** Local transactional storage fits a single-user agent [R22]. One catalogue service owns writes; readers consume versioned snapshots or bounded queries. Do not distribute a live WAL database across a sync folder or network share. A PostgreSQL service is not justified for the first single-machine product.

**Backups, integrate rather than invent.** Start with an explicit provider interface and evaluate restic [R26,R27]. Do not claim preservation of every Windows metadata class merely because a tool successfully stores file bytes. Test restore fidelity for the supported file classes.

**Native performance work, measure before claiming.** The Python reference is a functional oracle, not the proposed permanent scanner. The Rust sources were not compiled in the authoring container. Fast-path MFT/USN work, large-index queries, heap profiles and fair comparisons remain open issues.

## Source register

### R01: WinDirStat
https://windirstat.net/

WinDirStat is already open source; use it as a comparator, not a proprietary straw man.

### R02: Czkawka source
https://github.com/qarmin/czkawka

Reference an existing Rust duplicate-finding project; do not assume its benchmarks transfer to this prototype.

### R03: System Informer source
https://github.com/winsiderss/systeminformer

Established process-inspection comparator; do not claim an equivalent Windows internals surface.

### R04: Everything documentation
https://voidtools.com/support/everything/

Filename indexing and a storage adviser solve different problems. Compare index freshness and footprint separately.

### R05: Microsoft: change journals
https://learn.microsoft.com/en-us/windows/win32/fileio/change-journals

Incremental NTFS change tracking can avoid repeated full enumeration.

### R06: Microsoft: change journal records
https://learn.microsoft.com/en-us/windows/win32/fileio/change-journal-records

Records can be discarded or coalesced and cannot reconstruct file contents for undo.

### R07: Microsoft: ReadDirectoryChangesW
https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-readdirectorychangesw

Zero bytes after buffer overflow requires re-enumeration, not a clean-index assertion.

### R08: Microsoft: FILE_ID_INFO
https://learn.microsoft.com/en-us/windows/desktop/api/winbase/ns-winbase-file_id_info

Handle-derived volume and file identifiers are a stronger basis than path strings. Probe actual platform support.

### R09: Microsoft: reparse points
https://learn.microsoft.com/en-us/windows/win32/fileio/reparse-points

A directory entry can redirect access; path-prefix checks alone are not a sufficient execution boundary.

### R10: Microsoft: hard links and junctions
https://learn.microsoft.com/en-us/windows/win32/fileio/hard-links-and-junctions

Directory entries and underlying file objects are not one-to-one.

### R11: Microsoft: cloud placeholder attributes
https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfgetplaceholderstatefromattributetag

Treat cloud placeholders as a separate capability; inspection must not silently hydrate them.

### R12: Microsoft: fsutil behavior
https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/fsutil-behavior

Last-access behavior is configurable; timestamps do not establish reliable usage history.

### R13: Microsoft: CopyFile2
https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-copyfile2

Use a supported copy primitive behind a verified transaction, not a promise of cross-volume atomic moves.

### R14: Microsoft: known folders
https://learn.microsoft.com/en-us/windows/win32/shell/knownfolderid

Resolve supported Windows known folders instead of hardcoding an English Desktop path.

### R15: Microsoft: shell links
https://learn.microsoft.com/en-us/windows/win32/shell/links

Shortcuts have structured state; a filename replacement is not complete shortcut management.

### R16: Microsoft: VSS overview
https://learn.microsoft.com/en-us/windows/win32/vss/volume-shadow-copy-service-overview

Snapshot consistency and independent backup retention are distinct responsibilities.

### R17: Microsoft: job objects
https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects

Jobs can manage related processes and limits. Begin with workloads launched by Loomward.

### R18: Microsoft: SetProcessInformation
https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-setprocessinformation

Windows exposes process policy controls including power and memory priority; support and rights require probing.

### R19: Microsoft: working set sizing
https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-setprocessworkingsetsize

Working-set manipulation is not equivalent to safely reducing committed application memory.

### R20: Microsoft: working set
https://learn.microsoft.com/en-us/windows/win32/memory/working-set

Resident physical pages are one memory view, not the entire process-memory story.

### R21: Microsoft: Win32_Product
https://learn.microsoft.com/en-us/previous-versions/windows/desktop/msiprov/win32-product

Avoid inventory that triggers MSI consistency/repair side effects.

### R22: SQLite WAL
https://sqlite.org/wal.html

Keep the active catalogue local and account for WAL/checkpoint behavior; do not place it on a network share.

### R23: Tauri security
https://v2.tauri.app/security/

Separate frontend trust from native powers. A Tauri label alone is not a security guarantee.

### R24: Tauri capabilities
https://v2.tauri.app/security/capabilities/

Use explicit per-window capabilities rather than broad shell/filesystem grants.

### R25: Tauri prerequisites
https://v2.tauri.app/start/prerequisites/

Windows development requires its native toolchain and WebView2-related prerequisites.

### R26: restic backup documentation
https://restic.readthedocs.io/en/stable/040_backup.html

Candidate backup-provider integration; record actual provider results rather than infer protection.

### R27: restic repository operations
https://restic.readthedocs.io/en/stable/045_working_with_repos.html

Repository checks and restore tests belong in protection health, separate from a successful backup invocation.

### R28: LM Studio structured output
https://lmstudio.ai/docs/developer/openai-compat/structured-output

JSON-schema-constrained local teacher output is possible; still validate independently.

### R29: LM Studio TTL and eviction
https://lmstudio.ai/docs/developer/core/ttl-and-auto-evict

Model lifecycle controls can reduce idle model residency; do not unload a shared session blindly.

### R30: Snorkel paper
https://arxiv.org/abs/1711.10160

Weak supervision is a useful conceptual foundation for noisy teacher labels, not proof of their correctness.

### R31: Guo et al.: calibration
https://proceedings.mlr.press/v70/guo17a.html

Predictive confidence requires evaluation/calibration. Safety authority remains a separate question.

### R32: sysinfo 0.33.1 documentation
https://docs.rs/sysinfo/0.33.1/sysinfo/

A possible portable native telemetry adapter; refresh/delta semantics and missing support must be explicit.
