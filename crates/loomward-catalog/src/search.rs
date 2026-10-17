//! `search.query` over the name index (q-34, q-36; schema and triggers in `search.sql`).
//!
//! A query runs as ordered passes, each one FTS5 expression on the trigram index whose `NOT`
//! clauses exclude the earlier passes, so the passes are disjoint and every tier is decided by
//! SQLite under one case fold: exact (the whole name, or the name up to a dot), prefix, token (after
//! a separator), substring; then, for several words, all words at word starts, all words anywhere.
//! Inside a pass, folders come before files and both run in key order, which makes the cursor a
//! plain keyset position. An empty text lists by filters only (files of one extension largest
//! first). Visibility is checked per hit: active grant, requested root, no hidden ancestor.
use crate::query::{
    instance, is_budget_interrupt, raw, revisions, Raw, WorkGuard, DIR_COLUMNS, FILE_COLUMNS,
};
use crate::*;
use rusqlite::{types::Value, Connection};
use std::collections::HashMap;

mod analyze;
mod rank;
mod tokens;
pub use rank::{RankOptions, ALL_TIERS, TIER_CONTEXT, TIER_TOKEN, TIER_TRIGRAM};
pub(crate) use tokens::drain_tokens;

/// A file's index key is its id plus this; a folder's is its id.
pub(crate) const FILE_KEY: i64 = 1 << 48;
const BATCH: usize = 128;
/// What may precede a word: the name start sentinel or one of these.
const SEPARATORS: [char; 16] = [
    ' ', '_', '-', '.', '(', '[', '{', ',', '+', '&', '@', '#', '~', '\'', '=', '!',
];
const PASS_KIND: [&str; 7] = [
    "exact",
    "prefix",
    "token",
    "substring",
    "token",
    "substring",
    "filter",
];
const FILTER_PASS: u8 = 6;

fn phrase(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}
fn any(parts: impl IntoIterator<Item = String>) -> String {
    format!("({})", parts.into_iter().collect::<Vec<_>>().join(" OR "))
}

pub(crate) struct Plan {
    pub passes: Vec<(u8, String)>,
    pub substring_searched: bool,
}

/// The passes for a cleaned query. A trigram phrase needs three characters, so a one-character
/// text finds whole names only, and a two-character one everything but inner substrings.
pub(crate) fn plan(q: &str) -> Plan {
    if q.is_empty() {
        return Plan {
            passes: vec![(FILTER_PASS, String::new())],
            substring_searched: true,
        };
    }
    let starts = |t: &str| {
        any(std::iter::once(phrase(&format!("\u{1}{t}")))
            .chain(SEPARATORS.iter().map(|c| phrase(&format!("{c}{t}")))))
    };
    let exact = any([
        phrase(&format!("\u{1}{q}\u{2}")),
        phrase(&format!("\u{1}{q}.")),
    ]);
    let prefix = phrase(&format!("\u{1}{q}"));
    let token = any(SEPARATORS.iter().map(|c| phrase(&format!("{c}{q}"))));
    let n = q.chars().count();
    let mut passes = vec![(0, exact.clone())];
    if n >= 2 {
        passes.push((1, format!("{prefix} NOT {exact}")));
        passes.push((2, format!("{token} NOT {prefix}")));
    }
    if n >= 3 {
        passes.push((3, format!("{} NOT ({prefix} OR {token})", phrase(q))));
    }
    let mut substring_searched = n >= 3;
    let terms: Vec<&str> = q.split_whitespace().collect();
    if terms.len() > 1 {
        let whole = phrase(q);
        substring_searched = false;
        if terms.iter().all(|t| t.chars().count() >= 2) {
            let words = format!(
                "({})",
                terms
                    .iter()
                    .map(|t| starts(t))
                    .collect::<Vec<_>>()
                    .join(" AND ")
            );
            passes.push((4, format!("{words} NOT {whole}")));
            if terms.iter().all(|t| t.chars().count() >= 3) {
                let all = format!(
                    "({})",
                    terms
                        .iter()
                        .map(|t| phrase(t))
                        .collect::<Vec<_>>()
                        .join(" AND ")
                );
                passes.push((5, format!("{all} NOT ({whole} OR {words})")));
                substring_searched = true;
            }
        }
    }
    Plan {
        passes,
        substring_searched,
    }
}

/// Each character folded on its own (a multi-character lowering keeps the character) with the
/// UTF-16 offset where it starts; highlights only, the match itself is SQLite's.
fn folded(s: &str) -> Vec<(char, u32)> {
    let mut at = 0;
    s.chars()
        .map(|c| {
            let mut lower = c.to_lowercase();
            let f = match (lower.next(), lower.next()) {
                (Some(l), None) => l,
                _ => c,
            };
            let here = at;
            at += c.len_utf16() as u32;
            (f, here)
        })
        .collect()
}
fn find(name: &[(char, u32)], end: u32, needle: &str, word: bool, start: bool) -> Option<[u32; 2]> {
    let needle: Vec<char> = folded(needle).into_iter().map(|(c, _)| c).collect();
    if needle.is_empty() || needle.len() > name.len() {
        return None;
    }
    (0..=name.len() - needle.len())
        .take(if start { 1 } else { usize::MAX })
        .find(|&i| {
            (!word || i == 0 || SEPARATORS.contains(&name[i - 1].0))
                && name[i..i + needle.len()]
                    .iter()
                    .zip(&needle)
                    .all(|((a, _), b)| a == b)
        })
        .map(|i| [name[i].1, name.get(i + needle.len()).map_or(end, |c| c.1)])
}
fn highlights(name: &str, q: &str, pass: u8) -> Vec<[u32; 2]> {
    let f = folded(name);
    let end = name.encode_utf16().count() as u32;
    let mut out: Vec<[u32; 2]> = match pass {
        0 | 1 => find(&f, end, q, false, true).into_iter().collect(),
        2 => find(&f, end, q, true, false).into_iter().collect(),
        3 => find(&f, end, q, false, false).into_iter().collect(),
        4 | 5 => q
            .split_whitespace()
            .filter_map(|t| find(&f, end, t, pass == 4, false))
            .collect(),
        _ => vec![],
    };
    out.sort_unstable();
    out.dedup();
    out.truncate(8);
    out
}

struct Place {
    rel: String,
    root_name: String,
    hidden: bool,
}
/// Root-relative path and visibility of a folder, cached for the request.
fn place<'a>(conn: &Connection, cache: &'a mut HashMap<i64, Place>, dir: i64) -> Result<&'a Place> {
    let mut chain = Vec::new();
    let mut at = Some(dir);
    while let Some(id) = at.filter(|id| !cache.contains_key(id)) {
        let (parent, name, state): (Option<i64>, String, String) = conn
            .prepare_cached("SELECT parent_id,name,listing_state FROM dir WHERE id=?1")?
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        if chain.len() > 1024 {
            return Err(Error::Invalid("directory identity cycle"));
        }
        chain.push((id, parent, name, state == "absent_pending"));
        at = parent;
    }
    for (id, parent, name, hidden) in chain.into_iter().rev() {
        let place = match parent.and_then(|p| cache.get(&p)) {
            Some(p) => Place {
                rel: if p.rel.is_empty() {
                    name
                } else {
                    format!("{}\\{name}", p.rel)
                },
                root_name: p.root_name.clone(),
                hidden: p.hidden || hidden,
            },
            None => Place {
                rel: String::new(),
                root_name: name,
                hidden,
            },
        };
        cache.insert(id, place);
    }
    Ok(&cache[&dir])
}

fn root_states(conn: &Connection, root: Option<i64>) -> Result<Vec<SearchRootState>> {
    let mut s = conn.prepare_cached("SELECT r.id,r.state,coalesce(d.sub_complete,0) FROM root r JOIN root_grant g ON g.id=r.id LEFT JOIN dir d ON d.root_id=r.id AND d.parent_id IS NULL WHERE g.state='active' AND (?1 IS NULL OR r.id=?1) ORDER BY r.id LIMIT 64")?;
    let rows = s.query_map([root], |r| {
        let state: String = r.get(1)?;
        let complete: bool = r.get(2)?;
        // As roots.list: a finished traversal with excluded or skipped entries is partial coverage.
        let coverage = match state.as_str() {
            "never_scanned" => "unscanned",
            "complete" if complete => "complete",
            "stale" => "stale",
            "failed" => "unknown",
            _ => "partial",
        };
        Ok(SearchRootState {
            root_id: r.get(0)?,
            coverage: coverage.into(),
            scan_state: state,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The visibility and filter condition of one hit's row (`d` is its folder): an active grant, the
/// requested root, no pending absence, and the request's filters. NULL reads as false.
fn visible(files: bool) -> String {
    let filters = if files {
        "(?4 IS NULL OR e.ext=?4) AND (?5 IS NULL OR f.logical>=?5) AND (?6 IS NULL OR f.logical<=?6) AND (?7 IS NULL OR f.modified_ft>=?7) AND (?8 IS NULL OR f.modified_ft<=?8)"
    } else {
        "?4 IS NULL AND (?5 IS NULL OR d.sub_logical>=?5) AND (?6 IS NULL OR d.sub_logical<=?6) AND (?7 IS NULL OR d.modified_ft>=?7) AND (?8 IS NULL OR d.modified_ft<=?8)"
    };
    format!("d.root_id IN (SELECT id FROM root_grant WHERE state='active') AND d.listing_state!='absent_pending' AND (?3 IS NULL OR d.root_id=?3) AND {filters}")
}
/// The SQL of one batch: a pass's folders or files, or a filter-only listing of either.
fn statement(filter: bool, files: bool, by_size: bool) -> String {
    let matched = format!("coalesce({},0)", visible(files));
    match (filter, files) {
        (false, false) => format!("SELECT {DIR_COLUMNS},{matched},s.rowid FROM search_name s JOIN dir d ON d.id=s.rowid WHERE search_name MATCH ?1 AND s.rowid>?2 AND s.rowid<{FILE_KEY} ORDER BY s.rowid LIMIT {BATCH}"),
        (false, true) => format!("SELECT {FILE_COLUMNS},{matched},s.rowid FROM search_name s JOIN file f ON f.id=s.rowid-{FILE_KEY} JOIN dir d ON d.id=f.dir_id LEFT JOIN ext e ON e.id=f.ext_id WHERE search_name MATCH ?1 AND s.rowid>?2 ORDER BY s.rowid LIMIT {BATCH}"),
        (true, false) => format!("SELECT {DIR_COLUMNS},{matched},d.id FROM dir d WHERE ?1 IS NOT NULL AND d.id>?2 AND d.parent_id IS NOT NULL ORDER BY d.id LIMIT {BATCH}"),
        // One extension: its index, largest first, then by id.
        (true, true) if by_size => format!("SELECT {FILE_COLUMNS},{matched},f.id+{FILE_KEY} FROM file f INDEXED BY file_by_ext JOIN dir d ON d.id=f.dir_id LEFT JOIN ext e ON e.id=f.ext_id WHERE ?1 IS NOT NULL AND f.ext_id=(SELECT id FROM ext WHERE ext=?4) AND (?9 IS NULL OR f.logical<?9 OR (f.logical=?9 AND f.id>?2-{FILE_KEY})) ORDER BY f.logical DESC,f.id LIMIT {BATCH}"),
        (true, true) => format!("SELECT {FILE_COLUMNS},{matched},f.id+{FILE_KEY} FROM file f JOIN dir d ON d.id=f.dir_id LEFT JOIN ext e ON e.id=f.ext_id WHERE ?1 IS NOT NULL AND f.id>?2-{FILE_KEY} ORDER BY f.id LIMIT {BATCH}"),
}
}

/// A keyset position: pass, last key, and the last size of a by-size listing.
type Position = (u8, i64, Option<i64>);

struct Walk<'r> {
    req: &'r SearchRequest,
    text: &'r str,
    pass: u8,
    key: i64,
    size: Option<i64>,
    hits: Vec<(SearchHit, Position)>,
    places: HashMap<i64, Place>,
}
impl Walk<'_> {
    fn full(&self) -> bool {
        self.hits.len() > self.req.limit
    }
    /// One batch of one pass's folders or files. Returns whether the batch was full.
    fn batch(&mut self, conn: &Connection, expr: &str, files: bool) -> Result<bool> {
        let r = self.req;
        let by_size = files && self.pass == FILTER_PASS && r.extension.is_some();
        let sql = statement(self.pass == FILTER_PASS, files, by_size);
        let mut stmt = conn.prepare_cached(&sql)?;
        let values = [
            Value::Text(expr.into()),
            Value::Integer(self.key),
            r.root_id.map_or(Value::Null, Value::Integer),
            r.extension.clone().map_or(Value::Null, Value::Text),
            r.min_bytes
                .map_or(Value::Null, |v| Value::Integer(v as i64)),
            r.max_bytes
                .map_or(Value::Null, |v| Value::Integer(v as i64)),
            r.modified_after.map_or(Value::Null, Value::Integer),
            r.modified_before.map_or(Value::Null, Value::Integer),
            self.size.map_or(Value::Null, Value::Integer),
        ];
        let n = stmt.parameter_count();
        let mut rows = stmt.query(rusqlite::params_from_iter(values.into_iter().take(n)))?;
        let mut examined = 0;
        while let Some(row) = rows.next()? {
            examined += 1;
            let key: i64 = row.get(24)?;
            if row.get::<_, bool>(23)? {
                let hit = raw(row)?;
                if !hit.valid {
                    return Err(Error::RepairRequired);
                }
                self.hit(conn, hit, key)?;
            }
            self.key = key;
            if by_size {
                self.size = Some(row.get(3)?);
            }
            if self.full() {
                return Ok(false);
            }
        }
        Ok(examined == BATCH)
    }
    fn hit(&mut self, conn: &Connection, hit: Raw, key: i64) -> Result<()> {
        // A folder hit is never a root (roots are not indexed), so both kinds have a parent.
        let parent = place(conn, &mut self.places, hit.parent.ok_or(Error::NotFound)?)?;
        if parent.hidden {
            return Ok(());
        }
        let location = if parent.rel.is_empty() {
            parent.root_name.clone()
        } else {
            format!("{}\\{}", parent.root_name, parent.rel)
        };
        let path = if parent.rel.is_empty() {
            hit.name.clone()
        } else {
            format!("{}\\{}", parent.rel, hit.name)
        };
        let mut row = hit.entry();
        row.location_hint = Some(DisplayPath::bounded(location, false));
        let size = if self.pass == FILTER_PASS && self.req.extension.is_some() {
            Some(hit.totals.logical as i64)
        } else {
            None
        };
        self.hits.push((
            SearchHit {
                highlights: highlights(&hit.name, self.text, self.pass),
                row,
                root_id: hit.grant,
                path: DisplayPath::bounded(path, false),
                match_kind: PASS_KIND[self.pass as usize].into(),
            },
            (self.pass, key, size),
        ));
        Ok(())
    }
    fn run(&mut self, conn: &Connection, plan: &Plan) -> Result<()> {
        for (pass, expr) in &plan.passes {
            if *pass < self.pass {
                continue;
            }
            if *pass != self.pass {
                (self.pass, self.key, self.size) = (*pass, i64::MIN, None);
            }
            let dirs = self.req.kind != "file"
                && !(self.pass == FILTER_PASS && self.req.extension.is_some());
            if dirs && self.key < FILE_KEY {
                while self.batch(conn, expr, false)? {}
                if self.full() {
                    return Ok(());
                }
            }
            if self.req.kind != "dir" {
                if self.key < FILE_KEY {
                    (self.key, self.size) = (FILE_KEY - 1, None);
                }
                while self.batch(conn, expr, true)? {}
                if self.full() {
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

/// Bounds-check a request and return its cleaned text (trimmed, no control characters).
fn clean(req: &SearchRequest, deadline: Option<std::time::Instant>) -> Result<String> {
    if deadline.is_some_and(|at| std::time::Instant::now() >= at) {
        return Err(Error::Cancelled);
    }
    let bytes = |v: Option<u64>| v.is_some_and(|v| v > i64::MAX as u64);
    if !(1..=100).contains(&req.limit)
        || req.text.chars().count() > 256
        || req
            .extension
            .as_ref()
            .is_some_and(|s| s.chars().count() > 32)
        || bytes(req.min_bytes)
        || bytes(req.max_bytes)
        || !matches!(req.kind.as_str(), "any" | "file" | "dir")
    {
        return Err(Error::Invalid("search bounds"));
    }
    let text: String = req
        .text
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    Ok(text.trim().to_string())
}
/// What a cursor is bound to: the request's filters and cleaned text.
fn filters_key(req: &SearchRequest, text: &str) -> String {
    format!(
        "{:?}",
        (
            req.root_id,
            text,
            &req.extension,
            req.min_bytes,
            req.max_bytes,
            req.modified_after,
            req.modified_before,
            &req.kind
        )
    )
}
/// The page's `index_state` and `partial` from the searched roots.
fn index_meta(roots: &[SearchRootState]) -> (&'static str, bool) {
    let state = if roots.iter().any(|r| r.scan_state == "scanning") {
        "building"
    } else if roots
        .iter()
        .any(|r| matches!(r.scan_state.as_str(), "stale" | "failed" | "repairing"))
    {
        "stale"
    } else {
        "current"
    };
    (state, roots.iter().any(|r| r.coverage != "complete"))
}

impl Reader {
    pub fn search(&mut self, req: &SearchRequest) -> Result<SearchPage> {
        let deadline = self.deadline;
        let text = clean(req, deadline)?;
        let filters = filters_key(req, &text);
        let plan = plan(&text);
        self.read(|conn| {
            let revisions = revisions(conn, req.root_id)?;
            let catalog_instance = instance(conn)?;
            if let Some(c) = &req.cursor {
                if c.filters != filters {
                    return Err(Error::Invalid("search cursor binding"));
                }
                if c.catalog_instance != catalog_instance {
                    return Err(Error::StaleGeneration);
                }
            }
            let roots = root_states(conn, req.root_id)?;
            // Prepare afresh and before the budget: preparing the FTS table runs SQL, and a schema
            // error raised there under the progress handler would read as a budget cut.
            conn.flush_prepared_statement_cache();
            for (pass, _) in &plan.passes {
                let filter = *pass == FILTER_PASS;
                for (files, by_size) in [(false, false), (true, false), (true, true)] {
                    if !by_size || filter {
                        conn.prepare_cached(&statement(filter, files, by_size))?;
                    }
                }
            }
            let (pass, key, size) = req
                .cursor
                .as_ref()
                .map_or((plan.passes[0].0, i64::MIN, None), |c| {
                    (c.pass, c.last_key, c.last_size)
                });
            let mut walk = Walk {
                req,
                text: &text,
                pass,
                key,
                size,
                hits: Vec::new(),
                places: HashMap::new(),
            };
            let soft = req.time_budget.map(|t| std::time::Instant::now() + t);
            let guard = WorkGuard::with_soft_deadline(conn, req.work_budget, deadline, soft)?;
            let mut hit = false;
            if let Err(e) = walk.run(conn, &plan) {
                if guard.hit() && is_budget_interrupt(&e) {
                    hit = true;
                } else {
                    return Err(e);
                }
            }
            hit |= guard.hit();
            drop(guard);
            let more = walk.full();
            walk.hits.truncate(req.limit);
            let position = if more {
                walk.hits.last().map(|(_, at)| *at)
            } else if hit {
                Some((walk.pass, walk.key, walk.size))
            } else {
                None
            };
            let next_cursor = position.map(|(pass, last_key, last_size)| SearchCursor {
                catalog_instance: catalog_instance.clone(),
                filters: filters.clone(),
                pass,
                last_key,
                last_size,
            });
            let (index_state, partial) = index_meta(&roots);
            Ok(SearchPage {
                generation: if revisions.len() == 1 {
                    revisions[0].1.map(|g| g.to_string())
                } else {
                    None
                },
                hits: walk.hits.into_iter().map(|(h, _)| h).collect(),
                next_cursor,
                budget_hit: hit,
                index_state: index_state.into(),
                partial,
                substring_searched: plan.substring_searched,
                roots,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_texts_skip_the_passes_a_trigram_cannot_express() {
        let one = plan("a");
        assert_eq!(one.passes.iter().map(|p| p.0).collect::<Vec<_>>(), [0]);
        assert!(!one.substring_searched);
        let two = plan("ab");
        assert_eq!(
            two.passes.iter().map(|p| p.0).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert!(!two.substring_searched);
        let words = plan("annual report");
        assert_eq!(
            words.passes.iter().map(|p| p.0).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5]
        );
        assert!(words.substring_searched);
        assert!(!plan("a report").substring_searched);
        assert_eq!(plan("").passes[0].0, FILTER_PASS);
        assert_eq!(phrase("a\"b"), "\"a\"\"b\"");
    }
    #[test]
    fn filetime_inverts_timestamp() {
        let ft = filetime("2026-10-09T12:00:00Z").unwrap();
        assert_eq!(
            crate::model::timestamp(Some(ft)).unwrap(),
            "2026-10-09T12:00:00Z"
        );
        assert_eq!(filetime("1601-01-01T00:00:00Z"), Some(0));
        assert_eq!(filetime("not a time"), None);
    }
    #[test]
    fn highlights_are_utf16_offsets_of_the_folded_match() {
        assert_eq!(highlights("Annual Report.pdf", "report", 2), vec![[7, 13]]);
        assert_eq!(highlights("REPORT.pdf", "report", 0), vec![[0, 6]]);
        // An astral character before the match counts two UTF-16 units.
        assert_eq!(highlights("\u{1F600}xReport", "report", 3), vec![[3, 9]]);
        assert_eq!(
            highlights("my report draft", "draft report", 4),
            vec![[3, 9], [10, 15]]
        );
        assert!(highlights("x", "y", 3).is_empty());
    }
}
