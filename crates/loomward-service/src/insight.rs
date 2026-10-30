//! LW-138 (#877): `insight.folder`. Owner-only rule inference over one directory's catalogue facts:
//! evidence facts -> validated evidence packet -> labelling rules -> fact-only explanation. No teacher
//! or model call, no live filesystem, Git or EXIF read, no aggregate repair, no effect. A label is an
//! inference, never a probability, an approval or advice (invariants 3 and 4); wording is `explain`'s.
use crate::{catalog_error, db, fail, now_ns, sql_error, ts_ns, Inner, Out, ReadWork};
use loomward_catalog::NodeKey;
use loomward_engine::learning::packet_builder::{build, PacketIds};
use loomward_learn::{
    explain::explain,
    labelling::{combine, AbstainReason, Axis, Outcome},
};
use loomward_protocol::*;
use serde_json::{json, Value};

fn internal(e: impl std::fmt::Display) -> ErrorBody {
    // The message names a rule, never a path or a row; the detail goes to stderr.
    eprintln!("loomward-service: insight.folder: {e}");
    fail(ErrorCode::InternalError, "evidence packet unavailable")
}

fn votes(list: impl Iterator<Item = (&'static str, &'static str)>) -> Value {
    list.map(|(function, label)| json!({"function": function, "label": label}))
        .collect()
}

fn axis(axis: Axis, outcome: &Outcome) -> Value {
    let name = match axis {
        Axis::Meaning => "meaning",
        Axis::Lifecycle => "lifecycle",
        Axis::Recreatability => "recreatability",
    };
    let (kind, label, reason, voted) = match outcome {
        Outcome::Label { label, functions } => (
            "label",
            Some(*label),
            None,
            votes(functions.iter().map(|f| (*f, *label))),
        ),
        Outcome::Abstain(AbstainReason::NoVote) => {
            ("abstain", None, Some("no_vote"), votes(std::iter::empty()))
        }
        Outcome::Abstain(AbstainReason::Conflict(list)) => (
            "abstain",
            None,
            Some("conflict"),
            votes(list.iter().map(|v| (v.function, v.label))),
        ),
        Outcome::Abstain(AbstainReason::Coverage(_)) => {
            ("abstain", None, Some("coverage"), votes(std::iter::empty()))
        }
    };
    json!({"axis": name, "outcome": kind, "label": label, "abstain_reason": reason, "votes": voted})
}

impl Inner {
    pub(crate) fn insight_folder(
        &self,
        req: &RequestEnvelope,
        work: Option<&ReadWork>,
    ) -> Result<Out, ErrorBody> {
        let r: InsightFolderRequest = req.decode_payload()?;
        let revs = self.revisions()?;
        let row = self.root_row(&r.root_id)?;
        let (active, generation) = {
            let conn = self.db();
            (
                db::active_root_dir(&conn, row).map_err(sql_error)?,
                db::root_generation(&conn, row).map_err(sql_error)?,
            )
        };
        if active.is_none() {
            return Err(fail(ErrorCode::NotFound, "unknown root"));
        }
        let mut reader = self.read_reader(work)?;
        let key = self.node(&r.node_id, &mut reader)?;
        let NodeKey::Dir(dir) = key else {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "insight.folder needs a directory node",
            ));
        };
        // A node outside the named root reads as an unknown node: nothing says it exists elsewhere.
        if self.node_root(key)? != Some(row) {
            return Err(fail(ErrorCode::NotFound, "unknown node"));
        }
        let stale = self.freshness.stale(row);
        let mut facts = reader.evidence_facts(dir).map_err(catalog_error)?;
        facts.coverage = crate::clutter::coverage(&facts.coverage).to_owned();
        // Unresolved watcher changes since the scan: the figures describe an earlier state.
        if stale && matches!(facts.coverage.as_str(), "complete" | "partial") {
            facts.coverage = "stale".into();
        }
        let observed_secs = now_ns().div_euclid(1_000_000_000);
        let packet = build(
            &facts,
            PacketIds {
                packet_id: "insight-folder".into(),
                group_id: "insight-folder".into(),
                observed_at: observed_secs.max(0) as u64,
                synthetic: self.dataset == DatasetClass::Synthetic,
            },
        )
        .map_err(internal)?;
        let axes: Vec<_> = combine(&packet)
            .map_err(internal)?
            .iter()
            .map(|(a, o)| axis(*a, o))
            .collect();
        let lines: Vec<_> = explain(&packet)
            .map_err(internal)?
            .into_iter()
            .map(|l| json!({"text": l.text, "trace": l.trace}))
            .collect();
        let result = json!({
            "root_id": r.root_id,
            "node_id": r.node_id,
            "generation": generation.map(|g| g.to_string()),
            "observed_at": ts_ns(observed_secs.saturating_mul(1_000_000_000)),
            "coverage": facts.coverage,
            "partial": facts.coverage != "complete",
            "axes": axes,
            "lines": lines,
        });
        Ok(Out {
            result,
            catalog_rev: Some(revs.0),
            state_rev: Some(revs.1),
        })
    }
}
