use crate::{query, Error, NodeKey, Reader, Result, EXCLUDED_FLAGS};
use rusqlite::{params, Connection};
use std::time::Instant;

/// Names and structure only: advice, never permission for an effect.
#[derive(Clone, Debug)]
pub struct MachineStoreCandidate {
    pub node: NodeKey,
    pub name: String,
    pub class: &'static str,
    pub provider: &'static str,
    pub logical_bytes: Option<u64>,
    pub allocated_bytes: Option<u64>,
    pub coverage: String,
    pub owner_step: &'static str,
    pub relocation: Option<&'static str>,
    pub reason: &'static str,
    pub uncertainty: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct MachineStores {
    pub rules_version: u32,
    pub candidates: Vec<MachineStoreCandidate>,
    pub partial: bool,
    pub truncated: bool,
    pub budget_hit: bool,
}

impl Reader {
    /// Read bounded, name-only advice for an active granted root, without repairing it.
    /// Unknown sizes stay null; commands and relocation settings are display text only.
    pub fn machine_stores(&mut self, root: i64, cap: usize) -> Result<MachineStores> {
        if !(1..=200).contains(&cap) {
            return Err(Error::Invalid("machine stores cap"));
        }
        let deadline = self.deadline;
        if deadline.is_some_and(|at| Instant::now() >= at) {
            return Err(Error::Cancelled);
        }
        // Reader::read can ask the writer to repair aggregates; this advice must not write.
        let result = (|| {
            let tx = self.conn.transaction()?;
            let report = detect(&tx, root, cap, deadline)?;
            tx.commit()?;
            Ok(report)
        })();
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

fn detect(
    conn: &Connection,
    root: i64,
    cap: usize,
    deadline: Option<Instant>,
) -> Result<MachineStores> {
    let guard = query::WorkGuard::new(conn, 2_000_000, deadline)?;
    query::revisions(conn, Some(root))?;
    let root_dir = conn.query_row(
        "SELECT id FROM dir WHERE root_id=?1 AND parent_id IS NULL",
        [root],
        |r| r.get(0),
    )?;
    let observed = query::get(conn, NodeKey::Dir(root_dir))?;
    let scanning = query::live(conn, root)?;
    // Finish root advice before the walk can spend the remaining work budget.
    let mut report = query_candidates(
        conn,
        include_str!("machine-stores-root.sql"),
        root_dir,
        cap,
        scanning,
    )?;
    report.partial |= observed.coverage() != "complete" || !observed.valid || scanning;
    match query_candidates(
        conn,
        include_str!("machine-stores.sql"),
        root_dir,
        cap,
        scanning,
    ) {
        Ok(walk) => {
            report.partial |= walk.partial;
            report.truncated |= walk.truncated;
            report.candidates.extend(walk.candidates);
            report.candidates.sort_by_key(|c| {
                let key = match c.node {
                    NodeKey::Dir(id) => (id, 0),
                    NodeKey::File(id) => (id, 1),
                    _ => unreachable!(),
                };
                (std::cmp::Reverse(c.logical_bytes), key)
            });
            report.truncated |= report.candidates.len() > cap;
            report.candidates.truncate(cap);
        }
        Err(e) if query::is_budget_interrupt(&e) && guard.hit() => {
            report.partial = true;
            report.budget_hit = true;
        }
        Err(e) => return Err(e),
    }
    if deadline.is_some_and(|at| Instant::now() >= at) {
        return Err(Error::Cancelled);
    }
    Ok(report)
}

fn query_candidates(
    conn: &Connection,
    sql: &str,
    root_dir: i64,
    cap: usize,
    scanning: bool,
) -> Result<MachineStores> {
    let mut report = MachineStores {
        rules_version: 4,
        ..MachineStores::default()
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(
            params![
                root_dir,
                EXCLUDED_FLAGS | 1 | (1 << 1) | (1 << 6) | (1 << 7) | (1 << 9),
                cap as i64 + 1,
                scanning
            ],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, u8>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    report.truncated = rows.len() > cap;
    // The window in SQL includes candidates beyond the cap in the unknown-size assessment.
    report.partial |= rows.first().is_some_and(|r| r.3);
    for (id, kind, rule, _) in rows.into_iter().take(cap) {
        let key = if kind == 0 {
            NodeKey::Dir(id)
        } else {
            NodeKey::File(id)
        };
        let row = query::get(conn, key)?;
        report.candidates.push(candidate(row, &rule, scanning)?);
    }
    Ok(report)
}

fn candidate(row: query::Raw, rule: &str, scanning: bool) -> Result<MachineStoreCandidate> {
    let (class, provider, owner_step, relocation, reason, uncertainty) = match rule {
        "cargo-target" => ("shared_build_output", "cargo",
            "Use cargo clean --target-dir <dir> in the owning tool after reviewing the shared build output.", Some("CARGO_TARGET_DIR"),
            "Recorded CACHEDIR.TAG, .rustc_info.json and debug or release children, with no Cargo.toml in the parent.",
            "Names only: unique build inputs, active builds, toolchains and regeneration cost are unverified. This is advice, never a disposability claim."),
        "npm" => ("package_cache", "npm", "Use npm cache verify in the owning tool after review; it verifies and garbage-collects the cache.", Some("npm_config_cache"),
            "Recorded _cacache directory with content-v2 and index-v5 children.",
            "Cache contents, offline availability and active users are unverified. This is advice, never a disposability claim."),
        "pnpm" => ("package_cache", "pnpm", "Use pnpm store prune in the owning tool after reviewing the store.", Some("PNPM_STORE_DIR"),
            "Recorded store or .pnpm-store directory, or a directory under pnpm, with a v plus digits child containing files.",
            "Hard links outside this store may retain allocation, and active users and regeneration cost are unknown. This is advice, never a disposability claim."),
        "nuget" => ("package_cache", "nuget", "Use dotnet nuget locals all --list, then dotnet nuget locals all --clear only after owner review.", Some("NUGET_PACKAGES"),
            "Recorded packages directory directly under .nuget.",
            "Package availability, credentials, active builds and regeneration cost are unknown. This is advice, never a disposability claim."),
        "pip" => ("package_cache", "pip", "Use pip cache info, then pip cache purge only after owner review.", Some("PIP_CACHE_DIR"),
            "Recorded pip directory with an http-v2, http or wheels child.",
            "Cached packages, offline availability and active users are unverified. This is advice, never a disposability claim."),
        "uv" => ("package_cache", "uv", "Use uv cache prune in the owning tool after reviewing the cache.", Some("UV_CACHE_DIR"),
            "Recorded CACHEDIR.TAG file and an archive-v prefixed child directory.",
            "Cache contents, external links, offline availability and active users are unverified. This is advice, never a disposability claim."),
        "gradle" => ("package_cache", "gradle", "Review Gradle's own cache retention settings in the owning tool.", Some("GRADLE_USER_HOME"),
            "Recorded caches directory under .gradle with a modules-2 child.",
            "Gradle retention, active builds, dependencies and regeneration cost are unverified. This is advice, never a disposability claim."),
        "cargo-cache" => ("package_cache", "cargo", "Owner judgement: review registry/cache or git/db before deleting by hand, as cargo cache tools are third-party.", Some("CARGO_HOME"),
            "Recorded .cargo child registry with cache, or git with db.",
            "Source availability, active builds and regeneration cost are unknown, and third-party cleanup tools are unverified. This is advice, never a disposability claim."),
        "huggingface" => ("model_store", "huggingface", "Use the host's model manager or huggingface-cli delete-cache after owner review.", Some("HF_HOME / HF_HUB_CACHE"),
            "Recorded models-- or datasets-- child directory containing blobs and snapshots.",
            "External hard links, unique datasets, model availability, credentials and active users are unverified. This is advice, never a disposability claim."),
        "lmstudio" => ("model_store", "lmstudio", "Review models in LM Studio My Models before using its model manager.", Some("LM Studio models directory setting"),
            "Recorded models directory directly under .lmstudio.",
            "Model contents, unique local models, availability and active leases are unknown. This is advice, never a disposability claim."),
        "ollama" => ("model_store", "ollama", "Review the host's models before using ollama rm in the owning tool.", Some("OLLAMA_MODELS"),
            "Recorded models directory with blobs and manifests children.",
            "Model contents, unique local models, availability and active leases are unknown. This is advice, never a disposability claim."),
        "comfyui" => ("model_store", "comfyui", "Review this model folder by hand or in a model manager you use with ComfyUI; custom checkpoints and trained LoRAs may be unique.", Some("extra_model_paths.yaml"),
            "Recorded child of a models directory whose parent holds comfy and custom_nodes directories.",
            "Custom checkpoints and trained LoRAs may be unique and not re-downloadable; contents, links and active workflows are unverified. This is advice, never a disposability claim."),
        "a1111" => ("model_store", "a1111", "Review models in the web UI by hand; custom checkpoints and trained LoRAs may be unique.", Some("--ckpt-dir / --lora-dir launch arguments"),
            "Recorded models directory with a Stable-diffusion child, beside a webui.py file.",
            "Custom checkpoints and trained LoRAs may be unique and not re-downloadable; contents, links and active sessions are unverified. This is advice, never a disposability claim."),
        "steam" => ("game_library", "steam", "In Steam, use Properties, Installed Files, Move install folder; uninstalling is the owner's call after checking mods and local saves.", Some("another Steam library folder (Steam Settings, Storage)"),
            "Recorded directory directly under steamapps/common, where steamapps holds an appmanifest_*.acf file.",
            "Names cannot show that a game is re-downloadable or that mods and local saves survive an uninstall. Contents and active play are unverified. This is advice, never a disposability claim."),
        "wsl" => ("virtual_disk", "wsl", "Review WSL compaction using its documented offline procedure in the owning tool.", Some("wsl --manage <distro> --move"),
            "Recorded file named ext4.vhdx.",
            "The host observes container size only; guest free space is unknown, so reclaimable bytes are unknown. This is advice, never a disposability claim."),
        "docker" => ("virtual_disk", "docker", "Use docker system df -v before considering targeted docker prune commands in the owning tool.", Some("Docker Desktop disk-image location"),
            "Recorded file named docker_data.vhdx or Docker.vhdx, ignoring ASCII case.",
            "The host observes container size only; guest free space is unknown, so reclaimable bytes are unknown. This is advice, never a disposability claim."),
        "worktree" => ("agent_worktree", "git", "Check branch, HEAD and uncommitted work with git worktree list and git submodule status in the owning repository before removing anything.", None,
            "A recorded .git file marks a linked worktree, a submodule or a separate-git-dir checkout; names alone cannot tell which.",
            "The .git contents and target, worktree ownership, active leases and dirty state are unverified. This is advice, never a disposability claim."),
        "recycle-bin" => ("recycle_bin", "windows", "Emptying the Recycle Bin in Explorer is the owner's step after reviewing its contents.", None,
            "Recorded $Recycle.Bin directory directly under the granted root.",
            "Recycling on this volume frees nothing here until the bin is emptied, and its contents and recoverability are unverified. This is advice, never a disposability claim."),
        "system" => ("system_managed", "windows", "Informational: managed by Windows (Disk Cleanup or DISM); never counted as reclaimable.", None,
            "Recorded root system file or WinSxS or SoftwareDistribution directory directly under the root's Windows directory.",
            "Windows owns this space, and no reclaimable bytes or permission to alter it are established. This is advice, never a disposability claim."),
        // Rules v4: Windows' own leftovers. Each owner step names the tool Windows itself offers.
        "windows-old" => ("windows_leftover", "windows", "Use Settings > System > Storage > Temporary files (Previous Windows installation(s)) or Disk Cleanup, after owner review of whether to keep the rollback.", None,
            "Recorded Windows.old, $WINDOWS.~BT or $WINDOWS.~WS directory directly under the granted root.",
            "Names only: whether the upgrade finished, whether rollback is still wanted and what the folder holds are unverified. This is advice, never a disposability claim."),
        "memory-dump" => ("windows_leftover", "windows", "Use Disk Cleanup, category \"System error memory dump files\", after owner review.", None,
            "Recorded MEMORY.DMP file, or Minidump or LiveKernelReports directory, directly under the Windows directory directly under the granted root.",
            "Names only: a dump may be the only record of a crash still under investigation, and its contents are unverified. This is advice, never a disposability claim."),
        "error-reports" => ("windows_leftover", "windows", "Use Disk Cleanup, category \"Windows Error Reports and Feedback Diagnostics\", after owner review.", None,
            "Recorded ReportArchive or ReportQueue directory under ProgramData/Microsoft/Windows/WER directly under the granted root.",
            "Names only: reports may be wanted for a pending support case, and their contents are unverified. This is advice, never a disposability claim."),
        "temp" => ("windows_leftover", "windows", "Use Settings > System > Storage > Temporary files (Storage Sense) rather than deleting by hand; files in use are left alone.", None,
            "Recorded Temp directory directly under the Windows directory under the root, or directly under AppData/Local.",
            "Names only: running programs and installers may still use files here, and contents are unverified. This is advice, never a disposability claim."),
        "crash-dumps" => ("windows_leftover", "windows", "Review the dump files by hand and keep any still needed for a bug report; no Windows cleanup category is assumed.", None,
            "Recorded CrashDumps directory directly under AppData/Local.",
            "Names only: a dump may be the only record of a crash still under investigation, and its contents are unverified. This is advice, never a disposability claim."),
        "shader-cache" => ("windows_leftover", "windows", "Use Disk Cleanup, category \"DirectX Shader Cache\", or the GPU vendor's control panel, after owner review.", None,
            "Recorded D3DSCache, NVIDIA/DXCache, NVIDIA/GLCache or AMD/DxCache directory under AppData/Local.",
            "Names only: regeneration cost, such as stutter on the next game launch, and active users are unverified. This is advice, never a disposability claim."),
        "driver-extract" => ("windows_leftover", "windows", "Delete by hand after the driver is installed and the owner has confirmed the installer files are not needed for a rollback.", None,
            "Recorded AMD or NVIDIA directory directly under the granted root holding Setup.exe, or a Packages, Display.Driver or DisplayDriver child, or a child holding Setup.exe.",
            "Names only: the grant is not shown to be a volume root, and whether the driver installed or the installer is still wanted is unverified. This is advice, never a disposability claim."),
        "delivery-optimization" => ("windows_leftover", "windows", "Use Disk Cleanup, category \"Delivery Optimization Files\", or Settings > Windows Update > Advanced options > Delivery Optimization, after owner review.", None,
            "Recorded SoftwareDistribution/DeliveryOptimization directory, or a Cache directory under NetworkService/AppData/Local/Microsoft/Windows/DeliveryOptimization, under the granted root.",
            "Names only: the cache is shared with other devices and updates in progress when enabled, and its contents are unverified. This is advice, never a disposability claim."),
        "installer-cache" => ("system_managed", "windows", "Informational: Package Cache and Windows Installer back up repair and uninstall; never remove by hand, and never counted as reclaimable.", None,
            "Recorded ProgramData/Package Cache or Windows/Installer directory directly under the granted root.",
            "Windows and installed programs own this space, and removing files can break repair or uninstall; no reclaimable bytes are established. This is advice, never a disposability claim."),
        _ => return Err(Error::Invalid("machine store rule")),
    };
    let coverage = row.coverage();
    let known = coverage == "complete" && row.valid && !scanning;
    Ok(MachineStoreCandidate {
        node: row.key,
        name: row.name,
        class,
        provider,
        logical_bytes: known.then_some(row.totals.logical),
        allocated_bytes: (known && row.totals.allocation_unknown == 0)
            .then_some(row.totals.allocated),
        coverage,
        owner_step,
        relocation,
        reason,
        uncertainty,
    })
}
