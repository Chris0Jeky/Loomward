# LW-080: Rust port of the v2 allocator, with oracle parity

9 October 2026, DESKTOP-IHKOOJS, Windows 11, rustc/cargo 1.97.1, Python 3.14.3, base 42e34e7.
Scope: `crates/loomward-core/src/planner_v2.rs` ports `python/loomward/planner_v2.py`. The v1
planner (`planner.rs`, `planner.py`), its 51-case counterexample and all negative evidence are
untouched. Nothing here touches a filesystem; every proposal is `executable: false`.

## Commands and results

| Command | Result |
|---|---|
| `py -3 scripts/export_planner_v2_fixtures.py` | writes `fixtures/v2/planner-v2-parity.json`: 222 cases, byte-identical on rerun (sha256 5fc56dae...da0c0) |
| `cargo fmt --all --check` | pass |
| `cargo test --workspace` | 28 passed, 0 failed (19 existing, 9 new `planner_v2::tests`) |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `py -3 scripts/verify.py` | all gates PASS: Python 186 tests (1 skipped), JS 9 boundary + 80 admission fixtures, Rust fmt/tests/lint |

All first measurements of the Rust port; there is no hosted CI result in this record.

## Parity corpus

| Suite | Cases | Contents |
|---|---|---|
| `experiment-51` | 51 | the original corpus (`experiments/v2_benchmarks.cases`, seed 20261009, node budget 5000), case 0 is the explicit counterexample |
| `edge` | 42 | empty groups, missing `groups` key, goal already met, pinned/active/protected/unspecified flags, heat and cooldown boundaries (0.25, 0.2500001, days 6 vs 7), offline/read-only/lower-tier destinations, offline/read-only source, budget exactly at the limit and one short, reserve exactly at the limit and one over, cumulative reserve, independent source/destination/transfer costs, search cutoff (budget 50 and 0), budget equal to and one below the nodes a complete search needs (18072), 12 vs 13 groups and 5 targets (`problem_size_limit`), a case only the heuristic portfolio can solve, tie-breaks by id, non-ASCII ids, 128-character ids, byte counts near 2^53 |
| `tie-fuzz`, `fuzz` | 40 + 60 | seeded random scenarios; tie-fuzz uses equal costs and fine decimal heats so heat decides |
| `reject` | 29 | inputs Python refuses (`ValueError`): duplicates, unknown ids, out-of-range numbers, float or bool where an integer is required, null or string flags, `node_budget` 200001 / -1 / true |

193 accepted cases carry the full Python output; the Rust test compares every top-level field
(including the nested `search` object, feasibility, `search_complete`/`optimality_claim`,
stop `reason`, `optimality_scope`, `projected_free_bytes`, baseline and improvement) with
`assert_eq` per field and checks the key sets are equal. All 193 match and Rust rejects all 29.
Search stop reasons across accepted cases: 150 exhausted, 39 node_budget, 4 problem_size_limit.
v2 beat v1 on 26 cases. Mutation spot checks (heat term zeroed, `>=` to `>` on the node budget,
`<=` to `<` on the transfer budget, `>=` to `>` on the heuristic reserve test) each fail the
parity test.

## Oracle and property tests

- `original_51_cases_match_the_exhaustive_oracle`: over the 51 original scenarios, v2 misses 0
  feasible targets, has 0 cases worse than the exhaustive oracle (ported as a test helper), and
  every search completes. For contrast v1 on the same corpus misses 9 and is worse in 24
  (`docs/22-experiment-findings.md`; v1 still returns shortfall 10 on the counterexample, asserted).
- `no_constraint_violation_on_any_fixture_case`: every accepted case is re-checked independently
  (transfer budget, per-destination reserve on every receiving target, eligibility, tier, online
  and writable, single use of a group, projected free space, consent flags).
- `seeded_random_scenarios_hold_constraints_and_never_lose_to_v1`: 400 seeded small scenarios
  (splitmix64, seed 20261009); constraints hold, v2 shortfall <= v1 shortfall, and whenever the
  search is complete the shortfall equals the oracle's.

## Python/Rust discrepancies

1. **Missing `groups` key.** Python `plan_tiers_v2` raises `KeyError('groups')` (it reads
   `scenario['groups']`; v1 uses `.get('groups', [])`). The Rust port follows v1 and treats the
   key as empty. The fixture case `edge-missing-groups` carries a `python_discrepancy` note and
   Python's output for `groups: []`. `planner_v2.py` was not edited; if the parallel Muse fix
   lands, rerun the exporter and the note disappears.
2. **Heat arithmetic ceiling.** Python scores heat as `Fraction(str(heat)) * source_bytes`
   (unbounded). Rust keeps the same exact decimal as a u128 scaled by a shared power of ten and
   fails closed with an error if a scenario mixes heat scales so far apart that the sum leaves
   u128 (roughly 20 digits of scale difference). No fixture case reaches it; a bigint is the upgrade.
3. **Typed intake.** `scenario_from_value` reproduces Python's flag validation (including that
   flags are only evaluated for source-volume groups up to the first true one). Typed callers of
   `plan_v2(&Scenario, u64)` get the v1 Rust behaviour (a null flag is "unspecified"). A negative
   or boolean `node_budget` cannot be expressed as a `u64`; the test harness maps those to
   rejection, and `plan_v2` itself rejects values above 200000.
4. **Oracle helper.** The test oracle exempts targets that receive nothing from the reserve
   check; the Python oracle requires every target to satisfy it. The two agree on all 51 original
   cases (their targets start above their reserve) and differ only on degenerate scenarios where a
   target starts below its reserve.

## NOT verified

- `serde_json` is used without `float_roundtrip`; heats are plain short decimals here, but an
  input with 16+ significant digits could parse one ULP differently from Python and change the
  decimal. Not exercised.
- No timing comparison with Python, no run on Linux/macOS, no hosted CI, no IPC wiring or
  decimal-string byte transport (that is the rest of LW-080's "carry integer checks through IPC").
- The `/api/plan-v2` route and CLI do not call the Rust planner yet.
