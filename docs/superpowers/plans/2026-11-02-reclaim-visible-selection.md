# Keep Reclaim selection aligned with the current plan

Issue #1076 records a selected nested candidate remaining counted after a new
plan hides it. On a successful current response, keep only selected IDs that
are actually listed. Explain removed choices in a status message, and discard
an old union estimate when its membership no longer matches. A failed read
does not establish that a candidate disappeared; preserve choices but disable
selection observation while the plan is unavailable.

Use the existing actual-view-function test harness to establish the failure,
then check successful pruning, unchanged visible choices and late responses.
Retain the generation fence and existing read-only observation semantics.
Correct the old observe-worker comment: raw detector candidates include nested
entries even though the displayed plan deduplicates them. Further ancestry
budget/depth and cross-group design work stays explicitly open in #1076.
