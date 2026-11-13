use loomward_catalog::SearchPage;

pub(crate) fn pairing(timed: &SearchPage, counted: &SearchPage) -> serde_json::Value {
    let paired = timed.budget_hit == counted.budget_hit
        && timed.hits.len() == counted.hits.len()
        && timed.next_cursor.as_ref().map(|c| c.last_key)
            == counted.next_cursor.as_ref().map(|c| c.last_key);
    serde_json::json!({
        "counted_budget_hit": counted.budget_hit,
        "counted_hits": counted.hits.len(),
        "counted_has_cursor": counted.next_cursor.is_some(),
        "paired": paired,
        "unpaired_reason": if paired { None } else { Some("budget_cut_diverged") },
    })
}
