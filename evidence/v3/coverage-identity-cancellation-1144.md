# Coverage final identity cancellation — #1144

Status: implementation and independent source review complete; production qualification and final-head native execution pending. The public tests-only checkpoint for [#1144](https://github.com/Chris0Jeky/Loomward/issues/1144) is [27d4160e300db39b49eea03ee7e13d41f18d6d12](https://github.com/Chris0Jeky/Loomward/commit/27d4160e300db39b49eea03ee7e13d41f18d6d12) in [#1146](https://github.com/Chris0Jeky/Loomward/pull/1146). The receipts below establish failures before the production fix; positive qualification must use the integrated implementation head.

## Production scope

Only two production files change:

- `crates/loomward-windows/src/duplicate_reader/native/tree.rs` checks the existing meter before and after each final synchronous directory `observe`. The meter already prioritizes job/source-root/destination-root cancellation over deadline expiry. An early stop marks the current and remaining directories unknown, retaining a prior directory or listing error where present. One pass over tree nodes invalidates descendant digest or `not_requested` evidence while retaining observed file sizes and earlier unknown content reasons.
- `crates/loomward-operations/src/containment.rs` requires observed ancestors during the final source metadata recheck. Unverified files lose a digest or deferred `not_requested` absence verdict; other unknown content reasons remain. Matching recheck directory errors also reach previously clean A directory observations, so an A containing only empty directories cannot hide incomplete validation. Earlier outer-directory or children errors retain their cause. Directory-observation propagation is limited to matching error entries.

The native helper builds an unchecked-directory set, updates those directories, and visits the nodes once. Its cost includes ordered-set/map lookups and ancestor-path walks, with a shortcut when the root is unchecked. It does not perform a whole-tree rescan for each unchecked directory; strict linear runtime is not claimed.

## Tests and fixture boundaries

`directory_recheck_fixture` is compiled only for Windows tests or the existing `fixtures` feature. Hooks are keyed by the exact reader root and callbacks execute outside the registry lock. The service fixture registers the stored grant path, accounting for the canonical Windows verbatim prefix used by its folder setup.

The four native `directory_recheck_` tests cover the primary and both additional cancellation flags, stops before the loop and after one/two query returns, forced deadline expiry, cancellation priority, retained file sizes, descendant uncertainty through `recheck_source`, and unchanged uncancelled identity-change detection. The two pure operations regressions cover failed root/parent/grandparent identity, unchanged file metadata, deferred absence, prior unknown reasons, and empty directories.

`coverage_owns_the_identity_query_return_until_cancellation_drains` holds the reader at **QueryReturned, after the OS identity query has returned**. It is designed to check nonterminal cancellation, a held worker, busy admission, no report, one query only, and released ownership before a subsequent successful run. It releases the gate before its assertions. It does not hold a still-pending kernel metadata syscall or prove that cancellation can abort one. The production call remains synchronous on the owning reader's stack; driver-bound time until that call returns remains a platform limitation.

The published standard-user harness runs that service test first with a minimum of one executed pass, then the four native tests with a minimum of four, rejecting setup skips. Focused commands are:

```text
cargo test --locked -p loomward-service --lib coverage_owns_the_identity_query_return_until_cancellation_drains -- --show-output --test-threads=1
cargo test --locked -p loomward-windows --all-features --lib directory_recheck_ -- --show-output --test-threads=1
cargo test -p loomward-operations source_recheck_ -- --nocapture
```

## Confirmed negative evidence

| Tests-only revision and run | Observed result |
| --- | --- |
| `b80b7f4`, [Windows push run 41955690350 / job 127010071284](https://github.com/Chris0Jeky/Loomward/actions/runs/41955690350/job/127010071284) | Real standard-user NTFS execution (`TokenElevation=0`), child exit 101. Four native tests executed: **1 passed, 3 failed**. The uncancelled identity-change control passed. Combined cancellation/deadline observed four query returns instead of one; the cancellation and deadline tests each observed four instead of zero. |
| `b80b7f4`, [Linux PR run 41955696418 / job 127010371524](https://github.com/Chris0Jeky/Loomward/actions/runs/41955696418/job/127010371524) | Operations tests: **66 passed, 1 failed**. The new descendant regression's first root/cancelled case retained `Ok([2; 32])` instead of `Err("cancelled")`. |
| `27d4160`, [Linux PR run 41956916510 / job 127014211071](https://github.com/Chris0Jeky/Loomward/actions/runs/41956916510/job/127014211071) | Formatting passed; operations tests: **66 passed, 2 failed, 0 ignored**, exit 101. The added empty-directory test received a clean `Ok(Directory)` where it required an error. The descendant digest regression also failed. The checked-out PR merge was `74861f7f54db6e3a7e11e5d6ab08765be1f53eaf`. |
| `27d4160`, [Windows PR run 41956916510 / job 127014210755](https://github.com/Chris0Jeky/Loomward/actions/runs/41956916510/job/127014210755) | Real standard-user NTFS execution (`TokenElevation=0`). The service gate executed: **0 passed, 1 failed, 0 ignored**, child exit 101. It observed three identity-query returns instead of one after cancellation. |

These failures stop at their first failing assertions. Later native loop iterations, further cancellation flags, and subsequent uncertainty assertions are not claimed as executed negative evidence. Before its query-count failure, the service test passed assertions for the held worker during read/cancellation, `cancel_requested`, busy admission, no available report at the held boundary, and terminal `Cancelled` after release. Its post-count assertions for zero workers, no retained report and a succeeding new run were not reached. No positive service/native gate result is claimed here.

Earlier attempts `edfae0c` ([run 41955412822](https://github.com/Chris0Jeky/Loomward/actions/runs/41955412822)) and the empty-test addition `d35c285` ([run 41956626868](https://github.com/Chris0Jeky/Loomward/actions/runs/41956626868)) stopped at formatting and supply no behavioral red evidence. Their formatter corrections preserve the assertions.

## Verification still required

`git diff --check` and `git diff --cached --check` pass locally. Independent source review found no remaining blocker in the combined production change. Cargo, rustc and rustfmt are unavailable in the author environment, so local Rust compilation, execution and formatting are **UNVERIFIED**.

The corrected implementation still requires the focused service/native positive gates, both complete operations regressions, and the full final-head qualification after integration. A source-reviewed synchronous ownership path and a query-return gate are not a substitute for that hosted qualification.
