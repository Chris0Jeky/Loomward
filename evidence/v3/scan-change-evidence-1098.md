# #1098 comparison correctness and bounded query evidence

2026-11-02; synthetic fixtures only. Source base: remote main
`3f5aaf2b9dd6a9c49d1c0471f4390deb6df5d10a`, locally reconstructed as
`4db7f22494b7ba330945e7ba7df752e88da6ea55`.

## Behaviour

Failed/cancelled explicit baselines return `no_baseline`, null baseline and figures,
empty ranked lists, and `baseline_not_completed`. A completed but incomplete
baseline retains its explicitly partial counts. Directory exclusion flags and
excluded/absent-pending directory states prune the entire branch consistently;
the granted root itself never counts as a new folder. Direct children of the
root can be ranked even if an imported root row has a later birth stamp.

Schema and DTO enforce the existing outcome/baseline/count/list relationship;
there are no new wire fields or storage migrations. Today formats baseline dates
in the local calendar and says `At least` in the headline when a root was omitted.
A platform-independent service projection test pins `changes_since_scan`; it
proves disclosure from a stale tracker answer, not Windows change detection.

## Executed checks

- Before fixes: `TZ=America/Los_Angeles npm --prefix app test -- tests/unit/today.test.ts`
  had 40 passing tests and two expected behavioural failures: missing `At least`
  and a local late-evening baseline rendered on the next UTC date.
- Eleven invalid outcome/figure/list examples were accepted by the original
  Python JSON Schema validator, then rejected after the schema fix. The ordinary
  success fixture still validates. Rust DTO negatives are consumed by the existing
  contract fixture suite; local Rust execution is unavailable.
- After fixes: the focused Today suite passed all 42 tests in both
  `America/Los_Angeles` and `Pacific/Auckland`.
- `npm --prefix app test`: 75 files, 1,062 tests passed.
- `npm --prefix app run check`: zero errors and warnings.
- `npm --prefix app run build`: passed; existing bundle-size warning above 500 kB.
- `python scripts/gen_contracts_ts.py --check`: up to date; generated TS unchanged.
- `PYTHONPATH=/workspace/scratch/3baa036ab45d/test-deps:python python -m unittest discover -s tests -p test_v3_contracts.py`:
  15 passed. `git diff --check` and Python helper compilation passed.

Full Python verification in the restricted process ran 438 tests with one failure,
33 socket-permission errors, and 33 skips. The failure was
`test_categoriser_bench.Benchmark.test_existing_output_directory_is_refused_without_creating_files`:
the environment's temporary directory has a Git ancestor, so its outside-Git guard
wins before the existing-output-directory guard. The coordinator owns the combined
rerun with the correct temporary directory; no guard was weakened. A subsequent
unrestricted run was stopped in coordination before completion.

Browser verification was attempted: restricted local sockets were refused; the
unrestricted attempt passed the timing preflight and built the development app,
then could not launch because Chromium is absent. No browser installation was
attempted. `scripts/test_app_new_since.py` supplies a coordinator hook for both
local-date directions, real zero, missing/no/failed baselines, partial coverage,
watcher disclosure, lower-bound headlines and the Explorer target. These browser
assertions are authored but unverified locally.

## Hosted regression and review receipts

The tests-only checkpoint in [PR #1133](https://github.com/Chris0Jeky/Loomward/pull/1133),
`a67d02b475b438af7c634ef4a8e6675d5e55b812`, passed formatting and compiled on Linux.
[Native job 125277599659](https://github.com/Chris0Jeky/Loomward/actions/runs/41445545731/job/125277599659)
failed the two intended catalogue assertions at 12:38 UTC: excluded ancestry/root
counting returned `(6, 3, 2005)` rather than `(1, 1, 7)`, and failed/cancelled
explicit baselines retained figures. This establishes native behavioural red; it
is not evidence that the implementation passes.

A fresh-context review found and resolved a browser-proof issue: changing only a
fragment could reuse the previous document and answer. Each scenario now changes
`comparison_case` in the query string and waits for its `changes.new_since` HTTP
response. The review also moved a test module after implementation items and
compacted a nullable-field fixture. The reviewer re-read all three fixes and
cleared the source subject to integrated exact-head CI.

## Query measurement

`python scripts/measure_new_since_sql.py` extracts the three production query
strings directly from `new_since.rs`, uses the checked-in `catalog.sql` and its
indexes, and consumes every result on synthetic in-memory SQLite. No indexes or
history tables were added. Timings include Python iteration, but exclude native
snapshot setup, Rust heaps/DTO conversion, and ranked-row path resolution.

| Files | Directories excluding root | File pass steps / ms | Count pass steps / ms | Ranking pass steps / ms | Total steps |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 100,000 | 10,000 | 2,210,000 / 71.24 | 540,000 / 10.82 | 500,000 / 14.57 | 3,250,000 |
| 500,000 | 50,000 | 11,050,000 / 386.93 | 2,700,000 / 57.47 | 2,500,000 / 54.92 | 16,250,000 |

Counts were exact in both cases and the three queries stayed within 32M steps.
Steps are sampled in increments of 1,000 per query. This is one warm synthetic
Linux measurement, not native/Windows throughput, disk I/O, or a maximum supported
root size. Larger/deeper or differently distributed roots can still exhaust the
budget; the existing refusal remains honest. The bounded measurement does not
justify a storage migration, additional index, or query consolidation in this fix.
A native 50,000-file/5,000-directory regression checks exact totals and capped lists
within the existing reader budget; execution is deferred to hosted CI.

No Cargo, rustfmt, clippy, native build or Windows result is claimed locally.
