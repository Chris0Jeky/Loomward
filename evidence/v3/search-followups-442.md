# Search v1 follow-ups (#442)

## Post-#575 reconciliation (18 October 2026)

Lane `fix/442-search-followups`, base `4babf6710c6425086520a8697d495dd15dff2d12`.
The search files on the newer `origin/main` were unchanged when checked before editing.

Already fixed by #575 (`b28f266`, merge `4b05fd6`): the v1 cursor advance-or-refuse
guarantee, unknown unsettled-folder sizes, per-hit cyclic-ancestry refusal, and the
one-character schema/semantics/UI wording. The historical 10M receipt below already records
P8 and the long transactions: EndRun 583.665 s, reopen/backfill 600.907 s, and the isolated
optimize statement 188.973 s. These are existing measurements, not rerun timings. Progress
reporting or transaction chunking remains separate product work; this patch adds no API.

The three remaining follow-ups from the issue comments are addressed:

- Ranked candidate collection refuses with `ResourceBudget` when a budget cut leaves no
  returned hits, including after applying a ranked cursor's offset. Successful partial
  pages and genuinely absent results retain their existing behavior.
- The request-local path cache also remembers invalid ancestry. A repeated cycle is
  detected before the depth limit; its ancestors/descendants do not pay the same traversal
  again. Over-depth paths cache only their starting folder, so valid ancestors stay usable.
- The command note and matching semantics now describe the existing LW-067/LW-069
  grant-scoped adapter (#541, merge `04cb397`) rather than saying it has yet to land.
  No adapter, disclosure grant or effect is enabled by these wording corrections.

Regression-first commands, with `CARGO_BUILD_JOBS=4` and
`CARGO_TARGET_DIR=C:/lw/target/lane-search-442`, against the base implementation with new tests:

| Command (from this worktree) | Before the fix |
|---|---|
| `cargo test -p loomward-catalog cyclic_ancestry_is_negatively_cached_for_every_affected_folder` | FAIL, 0 passed / 1 failed: the repeated folder used another 10,260 SQLite VM steps |
| `cargo test -p loomward-catalog --test search_rank a_ranked_budget_cut_without_returned_hits_refuses` | FAIL, 0 passed / 1 failed: empty `budget_hit` page with no cursor |

The ranked regression checks the combined ranker and each individual tier, a sufficient-budget
success, and an absent-name success. The existing zero-time-budget test also requires a
nonempty partial page. The path regression observes SQLite VM work, including a newly seen
descendant of cached invalid ancestry; a separate depth-boundary test keeps valid ancestors
visible. Existing catalogue integration tests exercise both plain and ranked cyclic hits.

Final checks on Windows, with the same lane target and four build jobs:

| Exact command | Result |
|---|---|
| `cargo test --manifest-path C:/lw/wt/search-442/Cargo.toml -p loomward-catalog` | PASS: 268 passed, 0 failed, 0 ignored; 0 doc tests |
| `cargo clippy --manifest-path C:/lw/wt/search-442/Cargo.toml -p loomward-catalog --all-targets -- -D warnings` | PASS |
| `cargo fmt --manifest-path C:/lw/wt/search-442/Cargo.toml --all --check` | PASS |
| `git -C C:/lw/wt/search-442 diff --check` | PASS |
| `py -3 C:/lw/wt/search-442/scripts/gen_contracts_ts.py --check` | PASS: generated types are current |

Both originally failing regressions passed after the fix; the full catalogue suite includes
them and the additional depth-boundary check. The corrected command manifest also parsed as
valid JSON. No workspace suite (reserved for the driver), service/app/HTTP/desktop run, hosted
CI, fresh 10M benchmark or quiet-host performance comparison was run for this patch. The
existing soft-budget FTS5 single-step overrun limit remains; no new latency bound is claimed.
At final reconciliation, `origin/main` had advanced to `3ad1870`. The search implementation,
tests and command note were unchanged there; its separate learning additions to `semantics.md`
do not overlap this slice's search paragraph. Integration remains with the driver.

HUMAN_TODO.md is unchanged. Its search-reach decision q-36 is already answered; this slice
does not require a new owner decision. No app/service behavior is changed, and no commit,
push, PR, release or deployment is performed by this Sol lane.

17 October 2026, KRASPYON, Windows 11 Home, shared 16 GB host. Synthetic names generated
in-process only: no filesystem inventory, personal data, HTTP or owner-file/process operations. Base `04cb397`;
changes are left uncommitted for the driver. F2 ranking and F3 freshness are already in this base.

## Changed

- A v1 budget cut must change the keyset position or return `resource_budget`; it cannot return
  the input cursor with `budget_hit`. The soft time cut waits for one examined row; the VM work
  budget and the reader's hard deadline still apply. This does not bound the duration of one
  SQLite FTS5 `xNext`, which can perform substantial work between interrupt callbacks.
- Search skips a hit whose ancestry exceeds the existing cycle/depth guard, and marks the page
  partial. Other hits remain available. Both v1 and the opt-in ranker use this projection.
- An unsettled folder returns null logical/allocated sizes and file/folder counts, with unknown
  coverage. Such a folder cannot satisfy a size filter. The required nullable logical-size field
  is aligned across the catalogue, protocol DTO, schema and generated app types. Settled rows
  retain their existing string byte values. Learning refuses an unknown size rather than inventing zero.
- The schema comment, semantics and Explorer notice distinguish one-character exact names
  (including the name before a dot) from two-character name/word starts; ranked token prefixes
  remain a separately described capability.

## Regression-first evidence

Each command below ran against the original implementation from `HEAD`, with the new tests
present, before restoring the preserved implementation. Each ran one test and failed (exit 101):

| `cargo test` arguments | Observed failure |
|---|---|
| `-p loomward-catalog --test search a_common_token_budget_cut_must_advance_or_refuse` | `zero-progress cursor`: identical pass 3/key cursor |
| `-p loomward-catalog --test search_rank unsettled_folder_sizes_are_unknown_without_losing_other_hits` | `RepairRequired` for the unlisted folder |
| `-p loomward-catalog --test search_rank a_cyclic_hit_does_not_fail_other_search_hits` | `Invalid("directory identity cycle")` fails the page |
| `-p loomward-protocol --test contract an_unsettled_search_row_has_required_nullable_sizes` | schema rejects the null logical-size field |

The common-token fixture contains 4,096 `common-<serial>.txt` files. It resumes directly at
the expensive substring NOT pass, forces a VM cut, and also verifies that a sufficient budget
finishes the same query. Existing zero-time-budget pagination remains covered without weakening
its test. The folder test exercises both v1 and ranked search, checks every unknown field,
excludes the unsettled folder from size-filtered results, and verifies its settled zero later.
The protocol test also rejects an omitted logical-size field.

The first full touched-crate run exposed two existing `queries` tests that expected a
zero-progress `budget_hit` page with a one-step VM allowance. Their assertions now require the
specified `resource_budget` refusal; the progress-handler test additionally retains a progressing
`budget_hit` case (skipping the directory range for a file-only request) and its checks that later
queries work. The focused `cargo test -p loomward-catalog --test queries` rerun passed 23/23.

## Measurement and cost

The complete [native receipt](bench/search-followups-442-10m.json) is one release run on the
shared laptop. It built **10,000,000 files** and 10,001 directories including the root. After
100 targeted relists remove 1,000 files, the size denominator is **10,009,000 indexed entries**
(files and non-root folders). The token index is off, matching the current default.

Commands: `cargo build --release -p loomward-catalog --example search_bench`, then
`search_bench --rows 10000000 --no-token-index`. `CARGO_BUILD_JOBS=3` and
`CARGO_TARGET_DIR=C:/lw/target/search-442`; Rust/Cargo 1.97.1. The 1,000-row smoke run also passed.

| Observation | Result |
|---|---:|
| Staging | 190.565 s |
| EndRun call, including publication | 583.665 s |
| Complete reopen with backfill | 600.907 s |
| Trigram optimize statement, with half the files surviving | 188.973 s |
| 100 targeted relists, 10 renames + 10 removals per folder | 25.996 s |
| Through-EndRun fixture entry rate (not the separate P4 harness) | 12,929/s |
| Catalogue file | 3,758,628,864 B; 375.52 B/indexed entry |
| Trigram index tables (`dbstat`) | 1,034,407,936 B; 103.35 B/indexed entry |
| Sampled peak working set (1 s sampling) | 1,546,186,752 B = 1.44 GiB |
| Complete benchmark wall time, including untimed setup/cleanup | 2649.1 s |

For reopen, the harness drops the trigram table/triggers, closes that connection, then times
`Catalog::open_with`. This includes database inspection (`quick_check`), initialization and
backfill; it is **not** an isolated FTS insertion time. For optimize, a separate raw rusqlite
connection uses `secure_delete=ON` and the default page cache. It deletes half the files, then
measures the same optimize SQL used by `purge_root` with a substantial surviving index. The timer
excludes the expensive deletion setup, rollback and commit. Rollback then restores all 10,009,000
index entries, checked by an assertion. This is an isolated statement observation, not the full
production forget transaction or a quiet-host comparison.

### Native search samples at 10M

First page, limit 50, 50M VM-step allowance, 250 ms soft budget, 21 runs after one warm-up,
including hit serialization. Native catalogue only, warm and uncontrolled cache; no HTTP or UI.
The budget column describes the warm-up page; per-run refusals are recorded separately in JSON.

| Query class | p50 ms | p95 ms | max ms | Warm-up budget hit |
|---|---:|---:|---:|---|
| digits | 9.61 | 13.75 | 13.92 | false |
| extension_only_largest | 3.96 | 4.89 | 5.71 | false |
| inner_substring | 7.77 | 10.08 | 10.13 | false |
| no_match | 7.93 | 12.39 | 14.04 | false |
| rare_filter | 7.70 | 8.42 | 8.93 | false |
| two_chars | 4.12 | 6.01 | 7.20 | false |
| two_words | 21.05 | 36.75 | 40.35 | false |
| word | 9.37 | 11.78 | 11.85 | false |
| word_and_extension | 26.55 | 36.85 | 46.79 | false |
| word_size_and_time | 254.74 | 294.48 | 533.20 | true |

The additional resumed common-word NOT-pass probe (limit 1, query call only, without hit
serialization) ran 21 times: p50 11.28 ms,
p95 **16.45 ms**, max 17.01 ms, zero refusals and zero budget cuts. Every
response advanced, finished or would have refused, enforced by an assertion. The 4,096-row
all-common-token regression above separately forces the no-progress case.

All sampled query-class p95s were below P8's 300 ms target in this run. The size/time query's
maximum was **533.20 ms**, so this is not a hard per-request bound. Its warm-up page had zero
hits, `budget_hit=true` and a continuation; no hit is not absence. No measured request refused.
The `rare_filter` case searches both files and aggregate folders; it is not a file-only
selectivity measurement. Neither the timings nor the pass target establish a performance gain.

An initial harness additionally timed a complete-root `ForgetGrant`, beyond the requested
optimize statement. That extra call was stopped after 1,783 s total run time (sampled peak
1.44 GiB; no memory refusal). No completed full-forget time is claimed. Its partial logs stay
gitignored under `.loomward/aborted-extra-forget*`. The results above are from the complete
reduced harness, which logs optimize immediately and exits after rollback verification.

Measured source: base `04cb397ee2604113acfa0babe29dc1b0827d9b27`; SHA-256 of
`git diff --binary -- app contracts crates`: `f87de49a812f47aadbd0981c95479a82b1108c390417899b0bc1fc0d14e621c2`.

The existing [v1 cost receipt](search-v1.md) remains historical evidence: on DESKTOP-IHKOOJS
at 1M, the trigram index added about 101 B/entry (catalogue 266 to 367 B/entry), first-scan
throughput fell about 25%, and P4 changed from 101–119k to 80–89k rows/s. Those are not new
KRASPYON measurements or an A/B claim for this patch. P14's 200 B/entry target (#386) remains
unmet. The owner accepted the search engineering cost in HUMAN_TODO q-36; no new acceptance
is inferred. Deferred index upkeep remains a possible future tradeoff, not part of this fix.

The F2 token index remains opt-in and its separate cost/activation conditions remain in
[the F2 receipt](search-relevance-f2.md) and #520. No default or activation gate changes here.

## Verified

Windows, from `C:/lw/wt/search-442`; all Cargo commands use the lane target directory and
three build jobs. Full test passes use `RUST_TEST_THREADS=2`. No tests were disabled or skipped
by this patch. Production search, DTO and app sources stayed fixed during the completed
measurement and final proving run; the later executable edit was the browser harness assertion
described below.

| Command | Result |
|---|---|
| `cargo fmt --all --check` | PASS (also passed the verifier's `cargo fmt --all -- --check`) |
| `cargo test -p loomward-catalog -p loomward-protocol -p loomward-service` | 557 passed, 0 failed |
| `cargo test -p loomward-catalog -p loomward-service` (repeat) | 503 passed, 0 failed |
| `cargo test --workspace --no-fail-fast` | 1,345 passed, 0 failed, 1 existing ignored `effects::exec::tests::throughput_lab` |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `py -3 -m unittest discover -s tests` | 279 run: 278 passed, 1 skipped |
| `npm.cmd --prefix app run check` | PASS |
| `npm.cmd --prefix app run test -- --maxWorkers=2` | 467 passed in 45 files, 0 failed |
| `npm.cmd --prefix app run build` | PASS |
| `py -3 scripts/test_app.py` | ALL PASS, 275 reported checks; loopback only; no console/page errors |
| `py -3 scripts/gen_contracts_ts.py --check` | PASS |
| `git -C C:/lw/wt/search-442 diff --check` | PASS; new evidence files also checked for whitespace, JSON validity and local links |

The final gate used `scripts/verify.py`'s workflow through a local wrapper that adds the requested
`--no-fail-fast` and caps Vitest workers at two. Every leg passed except the first browser run:
`test_app_views.py` still expected the obsolete phrase `Type three characters`. Its assertion now
pins the corrected one-character exact-name wording and the two-character starts wording, with
all subsequent checks retained. The complete browser suite then passed on rerun. The combined
wrapper was not rerun after this test-assertion-only edit; unaffected Python/Rust/app unit/build
passes above are retained. This was not a product failure or a host timing flake.

Raw local logs are gitignored: `.loomward/regression-red.log`, `touched-checks.log` (initial
old-budget expectations), `queries-green.log`, `touched-checks-green.log`, `final-gates.log`
(includes the initial stale browser assertion), `app-e2e-green.log`, the benchmark stderr/monitor,
and `bench-exit-code.txt` (0, observed through a held process handle). The public receipt includes
its source hash and sampled memory metadata. The worktree stays uncommitted; no remote write,
release or deployment was performed. Protected lane-owned-by-driver files are unchanged.

## NOT verified / residual risk

Performance was not measured through HTTP or the desktop window. No cold-cache, quiet-host,
real-inventory or owner-usability result is claimed. These are functional fixes and standalone
timing observations, not a measured speedup. FTS5 can overrun
a soft time budget inside a single step; a refusal is preferable to an unchanged-cursor loop.
The long writer transactions still expose completion at commit, without a new user-facing
progress protocol. Chunking them or adding a truthful progress API is separate product work.
HUMAN_TODO.md is unchanged; owner decisions and the separate #386/#520 work remain open where
already tracked. The driver owns review, commit, publication and integration.
