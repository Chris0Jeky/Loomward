# Updater qualification after #445

Status: bounded version-identity repair in #447, with further #436 work tracked below. Do not merge this repair until its final-head applicable suites and fresh review pass. The other updater qualifications remain open and must be carried to a linked continuation, not inferred from this repair. No owner key or runtime capability is supplied by this plan.

## Landed boundary

#445 moved nightly concurrency behind the existing qualifying-job condition without changing its trigger, admission, permissions, source checks, signing or publication steps. Head 2779758487307fb0d039d7e41952da33307a42c7 passed all nine jobs in run 38317867708. Windows Python job 115038714881 pinned that head and base b6c0e0 and ran 246 tests with eight skips. Fresh review had no findings. Merge 1173f30d19f0e018f4d492394a688f9b51de3c04 also retains a concurrent docs-only SVG addition. The configuration tests do not prove a live multi-run queue race was executed.

## Version-identity repair

Observed RED: corrected test-only head 0975ad99f05b7f49119c94680ba8939b2de96663, run 38319947350, Windows desktop job 115045438768. Formatting and compilation passed; the malformed-running-version test failed at its intended assertion (`invalid running version "" must not become 0.0.0`). Both valid-version and fixture/unconfigured controls passed. This supersedes the earlier formatting-only failure, which was not behavioral evidence.

The repair retains the supplied version string for status and refuses planning with an explicit unavailable reason when semver parsing fails. It does not normalize an invalid identity into a valid version. The expanded tests also cover opt-in/channel changes, startup/user triggers, previous requests, status identity, genuine 0.0.0, and valid prerelease/build metadata. Current-head hosted GREEN and fresh review are pending; local Rust is unavailable.

## Remaining #436 implementation and validation

- [ ] Verify generated updater signatures against the configured public verifier before emitting update-eligible metadata. A present secret and a present public-key string are not proof they match. Use a throwaway fixture key only; test mismatched keys, missing/bad signatures and altered installer bytes. Do not publish test artifacts or credentials.
- [ ] Exercise wrong requested version, no offer, previous-nightly refusal and native Cancel through pure/injected seams. Assert no downloader/installer call and no second dialog where refused. Do not run a real installation merely to satisfy a unit test.
- [ ] Add request/preference generation checks for update responses so an in-flight old-channel check cannot republish an invalidated offer or overwrite a newer check/error. Use deterministic out-of-order and channel-switch cases; recheck the bound offer after native confirmation and before effects.
- [ ] Retain separate evidence for missing/bad/mismatched/valid signed updates, native confirmation/parenting, actual installer restart and state-coupled rollback. Owner-key setup and clean Windows installation remain explicit prerequisites; absent setup is UNAVAILABLE.

The version tests use disposable state only. They do not invoke an HTTP client, native dialog, installation, signing service or owner data. A version-identity fix is not a release or updater acceptance receipt. Driver-wide bookkeeping remains in #435; preserve its historical evidence rather than overwriting it with this checkpoint.
