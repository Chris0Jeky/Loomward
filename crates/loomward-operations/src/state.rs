//! Doc 47 section 5: the operation and item state machines as explicit legal-transition checks.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpState {
    Draft,
    Refused,
    Approved,
    Expired,
    Preparing,
    Copying,
    Staged,
    Blocked,
    NeedsAttention,
    Publishing,
    Committed,
    Cancelling,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IllegalTransition {
    pub from: OpState,
    pub to: OpState,
}

impl OpState {
    pub const ALL: [OpState; 13] = [
        OpState::Draft,
        OpState::Refused,
        OpState::Approved,
        OpState::Expired,
        OpState::Preparing,
        OpState::Copying,
        OpState::Staged,
        OpState::Blocked,
        OpState::NeedsAttention,
        OpState::Publishing,
        OpState::Committed,
        OpState::Cancelling,
        OpState::Cancelled,
    ];

    /// Exactly the edges of the section 5 diagram; everything else is illegal.
    pub fn can_transition(self, to: OpState) -> bool {
        use OpState::*;
        matches!(
            (self, to),
            (Draft, Refused | Approved)
                | (Approved, Expired | Preparing)
                | (Preparing, Copying | Refused | Cancelling)
                | (Copying, Staged | Blocked | NeedsAttention | Cancelling)
                | (Blocked, Copying | Cancelling)
                | (Staged, Cancelling | Publishing)
                | (
                    Publishing,
                    Cancelling | Committed | Blocked | NeedsAttention
                )
                | (NeedsAttention, Copying | Cancelling)
                | (Cancelling, Cancelled | NeedsAttention)
        )
    }

    pub fn transition(self, to: OpState) -> Result<OpState, IllegalTransition> {
        if self.can_transition(to) {
            Ok(to)
        } else {
            Err(IllegalTransition { from: self, to })
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            OpState::Refused | OpState::Expired | OpState::Committed | OpState::Cancelled
        )
    }

    pub fn name(self) -> &'static str {
        use OpState::*;
        match self {
            Draft => "draft",
            Refused => "refused",
            Approved => "approved",
            Expired => "expired",
            Preparing => "preparing",
            Copying => "copying",
            Staged => "staged",
            Blocked => "blocked",
            NeedsAttention => "needs_attention",
            Publishing => "publishing",
            Committed => "committed",
            Cancelling => "cancelling",
            Cancelled => "cancelled",
        }
    }
}

/// Per-item lifecycle: `pending -> created -> written -> metadata_set -> verified`, or `failed`.
/// Back to `pending` only for a redo after the staged object was removed by its journaled identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    Pending,
    Created,
    Written,
    MetadataSet,
    Verified,
    Failed,
}

impl ItemState {
    pub const ALL: [ItemState; 6] = [
        ItemState::Pending,
        ItemState::Created,
        ItemState::Written,
        ItemState::MetadataSet,
        ItemState::Verified,
        ItemState::Failed,
    ];

    pub fn can_transition(self, to: ItemState) -> bool {
        use ItemState::*;
        matches!(
            (self, to),
            (Pending, Created)
                | (Created, Written)
                | (Written, MetadataSet)
                | (MetadataSet, Verified)
                | (Pending | Created | Written | MetadataSet, Failed)
                | (Created | Written | MetadataSet | Failed, Pending)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every `a --> b` edge of the section 5 mermaid diagram, read from the doc itself so code and doc
    /// cannot drift apart silently.
    fn doc_edges() -> HashSet<(OpState, OpState)> {
        let doc = include_str!("../../../docs/47-copy-and-verify.md");
        let start = doc.find("stateDiagram-v2").expect("section 5 diagram");
        let block = &doc[start..start + doc[start..].find("```").unwrap()];
        let by_name = |n: &str| OpState::ALL.into_iter().find(|s| s.name() == n);
        let mut edges = HashSet::new();
        for line in block.lines() {
            let Some((a, rest)) = line.trim().split_once(" --> ") else {
                continue;
            };
            let b = rest.split(':').next().unwrap().trim();
            if a == "[*]" || b == "[*]" {
                continue;
            }
            edges.insert((
                by_name(a).unwrap_or_else(|| panic!("unknown state {a}")),
                by_name(b).unwrap_or_else(|| panic!("unknown state {b}")),
            ));
        }
        edges
    }

    #[test]
    fn every_pair_matches_the_doc_diagram() {
        let edges = doc_edges();
        assert_eq!(
            edges.len(),
            23,
            "diagram edge count changed: review the state machine"
        );
        for from in OpState::ALL {
            for to in OpState::ALL {
                let legal = edges.contains(&(from, to));
                assert_eq!(from.can_transition(to), legal, "{from:?} -> {to:?}");
                assert_eq!(from.transition(to).is_ok(), legal);
                if !legal {
                    assert_eq!(from.transition(to), Err(IllegalTransition { from, to }));
                }
            }
        }
    }

    #[test]
    fn terminal_states_have_no_exit_and_committed_is_never_left() {
        for from in OpState::ALL.into_iter().filter(|s| s.is_terminal()) {
            assert!(
                OpState::ALL.iter().all(|&to| !from.can_transition(to)),
                "{from:?}"
            );
        }
        // No path reaches committed except from publishing.
        for from in OpState::ALL {
            assert_eq!(
                from.can_transition(OpState::Committed),
                from == OpState::Publishing
            );
        }
    }

    #[test]
    fn item_states_exhaustive() {
        use ItemState::*;
        let legal = [
            (Pending, Created),
            (Created, Written),
            (Written, MetadataSet),
            (MetadataSet, Verified),
            (Pending, Failed),
            (Created, Failed),
            (Written, Failed),
            (MetadataSet, Failed),
            (Created, Pending),
            (Written, Pending),
            (MetadataSet, Pending),
            (Failed, Pending),
        ];
        for a in ItemState::ALL {
            for b in ItemState::ALL {
                assert_eq!(
                    a.can_transition(b),
                    legal.contains(&(a, b)),
                    "{a:?} -> {b:?}"
                );
            }
        }
        // Verification cannot be skipped and a verified item is final.
        assert!(!Created.can_transition(Verified) && !Written.can_transition(Verified));
        assert!(ItemState::ALL.iter().all(|&b| !Verified.can_transition(b)));
    }
}
