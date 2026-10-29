use crate::{query, DisplayPath, Error, NodeKey, Reader, Result, EXCLUDED_FLAGS};
use rusqlite::params;

/// The same exact, case-sensitive marker families used by developer clutter.
pub(crate) fn project_provider(name: &str) -> Option<&'static str> {
    match name {
        "Cargo.toml" => Some("cargo"),
        "package.json" => Some("npm"),
        "pyproject.toml" | "requirements.txt" => Some("python"),
        "build.gradle" | "build.gradle.kts" => Some("gradle"),
        n if (n.ends_with(".csproj") && n.len() > 7) || (n.ends_with(".sln") && n.len() > 4) => {
            Some("dotnet")
        }
        _ => None,
    }
}

const MARKERS: &str = "(f.name IN ('Cargo.toml','package.json','pyproject.toml','requirements.txt','build.gradle','build.gradle.kts') OR f.name GLOB '?*.csproj' OR f.name GLOB '?*.sln')";
const SHIELDS: u32 = EXCLUDED_FLAGS | 1 | (1 << 1) | (1 << 6) | (1 << 7) | (1 << 9);

#[derive(Clone, Debug)]
pub struct ProjectCandidate {
    pub directory: NodeKey,
    pub born_run: i64,
    pub name: String,
    pub path: DisplayPath,
    pub coverage: String,
    pub markers: Vec<(String, &'static str)>,
    pub markers_truncated: bool,
}

#[derive(Clone, Debug)]
pub struct ProjectPage {
    pub generation: Option<i64>,
    pub catalog_rev: i64,
    pub coverage: String,
    pub partial: bool,
    pub candidates: Vec<ProjectCandidate>,
    pub next_after: Option<i64>,
}

impl Reader {
    /// Recorded metadata only. Never opens a source path or repairs catalogue/state rows.
    pub fn projects(&mut self, root: i64, cap: usize, after: i64) -> Result<ProjectPage> {
        if !(1..=200).contains(&cap) || after < 0 {
            return Err(Error::Invalid("project page"));
        }
        let deadline = self.deadline;
        if deadline.is_some_and(|at| std::time::Instant::now() >= at) {
            return Err(Error::Cancelled);
        }
        let result = self.read_observations(|conn| {
            let _guard = query::WorkGuard::new(conn, 2_000_000, deadline)?;
            let generation = query::revisions(conn, Some(root))?[0].1;
            let catalog_rev = conn.query_row("SELECT catalog_rev FROM main.revision", [], |r| r.get(0))?;
            let root_dir: i64 = conn.query_row("SELECT id FROM dir WHERE root_id=?1 AND parent_id IS NULL", [root], |r| r.get(0))?;
            let scanning = query::live(conn, root)?;
            let coverage = query::get(conn, NodeKey::Dir(root_dir))?.coverage();
            // Keep shielded boundary rows in the coverage summary, but never descend into them.
            let sql = format!("WITH RECURSIVE tree(id) AS (
                SELECT ?1 UNION ALL
                SELECT d.id FROM tree t JOIN dir p ON p.id=t.id JOIN dir d ON d.parent_id=p.id
                WHERE p.flags & ?2=0 AND p.attrs & 1024=0
                  AND p.listing_state IN ('complete','incomplete')
                  AND coalesce(p.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned')
                  AND (p.listing_state='complete' OR d.seen_run=p.seen_run)
                  AND d.listing_state!='absent_pending'
            ), observed AS (
                SELECT d.*, (d.flags & ?2=0 AND d.attrs & 1024=0
                  AND d.listing_state IN ('complete','incomplete')
                  AND coalesce(d.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned')) AS usable
                FROM tree t JOIN dir d ON d.id=t.id
            ), page AS (
                SELECT d.id FROM observed d WHERE d.usable AND d.id>?3
                  AND EXISTS(SELECT 1 FROM file f WHERE f.dir_id=d.id AND {MARKERS}
                    AND f.flags & ?2=0 AND f.attrs & 1024=0
                    AND (d.listing_state='complete' OR f.seen_run=d.seen_run))
                ORDER BY d.id LIMIT ?4
            ) SELECT page.id, EXISTS(SELECT 1 FROM observed WHERE NOT usable
                OR listing_state!='complete' OR coalesce(listing_outcome,'complete')!='complete'
                OR sub_complete=0 OR agg_valid_rev<dirty_rev)
              FROM (SELECT 1) LEFT JOIN page ON true ORDER BY page.id");
            let rows = conn.prepare(&sql)?.query_map(params![root_dir, SHIELDS, after, cap as i64 + 1], |r| {
                Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, bool>(1)?))
            })?.collect::<rusqlite::Result<Vec<_>>>()?;
            let mut page = ProjectPage { generation, catalog_rev, coverage,
                partial: generation.is_none() || scanning || rows.iter().any(|r| r.1),
                candidates: Vec::new(), next_after: None };
            let ids: Vec<i64> = rows.into_iter().filter_map(|r| r.0).collect();
            if ids.len() > cap { page.next_after = ids.get(cap - 1).copied(); }
            let marker_sql = format!("SELECT f.name FROM file f JOIN dir d ON d.id=f.dir_id
                WHERE f.dir_id=?1 AND {MARKERS} AND f.flags & ?2=0 AND f.attrs & 1024=0
                AND (d.listing_state='complete' OR f.seen_run=d.seen_run) ORDER BY f.name,f.id LIMIT 17");
            let mut markers = conn.prepare(&marker_sql)?;
            for id in ids.into_iter().take(cap) {
                let row = query::get(conn, NodeKey::Dir(id))?;
                let names = markers.query_map(params![id, SHIELDS], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let path = query::node_path(conn, row.key)?;
                let coverage = if scanning { "partial".into() } else { row.coverage() };
                page.candidates.push(ProjectCandidate {
                    directory: row.key, born_run: row.born_run, name: row.name,
                    path: DisplayPath::bounded(path.ancestors.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(" / "), path.truncated),
                    coverage, markers_truncated: names.len() > 16,
                    markers: names.into_iter().take(16).map(|name| {
                        let provider = project_provider(&name).ok_or(Error::Invalid("project marker"))?;
                        Ok((name, provider))
                    }).collect::<Result<_>>()?,
                });
            }
            Ok(page)
        });
        match result {
            Err(e) if query::is_budget_interrupt(&e) => Err(
                if deadline.is_some_and(|at| std::time::Instant::now() >= at) {
                    Error::Cancelled
                } else {
                    Error::ResourceBudget
                },
            ),
            result => result,
        }
    }
}
