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
- [ ] Require the integration PR's exact-source Linux/Windows native, desktop and browser checks before merge. Record the definitive results in that PR.

## Local validation and limitations

The combined Python suite ran 440 tests successfully with five platform-specific skips. The combined app suite passed 1,032 tests across 73 files before the final uncertain-outcome regression was added; the final focused lifecycle check and type check are recorded in the PR. Production build and generated-contract comparison passed. The existing large-bundle advisory remains.

Local Rust and live browser execution are unavailable. Chromium installation returned an invalid download archive. Initial Python attempts exposed sandbox socket restrictions and Git-ancestor guards on temporary directories; the successful run used synthetic loopback servers and `/var/tmp`. Those environment failures are not classified as product failures or passing tests. Hosted proof remains required, and this document does not claim physical-device, WebView2 or assistive-technology acceptance.

The first published head, `b10e6117ca4804e07d68295e43fba4bc38a4cf4c`, passed Linux native, desktop, Windows app and all four Python jobs. Ubuntu app exposed the saved-scenario loading regression in `check_tiers_loading_followups`; runs 41385000577 and 41385030679 retain that failure. The repair keeps this browser assertion and the reserve uncertain-outcome protections. Its focused lifecycle suite passes 87 tests, and app check reports zero errors and warnings. The replacement head still requires the complete hosted gates.
