# Parked PR recovery implementation plan

> **For agentic workers:** use the repository's independent implementation and review workflow. GitHub is authoritative for source refs and hosted checks.

**Goal:** finish the reserve, Learning deferral and collection curation changes already proposed in PRs #1109, #1113 and #1118, and repair Explorer search refresh (#1116).

**Architecture:** reconcile all three feature branches with `6ac587c1564decf342ca036a0b12bd5270e2ab65`, preserve their source commits as integration parents, and validate one combined tree. Shared contracts and exhaustive fixtures must include both additive commands.

**Tech stack:** Rust service/protocol, Svelte 5/TypeScript, Python Playwright, GitHub Actions.

**Specs:** issues #1111, #1114, #1119 and #1116; `contracts/v3/semantics.md`; `docs/06-learning.md`; `docs/41-v03-architecture.md`.

## Constraints

- Preserve owner-local scope, revision checks, durable identities, replay semantics and the existing personal-file effect gates.
- Use synthetic fixtures only. A session-sealed wire ID is not a durable grant identifier.
- Preserve the existing race, read-only label-view, original-file and grant assertions. A failing test is never removed to obtain a passing gate.
- Keep #1110 cross-client refresh and the unpublished #1121/#1124 work separate.

## Review focus

- Both new commands must appear in the registry, envelope allowlist and exhaustive service fixtures; the combined registry has 85 commands.
- A committed or uncertain reserve write must not leave old scenarios presented as current if the following refresh fails.
- Browser proof must follow UI-created scenarios and select the heat-policy alternative it asserts.
- Collection removal must preserve other memberships and grants across restart; late reads and uncertain writes must remain fenced.
- Explorer search must depend on root validation while folder navigation retains its own Atlas error.

## Tasks and evidence

- [x] Reconcile the uploaded ZIP against all 1,641 blobs of current main and inspect open PRs, issues, reviews and prior failing logs.
- [x] Restore `LearningSetDeferred` in `crates/loomward-service/src/tests.rs` and `gap_tests.rs`; reconcile both additive commands in shared contracts and regenerate TypeScript types.
- [x] Repair `scripts/test_app_reserve.py` to inspect newly persisted UI-created proposals, check all heat policies and select the assumed-only alternative; retain narrow-layout checks.
- [x] Reproduce the reserve refresh failure using the actual Tiers instance functions; invalidate old plans before submission and distinguish a committed preference from a failed refresh. Test success, uncertain outcome and stale refusal followed by a failed read.
- [x] Compare complete durable grant rows across reopen in `collections_tests.rs`, retain same-session wire comparisons, and identify the Collections dropdown by its combobox role in the browser journey.
- [x] Reproduce Explorer's Atlas/search coupling and failed root-validation behavior; test the actual instance functions before changing `reload()`.
- [x] Independently review the feature changes and combined seams; correct actionable findings before publication.
- [x] Repair the first hosted browser failure without weakening its concurrent saved-scenario journey: separate reserve submission from tier declaration, and fence old scenario-list replies before reserve submission. Four new actual-function interleaving tests pass within the 87-test lifecycle suite.
- [x] Require the integration PR's exact-source Linux/Windows native, desktop and browser checks before merge. Both exact-head runs passed all nine jobs; final landing is recorded below and in the PR.

## Local validation and limitations

The combined Python suite ran 440 tests successfully with five platform-specific skips. The combined app suite passed 1,032 tests across 73 files before the final uncertain-outcome regression was added; the final focused lifecycle check and type check are recorded in the PR. Production build and generated-contract comparison passed. The existing large-bundle advisory remains.

Local Rust and live browser execution are unavailable. Chromium installation returned an invalid download archive. Initial Python attempts exposed sandbox socket restrictions and Git-ancestor guards on temporary directories; the successful run used synthetic loopback servers and `/var/tmp`. Those environment failures are not classified as product failures or passing tests. Hosted proof remains required, and this document does not claim physical-device, WebView2 or assistive-technology acceptance.

The first published head, `b10e6117ca4804e07d68295e43fba4bc38a4cf4c`, passed Linux native, desktop, Windows app and all four Python jobs. Ubuntu app exposed the saved-scenario loading regression in `check_tiers_loading_followups`; runs 41385000577 and 41385030679 retain that failure. The repair keeps this browser assertion and the reserve uncertain-outcome protections. Its focused lifecycle suite passes 87 tests, and app check reports zero errors and warnings. At that point the replacement head still required the complete hosted gates; its subsequent results are recorded below.


## Hosted qualification and landing

Exact integration head `d7eab8c045caa16dad8244e2526390b06a8614cc`, tree
`d3c65c0dce034c565d38ea5f7c55a54f524d691f`, passed all nine jobs in both
[PR run 41385886994](https://github.com/Chris0Jeky/Loomward/actions/runs/41385886994) and
[push run 41385882882](https://github.com/Chris0Jeky/Loomward/actions/runs/41385882882).
The hosted app suite ran 1,038 tests across 73 files and completed the mock and live HTTP/browser
journeys. Linux native, standard-user Windows native, desktop, both app jobs and all four Python
matrix jobs passed. Ignored and platform-specific cases remain explicit in their logs.

Original reserve PR #1109 merged concurrently as `280aa69b3f29577f117db20c87532fe69f769a0b`.
Its latest source was already preserved in the follow-up ancestry. The remaining integration therefore
landed through [PR #1127](https://github.com/Chris0Jeky/Loomward/pull/1127), merged at 05:07:11 UTC
on 2 November 2026 as `ad58c9036931b981c9c47a99fb54dccd50377773`. Its complete nine-job gate
passed before merge; the actual merge tree exactly equals the reviewed follow-up source
`fa59a3cf5ab194f1ca93298c2384d1caddb10f24`. GitHub recognizes #1125, #1113 and #1118 as merged
through that ancestry, without a separate main merge of each original PR.

Issues #1111, #1114, #1119 and #1116 are closed as completed; #1111 closed with the concurrent
reserve merge. Broader #31/#36 and all manual acceptance/effect gates remain separate. The first
failed runs above remain part of the evidence, and no browser assertion was weakened to clear them.
