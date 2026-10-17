//! Ranking v1 (LW-113 F2, docs/46 sections 5.2 and 10): weighted reciprocal-rank fusion of three
//! ranked candidate lists, plus additive boosts and a stable tie-break.
//!
//! - **trigram** (weight 1): the v1 passes (exact, prefix, token, substring, then the several-word
//!   passes), shorter names first within a pass. Substring evidence.
//! - **token** (weight 1): the token index, every query token a prefix of some name token, best
//!   bm25 first (a shorter name ranks higher). A query of one- and two-character tokens skips the
//!   sort and takes the first matches in key order: that is the short-query path, bounded.
//! - **context** (weight 0.5): entries whose own name plus the names of their two nearest ancestors
//!   cover every query token, found through the folder rows of the token index at query time.
//!
//! A hit's score is the sum over lists of `weight / (60 + rank)` plus a boost for an exact name,
//! a name prefix, all tokens at token starts, and (when the caller gives a clock) recency. Ties
//! break by index key (folders, then files, by id), so an order never depends on SQLite's.
//!
//! Ranked mode answers at most [`HORIZON`] hits per query and pages by offset: it is not an
//! exhaustive enumeration (use `search` for that), so a budget cut during candidate collection
//! ranks what was seen and says `budget_hit`, without a scan position to resume.
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
/// Entries a short query reads from the index before it ranks them.
const SHORT_SCAN: usize = 1024;
/// Folder anchors per query token, and the folder rows the context tier reads.
const ANCHORS: usize = 64;
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
/// A token-index query for folders or files (`ranked`: best bm25 first; else the first matches
/// in key order). Columns match `raw`, then the match flag, the key and the rank.
fn token_statement(files: bool, ranked: bool) -> String {
    let (columns, from, range) = if files {
        (
            FILE_COLUMNS,
            format!("search_token t JOIN file f ON f.id=t.rowid-{FILE_KEY} JOIN dir d ON d.id=f.dir_id LEFT JOIN ext e ON e.id=f.ext_id"),
            format!("t.rowid>={FILE_KEY}"),
        )
    } else {
        (
            DIR_COLUMNS,
            "search_token t JOIN dir d ON d.id=t.rowid".to_string(),
            format!("t.rowid<{FILE_KEY}"),
        )
    };
    let (rank, order) = if ranked {
        ("t.rank", "ORDER BY t.rank")
    } else {
        ("0.0", "")
    };
    format!(
        "SELECT {columns},1,t.rowid,{rank} FROM {from} WHERE search_token MATCH ?1 AND {range} AND {} {order} LIMIT ?9",
        visible(files)
    )
}
fn anchor_statement() -> String {
    format!("SELECT t.rowid FROM search_token t WHERE search_token MATCH ?1 AND t.rowid<{FILE_KEY} ORDER BY t.rank LIMIT {ANCHORS}")
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
    /// Run a token-index statement and keep the visible hits with their rank.
    fn token_rows(
        &mut self,
        conn: &Connection,
        files: bool,
        ranked: bool,
        expr: &str,
        limit: usize,
    ) -> Result<Vec<(f64, SearchHit, i64)>> {
        let mut stmt = conn.prepare_cached(&token_statement(files, ranked))?;
        let mut rows = stmt.query(rusqlite::params_from_iter(values(self.req, expr, limit)))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let key: i64 = row.get(24)?;
            let rank: f64 = row.get(25)?;
            if let Some((hit, key)) = self.keep(conn, raw(row)?, key)? {
                out.push((rank, hit, key));
            }
        }
        Ok(out)
    }
    /// One raw row as a hit, or none when its ancestors hide it.
    fn keep(&mut self, conn: &Connection, hit: Raw, key: i64) -> Result<Option<(SearchHit, i64)>> {
        if !hit.valid {
            return Err(Error::RepairRequired);
        }
        let before = self.hits.len();
        self.hit(conn, hit, key)?;
        Ok((self.hits.len() > before).then(|| (self.hits.pop().expect("a hit").0, key)))
    }
}

/// Candidates for the token list: folders and files (as the request's kind allows), best first.
fn token_list(walk: &mut Walk, conn: &Connection, qtokens: &[String], k: usize) -> Result<List> {
    let expr = fts(qtokens);
    // Only tokens of one or two characters: the match set can be most of the catalogue, so read
    // a bounded run in key order rather than rank all of it.
    let ranked = qtokens.iter().any(|t| t.chars().count() >= 3);
    let limit = if ranked { k } else { k.min(SHORT_SCAN) };
    let mut all = Vec::new();
    if walk.req.kind != "file" {
        all.extend(walk.token_rows(conn, false, ranked, &expr, limit)?);
    }
    if walk.req.kind != "dir" {
        all.extend(walk.token_rows(conn, true, ranked, &expr, limit)?);
    }
    all.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
    all.truncate(k);
    Ok(all.into_iter().map(|(_, hit, key)| (hit, key)).collect())
}

/// Does `q` start a token of `tokens` or occur inside the folded `name`?
fn covers(q: &str, tokens: &[String], folded: &str) -> bool {
    tokens.iter().any(|t| t.starts_with(q)) || folded.contains(q)
}

/// Candidates for the context list: entries directly in folders whose own name or parent name
/// starts a query token, kept when every token is covered by the entry's name, its folder's name
/// or its folder's parent's name (the two nearest ancestors). Anchor folders come from the token
/// index; their children are read too, because a child's parent is the anchor.
fn context_list(walk: &mut Walk, conn: &Connection, qtokens: &[String]) -> Result<List> {
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
            visible(files)
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
            }
        }
    }
    Ok(out)
}

impl Reader {
    /// `search` ranked by fusion (see the module docs). An empty text has nothing to rank and is
    /// answered by `search`. A ranked cursor does not work with `search`, nor the reverse.
    pub fn search_ranked(&mut self, req: &SearchRequest, opts: &RankOptions) -> Result<SearchPage> {
        let deadline = self.deadline;
        let text = clean(req, deadline)?;
        if text.is_empty() {
            return self.search(req);
        }
        let filters = format!("ranked:{}", filters_key(req, &text));
        let qtokens = query_tokens(&text, 6);
        let plan = plan(&text);
        let offset = match &req.cursor {
            None => 0,
            Some(c) if c.pass == RANKED_PASS && c.last_key >= 0 => c.last_key as usize,
            Some(_) => return Err(Error::Invalid("search cursor binding")),
        };
        let want = (offset + req.limit).min(HORIZON);
        let k = (want * 3).clamp(150, 1500);
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
            // Prepare afresh and before the budget, as `search` does.
            conn.flush_prepared_statement_cache();
            for (pass, _) in &plan.passes {
                for (files, by_size) in [(false, false), (true, false)] {
                    conn.prepare_cached(&statement(*pass == FILTER_PASS, files, by_size))?;
                }
            }
            for (files, ranked) in [(false, true), (true, true), (false, false), (true, false)] {
                conn.prepare_cached(&token_statement(files, ranked))?;
            }
            conn.prepare_cached(&anchor_statement())?;
            let inner = SearchRequest {
                limit: k,
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
            };
            let soft = req.time_budget.map(|t| std::time::Instant::now() + t);
            let guard = WorkGuard::with_soft_deadline(conn, req.work_budget, deadline, soft)?;
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
                    _ => context_list(walk, conn, &qtokens).map(|mut v| {
                        v.sort_by_key(|(h, key)| (h.row.name.chars().count(), *key));
                        lists[2] = v;
                    }),
                };
                match r {
                    Ok(()) => Ok(false),
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
            let more = scored.len() > want && want < HORIZON;
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
                last_key: want as i64,
                last_size: None,
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
                partial,
                substring_searched: plan.substring_searched,
                roots,
            })
        })
    }
}
