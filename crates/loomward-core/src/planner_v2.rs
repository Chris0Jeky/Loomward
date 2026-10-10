//! Bounded allocation search over estimates: a port of `python/loomward/planner_v2.py`.
//!
//! The v1 greedy planner (`planner.rs`) is unchanged and is reused for input validation and the
//! baseline incumbent. A deterministic heuristic portfolio supplies a feasible plan; small
//! problems additionally get a node-bounded exact search. "Optimal" refers only to this static
//! disjoint-group model and its lexicographic objective, never to real-world safety or future
//! access costs. Nothing here touches a filesystem; every proposal is `executable: false`.
//!
//! Byte arithmetic is integer only (u128 sums, i128 for free-space deltas). Heat is compared
//! exactly as Python does: `Fraction(str(heat)) * source_bytes`, i.e. the shortest round-trip
//! decimal of the f64, kept as an integer scaled by a common power of ten.
use crate::planner::{plan, Group, Proposal, Rejection, Scenario, Volume};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_SEARCH_GROUPS: usize = 12;
pub const MAX_SEARCH_TARGETS: usize = 4;
pub const DEFAULT_NODE_BUDGET: u64 = 50_000;
const MAX_NODE_BUDGET: u64 = 200_000;
const HEAT_RANGE: &str = "heat decimals exceed the exact integer range of the Rust port";

#[derive(Debug, Serialize)]
pub struct SearchReport {
    pub complete: bool,
    pub nodes_visited: u64,
    pub node_budget: u64,
    pub reason: &'static str,
    pub eligible_groups: usize,
    pub eligible_targets: usize,
    pub lower_bound_shortfall_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct PlanV2 {
    pub mode: &'static str,
    pub algorithm: &'static str,
    pub optimality_claim: bool,
    pub optimality_scope: &'static str,
    pub shortfall_optimal: bool,
    pub search: SearchReport,
    pub proposals: Vec<Proposal>,
    pub rejected: Vec<Rejection>,
    pub projected_free_bytes: BTreeMap<String, u64>,
    pub target_free_bytes: u64,
    pub shortfall_bytes: u64,
    pub satisfied: bool,
    pub transfer_bytes: u64,
    pub filesystem_changed: bool,
    pub baseline_shortfall_bytes: u64,
    pub shortfall_improvement_bytes: i64,
    pub assumption: &'static str,
}

type Pair<'a> = (&'a str, &'a str); // (group id, target volume id)

/// Lexicographic objective, field order = priority: unmet demand, transfer cost, disruption
/// (group count), heat, then the sorted assignment ids as the deterministic tie-break.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Score<'a> {
    shortfall: u128,
    transfer: u128,
    count: usize,
    heat: u128,
    ids: Vec<Pair<'a>>,
}

struct Ctx<'a> {
    need: u128,
    groups: BTreeMap<&'a str, &'a Group>,
    heat: BTreeMap<&'a str, u128>, // heat * source_bytes, scaled by a shared power of ten
}

fn score<'a>(c: &Ctx<'a>, a: &[Pair<'a>]) -> Score<'a> {
    let (mut relief, mut transfer, mut heat) = (0u128, 0u128, 0u128);
    for (g, _) in a {
        let gr = c.groups[g];
        relief += u128::from(gr.source_bytes);
        transfer += u128::from(gr.transfer_bytes);
        heat += c.heat[g];
    }
    let mut ids = a.to_vec();
    ids.sort_unstable();
    Score {
        shortfall: c.need.saturating_sub(relief),
        transfer,
        count: a.len(),
        heat,
        ids,
    }
}

/// `Fraction(str(h))` as (digits, scale): h == digits / 10^scale, from the shortest round-trip
/// decimal (Rust and Python both print the shortest digits that round-trip).
fn decimal(h: f64) -> (u128, u32) {
    let s = format!("{:e}", h.abs());
    let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
    let e: i32 = e.parse().unwrap_or(0);
    let digits: String = m.chars().filter(|c| *c != '.').collect();
    let d: u128 = digits.parse().unwrap_or(0);
    let k = digits.len() as i32 - 1 - e;
    if d == 0 {
        (0, 0)
    } else if k >= 0 {
        (d, k as u32)
    } else {
        (d.saturating_mul(10u128.saturating_pow((-k) as u32)), 0)
    }
}

struct Search<'a, 'c> {
    ctx: &'c Ctx<'a>,
    ordered: Vec<&'a Group>,
    targets: &'c [&'a Volume],
    free: Vec<i128>,
    suffix: Vec<u128>,
    budget: u128,
    node_budget: u64,
    visited: u64,
    cutoff: bool,
    best: Vec<Pair<'a>>,
    best_score: Score<'a>,
    assignment: Vec<Pair<'a>>,
}

impl<'a> Search<'a, '_> {
    fn visit(&mut self, i: usize, relief: u128, spent: u128) {
        if self.visited >= self.node_budget {
            self.cutoff = true;
            return;
        }
        self.visited += 1;
        let optimistic = self.ctx.need.saturating_sub(relief + self.suffix[i]);
        if optimistic > self.best_score.shortfall
            || (optimistic == self.best_score.shortfall && spent > self.best_score.transfer)
        {
            return;
        }
        if i == self.ordered.len() || relief >= self.ctx.need {
            let current = score(self.ctx, &self.assignment);
            if current < self.best_score {
                self.best = self.assignment.clone();
                self.best_score = current;
            }
            return;
        }
        let g = self.ordered[i];
        let (dest, xfer, src) = (
            i128::from(g.destination_bytes),
            u128::from(g.transfer_bytes),
            u128::from(g.source_bytes),
        );
        if spent + xfer <= self.budget {
            for j in 0..self.targets.len() {
                let t = self.targets[j];
                if self.free[j] - dest < i128::from(t.reserve_bytes) {
                    continue;
                }
                self.free[j] -= dest;
                self.assignment.push((g.id.as_str(), t.id.as_str()));
                self.visit(i + 1, relief + src, spent + xfer);
                self.assignment.pop();
                self.free[j] += dest;
                if self.cutoff {
                    return;
                }
            }
        }
        self.visit(i + 1, relief, spent);
    }
}

pub fn plan_v2(s: &Scenario, node_budget: u64) -> Result<PlanV2, String> {
    if node_budget > MAX_NODE_BUDGET {
        return Err(format!(
            "node_budget must be an integer in [0, {MAX_NODE_BUDGET}]"
        ));
    }
    let baseline = plan(s)?; // the complete v1 input/capacity checks
    let source_id = s.source_id.as_str();
    let volumes: BTreeMap<&str, &Volume> = s.volumes.iter().map(|v| (v.id.as_str(), v)).collect();
    let source = volumes[source_id];
    let source_open = source.online == Some(true) && source.writable == Some(true);
    let need = u128::from(s.target_free_bytes.saturating_sub(source.free_bytes));
    let budget = u128::from(s.max_transfer_bytes);
    let groups: BTreeMap<&str, &Group> = s.groups.iter().map(|g| (g.id.as_str(), g)).collect();
    let mut targets: Vec<&Volume> = s
        .volumes
        .iter()
        .filter(|v| {
            v.id != s.source_id
                && v.online == Some(true)
                && v.writable == Some(true)
                && v.tier >= source.tier
        })
        .collect();
    targets.sort_by(|a, b| a.id.cmp(&b.id));

    let mut candidates: Vec<&Group> = Vec::new();
    let mut rejected: Vec<Rejection> = Vec::new();
    for g in &s.groups {
        let reason = if g.volume_id != s.source_id {
            Some("not_on_source")
        } else if g.pinned != Some(false) || g.active != Some(false) || g.protected != Some(false) {
            Some("pinned_active_protected_or_unspecified")
        } else if g.heat.is_none() {
            Some("heat_unknown")
        } else if g.heat.is_some_and(|h| h > 0.25) {
            Some("not_cold")
        } else if g.days_since_move.is_none_or(|d| d < s.cooldown_days) {
            Some("cooldown_or_history_unknown")
        } else if !source_open {
            Some("source_offline_or_readonly")
        } else {
            None
        };
        match reason {
            Some(reason) => rejected.push(Rejection {
                group_id: g.id.clone(),
                reason,
            }),
            None => candidates.push(g),
        }
    }

    // Exact heat terms on one shared scale; the total bounds every subset sum.
    let dec: Vec<(u128, u32)> = candidates
        .iter()
        .map(|g| decimal(g.heat.unwrap_or(0.0)))
        .collect();
    let kmax = dec.iter().map(|d| d.1).max().unwrap_or(0);
    let mut heat = BTreeMap::new();
    let mut total = 0u128;
    for (g, (d, k)) in candidates.iter().zip(&dec) {
        // ponytail: u128 ceiling; Python's Fraction is unbounded. Upgrade to a bigint if a
        // real scenario ever mixes heats whose decimal scales differ by ~20 digits.
        let t = 10u128
            .checked_pow(kmax - k)
            .and_then(|p| p.checked_mul(*d))
            .and_then(|x| x.checked_mul(u128::from(g.source_bytes)))
            .ok_or(HEAT_RANGE)?;
        total = total.checked_add(t).ok_or(HEAT_RANGE)?;
        heat.insert(g.id.as_str(), t);
    }
    let _ = total;
    let ctx = Ctx { need, groups, heat };

    let mut best: Vec<Pair> = baseline
        .proposals
        .iter()
        .map(|p| (p.group_id.as_str(), p.target_id.as_str()))
        .collect();
    let mut best_score = score(&ctx, &best);

    let mut by_ratio = candidates.clone();
    by_ratio.sort_by(|a, b| {
        let (asrc, axfer) = (u128::from(a.source_bytes), u128::from(a.transfer_bytes));
        let (bsrc, bxfer) = (u128::from(b.source_bytes), u128::from(b.transfer_bytes));
        (bsrc * axfer)
            .cmp(&(asrc * bxfer)) // descending source/transfer, compared exactly
            .then(b.source_bytes.cmp(&a.source_bytes))
            .then(a.id.cmp(&b.id))
    });
    let mut by_relief = candidates.clone();
    by_relief.sort_by(|a, b| {
        b.source_bytes
            .cmp(&a.source_bytes)
            .then(a.transfer_bytes.cmp(&b.transfer_bytes))
            .then(a.id.cmp(&b.id))
    });
    let mut by_cost = candidates.clone();
    by_cost.sort_by(|a, b| {
        a.transfer_bytes
            .cmp(&b.transfer_bytes)
            .then(b.source_bytes.cmp(&a.source_bytes))
            .then(a.id.cmp(&b.id))
    });
    for ordered in [&by_ratio, &by_relief, &by_cost] {
        let mut free: Vec<i128> = targets.iter().map(|v| i128::from(v.free_bytes)).collect();
        let mut allocation: Vec<Pair> = Vec::new();
        let (mut spent, mut relief) = (0u128, 0u128);
        for g in ordered {
            if relief >= need {
                break;
            }
            if spent + u128::from(g.transfer_bytes) > budget {
                continue;
            }
            let dest = i128::from(g.destination_bytes);
            // Best fit reduces fragmentation; it is a heuristic, not a proof.
            let key = |j: usize| {
                let t = targets[j];
                (
                    free[j] - i128::from(t.reserve_bytes) - dest,
                    -i16::from(t.tier),
                    t.id.as_str(),
                )
            };
            let Some(j) = (0..targets.len())
                .filter(|&j| free[j] - dest >= i128::from(targets[j].reserve_bytes))
                .min_by_key(|&j| key(j))
            else {
                continue;
            };
            free[j] -= dest;
            relief += u128::from(g.source_bytes);
            spent += u128::from(g.transfer_bytes);
            allocation.push((g.id.as_str(), targets[j].id.as_str()));
        }
        let candidate = score(&ctx, &allocation);
        if candidate < best_score {
            best = allocation;
            best_score = candidate;
        }
    }

    let mut suffix = vec![0u128; by_ratio.len() + 1];
    for i in (0..by_ratio.len()).rev() {
        suffix[i] = suffix[i + 1] + u128::from(by_ratio[i].source_bytes);
    }
    let too_large = by_ratio.len() > MAX_SEARCH_GROUPS || targets.len() > MAX_SEARCH_TARGETS;
    let lower_bound = need.saturating_sub(suffix[0]);
    let mut search = Search {
        ctx: &ctx,
        ordered: by_ratio,
        targets: &targets,
        free: targets.iter().map(|v| i128::from(v.free_bytes)).collect(),
        suffix,
        budget,
        node_budget,
        visited: 0,
        cutoff: false,
        best,
        best_score,
        assignment: Vec::new(),
    };
    if !too_large {
        search.visit(0, 0, 0);
    }
    let (visited, cutoff, best, best_score) = (
        search.visited,
        search.cutoff,
        search.best,
        search.best_score,
    );
    let complete = !too_large && !cutoff;

    let mut projected: BTreeMap<String, i128> = s
        .volumes
        .iter()
        .map(|v| (v.id.clone(), i128::from(v.free_bytes)))
        .collect();
    let mut chosen = best;
    chosen.sort_unstable();
    let mut proposals = Vec::new();
    for (group_id, target_id) in &chosen {
        let g = ctx.groups[group_id];
        *projected.entry(s.source_id.clone()).or_default() += i128::from(g.source_bytes);
        *projected.entry((*target_id).to_string()).or_default() -= i128::from(g.destination_bytes);
        proposals.push(Proposal {
            group_id: g.id.clone(),
            source_id: s.source_id.clone(),
            target_id: (*target_id).to_string(),
            source_bytes_relieved: g.source_bytes,
            destination_bytes_required: g.destination_bytes,
            transfer_bytes: g.transfer_bytes,
            reason: "bounded_capacity_budget_allocation",
            requires_consent: true,
            executable: false,
        });
    }
    let selected: BTreeSet<&str> = chosen.iter().map(|(g, _)| *g).collect();
    for g in &candidates {
        if !selected.contains(g.id.as_str()) {
            rejected.push(Rejection {
                group_id: g.id.clone(),
                reason: "not_selected_by_bounded_allocator",
            });
        }
    }
    rejected.sort_by(|a, b| a.group_id.cmp(&b.group_id));

    let to_u64 = |x: u128| u64::try_from(x).map_err(|_| "result exceeds u64".to_string());
    let shortfall = best_score.shortfall;
    let mut free_out = BTreeMap::new();
    for (id, v) in projected {
        free_out.insert(
            id,
            u64::try_from(v).map_err(|_| "projected free space left u64 range".to_string())?,
        );
    }
    Ok(PlanV2 {
        mode: "simulation",
        algorithm: "bounded_portfolio_search_v2",
        optimality_claim: complete,
        optimality_scope: "Static supplied estimates; lexicographic shortfall, transfer, group count, heat, IDs only.",
        shortfall_optimal: complete || shortfall == 0,
        search: SearchReport {
            complete,
            nodes_visited: visited,
            node_budget,
            reason: if too_large {
                "problem_size_limit"
            } else if cutoff {
                "node_budget"
            } else {
                "exhausted"
            },
            eligible_groups: candidates.len(),
            eligible_targets: targets.len(),
            lower_bound_shortfall_bytes: to_u64(if complete { shortfall } else { lower_bound })?,
        },
        proposals,
        rejected,
        projected_free_bytes: free_out,
        target_free_bytes: s.target_free_bytes,
        shortfall_bytes: to_u64(shortfall)?,
        satisfied: shortfall == 0,
        transfer_bytes: to_u64(best_score.transfer)?,
        filesystem_changed: false,
        baseline_shortfall_bytes: baseline.shortfall_bytes,
        shortfall_improvement_bytes: i64::try_from(
            i128::from(baseline.shortfall_bytes) - shortfall as i128,
        )
        .map_err(|_| "improvement exceeds i64".to_string())?,
        assumption: baseline.assumption,
    })
}

/// Python-compatible intake. Both v1 and v2 Python treat a missing `groups` key as empty
/// (`.get('groups', [])`) and raise on a non-boolean flag only where it
/// evaluates one: every volume's `online`/`writable`, and the pinned/active/protected flags of
/// source-volume groups up to the first true one. Flags Python never evaluates are dropped so
/// the typed `Scenario` does not reject what Python accepts.
pub fn scenario_from_value(v: &Value) -> Result<Scenario, String> {
    let mut v = v.clone();
    let obj = v.as_object_mut().ok_or("Scenario must be an object")?;
    obj.entry("groups").or_insert_with(|| Value::Array(vec![]));
    let source = obj
        .get("source_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    for vol in obj
        .get("volumes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
    {
        for k in ["online", "writable"] {
            if vol.get(k).is_some_and(Value::is_null) {
                return Err(format!("{k} must be boolean"));
            }
        }
    }
    for g in obj
        .get_mut("groups")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object_mut)
    {
        let mut evaluating = g.get("volume_id").and_then(Value::as_str) == source.as_deref();
        for k in ["pinned", "active", "protected"] {
            if !evaluating {
                g.remove(k);
                continue;
            }
            match g.get(k) {
                Some(Value::Bool(false)) => {}
                None | Some(Value::Bool(true)) => evaluating = false,
                Some(_) => return Err(format!("{k} must be boolean")),
            }
        }
    }
    serde_json::from_value(v).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::plan as plan_v1;
    use serde_json::json;
    use std::collections::BTreeMap;

    const PARITY: &str = include_str!("../../../fixtures/v2/planner-v2-parity.json");

    fn cases() -> Vec<Value> {
        serde_json::from_str(PARITY).unwrap()
    }
    fn run(case: &Value) -> Result<PlanV2, String> {
        let s = scenario_from_value(&case["input"])?;
        plan_v2(&s, case["node_budget"].as_u64().ok_or("node_budget")?)
    }

    /// Exhaustive tiny-case oracle (port of `experiments/reference_smoke.oracle_shortfall`).
    fn oracle_shortfall(s: &Scenario) -> u64 {
        let source = s.volumes.iter().find(|v| v.id == s.source_id).unwrap();
        let open = |v: &Volume| v.online == Some(true) && v.writable == Some(true);
        let targets: Vec<&Volume> = s
            .volumes
            .iter()
            .filter(|v| v.id != source.id && open(v) && v.tier >= source.tier)
            .collect();
        let groups: Vec<&Group> = s
            .groups
            .iter()
            .filter(|g| {
                g.volume_id == source.id
                    && g.pinned == Some(false)
                    && g.active == Some(false)
                    && g.protected == Some(false)
                    && g.heat.is_some_and(|h| h <= 0.25)
                    && g.days_since_move.is_some_and(|d| d >= s.cooldown_days)
            })
            .collect();
        let base = s.target_free_bytes.saturating_sub(source.free_bytes);
        let mut best = base;
        if !open(source) {
            return best;
        }
        let radix = targets.len() as u64 + 1;
        for mut code in 0..radix.pow(groups.len() as u32) {
            let mut used = vec![0u64; targets.len()];
            let (mut cost, mut relief) = (0u64, 0u64);
            for g in &groups {
                let c = (code % radix) as usize;
                code /= radix;
                if c > 0 {
                    used[c - 1] += g.destination_bytes;
                    cost += g.transfer_bytes;
                    relief += g.source_bytes;
                }
            }
            if cost <= s.max_transfer_bytes
                && targets
                    .iter()
                    .zip(&used)
                    // Unused targets are exempt: one that starts below its reserve is simply
                    // not receiving anything (the Python oracle checks every target).
                    .all(|(v, u)| *u == 0 || v.free_bytes >= v.reserve_bytes + u)
            {
                best = best.min(base.saturating_sub(relief));
            }
        }
        best
    }

    /// Independent re-check of every hard constraint on a produced plan.
    fn assert_constraints(s: &Scenario, p: &PlanV2, what: &str) {
        let src = s.volumes.iter().find(|v| v.id == s.source_id).unwrap();
        let mut free: BTreeMap<&str, i128> = s
            .volumes
            .iter()
            .map(|v| (v.id.as_str(), i128::from(v.free_bytes)))
            .collect();
        let (mut cost, mut seen) = (0u64, BTreeSet::new());
        for pr in &p.proposals {
            let g = s.groups.iter().find(|g| g.id == pr.group_id).unwrap();
            let t = s.volumes.iter().find(|v| v.id == pr.target_id).unwrap();
            assert!(seen.insert(&pr.group_id), "{what}: group twice");
            assert_eq!(g.volume_id, s.source_id, "{what}");
            assert!(
                g.pinned == Some(false) && g.active == Some(false) && g.protected == Some(false),
                "{what}: pinned/active/protected group proposed"
            );
            assert!(
                g.heat.is_some_and(|h| h <= 0.25)
                    && g.days_since_move.is_some_and(|d| d >= s.cooldown_days),
                "{what}: not cold/cooled"
            );
            assert!(t.online == Some(true) && t.writable == Some(true), "{what}");
            assert!(t.id != s.source_id && t.tier >= src.tier, "{what}");
            assert!(pr.requires_consent && !pr.executable, "{what}");
            cost += g.transfer_bytes;
            *free.get_mut(pr.target_id.as_str()).unwrap() -= i128::from(g.destination_bytes);
            *free.get_mut(s.source_id.as_str()).unwrap() += i128::from(g.source_bytes);
        }
        assert!(src.online == Some(true) || p.proposals.is_empty(), "{what}");
        assert!(cost <= s.max_transfer_bytes, "{what}: budget");
        assert_eq!(cost, p.transfer_bytes, "{what}");
        for v in &s.volumes {
            // A target that starts below its reserve may stay so, but must receive nothing.
            if p.proposals.iter().any(|pr| pr.target_id == v.id) {
                assert!(
                    free[v.id.as_str()] >= i128::from(v.reserve_bytes),
                    "{what}: reserve breached on {}",
                    v.id
                );
            }
            assert_eq!(
                free[v.id.as_str()],
                i128::from(p.projected_free_bytes[&v.id])
            );
        }
        assert!(!p.filesystem_changed, "{what}");
    }

    #[test]
    fn parity_with_python_field_by_field() {
        let (mut ok, mut rejected) = (0, 0);
        for case in cases() {
            let name = case["name"].as_str().unwrap();
            match case.get("output") {
                Some(expected) => {
                    let actual = serde_json::to_value(run(&case).unwrap_or_else(|e| {
                        panic!("{name}: Rust rejected a scenario Python accepts: {e}")
                    }))
                    .unwrap();
                    let (a, e) = (actual.as_object().unwrap(), expected.as_object().unwrap());
                    assert_eq!(a.keys().collect::<Vec<_>>(), e.keys().collect::<Vec<_>>());
                    for (field, want) in e {
                        assert_eq!(&a[field], want, "{name}: field {field}");
                    }
                    ok += 1;
                }
                None => {
                    assert!(run(&case).is_err(), "{name}: Python rejects, Rust accepted");
                    rejected += 1;
                }
            }
        }
        assert!(
            ok >= 100 && rejected >= 10,
            "fixture shrank: {ok}/{rejected}"
        );
    }

    #[test]
    fn original_51_cases_match_the_exhaustive_oracle() {
        let (mut n, mut missed, mut worse, mut incomplete) = (0, 0, 0, 0);
        for case in cases().iter().filter(|c| c["suite"] == "experiment-51") {
            let s = scenario_from_value(&case["input"]).unwrap();
            let p = run(case).unwrap();
            let best = oracle_shortfall(&s);
            assert_constraints(&s, &p, case["name"].as_str().unwrap());
            missed += usize::from(best == 0 && p.shortfall_bytes > 0);
            worse += usize::from(p.shortfall_bytes > best);
            incomplete += usize::from(!p.search.complete);
            assert!(p.shortfall_bytes >= best, "beat the oracle: oracle bug");
            n += 1;
        }
        assert_eq!((n, missed, worse, incomplete), (51, 0, 0, 0));
    }

    #[test]
    fn explicit_counterexample_is_fixed_and_v1_stays_wrong() {
        let case = &cases()[0];
        assert_eq!(case["name"], "experiment-00-explicit-counterexample");
        let s = scenario_from_value(&case["input"]).unwrap();
        assert_eq!(plan_v1(&s).unwrap().shortfall_bytes, 10);
        let p = run(case).unwrap();
        assert_eq!(p.shortfall_bytes, 0);
        assert_eq!(p.proposals[0].group_id, "large");
    }

    #[test]
    fn no_constraint_violation_on_any_fixture_case() {
        for case in cases().iter().filter(|c| c.get("output").is_some()) {
            let s = scenario_from_value(&case["input"]).unwrap();
            assert_constraints(&s, &run(case).unwrap(), case["name"].as_str().unwrap());
        }
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        fn range(&mut self, lo: u64, hi: u64) -> u64 {
            lo + self.next() % (hi - lo + 1)
        }
        fn chance(&mut self, pct: u64) -> bool {
            self.range(1, 100) <= pct
        }
    }

    fn random_scenario(r: &mut Rng) -> Value {
        let heats = [
            json!(0),
            json!(0.01),
            json!(0.1),
            json!(0.2),
            json!(0.25),
            json!(0.4),
            Value::Null,
        ];
        let mut volumes = vec![
            json!({"id": "fast", "capacity_bytes": 1000, "free_bytes": r.range(0, 60),
            "reserve_bytes": r.range(0, 10), "tier": 0, "online": true, "writable": r.chance(95)}),
        ];
        for t in 0..r.range(1, 3) {
            volumes.push(json!({"id": format!("t{t}"), "capacity_bytes": 1000,
                "free_bytes": r.range(0, 160), "reserve_bytes": r.range(0, 40), "tier": r.range(0, 2),
                "online": r.chance(90), "writable": r.chance(90)}));
        }
        let groups: Vec<Value> = (0..r.range(0, 7))
            .map(|j| {
                json!({"id": format!("g{j}"), "volume_id": "fast", "source_bytes": r.range(1, 60),
                    "destination_bytes": r.range(1, 60), "transfer_bytes": r.range(1, 60),
                    "heat": heats[r.range(0, 6) as usize], "pinned": r.chance(8), "active": r.chance(8),
                    "protected": r.chance(8),
                    "days_since_move": if r.chance(90) { json!(r.range(0, 30)) } else { Value::Null }})
            })
            .collect();
        json!({"source_id": "fast", "target_free_bytes": r.range(0, 220), "max_transfer_bytes": r.range(0, 200),
            "cooldown_days": r.range(0, 14), "volumes": volumes, "groups": groups})
    }

    #[test]
    fn seeded_random_scenarios_hold_constraints_and_never_lose_to_v1() {
        let mut r = Rng(20_261_009);
        let (mut complete, mut improved) = (0, 0);
        for i in 0..400 {
            let raw = random_scenario(&mut r);
            let Ok(s) = scenario_from_value(&raw) else {
                continue; // generator can break the disjoint-allocation bound; v1 rejects those
            };
            let Ok(p) = plan_v2(&s, DEFAULT_NODE_BUDGET) else {
                assert!(
                    plan_v1(&s).is_err(),
                    "case {i}: v2 rejected what v1 accepts"
                );
                continue;
            };
            let what = format!("random case {i}");
            assert_constraints(&s, &p, &what);
            let v1 = plan_v1(&s).unwrap();
            assert!(
                p.shortfall_bytes <= v1.shortfall_bytes,
                "{what}: worse than v1"
            );
            if p.search.complete {
                complete += 1;
                assert_eq!(
                    p.shortfall_bytes,
                    oracle_shortfall(&s),
                    "{what}: not oracle-optimal"
                );
            }
            improved += usize::from(p.shortfall_bytes < v1.shortfall_bytes);
        }
        assert!(complete >= 300, "too few complete searches: {complete}");
        assert!(improved >= 1, "generator never exercised an improvement");
    }

    #[test]
    fn cutoff_never_claims_completeness() {
        let case = cases()
            .into_iter()
            .find(|c| c["name"] == "edge-cutoff-node-budget")
            .unwrap();
        let p = run(&case).unwrap();
        assert!(!p.search.complete && !p.optimality_claim);
        assert_eq!(p.search.reason, "node_budget");
        assert_eq!(p.search.nodes_visited, p.search.node_budget);
    }

    #[test]
    fn node_budget_bounds_are_enforced() {
        let case = &cases()[0];
        let s = scenario_from_value(&case["input"]).unwrap();
        assert!(plan_v2(&s, 200_001).is_err());
        assert!(plan_v2(&s, 200_000).is_ok());
        let p = plan_v2(&s, 0).unwrap();
        assert!(!p.search.complete && p.search.nodes_visited == 0);
    }

    #[test]
    fn heat_decimals_are_exact() {
        assert_eq!(decimal(0.1), (1, 1));
        assert_eq!(decimal(0.25), (25, 2));
        assert_eq!(decimal(0.01), (1, 2));
        assert_eq!(decimal(0.00001), (1, 5));
        assert_eq!(decimal(0.0), (0, 0));
        assert_eq!(decimal(-0.0), (0, 0));
        assert_eq!(decimal(0.3), (3, 1)); // not 0.1 + 0.2's 0.30000000000000004
    }

    #[test]
    fn missing_groups_key_is_empty_like_v1() {
        let mut raw = cases()[0]["input"].clone();
        raw.as_object_mut().unwrap().remove("groups");
        let p = plan_v2(&scenario_from_value(&raw).unwrap(), 100).unwrap();
        assert!(p.proposals.is_empty() && p.search.complete);
    }

    #[test]
    fn missing_groups_fixture_records_no_discrepancy() {
        let missing = cases()
            .into_iter()
            .find(|c| c["name"] == "edge-missing-groups")
            .expect("edge-missing-groups fixture case");
        assert!(
            missing.get("python_discrepancy").is_none(),
            "edge-missing-groups still carries a python_discrepancy note: {:?}",
            missing.get("python_discrepancy")
        );
        let empty = cases()
            .into_iter()
            .find(|c| c["name"] == "edge-empty-groups")
            .expect("edge-empty-groups fixture case");
        assert_eq!(
            missing.get("output"),
            empty.get("output"),
            "edge-missing-groups output should equal the empty-group plan"
        );
    }
}
