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

## Integration checks before final qualification

The first production head `26119c36c9f6c37a045b9794f7a5c5692a7b5034` stopped at one rustfmt
layout hunk in `recheck_file`; no native compilation or test success is claimed for that attempt.
The formatter-only follow-up `f630e26ad79a32205493d8f181243555c3e76e63` passed formatting and
compiled the Linux workspace, but both attempt-1 Linux jobs failed the existing architecture guard:

- [Push run 41958789362 / job 127019935998](https://github.com/Chris0Jeky/Loomward/actions/runs/41958789362/job/127019935998).
- [PR run 41958794897 / job 127020439876](https://github.com/Chris0Jeky/Loomward/actions/runs/41958794897/job/127020439876).

`content_reader_is_feature_gated_and_engine_only` rejected the new service fixture's direct
low-level content-reader reference in `covers_tests.rs`. The windows-crate result was **70 passed,
1 failed** and exit 101; Clippy was not reached. The preceding temporary-manifest panic messages
are intentional guard self-tests, not additional failing tests. Both new operations regressions and
the Reclaim intermediate-chain regression did execute successfully before this later failure.

The required repair routes the fixture through an explicit engine test-support facade, enabled only
by the service's dev-dependency. The architecture guard, strict Windows gates and behavioral
assertions remain intact. The ordinary production build does not enable the fixture facade.
A fresh complete workflow on the repaired head is still required; selected passing tests do not
qualify the failed integration.

### Corrected-head Linux and app receipt, 03:24 UTC

The reviewed fixture-facade repair is published at
`c0ab2262bcfaf5c58fea09e181ca12637a80b72a`, tree
`29cf9220d575824e1e7e17b3ccf5d0ad8492de42`. Both Linux native jobs completed successfully:
[PR 41960178507 / job 127024899340](https://github.com/Chris0Jeky/Loomward/actions/runs/41960178507/job/127024899340)
and [push 41960173043 / job 127024835587](https://github.com/Chris0Jeky/Loomward/actions/runs/41960173043/job/127024835587).
Each workspace run reported **1,967 passed, 0 failed, 4 ignored**, including three doc tests and
excluding the overlapping seven-test publication invocation. The unchanged architecture guard,
both operations regressions (68-test suite) and the chain regression (44-test machine-store suite)
all actually passed, followed by workspace Clippy with warnings denied.

All four app jobs passed **1,080 tests in 77 files**, zero Svelte errors/warnings and successful
builds; both Linux mock and live browser suites passed. At this dated checkpoint each complete
workflow was still **8/9**, with real standard-user Windows qualification pending. These Linux/app
receipts do not substitute for the strict one-service/four-native Windows gates.


## Final qualification and landing — 5 November 2026

The dated pending statements above are superseded by the complete final-head receipts.
Head `c0ab2262bcfaf5c58fea09e181ca12637a80b72a`, tree
`29cf9220d575824e1e7e17b3ccf5d0ad8492de42`, passed both complete attempt-1 workflows:

| Workflow | Windows native job | Result |
| --- | --- | --- |
| [PR 41960178507](https://github.com/Chris0Jeky/Loomward/actions/runs/41960178507) | [127024899379](https://github.com/Chris0Jeky/Loomward/actions/runs/41960178507/job/127024899379) | 9/9 jobs successful |
| [Push 41960173043](https://github.com/Chris0Jeky/Loomward/actions/runs/41960173043) | [127024835838](https://github.com/Chris0Jeky/Loomward/actions/runs/41960173043/job/127024835838) | 9/9 jobs successful |

The push checked out that head. The PR checkout was
`a10ad95d17f3f15d5facd0bbbd2c710d0667af3d`, with the same tree and parents current main
`2aceefa0a51cbe8776ec0683f93707a17a3c0a0d` plus the final head.
Independent review found no remaining blocker; the [automated code review](https://github.com/Chris0Jeky/Loomward/pull/1146#issuecomment-6649035817)
also completed on that head with no findings.

Both standard-user executions explicitly report Administrator membership false, TokenElevation 0,
NTFS, child exit 0, no timeout or parent error, completed cleanup and no cleanup failure.
Their strict gate results are identical:

| Gate | Passed | Failed | Ignored | Setup skip |
| --- | ---: | ---: | ---: | --- |
| Service query-return ownership | 1 | 0 | 0 | None |
| Native directory recheck | 4 | 0 | 0 | None |
| Destination ACL refusal | 4 | 0 | 0 | None |
| Grant/regrant consent | 4 | 0 | 0 | None |

All named tests have actual `ok` lines. The native cases reach all three flags and 0/1/2-return
boundaries, deadline expiry, cancellation priority, retained sizes and uncertainty propagation.
The uncancelled identity-change control performs four queries and detects the changed directory.
The service gate reaches its complete assertion sequence, including one query, zero workers,
no retained report at its controlled cancellation timing and a subsequent successful run.

Each full Windows workspace reports **2,700 passed, 0 failed, 10 ignored**; service **708/0/2**,
operations **68/0/0**, machine stores **44/0/0**, and engine **291/0/0**. The separate all-features
Windows invocation reports **389/0/3** and overlaps the workspace. The Linux workspace reports
**1,967/0/4**. Both totals include three doc tests and exclude the separate overlapping seven-test
publication invocation. Both pure source-recheck regressions, the Reclaim intermediate-chain case
and the unchanged architecture guard actually pass. Formatting, both Windows Clippy gates,
Linux Clippy, the Python matrix and desktop qualification pass. App qualification is
**1,080 tests / 77 files**, zero Svelte errors/warnings, successful builds and Linux mock/live
browser success; Windows browser steps are intentionally not executed.

The only setup-return marker outside the strict gates is the existing no-8.3-alias fixture,
once in the workspace and once in the overlapping all-features invocation. A captured
`0 passed / 1 failed` child result belongs to the intentional
`a_panicking_child_is_not_an_unreached_crash_point` negative control: it launches an invalid
synthetic scenario, catches the resulting child-failure diagnostic and checks its classification.
That outer test and the 20-test crash suite pass; it is not a failed workspace gate.

Both downloaded diagnostic ZIPs were independently hash-verified against the API digests:

- PR artifact **13386861576**, **124,836 bytes**, SHA-256
  `3ef787326c4d54d1b00cfe5bfe2c8eb3f2c1b13ec908404afe35b904d71b9506`.
  Members: stdout **685,536 bytes**, stderr **0**, status **386**.
- Push artifact **13386069142**, **124,805 bytes**, SHA-256
  `e3d56c03dc1dc62f03cfa2782d24676847c3dc987d0e8f70d94a691a500dacd1`.
  Members: stdout **685,639 bytes**, stderr **0**, status **386**.

[PR #1146](https://github.com/Chris0Jeky/Loomward/pull/1146) merged as
`1d5aaf0fd318bdeb692e9e0ec420f05266db96a9`; its actual main tree exactly matches the qualified
tree. Included #1147 is also marked merged through ancestry. Issues #1144 and #1108 are closed
after their scoped acceptance completed.

The precise service guarantee is **no retrievable cancelled report**. A late cancellation between
storage and terminal publication can leave a bounded internal report, but retrieval requires
Completed. The controlled query-return regression proves no retention for its own timing.
The fixture holds after the OS query has returned; in-flight kernel-call interruption and a hard
cancellation-latency bound are not claimed. C8 remains closed.
