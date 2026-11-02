# Interface follow-ups implementation and proof plan

## Scope

Build on the repaired parked-feature integration from PR #1125. Work in separate lanes for #1073 (Find), #1081 (Applications ordering), #1110 (cross-client Tiers refresh), and the app-retry item of #1108 (coverage results). Keep the remaining engine cancellation, worker reservation and progress work in #1108 open.

GitHub remains authoritative for branch heads, discussions and hosted results. The uploaded source was reconciled with main before these lanes began. All new browser fixtures are synthetic; no host application inventory or personal file content is used.

## Implementation

- Find: preserve the applied request and its explanation during draft edits and pagination; explain effective controls, binary units and UTC dates; reject contradictory bounds; preserve routed query values and rejected-query notices. Exercise the actual instance functions and nonempty rendered preset results.
- Applications: retain the existing corrected mock ordering and null rejection. Replace source-string wiring evidence with a 203-row routed browser journey that changes order, pages in both orders, resets the cursor in both directions and retains focus after rendering.
- Coverage: retry only the retained result of a completed job. Coalesce repeated clicks, keep expired results unavailable, fence responses from replaced jobs and retain retry availability across job-list resync. Keep the pending control visible and move focus to the result only after an explicit retry, without stealing focus after navigation.
- Tiers: consume volume-preference events, invalidate displayed observations, refresh current preferences and saved scenarios, preserve dirty reserve inputs, and fence client changes, reconnections, newer reads and unmount. Exercise two tabs against the same native synthetic service.

## Review and validation

Each lane receives source review and regression-first tests where behavior changes. The combined source must retain the four PR #1125 reserve concurrency regressions. Reconcile overlapping Tiers/list-ticket changes and the shared lifecycle test file explicitly before publishing.

Run the combined app unit suite, Svelte/type/viz check, production build and Python syntax checks. Require exact-published-head hosted app/browser, native, desktop and Python jobs before merging. Browser scripts are authored locally but are not considered executed until the corresponding hosted step succeeds.

Keep first failures and their causal fixes in the PR history. No physical-device, Windows WebView2, assistive-technology, personal-copy or model-provider acceptance is implied by synthetic CI. Record final results and remaining work in the PR and the separate driver checkpoint.
