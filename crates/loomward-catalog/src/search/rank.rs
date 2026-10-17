//! Ranking v1 (LW-113 F2, docs/46 sections 5.2 and 10): weighted reciprocal-rank fusion of three
//! ranked candidate lists, plus additive boosts and a stable tie-break.
//!
//! - **trigram** (weight 1): the v1 passes (exact, prefix, token, substring, then the several-word
//!   passes), shorter names first within a pass. Substring evidence.
//! - **token** (weight 1): the token index, every query token a prefix of some name token; the
//!   first 256 matches per kind in key order, shorter names first. This is the short-query path
//!   too: one- and two-character prefixes are indexed, and the scan is bounded by that count.
//! - **context** (weight 0.5): entries whose own name plus the names of their two nearest ancestors
//!   cover every query token, found through the folder rows of the token index at query time.
//!
//! A hit's score is the sum over lists of `weight / (60 + rank)` plus a boost for an exact name,
//! a name prefix, all tokens at token starts, and (when the caller gives a clock) recency. Ties
//! break by index key (folders, then files, by id), so an order never depends on SQLite's.
//!
//! Ranked mode pages by offset over one pool that depends only on the query, its filters, the
//! ranking options and the page size (the cursor binds all four), so pages neither overlap nor skip
//! while the catalogue is unchanged. The pool is at least 100 hits deep (twice the page size above
//! 50), so a query answers at most [`HORIZON`] hits and usually fewer: it is not an exhaustive
//! enumeration (use `search` for that), and a budget cut during candidate collection ranks what was
//! seen and says `budget_hit`, without a scan position to resume. The cursor also binds the
//! catalogue revision: after any publication it is refused with `StaleGeneration`. Continuity is
//! not promised across a `budget_hit` page (a later page's pool may be cut elsewhere).
//! `search_ranked` returns `RankedIndexDisabled` on a store opened without
//! `CatalogOptions::ranked_index`.
use super::analyze::{query_tokens, shape, tokens};
use super::*;
use crate::query::Raw;

pub const TIER_TRIGRAM: u8 = 1;
pub const TIER_TOKEN: u8 = 2;
pub const TIER_CONTEXT: u8 = 4;
pub const ALL_TIERS: u8 = 7;

#[derive(Clone, Debug)]
pub struct RankOptions {
    /// FILETIME "now" for the recency boost; `None` switches recency off (deterministic).
    pub now_ft: Option<i64>,
    /// Which candidate lists take part (evaluation switch); [`ALL_TIERS`] by default.
    pub tiers: u8,
}
impl Default for RankOptions {
    fn default() -> Self {
        Self {
            now_ft: None,
            tiers: ALL_TIERS,
        }
    }
}

/// The cursor `pass` of ranked paging; `last_key` is the offset of the next hit.
const RANKED_PASS: u8 = 100;
/// The most ranked hits one query answers.
const HORIZON: usize = 500;
const RRF_K: f64 = 60.0;
const WEIGHTS: [f64; 3] = [1.0, 1.0, 0.5];
const BOOST_EXACT: f64 = 0.05;
const BOOST_PREFIX: f64 = 0.02;
const BOOST_TOKEN_START: f64 = 0.01;
const BOOST_RECENT: f64 = 0.006;
const TICKS_PER_DAY: f64 = 864_000_000_000.0;
/// Token-index matches read per kind before they are ranked.
const TOKEN_SCAN: usize = 256;
/// Index matches the token list looks at per kind, however few pass the filters.
const TOKEN_MAX: usize = 2048;
/// Folder anchors per query token, and the folder rows the context tier reads.
const ANCHORS: usize = 128;
const CONTEXT_DIRS: usize = 2048;
/// Entries the context tier reads per kind before it gates them.
const CONTEXT_ROWS: usize = 4096;

/// One ranked candidate list: hits in rank order.
type List = Vec<(SearchHit, i64)>;

struct Cand {
    hit: SearchHit,
    ranks: [Option<usize>; 3],
}

fn fts(qtokens: &[String]) -> String {
    qtokens
        .iter()
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}
/// The next window of token-index keys: up to `?4` matches of `?1` with a key above `?2` and below
/// `?3`, in key order. The only statement that touches the FTS table, so it is the one prepared
/// before the budget starts.
fn window_statement() -> &'static str {
    "SELECT rowid FROM search_token WHERE search_token MATCH ?1 AND rowid>?2 AND rowid<?3 LIMIT ?4"
}
/// The rows of one window that pass the visibility and filter conditions. Columns match `raw`,
/// then the match flag and the key.
fn window_rows_sql(files: bool, keys: &[i64], scope: Option<&[i64]>) -> String {
    // Row ids, not keys, so the primary-key lookup applies.
    let list = keys
        .iter()
        .map(|k| (if files { k - FILE_KEY } else { *k }).to_string())
        .collect::<Vec<_>>()
        .join(",");
    if files {
        format!(
            "SELECT {FILE_COLUMNS},1,f.id+{FILE_KEY} FROM file f JOIN dir d ON d.id=f.dir_id LEFT JOIN ext e ON e.id=f.ext_id WHERE f.id IN ({list}) AND {}",
            visible(true, scope)
        )
    } else {
        format!(
            "SELECT {DIR_COLUMNS},1,d.id FROM dir d WHERE d.id IN ({list}) AND {}",
            visible(false, scope)
        )
    }
}
fn anchor_statement() -> String {
    format!("SELECT rowid FROM search_token WHERE search_token MATCH ?1 AND rowid<{FILE_KEY} LIMIT {ANCHORS}")
}

/// The request's filter values in the order `visible` numbers them (?3 to ?8), after ?1, ?2.
fn values(req: &SearchRequest, expr: &str, limit: usize) -> Vec<Value> {
    vec![
        Value::Text(expr.into()),
        Value::Integer(0),
        req.root_id.map_or(Value::Null, Value::Integer),
        req.extension.clone().map_or(Value::Null, Value::Text),
        req.min_bytes
            .map_or(Value::Null, |v| Value::Integer(v as i64)),
        req.max_bytes
            .map_or(Value::Null, |v| Value::Integer(v as i64)),
        req.modified_after.map_or(Value::Null, Value::Integer),
        req.modified_before.map_or(Value::Null, Value::Integer),
        Value::Integer(limit as i64),
    ]
}

impl Walk<'_> {
    /// The entries of one kind whose every query token starts a name token, read in key order in
    /// windows of `TOKEN_SCAN` index matches until `TOKEN_SCAN` of them pass the request's filters
    /// (the bound counts eligible entries) or `TOKEN_MAX` matches have been looked at.
    fn token_rows(
        &mut self,
        conn: &Connection,
        files: bool,
        expr: &str,
    ) -> Result<Vec<(Raw, i64)>> {
        let (mut after, upper) = if files {
            (FILE_KEY - 1, i64::MAX)
        } else {
            (0, FILE_KEY)
        };
        let mut out = Vec::new();
        let mut looked = 0;
        loop {
            let keys: Vec<i64> = conn
                .prepare_cached(window_statement())?
                .query_map(
                    rusqlite::params![expr, after, upper, TOKEN_SCAN as i64],
                    |r| r.get(0),
                )?
                .collect::<rusqlite::Result<_>>()?;
            let Some(&last) = keys.last() else { break };
            after = last;
            looked += keys.len();
            let mut vals = values(self.req, "", 0);
            vals.truncate(8);
            let mut stmt =
                conn.prepare(&window_rows_sql(files, &keys, self.req.scope.as_deref()))?;
            let mut rows = stmt.query(rusqlite::params_from_iter(vals))?;
            while let Some(row) = rows.next()? {
                let key: i64 = row.get(24)?;
                out.push((raw(row)?, key));
            }
            if out.len() >= TOKEN_SCAN || keys.len() < TOKEN_SCAN || looked >= TOKEN_MAX {
                break;
            }
        }
        Ok(out)
    }
    /// One raw row as a hit, or none when its ancestors hide it.
    fn keep(&mut self, conn: &Connection, hit: Raw, key: i64) -> Result<Option<(SearchHit, i64)>> {
        let before = self.hits.len();
        self.hit(conn, hit, key)?;
        Ok((self.hits.len() > before).then(|| (self.hits.pop().expect("a hit").0, key)))
    }
}

/// Candidates for the token list: folders and files (as the request's kind allows) whose every
/// query token starts a name token. The index is read in key order, `TOKEN_SCAN` matches per kind
/// at most, and the shorter name is the better evidence; ranking all of a large match set would
/// cost more than the page it orders (measured: a prefix with 40k matches sorted in 70 ms alone).
fn token_list(walk: &mut Walk, conn: &Connection, qtokens: &[String], k: usize) -> Result<List> {
    let expr = fts(qtokens);
    let mut all = Vec::new();
    if walk.req.kind != "file" {
        all.extend(walk.token_rows(conn, false, &expr)?);
    }
    if walk.req.kind != "dir" {
        all.extend(walk.token_rows(conn, true, &expr)?);
    }
    all.sort_by_key(|(r, key)| (r.name.chars().count(), *key));
    // Only the rows that can be ranked become hits: a hit costs its path and its row.
    let mut out = Vec::new();
    for (r, key) in all {
        if let Some(h) = walk.keep(conn, r, key)? {
            out.push(h);
            if out.len() == k {
                break;
            }
        }
    }
    Ok(out)
}

/// Does `q` start a token of `tokens` or occur inside the folded `name`?
fn covers(q: &str, tokens: &[String], folded: &str) -> bool {
    tokens.iter().any(|t| t.starts_with(q)) || folded.contains(q)
}

/// Candidates for the context list: entries directly in folders whose own name or parent name
/// starts a query token, kept when every token is covered by the entry's name, its folder's name
/// or its folder's parent's name (the two nearest ancestors). Anchor folders come from the token
/// index; their children are read too, because a child's parent is the anchor.
fn context_list(walk: &mut Walk, conn: &Connection, qtokens: &[String], k: usize) -> Result<List> {
    let anchors: Vec<&String> = qtokens.iter().filter(|t| t.chars().count() >= 3).collect();
    if anchors.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids: Vec<i64> = Vec::new();
    for t in anchors.iter().take(4) {
        let mut stmt = conn.prepare_cached(&anchor_statement())?;
        let found = stmt
            .query_map([fts(std::slice::from_ref(t))], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.extend(found);
    }
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let anchor_ids = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    // Each folder's context: its own name and its parent's.
    let mut stmt = conn.prepare(&format!(
        "SELECT x.id,x.name,p.name FROM dir x LEFT JOIN dir p ON p.id=x.parent_id WHERE x.id IN ({anchor_ids}) OR x.parent_id IN ({anchor_ids}) LIMIT {CONTEXT_DIRS}"
    ))?;
    let mut folders: HashMap<i64, Vec<String>> = HashMap::new();
    let found = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, name, parent) in found {
        let have: Vec<String> = tokens(&name)
            .into_iter()
            .chain(parent.iter().flat_map(|p| tokens(p)))
            .collect();
        if qtokens
            .iter()
            .any(|q| have.iter().any(|t| t.starts_with(q.as_str())))
        {
            folders.insert(id, have);
        }
    }
    if folders.is_empty() {
        return Ok(Vec::new());
    }
    let list = folders
        .keys()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut out = Vec::new();
    for files in [true, false] {
        if (files && walk.req.kind == "dir") || (!files && walk.req.kind == "file") {
            continue;
        }
        let (columns, from, owner, key) = if files {
            (
                FILE_COLUMNS,
                "file f JOIN dir d ON d.id=f.dir_id LEFT JOIN ext e ON e.id=f.ext_id",
                "f.dir_id",
                format!("f.id+{FILE_KEY}"),
            )
        } else {
            (DIR_COLUMNS, "dir d", "d.parent_id", "d.id".to_string())
        };
        let sql = format!(
            "SELECT {columns},1,{key} FROM {from} WHERE {owner} IN ({list}) AND {} LIMIT {CONTEXT_ROWS}",
            visible(files, walk.req.scope.as_deref())
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut vals = values(walk.req, "", 0);
        vals.truncate(8);
        let mut rows = stmt.query(rusqlite::params_from_iter(vals))?;
        while let Some(row) = rows.next()? {
            let key: i64 = row.get(24)?;
            let hit = raw(row)?;
            let around = hit.parent.and_then(|p| folders.get(&p));
            let folded = hit.name.to_lowercase();
            let name = tokens(&hit.name);
            let covered = qtokens
                .iter()
                .all(|q| covers(q, &name, &folded) || around.is_some_and(|a| covers(q, a, "")));
            if covered {
                if let Some(h) = walk.keep(conn, hit, key)? {
                    out.push(h);
                }
                // A hit costs its path and row; the list is cut where fusion cannot use more.
                if out.len() >= k {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
}

/// An FTS5 error caused by the query text (a phrase or syntax the index refuses), not by the store.
fn is_fts_syntax(e: &Error) -> bool {
    matches!(e, Error::Sql(rusqlite::Error::SqliteFailure(_, Some(m)))
        if m.contains("phrase queries are not supported") || m.starts_with("fts5: syntax error"))
}

impl Reader {
    /// `search` ranked by fusion (see the module docs). An empty text has nothing to rank and is
    /// answered by `search`. A ranked cursor does not work with `search`, nor the reverse.
    pub fn search_ranked(&mut self, req: &SearchRequest, opts: &RankOptions) -> Result<SearchPage> {
        let deadline = self.deadline;
        let text = clean(req, deadline)?;
        if !self.read(super::tokens::enabled)? {
            return Err(Error::RankedIndexDisabled);
        }
        if text.is_empty() {
            return self.search(req);
        }
        // The page size and the ranking options shape the pool every page is cut from, so they bind the cursor.
        let filters = format!(
            "ranked:{}:{}:{:?}:{}",
            filters_key(req, &text),
            opts.tiers,
            opts.now_ft,
            req.limit
        );
        let qtokens = query_tokens(&text, 6);
        let plan = plan(&text);
        let offset = match &req.cursor {
            None => 0,
            Some(c) if c.pass == RANKED_PASS && c.last_key >= 0 => c.last_key as usize,
            Some(_) => return Err(Error::Invalid("search cursor binding")),
        };
        // Every page of a query ranks the same pool, whatever its offset: depth follows the page size.
        let k = (req.limit * 2).clamp(100, 1000);
        self.read(|conn| {
            let revisions = in_scope(req, revisions(conn, req.root_id)?, |r| r.0);
            let catalog_instance = instance(conn)?;
            let rev: i64 = conn.query_row("SELECT catalog_rev FROM revision", [], |r| r.get(0))?;
            if let Some(c) = &req.cursor {
                if c.filters != filters {
                    return Err(Error::Invalid("search cursor binding"));
                }
                if c.catalog_instance != catalog_instance || c.revision != Some(rev) {
                    return Err(Error::StaleGeneration);
                }
            }
            let roots = in_scope(req, root_states(conn, req.root_id)?, |r| r.root_id);
            // Prepare afresh and before the budget, as `search` does.
            conn.flush_prepared_statement_cache();
            for (pass, _) in &plan.passes {
                for (files, by_size) in [(false, false), (true, false)] {
                    conn.prepare_cached(&statement(
                        *pass == FILTER_PASS,
                        files,
                        by_size,
                        req.scope.as_deref(),
                    ))?;
                }
            }
            conn.prepare_cached(window_statement())?;
            conn.prepare_cached(&anchor_statement())?;
            // The trigram passes read no deeper than `search` would for the same page: a later
            // pass costs far more than an earlier one (a filter can force it), and the other
            // lists supply the depth.
            let inner = SearchRequest {
                limit: req.limit,
                cursor: None,
                ..req.clone()
            };
            let mut walk = Walk {
                req: &inner,
                text: &text,
                pass: plan.passes[0].0,
                key: i64::MIN,
                size: None,
                hits: Vec::new(),
                places: HashMap::new(),
                skipped: false,
                progress: Arc::new(AtomicBool::new(false)),
            };
            let soft = req.time_budget.map(|t| std::time::Instant::now() + t);
            let guard = WorkGuard::with_soft_deadline(conn, req.work_budget, deadline, soft, None)?;
            let mut lists: [List; 3] = Default::default();
            let mut hit = false;
            let mut run = |tier: usize, walk: &mut Walk| -> Result<bool> {
                let r = match tier {
                    0 => {
                        let r = walk.run(conn, &plan);
                        // Position within a pass is key order; shorter names are the better evidence.
                        // Kept even when the budget cut the run: a partial list still ranks.
                        let mut v: Vec<_> = std::mem::take(&mut walk.hits)
                            .into_iter()
                            .map(|(h, (pass, key, _))| (pass, h, key))
                            .collect();
                        v.sort_by_key(|(pass, h, key)| (*pass, h.row.name.chars().count(), *key));
                        lists[0] = v.into_iter().map(|(_, h, key)| (h, key)).collect();
                        r
                    }
                    1 => token_list(walk, conn, &qtokens, k).map(|v| lists[1] = v),
                    _ => context_list(walk, conn, &qtokens, k).map(|mut v| {
                        v.sort_by_key(|(h, key)| (h.row.name.chars().count(), *key));
                        lists[2] = v;
                    }),
                };
                match r {
                    Ok(()) => Ok(false),
                    // A query text the index cannot parse has no token or context candidates; the
                    // trigram list still answers it. (The budget is handled below.)
                    Err(e) if tier > 0 && is_fts_syntax(&e) => Ok(false),
                    Err(e) if guard.hit() && is_budget_interrupt(&e) => Ok(true),
                    Err(e) => Err(e),
                }
            };
            for tier in 0..3 {
                // A text with no token (only separators) has nothing for the token tiers to ask.
                if opts.tiers & (1 << tier) != 0 && !hit && (tier == 0 || !qtokens.is_empty()) {
                    hit = run(tier, &mut walk)?;
                }
            }
            hit |= guard.hit();
            drop(guard);

            // Fuse: one candidate per key, the rank each list gave it.
            let mut cands: HashMap<i64, Cand> = HashMap::new();
            for (tier, list) in lists.into_iter().enumerate() {
                for (rank, (hit, key)) in list.into_iter().enumerate() {
                    let c = cands.entry(key).or_insert_with(|| Cand {
                        hit,
                        ranks: [None; 3],
                    });
                    c.ranks[tier] = Some(rank);
                }
            }
            let now = opts.now_ft;
            let mut scored: Vec<(f64, i64, SearchHit)> = Vec::new();
            for (key, c) in cands {
                let mut hit = c.hit;
                let sh = shape(&hit.row.name, &text, &qtokens);
                let mut score = 0.0;
                for (tier, rank) in c.ranks.iter().enumerate() {
                    if let Some(r) = rank {
                        score += WEIGHTS[tier] / (RRF_K + *r as f64 + 1.0);
                    }
                }
                if sh.exact {
                    score += BOOST_EXACT;
                }
                if sh.prefix {
                    score += BOOST_PREFIX;
                }
                if sh.token_start {
                    score += BOOST_TOKEN_START;
                }
                if let (Some(now), Some(m)) = (
                    now,
                    hit.row
                        .modified_at
                        .as_deref()
                        .and_then(crate::model::filetime),
                ) {
                    let days = (now - m).max(0) as f64 / TICKS_PER_DAY;
                    score += BOOST_RECENT * 0.5_f64.powf(days / 365.0);
                }
                let kind = if sh.exact {
                    "exact"
                } else if sh.prefix {
                    "prefix"
                } else if sh.token_start || c.ranks[1].is_some() || !sh.inside {
                    "token"
                } else {
                    "substring"
                };
                let several = text.split_whitespace().count() > 1;
                let pass = match (kind, several) {
                    ("exact", _) => 0,
                    ("prefix", _) => 1,
                    ("token", false) => 2,
                    ("token", true) => 4,
                    (_, false) => 3,
                    (_, true) => 5,
                };
                hit.match_kind = kind.into();
                hit.highlights = highlights(&hit.row.name, &text, pass);
                scored.push((score, key, hit));
            }
            scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            let end = (offset + req.limit).min(HORIZON);
            let more = scored.len() > end && end < HORIZON;
            let hits: Vec<SearchHit> = scored
                .into_iter()
                .skip(offset)
                .take(req.limit.min(HORIZON.saturating_sub(offset)))
                .map(|(_, _, h)| h)
                .collect();
            let next_cursor = more.then(|| SearchCursor {
                catalog_instance: catalog_instance.clone(),
                filters: filters.clone(),
                pass: RANKED_PASS,
                last_key: end as i64,
                last_size: None,
                revision: Some(rev),
            });
            let (index_state, partial) = index_meta(&roots);
            // Names changed by a raw writer still wait for the token index: say it is catching up.
            let index_state = if super::tokens::pending(conn)? {
                "building"
            } else {
                index_state
            };
            Ok(SearchPage {
                generation: if revisions.len() == 1 {
                    revisions[0].1.map(|g| g.to_string())
                } else {
                    None
                },
                hits,
                next_cursor,
                budget_hit: hit,
                index_state: index_state.into(),
                partial: partial || walk.skipped,
                substring_searched: plan.substring_searched,
                roots,
            })
        })
    }
}
