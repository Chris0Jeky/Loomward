//! A pure state-transition model for future crash-recovery tests. NO I/O executor.
//! Advancing this model does not approve, copy, verify, publish or delete a file.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage { Draft, Approved, Prepared, Copying, Verified, Published, SourceRetained, Committed, NeedsAttention }

#[derive(Debug, Serialize)]
pub struct Simulation {
    stage: Stage,
    transitions: Vec<Stage>,
}
impl Default for Simulation {
    fn default() -> Self { Self { stage: Stage::Draft, transitions: vec![Stage::Draft] } }
}
impl Simulation {
    pub fn stage(&self) -> Stage { self.stage }
    pub fn advance(&mut self, next: Stage) -> Result<(), &'static str> {
        use Stage::*;
        if self.transitions.len() >= 16 { return Err("transition_budget"); }
        let ordered = matches!((self.stage, next), (Draft,Approved)|(Approved,Prepared)|(Prepared,Copying)|
            (Copying,Verified)|(Verified,Published)|(Published,SourceRetained)|(SourceRetained,Committed));
        let failure = next == NeedsAttention && !matches!(self.stage, Committed|NeedsAttention);
        if !ordered && !failure { return Err("invalid_transition"); }
        self.stage = next; self.transitions.push(next); Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cannot_skip_verification() {
        let mut x=Simulation::default();
        for s in [Stage::Approved,Stage::Prepared,Stage::Copying] { x.advance(s).unwrap(); }
        assert!(x.advance(Stage::Published).is_err());
        assert_eq!(x.stage(),Stage::Copying);
    }
    #[test]
    fn source_retention_is_a_distinct_stage() {
        let mut x=Simulation::default();
        for s in [Stage::Approved,Stage::Prepared,Stage::Copying,Stage::Verified,Stage::Published] {x.advance(s).unwrap();}
        assert!(x.advance(Stage::Committed).is_err());
        x.advance(Stage::SourceRetained).unwrap();x.advance(Stage::Committed).unwrap();
        assert!(x.advance(Stage::Draft).is_err());
    }
    #[test]
    fn failure_is_not_success_or_automatic_retry() {
        let mut x=Simulation::default();x.advance(Stage::NeedsAttention).unwrap();
        assert!(x.advance(Stage::Committed).is_err());assert!(x.advance(Stage::Approved).is_err());
    }
}
