# Updater qualification after #445

Status: draft continuation for #436. Initial native tests are a regression checkpoint, not a completed repair. Keep this PR unmerged until its demonstrated failures are repaired, the applicable exact-head suites pass and a fresh review completes. No owner key or runtime capability is supplied by this plan.

## Landed boundary

#445 moved nightly concurrency behind the existing qualifying-job condition without changing its trigger, admission, permissions, source checks, signing or publication steps. Head 2779758487307fb0d039d7e41952da33307a42c7 passed all nine jobs in run 38317867708. Windows Python job 115038714881 pinned that head and base b6c0e0 and ran 246 tests with eight skips. Fresh review had no findings. Merge 1173f30d19f0e018f4d492394a688f9b51de3c04 also retains a concurrent docs-only SVG addition. The configuration tests do not prove a live multi-run queue race was executed.

## Remaining implementation and validation

- [ ] Observe the new malformed-running-version test failing at the intended assertion, not at parsing or compilation. Reject invalid version identity before planning/checking/installing; never substitute 0.0.0. Preserve valid-version, fixture and unconfigured controls. Record RED and repaired GREEN heads.
- [ ] Verify generated updater signatures against the configured public verifier before emitting update-eligible metadata. A present secret and a present public-key string are not proof they match. Use a throwaway fixture key only; test mismatched keys, missing/bad signatures and altered installer bytes. Do not publish test artifacts or credentials.
- [ ] Exercise wrong requested version, no offer, previous-nightly refusal and native Cancel through pure/injected seams. Assert no downloader/installer call and no second dialog where refused. Do not run a real installation merely to satisfy a unit test.
- [ ] Add request/preference generation checks for update responses so an in-flight old-channel check cannot republish an invalidated offer or overwrite a newer check/error. Use deterministic out-of-order and channel-switch cases; recheck the bound offer after native confirmation and before effects. The review on #432 established source ordering, not an executed exploit.
- [ ] Retain separate evidence for missing/bad/mismatched/valid signed updates, native confirmation/parenting, actual installer restart and state-coupled rollback. Owner-key setup and clean Windows installation remain explicit prerequisites; absent setup is UNAVAILABLE.
- [ ] Reconcile main and all review comments before final publication. Run the native manifest build/test/clippy and the full applicable reference/app suites at the exact final head. Keep #436 open for any unexecuted obligations or split them into explicitly linked follow-up PRs.

The initial tests use Updates::plan and disposable empty state only. They do not invoke an HTTP client, native dialog, installation, signing service or owner data. This plan is not a release or updater acceptance receipt. Driver-wide bookkeeping remains in #435; preserve its historical evidence rather than overwriting it with this checkpoint.
