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


## Qualification and landing

Exact candidate `ad725931b2db276416302b5376b1fd56f98d45c1`, tree
`fa59a3cf5ab194f1ca93298c2384d1caddb10f24`, passed all nine jobs in
[run 41389070502](https://github.com/Chris0Jeky/Loomward/actions/runs/41389070502). This includes
both app jobs, the complete mock and live HTTP/browser journeys, all four Python matrix jobs,
Linux native, standard-user Windows native and desktop. The app suite passes 1,061 tests across
75 files; type/Svelte checks report zero errors and warnings, and the production build passes.
The Windows service suite reports 684 passed and two ignored, with successful standard-user
cleanup and Clippy. Existing platform-specific and ignored cases remain recorded in the logs.

The first hosted follow-up attempt failed because the coverage error appeared without its Retry
result button ([run 41387093488, job 125090246259](https://github.com/Chris0Jeky/Loomward/actions/runs/41387093488/job/125090246259)).
The getter short-circuited on a plain private field before observing reactive state. A regression
using the actual Svelte client compiler and effect runtime reproduced a true direct getter with a
stale false rendered observer. Making `fetched` reactive corrected the observer; the unchanged
browser assertion then passed on the corrected head. The failed run is retained.

After the concurrent reserve merge, #1127 was retargeted to main without changing its source.
Independent review confirmed the prospective merge tree matched the qualified candidate. The
actual [#1127 merge](https://github.com/Chris0Jeky/Loomward/pull/1127) at 05:07:11 UTC on
2 November 2026 is `ad58c9036931b981c9c47a99fb54dccd50377773`, with that identical tree.
The separate PR-triggered run was 8/9 with no failure at merge check; the completed push matrix
is the qualification record.

Issues #1073, #1081 and #1110 are closed. The app-retry checkbox in #1108 is completed; its
cancellation, registry/native test gaps, worker accounting and progress items remain open, with
[source-grounded next steps](https://github.com/Chris0Jeky/Loomward/issues/1108#issuecomment-6569289567).
Physical-device, WebView2, assistive-technology and owner-only effect acceptance remain separate.
