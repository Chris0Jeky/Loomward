//! Constrained greedy simulation. Estimates are not measured reclaimable space.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
const MAX_SAFE: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Deserialize)]
pub struct Volume {
    pub id: String,
    pub capacity_bytes: u64,
    pub free_bytes: u64,
    pub reserve_bytes: u64,
    pub tier: u8,
    pub online: Option<bool>,
    pub writable: Option<bool>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Group {
    pub id: String,
    pub volume_id: String,
    pub source_bytes: u64,
    pub destination_bytes: u64,
    pub transfer_bytes: u64,
    pub heat: Option<f64>,
    pub pinned: Option<bool>,
    pub active: Option<bool>,
    pub protected: Option<bool>,
    pub days_since_move: Option<u64>,
}
fn default_cooldown() -> u64 {7}
#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    pub source_id: String,
    pub target_free_bytes: u64,
    pub max_transfer_bytes: u64,
    #[serde(default="default_cooldown")]
    pub cooldown_days: u64,
    pub volumes: Vec<Volume>,
    pub groups: Vec<Group>,
}
#[derive(Debug, Serialize)]
pub struct Proposal {
    pub group_id: String, pub source_id: String, pub target_id: String,
    pub source_bytes_relieved: u64, pub destination_bytes_required: u64, pub transfer_bytes: u64,
    pub reason: &'static str, pub requires_consent: bool, pub executable: bool,
}
#[derive(Debug, Serialize)]
pub struct Rejection {pub group_id: String, pub reason: &'static str}
#[derive(Debug, Serialize)]
pub struct Plan {
    pub mode: &'static str, pub algorithm: &'static str, pub optimality_claim: bool,
    pub proposals: Vec<Proposal>, pub rejected: Vec<Rejection>,
    pub projected_free_bytes: BTreeMap<String,u64>, pub target_free_bytes: u64,
    pub shortfall_bytes: u64, pub satisfied: bool, pub transfer_bytes: u64,
    pub filesystem_changed: bool, pub assumption: &'static str,
}
fn valid_id(s:&str)->bool {!s.is_empty()&&s.chars().count()<=128}
fn reject(id:&str,reason:&'static str)->Rejection {Rejection{group_id:id.into(),reason}}

pub fn plan(s:&Scenario)->Result<Plan,String> {
    if !(1..=32).contains(&s.volumes.len())||s.groups.len()>10_000||s.cooldown_days>36500||s.max_transfer_bytes>MAX_SAFE {
        return Err("invalid scenario bounds".into());
    }
    let mut volumes=BTreeMap::new();
    for v in &s.volumes {
        if !valid_id(&v.id)||v.capacity_bytes==0||v.capacity_bytes>MAX_SAFE||v.free_bytes>v.capacity_bytes||v.reserve_bytes>v.capacity_bytes||v.tier>9||volumes.insert(v.id.clone(),v).is_some(){return Err("invalid or duplicate volume".into());}
    }
    let source=*volumes.get(&s.source_id).ok_or("unknown source")?;
    if s.target_free_bytes>source.capacity_bytes{return Err("goal exceeds capacity".into());}
    let mut ids=BTreeSet::new();let mut accounted:BTreeMap<String,u64>=BTreeMap::new();
    let mut candidates=Vec::new();let mut rejected=Vec::new();
    for g in &s.groups {
        if !valid_id(&g.id)||!ids.insert(&g.id)||!volumes.contains_key(&g.volume_id)||[g.source_bytes,g.destination_bytes,g.transfer_bytes].iter().any(|x|*x==0||*x>MAX_SAFE) {
            return Err("invalid or duplicate group".into());
        }
        let n=accounted.entry(g.volume_id.clone()).or_insert(0);
        *n=n.checked_add(g.source_bytes).ok_or("allocation overflow")?;
        if g.heat.is_some_and(|h|!h.is_finite()||!(0.0..=1.0).contains(&h))||g.days_since_move.is_some_and(|d|d>36500){return Err("invalid heat or history".into());}
        let reason=if g.volume_id!=s.source_id {Some("not_on_source")}
            else if g.pinned!=Some(false)||g.active!=Some(false)||g.protected!=Some(false){Some("pinned_active_protected_or_unspecified")}
            else if g.heat.is_none(){Some("heat_unknown")}
            else if g.heat.unwrap_or(1.0)>0.25{Some("not_cold")}
            else if g.days_since_move.is_none()||g.days_since_move.unwrap_or(0)<s.cooldown_days{Some("cooldown_or_history_unknown")}
            else {None};
        if let Some(r)=reason{rejected.push(reject(&g.id,r));}else{candidates.push(g);}
    }
    for (id,n) in accounted {let v=volumes[&id];if n>v.capacity_bytes-v.free_bytes{return Err("disjoint source allocations exceed used capacity".into());}}
    if source.online!=Some(true)||source.writable!=Some(true){
        for g in &candidates{rejected.push(reject(&g.id,"source_offline_or_readonly"));}candidates.clear();
    }
    candidates.sort_by(|a,b|a.heat.unwrap_or(1.0).total_cmp(&b.heat.unwrap_or(1.0)).then_with(||b.source_bytes.cmp(&a.source_bytes)).then_with(||a.id.cmp(&b.id)));
    let mut free:BTreeMap<String,u64>=volumes.iter().map(|(id,v)|(id.clone(),v.free_bytes)).collect();
    let mut proposals=Vec::new();let mut transferred=0;
    for g in candidates {
        if free[&s.source_id]>=s.target_free_bytes{break;}
        if g.transfer_bytes>s.max_transfer_bytes-transferred{rejected.push(reject(&g.id,"transfer_budget"));continue;}
        let mut targets:Vec<&Volume>=s.volumes.iter().filter(|v|v.id!=s.source_id&&v.online==Some(true)&&v.writable==Some(true)&&v.tier>=source.tier&&free[&v.id].checked_sub(g.destination_bytes).is_some_and(|n|n>=v.reserve_bytes)).collect();
        targets.sort_by(|a,b|b.tier.cmp(&a.tier).then_with(||free[&b.id].cmp(&free[&a.id])).then_with(||a.id.cmp(&b.id)));
        let Some(target)=targets.first() else{rejected.push(reject(&g.id,"no_eligible_destination_with_reserve"));continue;};
        *free.get_mut(&s.source_id).ok_or("source disappeared")?+=g.source_bytes;
        *free.get_mut(&target.id).ok_or("target disappeared")?-=g.destination_bytes;
        transferred+=g.transfer_bytes;
        proposals.push(Proposal{group_id:g.id.clone(),source_id:s.source_id.clone(),target_id:target.id.clone(),source_bytes_relieved:g.source_bytes,destination_bytes_required:g.destination_bytes,transfer_bytes:g.transfer_bytes,reason:"cold_disjoint_group_within_capacity_and_budget",requires_consent:true,executable:false});
    }
    let shortfall=s.target_free_bytes.saturating_sub(free[&s.source_id]);
    Ok(Plan{mode:"simulation",algorithm:"constrained_greedy_v1",optimality_claim:false,proposals,rejected,projected_free_bytes:free,target_free_bytes:s.target_free_bytes,shortfall_bytes:shortfall,satisfied:shortfall==0,transfer_bytes:transferred,filesystem_changed:false,assumption:"Group allocations are user-supplied estimates; source, destination and transfer sizes are independent."})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scenario()->Scenario{serde_json::from_str(include_str!("../../../fixtures/tier-scenario.json")).unwrap()}
    #[test]
    fn matches_python_golden_scenario(){
        let actual=serde_json::to_value(plan(&scenario()).unwrap()).unwrap();
        let expected:serde_json::Value=serde_json::from_str(include_str!("../../../fixtures/tier-plan-golden.json")).unwrap();
        assert_eq!(actual,expected);
    }
    #[test]
    fn all_offline_gives_shortfall(){let mut s=scenario();for v in &mut s.volumes{v.online=Some(false);}let p=plan(&s).unwrap();assert!(p.proposals.is_empty());assert!(!p.satisfied);}
    #[test]
    fn duplicate_group_is_error(){let mut s=scenario();s.groups.push(s.groups[0].clone());assert!(plan(&s).is_err());}
    #[test]
    fn unknown_heat_is_not_cold(){let mut s=scenario();for g in &mut s.groups{g.heat=None;}assert!(plan(&s).unwrap().proposals.is_empty());}
    #[test]
    fn transfer_budget_is_not_source_allocation(){let mut s=scenario();s.max_transfer_bytes=1;let p=plan(&s).unwrap();assert_eq!(p.transfer_bytes,0);assert!(!p.satisfied);}
    #[test]
    fn destination_reserve_never_breached(){let s=scenario();let p=plan(&s).unwrap();for v in &s.volumes{if v.id!=s.source_id{assert!(p.projected_free_bytes[&v.id]>=v.reserve_bytes);}}}
    #[test]
    fn unknown_permission_fails_closed(){let mut s=scenario();for g in &mut s.groups{g.protected=None;}assert!(plan(&s).unwrap().proposals.is_empty());}
}
