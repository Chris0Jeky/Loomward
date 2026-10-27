//! LW-132 / ADR R1: evidence vocabulary only, never operation authority.
use crate::OpState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptLabel {
    ReadBack,
    CrashQualified,
    PowerLossQualified,
    ArchiveVerified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptState {
    Unverified,
    ReadBack,
    PublishedUnverified,
}

/// Journal read-back counts describe earlier checks, not necessarily the current published tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyReceipt {
    pub state: ReceiptState,
    pub highest_label: Option<ReceiptLabel>,
    pub items_read_back: u32,
    pub archive_verified_at: Option<String>,
    pub explanation: String,
}

impl CopyReceipt {
    pub fn from_copy(state: OpState, total: u32, checked: u32, published: bool) -> Self {
        let verified = state == OpState::Committed && total > 0 && checked == total;
        let receipt_state = if verified {
            ReceiptState::ReadBack
        } else if published {
            ReceiptState::PublishedUnverified
        } else {
            ReceiptState::Unverified
        };
        let outcome = match receipt_state {
            ReceiptState::ReadBack => "read_back: file streams passed non-cached read-back; directory structure and metadata checked",
            ReceiptState::PublishedUnverified => "published_unverified: publication observed, current contents are not verified",
            ReceiptState::Unverified => "unverified: no completed verified copy",
        };
        // G5 is NOT earned (docs/53); LW-134 and LW-136 evidence is not built. Never promote
        // today's copies to crash_qualified, power_loss_qualified or archive_verified.
        Self {
            state: receipt_state,
            highest_label: verified.then_some(ReceiptLabel::ReadBack),
            items_read_back: checked,
            archive_verified_at: None,
            explanation: format!("{outcome}. {checked} of {total} entries checked in the journal (earlier checks only if unverified). Ladder: read back; crash-qualified (copy engine, requires C6 and G5); power-loss qualified (this storage path, requires LW-134); archive verified at a date (requires a later independent scrub, LW-136). G5 not earned; not power-loss tested on this disk; archive not verified. No space was freed on the source."),
        }
    }

    /// Invalidate imported qualification without inventing replacement campaign evidence.
    pub fn downgrade_power_loss(&mut self) {
        if self.highest_label == Some(ReceiptLabel::PowerLossQualified) {
            self.highest_label =
                (self.state == ReceiptState::ReadBack).then_some(ReceiptLabel::ReadBack);
            self.explanation = "Storage path changed or unknown; power-loss qualification invalidated. not power-loss tested on this disk. Only the recorded read-back remains; no crash qualification is inferred.".into();
        }
    }
}
