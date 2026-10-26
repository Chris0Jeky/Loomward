//! Catalogue-only evidence, never permission, contents, EXIF or live filesystem observations.
//! Marker inputs are safe names immediately inside the subject or its immediate children.
//! Exact manifest/output spelling follows developer-clutter v1; suffixes use ASCII lower case.
//!
//! | Recorded name / structure | Marker |
//! |---|---|
//! | Cargo.toml; adjacent target directory | cargo_toml; cargo_target_dir |
//! | package.json; adjacent node_modules directory | package_json; node_modules_dir |
//! | pnpm-lock.yaml | pnpm_lock |
//! | pyproject.toml; requirements.txt | pyproject_toml; requirements_txt |
//! | .venv or venv; __pycache__, beside a Python manifest | python_venv_dir; pycache_dir |
//! | *.csproj or *.sln; adjacent bin or obj directory | dotnet_project; dotnet_bin_obj_dirs |
//! | build.gradle or build.gradle.kts; adjacent .gradle directory | gradle_build; gradle_cache_dir |
//! | .git directory; .git file | git_dir; git_file_worktree |
//! | CACHEDIR.TAG | cachedir_tag |
//! | *.gguf; *.safetensors; *.vhdx | gguf_file; safetensors_file; vhdx_file |
//! | models--* or datasets--* directory with blobs and snapshots | hf_hub_layout |
//! | .lmstudio/models directories | lmstudio_models_layout |
//! | models directory with blobs and manifests | ollama_blobs_layout |
//! | *.lrcat; *.lrcat.lock | lightroom_catalog; lightroom_catalog_lock |
//! | *Previews.lrdata directory (ASCII case-insensitive) | lightroom_previews |
//! | Backups/YYYY-MM-DD[...], numeric date components in range | lightroom_backups_dated |
//! | DCIM directory (ASCII case-insensitive) | dcim_layout |
//! | RAW and XMP files in the same directory with ASCII case-insensitive stems | raw_xmp_pairs |
//! | *.msi, *.exe, *.msix | installer_files |
//! | *.zip, *.7z, *.rar, *.tar, *.gz | archive_files |
//! | *.doc, *.docx, *.odt, *.xls, *.xlsx, *.ods, *.ppt, *.pptx, *.odp | office_documents |
//! | ~lw-staging-* directory (the copy executor's recorded naming convention) | loomward_staging |
//!
//! Generated time uses v1's adjacent manifest/output pairs (including venv); descendants
//! inherit that classification. Byte totals require complete, valid, non-scanning coverage.
//! Family/extension bytes additionally require every observed allocation to be known.
//! File depth is its parent directory depth plus one; the subject itself has depth zero.

use crate::{query, Error, NodeKey, Reader, Result, EXCLUDED_FLAGS, FAMILIES};
use rusqlite::{params, Connection};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};

#[derive(Clone, Debug, Default)]
pub struct EvidenceFamily {
    pub family: String,
    pub files: u64,
    pub bytes: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct EvidenceExtension {
    pub ext: String,
    pub files: u64,
    pub bytes: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct EvidenceTime {
    pub newest_content_write: Option<u64>,
    pub oldest_content_write: Option<u64>,
    pub newest_source_write: Option<u64>,
    pub newest_generated_write: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct EvidenceMedia {
    pub raw_files: u64,
    pub sidecar_files: u64,
    pub raw_with_sidecar: u64,
    /// Catalogue metadata has no EXIF; capture times and camera count stay unknown.
    pub capture_start: Option<u64>,
    pub capture_end: Option<u64>,
    pub camera_models: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct EvidenceFacts {
    /// Local input to tokenisation only; the raw name must never enter a packet.
    pub name: String,
    pub files: u64,
    pub folders: u64,
    pub logical_bytes: Option<u64>,
    pub allocated_bytes: Option<u64>,
    pub max_depth: u32,
    pub families: Vec<EvidenceFamily>,
    pub extensions: Vec<EvidenceExtension>,
    pub markers: Vec<&'static str>,
    pub time: EvidenceTime,
    pub media: Option<EvidenceMedia>,
    pub coverage: String,
    pub allocation_known: bool,
    pub link_counts_observed: bool,
    /// The recorded root volume's online bit; true only as a fallback if it has no volume.
    pub online: bool,
    pub partial: bool,
}

const SHIELDS: u32 = EXCLUDED_FLAGS | 1 | (1 << 1) | (1 << 6) | (1 << 7) | (1 << 9);
const RAW: &[&str] = &[
    "cr2", "cr3", "nef", "arw", "raf", "dng", "orf", "rw2", "pef", "srw",
];

impl Reader {
    /// A bounded snapshot read, deliberately bypassing `Reader::read` and its writer repairs.
    pub fn evidence_facts(&mut self, dir: i64) -> Result<EvidenceFacts> {
        let deadline = self.deadline;
        check_deadline(deadline)?;
        let result = (|| {
            let tx = self.conn.transaction()?;
            let facts = extract(&tx, dir, deadline)?;
            tx.commit()?;
            Ok(facts)
        })();
        match result {
            Err(e) if query::is_budget_interrupt(&e) => {
                check_deadline(deadline)?;
                Err(Error::ResourceBudget)
            }
            result => result,
        }
    }
}

fn check_deadline(deadline: Option<Instant>) -> Result<()> {
    if deadline.is_some_and(|at| Instant::now() >= at) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

#[derive(Default)]
struct Directory {
    parent: Option<i64>,
    name: String,
    depth: u32,
    manifests: u8,
    children: BTreeSet<String>,
    newest: Option<u64>,
    generated: bool,
}

#[derive(Default)]
struct Bucket {
    files: u64,
    bytes: u64,
    unknown: bool,
}

fn extract(conn: &Connection, dir: i64, deadline: Option<Instant>) -> Result<EvidenceFacts> {
    let _guard = query::WorkGuard::new(conn, 2_000_000, deadline)?;
    let subject = query::get(conn, NodeKey::Dir(dir))?;
    query::revisions(conn, Some(subject.grant))?;
    let scanning = query::live(conn, subject.grant)?;
    let mut facts = EvidenceFacts {
        coverage: subject.coverage(), name: subject.name,
        online: conn.query_row("SELECT coalesce(v.online,1) FROM root r LEFT JOIN volume v ON v.id=r.volume_id WHERE r.id=?1", [subject.grant], |r| r.get(0))?,
        ..EvidenceFacts::default()
    };
    // A direct subject query must not bypass a shield on an ancestor.
    let blocked: bool = conn.query_row(
        "WITH RECURSIVE a(id,parent_id,flags,attrs,listing_state,listing_outcome) AS (
          SELECT id,parent_id,flags,attrs,listing_state,listing_outcome FROM dir WHERE id=?1
          UNION ALL SELECT d.id,d.parent_id,d.flags,d.attrs,d.listing_state,d.listing_outcome FROM dir d JOIN a ON d.id=a.parent_id)
        SELECT EXISTS(SELECT 1 FROM a WHERE flags & ?2!=0 OR attrs & 1024!=0
          OR listing_state IN ('excluded','denied','absent_pending')
          OR (id!=?1 AND (listing_state NOT IN ('complete','incomplete')
            OR coalesce(listing_outcome,'complete') IN ('stale','excluded','denied','unscanned','cancelled'))))",
        params![dir,SHIELDS], |r| r.get(0))?;
    if blocked {
        if facts.coverage == "complete" {
            facts.coverage = "excluded".into();
        }
        facts.partial = true;
        return Ok(facts);
    }
    // A subject can itself be an output directory (or lie inside one). Only recorded,
    // safe adjacent manifests classify it; this reads no manifest contents or live paths.
    let mut inherited_generated = false;
    let mut anchors = conn.prepare(
        "WITH RECURSIVE a(id,parent_id,name) AS (
          SELECT id,parent_id,name FROM dir WHERE id=?1
          UNION ALL SELECT d.id,d.parent_id,d.name FROM dir d JOIN a ON d.id=a.parent_id)
        SELECT a.name,f.name FROM a JOIN dir p ON p.id=a.parent_id JOIN file f ON f.dir_id=p.id
        WHERE a.name IN ('target','node_modules','.venv','venv','__pycache__','bin','obj','build','.gradle')
          AND f.flags & ?2=0 AND f.attrs & 1024=0
          AND p.listing_state IN ('complete','incomplete')
          AND coalesce(p.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned','cancelled')
          AND (p.listing_state='complete' OR f.seen_run=p.seen_run)",
    )?;
    let mut anchor_rows = anchors.query(params![dir, SHIELDS])?;
    while let Some(row) = anchor_rows.next()? {
        check_deadline(deadline)?;
        inherited_generated |= generated(
            &row.get::<_, String>(0)?,
            manifest(&row.get::<_, String>(1)?),
        );
    }
    let mut known = facts.coverage == "complete" && subject.valid && !scanning;
    let mut allocation_known = subject.totals.allocation_unknown == 0;
    let mut logical = 0_u64;
    let mut allocated = 0_u64;
    let mut dirs = BTreeMap::<i64, Directory>::new();
    let mut families = BTreeMap::<String, Bucket>::new();
    let mut extensions = BTreeMap::<String, Bucket>::new();
    let mut markers = BTreeSet::new();
    let mut media = EvidenceMedia::default();
    let mut pairs = BTreeMap::<(i64, String), (u64, u64, u32)>::new();
    let mut links_known = true;
    let mut stmt = conn.prepare(include_str!("evidence-facts.sql"))?;
    let mut rows = stmt.query(params![dir, SHIELDS])?;
    while let Some(row) = rows.next()? {
        check_deadline(deadline)?;
        let is_file: bool = row.get(0)?;
        let id: i64 = row.get(1)?;
        let name: String = row.get(3)?;
        let depth: u32 = row.get(4)?;
        facts.max_depth = facts.max_depth.max(depth);
        if !is_file {
            known &= row.get::<_, bool>(5)?;
            facts.folders += u64::from(id != dir);
            dirs.insert(
                id,
                Directory {
                    parent: row.get(2)?,
                    name,
                    depth,
                    generated: id == dir && inherited_generated,
                    ..Directory::default()
                },
            );
            continue;
        }
        let size = nonnegative(row.get(6)?)?;
        let alloc = row.get::<_, Option<i64>>(7)?.map(nonnegative).transpose()?;
        let modified = unix_seconds(row.get(8)?);
        links_known &= row.get::<_, Option<i64>>(9)?.is_some_and(|n| n > 0);
        let ext = row
            .get::<_, Option<String>>(10)?
            .unwrap_or_default()
            .to_ascii_lowercase();
        let family = row
            .get::<_, Option<String>>(11)?
            .unwrap_or_else(|| "none".into());
        if !FAMILIES.contains(&family.as_str()) {
            return Err(Error::Invalid("evidence family"));
        }
        facts.files += 1;
        logical = logical.checked_add(size).ok_or(Error::ResourceBudget)?;
        allocated = allocated
            .checked_add(alloc.unwrap_or(0))
            .ok_or(Error::ResourceBudget)?;
        allocation_known &= alloc.is_some();
        for bucket in [
            families.entry(family).or_default(),
            extensions.entry(ext.clone()).or_default(),
        ] {
            bucket.files += 1;
            bucket.bytes = bucket
                .bytes
                .checked_add(size)
                .ok_or(Error::ResourceBudget)?;
            bucket.unknown |= alloc.is_none();
        }
        let d = dirs
            .get_mut(&id)
            .ok_or(Error::Invalid("evidence directory"))?;
        d.manifests |= manifest(&name);
        d.newest = d.newest.max(modified);
        facts.time.newest_content_write = facts.time.newest_content_write.max(modified);
        if let Some(time) = modified {
            facts.time.oldest_content_write = Some(
                facts
                    .time
                    .oldest_content_write
                    .map_or(time, |old| old.min(time)),
            );
        }
        let name_ext = name
            .rsplit_once('.')
            .filter(|(stem, _)| !stem.is_empty())
            .map_or_else(String::new, |(_, ext)| ext.to_ascii_lowercase());
        if depth <= 2 {
            file_markers(&name, &name_ext, &mut markers);
        }
        if RAW.contains(&name_ext.as_str()) || name_ext == "xmp" {
            let stem = name
                .rsplit_once('.')
                .map_or(name.as_str(), |(stem, _)| stem)
                .to_ascii_lowercase();
            let pair = pairs.entry((id, stem)).or_insert((0, 0, depth));
            if name_ext == "xmp" {
                media.sidecar_files += 1;
                pair.1 += 1;
            } else {
                media.raw_files += 1;
                pair.0 += 1;
            }
        }
    }
    let mut order: Vec<_> = dirs.keys().copied().collect();
    order.sort_by_key(|id| (dirs[id].depth, *id));
    for &id in &order {
        let d = &dirs[&id];
        if d.depth <= 2 {
            let (parent, name) = (d.parent, d.name.clone());
            if let Some(p) = parent.and_then(|p| dirs.get_mut(&p)) {
                p.children.insert(name);
            }
        }
    }
    for id in order {
        check_deadline(deadline)?;
        let d = &dirs[&id];
        let parent = d.parent.and_then(|p| dirs.get(&p));
        let generated =
            d.generated || parent.is_some_and(|p| p.generated || generated(&d.name, p.manifests));
        if d.depth > 0 && d.depth <= 2 {
            directory_markers(d, parent, &mut markers);
        }
        // Layouts can start at the subject itself; all participating child names stay depth <=2.
        if d.depth <= 1 {
            layout_markers(d, &mut markers);
        }
        let newest = if generated {
            &mut facts.time.newest_generated_write
        } else {
            &mut facts.time.newest_source_write
        };
        *newest = (*newest).max(d.newest);
        dirs.get_mut(&id).unwrap().generated = generated;
    }
    for (_, (raw, sidecars, depth)) in pairs {
        // One-to-one pairing keeps counts honest when multiple RAW formats share a stem.
        let paired = raw.min(sidecars);
        media.raw_with_sidecar += paired;
        if paired > 0 && depth <= 2 {
            markers.insert("raw_xmp_pairs");
        }
    }
    facts.partial = !known || !allocation_known;
    if !known && facts.coverage == "complete" {
        facts.coverage = "partial".into();
    }
    facts.logical_bytes = known.then_some(logical);
    facts.allocation_known = known && allocation_known;
    facts.allocated_bytes = facts.allocation_known.then_some(allocated);
    facts.link_counts_observed = known && links_known;
    facts.families = families
        .into_iter()
        .map(|(family, b)| EvidenceFamily {
            family,
            files: b.files,
            bytes: (known && !b.unknown).then_some(b.bytes),
        })
        .collect();
    facts.extensions = extensions
        .into_iter()
        .map(|(ext, b)| EvidenceExtension {
            ext,
            files: b.files,
            bytes: (known && !b.unknown).then_some(b.bytes),
        })
        .collect();
    facts
        .extensions
        .sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.ext.cmp(&b.ext)));
    // Unrepresentable extension names stay in family totals but never enter the closed packet.
    facts.extensions.retain(|e| {
        e.ext.len() <= 32
            && e.ext
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_+-".contains(&c))
    });
    facts.extensions.truncate(16);
    facts.markers = markers.into_iter().collect();
    facts.media = (media.raw_files > 0).then_some(media);
    check_deadline(deadline)?;
    Ok(facts)
}

fn unix_seconds(ft: Option<i64>) -> Option<u64> {
    ft.and_then(|ft| ft.checked_sub(116_444_736_000_000_000))
        .filter(|ticks| *ticks >= 0)
        .map(|ticks| ticks as u64 / 10_000_000)
        .filter(|unix| *unix <= 32_503_680_000)
}

fn nonnegative(number: i64) -> Result<u64> {
    u64::try_from(number).map_err(|_| Error::Invalid("negative evidence size"))
}

fn manifest(name: &str) -> u8 {
    match name {
        "Cargo.toml" => 1,
        "package.json" => 2,
        "pyproject.toml" | "requirements.txt" => 4,
        "build.gradle" | "build.gradle.kts" => 16,
        _ if name.ends_with(".csproj") && name.len() > 7
            || name.ends_with(".sln") && name.len() > 4 =>
        {
            8
        }
        _ => 0,
    }
}

fn generated(name: &str, manifests: u8) -> bool {
    let bit = match name {
        "target" => 1,
        "node_modules" => 2,
        ".venv" | "venv" | "__pycache__" => 4,
        "bin" | "obj" => 8,
        "build" | ".gradle" => 16,
        _ => 0,
    };
    manifests & bit != 0
}

fn file_markers(name: &str, ext: &str, markers: &mut BTreeSet<&'static str>) {
    let exact = match name {
        "Cargo.toml" => Some("cargo_toml"),
        "package.json" => Some("package_json"),
        "pnpm-lock.yaml" => Some("pnpm_lock"),
        "pyproject.toml" => Some("pyproject_toml"),
        "requirements.txt" => Some("requirements_txt"),
        "build.gradle" | "build.gradle.kts" => Some("gradle_build"),
        ".git" => Some("git_file_worktree"),
        "CACHEDIR.TAG" => Some("cachedir_tag"),
        _ => None,
    };
    if let Some(marker) = exact {
        markers.insert(marker);
    }
    if manifest(name) == 8 {
        markers.insert("dotnet_project");
    }
    let suffix = match ext {
        "gguf" => Some("gguf_file"),
        "safetensors" => Some("safetensors_file"),
        "vhdx" => Some("vhdx_file"),
        "lrcat" => Some("lightroom_catalog"),
        "msi" | "exe" | "msix" => Some("installer_files"),
        "zip" | "7z" | "rar" | "tar" | "gz" => Some("archive_files"),
        "doc" | "docx" | "odt" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp" => {
            Some("office_documents")
        }
        _ => None,
    };
    if let Some(marker) = suffix {
        markers.insert(marker);
    }
    if name.to_ascii_lowercase().ends_with(".lrcat.lock") {
        markers.insert("lightroom_catalog_lock");
    }
}

fn directory_markers(
    d: &Directory,
    parent: Option<&Directory>,
    markers: &mut BTreeSet<&'static str>,
) {
    let name = d.name.to_ascii_lowercase();
    let marker = match name.as_str() {
        ".git" => Some("git_dir"),
        "dcim" => Some("dcim_layout"),
        _ if name.starts_with("~lw-staging-") => Some("loomward_staging"),
        _ if name.ends_with("previews.lrdata") => Some("lightroom_previews"),
        _ => None,
    };
    if let Some(marker) = marker {
        markers.insert(marker);
    }
    if let Some(p) = parent {
        if generated(&d.name, p.manifests) {
            let marker = match d.name.as_str() {
                "target" => Some("cargo_target_dir"),
                "node_modules" => Some("node_modules_dir"),
                ".venv" | "venv" => Some("python_venv_dir"),
                "__pycache__" => Some("pycache_dir"),
                "bin" | "obj" => Some("dotnet_bin_obj_dirs"),
                ".gradle" => Some("gradle_cache_dir"),
                _ => None,
            };
            if let Some(marker) = marker {
                markers.insert(marker);
            }
        }
        if p.name == ".lmstudio" && d.name == "models" {
            markers.insert("lmstudio_models_layout");
        }
        if p.name == "Backups" && dated(&d.name) {
            markers.insert("lightroom_backups_dated");
        }
    }
}

fn layout_markers(d: &Directory, markers: &mut BTreeSet<&'static str>) {
    if (d.name.starts_with("models--") || d.name.starts_with("datasets--"))
        && d.children.contains("blobs")
        && d.children.contains("snapshots")
    {
        markers.insert("hf_hub_layout");
    }
    if d.name == "models" && d.children.contains("blobs") && d.children.contains("manifests") {
        markers.insert("ollama_blobs_layout");
    }
}

fn dated(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..10]
            .iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        && (1..=12).contains(&name[5..7].parse::<u8>().unwrap_or(0))
        && (1..=31).contains(&name[8..10].parse::<u8>().unwrap_or(0))
}
