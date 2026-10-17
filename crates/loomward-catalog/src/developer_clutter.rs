use crate::{query, Error, NodeKey, Reader, Result, EXCLUDED_FLAGS};
use rusqlite::params;

/// Advice from recorded names only, never permission or a claim of safe disposability.
#[derive(Clone, Debug)]
pub struct DeveloperClutterCandidate {
    pub directory: NodeKey,
    pub directory_name: String,
    pub anchor: NodeKey,
    pub anchor_name: String,
    pub provider: &'static str,
    /// What the conventional name is: `build_output`, `dependency_install`, `environment`,
    /// `bytecode_cache` or `build_cache`. A reading of the name, not of the contents.
    pub kind: &'static str,
    pub logical_bytes: Option<u64>,
    pub allocated_bytes: Option<u64>,
    pub coverage: String,
    pub regeneration_hint: &'static str,
    pub regeneration_cost: &'static str,
    pub reason: &'static str,
    pub uncertainty: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct DeveloperClutter {
    /// Version of the conventional-name rules, not an executable provider contract.
    pub rules_version: u32,
    pub candidates: Vec<DeveloperClutterCandidate>,
    pub partial: bool,
    pub truncated: bool,
}

impl Reader {
    /// Catalogue-only advice for one active root. Largest known logical aggregates first,
    /// then unknown sizes, with at most 200 candidates. No manifest contents or live paths
    /// are read. Node keys are catalogue references, never effect operands.
    pub fn developer_clutter(&mut self, root: i64, cap: usize) -> Result<DeveloperClutter> {
        if !(1..=200).contains(&cap) {
            return Err(Error::Invalid("developer clutter cap"));
        }
        let deadline = self.deadline;
        if deadline.is_some_and(|at| std::time::Instant::now() >= at) {
            return Err(Error::Cancelled);
        }
        let result = self.read(|conn| {
            let _guard = query::WorkGuard::new(conn, 2_000_000, deadline)?;
            query::revisions(conn, Some(root))?;
            let root_dir = conn.query_row(
                "SELECT id FROM dir WHERE root_id=?1 AND parent_id IS NULL",
                [root], |r| r.get(0),
            )?;
            let observed = query::get(conn, NodeKey::Dir(root_dir))?;
            let scanning = query::live(conn, root)?;
            let mut report = DeveloperClutter {
                rules_version: 1,
                partial: observed.coverage() != "complete" || !observed.valid || scanning,
                ..DeveloperClutter::default()
            };
            // Exact, case-sensitive names are conservative even on case-sensitive Windows roots.
            // Shields stop traversal; covered prevents double counting dependencies inside outputs.
            let mut stmt = conn.prepare(include_str!("developer-clutter.sql"))?;
            let rows = stmt.query_map(params![root_dir,
                EXCLUDED_FLAGS | 1 | (1 << 1) | (1 << 6) | (1 << 7) | (1 << 9),
                cap as i64 + 1, scanning], |r| Ok((
                    r.get::<_, i64>(0)?, r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?, r.get::<_, String>(3)?,
                )))?.collect::<rusqlite::Result<Vec<_>>>()?;
            report.truncated = rows.len() > cap;
            for (id, anchor, anchor_name, provider) in rows.into_iter().take(cap) {
                let row = query::get(conn, NodeKey::Dir(id))?;
                let coverage = row.coverage();
                let known = coverage == "complete" && row.valid && !scanning;
                report.partial |= !known;
                let (provider, hint, reason) = match provider.as_str() {
                    "cargo" => ("cargo", "Rebuild with Cargo; toolchain, dependencies and build-script inputs are unverified.",
                        "Cargo conventional target directory beside a recorded Cargo.toml."),
                    "npm" => ("npm", "Restore dependencies using the project's package manager and lockfile, if available; network and credentials are unverified.",
                        "Node conventional dependency directory beside a recorded package.json."),
                    "python" => ("python", "Recreate the environment and reinstall declared dependencies, or let Python regenerate bytecode; interpreter and inputs are unverified.",
                        "Python conventional environment or bytecode directory beside a recorded Python manifest."),
                    "dotnet" => ("dotnet", "Restore dependencies and rebuild with the matching .NET toolchain; inputs are unverified.",
                        ".NET conventional build directory beside a recorded project or solution."),
                    "gradle" => ("gradle", "Rebuild with the project's Gradle wrapper and dependencies, if available; toolchain and inputs are unverified.",
                        "Gradle conventional build or cache directory beside a recorded build manifest."),
                    _ => return Err(Error::Invalid("developer clutter provider")),
                };
                let kind = match row.name.as_str() {
                    "target" | "bin" | "obj" | "build" => "build_output",
                    "node_modules" => "dependency_install",
                    ".venv" | "venv" => "environment",
                    "__pycache__" => "bytecode_cache",
                    ".gradle" => "build_cache",
                    _ => return Err(Error::Invalid("developer clutter output name")),
                };
                report.candidates.push(DeveloperClutterCandidate {
                    directory: row.key,
                    directory_name: row.name,
                    anchor: NodeKey::File(anchor),
                    anchor_name,
                    provider,
                    kind,
                    logical_bytes: known.then_some(row.totals.logical),
                    allocated_bytes: (known && row.totals.allocation_unknown == 0)
                        .then_some(row.totals.allocated),
                    coverage,
                    regeneration_hint: hint,
                    regeneration_cost: "unknown",
                    reason,
                    uncertainty: "Names only: contents may contain unique source or datasets. Prerequisites, offline availability, active builds and rebuild cost are unknown. This is advice, never a disposability claim.",
                });
            }
            Ok(report)
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
