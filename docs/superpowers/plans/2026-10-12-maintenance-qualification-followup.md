# Maintenance qualification follow-up, 12 October 2026

Status: implementation and native qualification pending. This is the continuation PR for residual work after the safe partial maintenance merges, not a completed gate or permission to enable effects. Keep the PR draft until its applicable checkboxes are proved or split into explicitly linked implementation PRs. No owner decisions are changed.

## Starting evidence

The supplied ZIP exactly matched main `979380dd4d37162e8bbb315f6b24e852e3107a7d`, tree `12819f412e660adf366077dd6401053dae57fa17`. Its Linux reference baseline passed 227 Python tests, nine historical JavaScript boundary assertions and 80 cross-language fixtures. Rust tooling was unavailable locally; native execution used the repository's existing Actions jobs. The legacy UI and its checks were later retired by another session in #424, so the old JavaScript counts are provenance, not current-main qualification.

This follow-up branches from `dda691322f8e6ceafa021e18a017934ad47105dc`, the #430 merge. Concurrent merges must be reconciled before publication, testing and integration. The companion JSON receipt records immutable tested heads and runs, not floating branch claims.

## 1. Complete the #361 coverage contract

Merged #430 fixes the persisted-dirty aggregate witness. It does not prove every interleaving between separate service reads.

- [ ] Add a deterministic public-service case that reaches the actual `RepairRequired` path and returns null totals for a completed root; require coverage other than complete.
- [ ] Add a controlled writer transition between the initial root projection and later catalogue inspection. Use a test-only rendezvous with bounded release, not sleeps or a production pause endpoint.
- [ ] Where necessary, derive coverage from both the fresh root witness and the returned totals state. Do not replace unknown totals with zero, hide a reader error, or fabricate a completed scan.
- [ ] Retain known-empty complete roots, repaired numeric-but-incomplete totals, active scans, paused/revoked/forgotten roots and restored completeness as separate cases.
- [ ] Run full Linux/Windows workspace tests and warnings-as-errors clippy at the repaired head, then obtain fresh review before closing #361.

Why the extra case matters: the first attempted regression assumed dirty totals stayed null. `Reader::repair_committed` instead repairs inactive roots via `rollup(clean=false)`. The corrected pre-fix test established `totals.complete=false` and then failed because coverage was still complete. Do not revert that valid expectation or treat the initial mistaken assertion as proof of the null-totals path.

## 2. Finish the bounded #398 follow-ups

PR #428 addresses empty prefixes, Unicode preservation, the UTF-16 cutoff and verbatim component identity. Its leading-space review requires actual SQLite Windows-VFS identity evidence, not assumptions from a generic path document or Rust's internally normalised opens.

- [ ] Reconcile and finish #428's exact-head native identity regression and fresh review before relying on its implementation.
- [ ] Replace the unbounded rendezvous in `crates/loomward-catalog/src/query.rs::concurrent_readers_never_pin_the_state_wal` with a test helper that releases all participants when a worker fails before arrival. Preserve simultaneous read transactions and the WAL checkpoint assertion; do not serialize away the race.
- [ ] Add a failure-injection canary for a worker error/panic before the first rendezvous. It must terminate with an assertion failure under a bounded deadline, not hang until the CI timeout. Avoid raw-thread abandonment that leaves database handles alive.
- [ ] Normalise the service preflight/validate-stores and backup-source SQLite opens through the same identity-preserving helper, keeping grant comparisons in canonical form. Prove ordinary, non-ASCII, reserved/trailing-component and non-Unicode cases without opening owner data.
- [ ] Recheck the backup source identity and handle lifetime: changing a path's display form must not change the source selected or weaken readonly/open semantics.
- [ ] Prepare an upstream SQLite report from the existing plain-rusqlite reproduction, pinning SQLite version, Windows build, path form, reader count, lock/checkpoint result and minimal synthetic fixture. Distinguish preparing the report from actually submitting it; do not claim upstream acknowledgement or a fixed release without evidence.

The one-shot opens are currently a consistency follow-up, not demonstrated concurrent lock leakage. Keep that severity distinction in the issue record.

## 3. Finish #417 / LW-126 package qualification

#425's manifest hardening and discovered tests are merged. They do not prove a distributable was installed, signed, submitted to a package registry or restored.

- [ ] Generate manifests offline from an immutable release's own checksum set, reject duplicate target entries, and validate every emitted manifest against the pinned schema.
- [ ] On a clean non-elevated Windows fixture, run install, ARP identity/version/publisher assertions, signature inspection while files still exist, launch, uninstall and intentional state-retention checks. Record the exact release artifact and hash.
- [ ] Verify Scoop placeholder refusal and the generated real manifest separately. No placeholder or unqualified artifact is published merely to make the test pass.
- [ ] Keep owner signing credentials, publisher identity, Store setup and first external submission as explicit prerequisites. Missing owner setup is UNAVAILABLE, not passed.
- [ ] Preserve the distinction between Windows Authenticode, updater package signatures and release provenance. Refer to #429's contract and the separate updater implementation lane; do not silently enable an installer or network request here.

## 4. Driver receipt and backlog consistency

- [ ] After the runtime PRs settle, fetch current `docs/16-implementation-status.md` and `handoff/CHECKPOINT.json`, preserve their historical sections, and append one driver-wave receipt to both. Reconcile concurrent driver updates; never overwrite them from the original ZIP.
- [ ] Record #422 and #430 as this session's merges. Record #425 as another session's merge containing this session's generator fixes. Do not attribute the concurrent #423, #424 or #426 merges to this session.
- [ ] Resolve #422's remaining contract review threads only after the corresponding #429 changes are actually merged. The search/updater runtime gates remain closed until their own acceptance evidence exists.
- [ ] Validate all 130 backlog rows, manifest dependencies and issue-body references. LW-124 includes LW-041. Shared-search scope is owner-local until LW-067; unsigned publication is not authenticated-install eligibility.
- [ ] Preserve manual WebView2/dialog, clean-machine installer, owner-key, crash/restore and real-host qualification as unverified where they were not executed.

## Verification and integration rules

For each implementation slice: a failing regression at the old head, the minimal fix, the full applicable suite at the exact repaired head, review of the complete diff, and fresh review feedback. Linux and Windows are distinct legs. Formatting-only failures do not prove a behavioural regression. Hosted builds/self-tests do not substitute for manual native-dialog or clean-install evidence.

This PR initially contains the evidence receipt and this executable acceptance plan only. Its checklist is intentionally incomplete. It changes no production file, schema, dependency, workflow, capability, grant, or release gate. Do not close #361, #398 or #417 merely because this plan merges.
