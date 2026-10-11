# LW-108: placement service and Tiers wiring

Measured on 2026-10-11 on Windows, on a shared host. Branch:
`feat/placement-wiring-108`; starting commit:
`f42d0a15c45bbab9b0d6aa16f9cbb556d52edf3d`. All fixtures are synthetic.
No commit, push, release or deployment was performed by this lane.

## Behaviour

1. `loomward-service/src/placement.rs` implements `tiers.model`,
   `placement.candidates`, `placement.simulate`, `proposals.list` and
   `proposals.get` through the existing engine interfaces. `volumes.list` shares
   the same scoped observations. Payloads are decoded into contract DTOs;
   returned IDs are session-bound. Read calls use read permits, deadlines and a
   bounded, query-only SQLite snapshot. Only `save: true` uses the mutation
   queue and idempotency cache.
2. Input groups come from the catalogue of active granted roots. Hidden or
   excluded descendants are pruned. Overlapping grants require provable native
   ancestry; otherwise the service returns `partial_coverage` with
   `overlapping_grant_ancestry_unknown`. Simulation reads metadata, never file
   contents. The only write is an explicitly requested stored scenario in the
   existing state-store `proposal` table; there is no operation manifest or
   executable plan.
3. Missing fresh link observations leave relief Unknown. Unknown target clusters,
   hidden-descendant assumptions, unusable volumes, `excluded_volumes` and
   `pre_rejected` reasons survive the wire. Missing reserve preferences are
   disclosed as an assumed zero reserve. Default verified-only simulations can
   legitimately produce no proposals. An entry-allocation what-if with explicit
   heat/protection overrides produces a proposal without claiming verified
   relief. Scores remain scores, never probabilities.
4. A registered synthetic lab root may supply scoped native volume-space
   observations. Other volumes are not enumerated. Personal placement uses only
   its granted roots' catalogue metadata. Missing source volume physics refuses
   with `capability_unavailable` and the named reason
   `personal_placement_physics_unobserved`, explaining the prerequisite. It does
   not compensate with personal native opens. Personal tests use synthetic
   metadata, not an owner's inventory.
5. Saved scenarios retain canonical row references, catalogue instance/revision,
   root generations, declaration revision and a canonical inputs digest.
   Duplicate inputs reuse the saved scenario, including across restart. Wire
   IDs are rebound on retrieval; old-session IDs are refused. Catalogue,
   declaration or generation changes mark scenarios stale; revoked grants hide
   them. Saving rechecks active grants and revisions, including changes within
   a generation and revocation of an unscanned root. The existing proposal table
   is reused, with a version-1 migration-to-save regression test. No new table
   or migration was necessary.
6. The Tiers view declares tiers through the typed client, lists real candidates,
   runs three heat alternatives, saves them when requested, lists saved
   scenarios and retrieves a historical scenario. Unknown values remain visible;
   a stale scenario is labelled historical. The mock implements the same calls
   for existing fixture tests. The live browser leg uses the production app and
   a real synthetic `loomward-serve` session.
7. Contract DTOs, commands, schema, examples and generated TypeScript already
   describe these payloads correctly; no shape change was needed. Semantics now
   documents read-versus-save behaviour, assumptions, refusals and scenario
   durability. Protocol, schema and cross-language checks pass unchanged.

## Regression and mutation evidence

Seven service tests, one mock unit test and one live-service Tiers leg were
added. The initial service tests failed against the missing dispatch; the mock
test failed against the missing tier-declaration implementation. Two subsequent
regressions were also observed red before repair: an unsaved simulation timeout
incorrectly marked nonretryable, and a save accepting revocation of an unscanned
root. Both now pass.

Each mutation below was applied temporarily, failed its named assertion, and
was restored before the final green checks. Failures were test failures, not
compilation failures.

| Mutation | Detecting test |
|---|---|
| Convert Unknown relief to zero | `placement_wire_preserves_unknowns_and_exclusions` |
| Accept an old-session proposal ID | `placement_saved_scenarios_survive_restart_and_hide_revoked_roots` |
| Ignore a save deadline | `placement_reads_do_not_take_mutation_tickets_and_expired_save_writes_nothing` |
| Take a mutation ticket for an unsaved read | Same read/deadline test |
| Lose the personal missing-physics refusal | `placement_personal_catalogue_scope_and_migrated_proposal_table` |
| Drop the unknown-cluster assumption | `placement_wire_discloses_unknown_clusters_hidden_descendants_and_bad_source` |
| Accept a same-generation catalogue save race | `placement_save_refuses_same_generation_changes_and_unknown_overlap` |
| Write a synthetic file during simulate | `placement_registered_lab_simulation_preserves_every_file_and_mtime` |
| Ignore mock tier declaration | New `mock-views.test.ts` declaration/save/get test |
| Lose a mock saved scenario | Same mock test |
| Remove the Tiers tier-declaration call | `live_placement` in `scripts/test_app.py` |
| Drop the positive what-if proposal | Unknown-cluster test's positive control |

The native lab test registers and scans an actual disposable synthetic root.
After simulation, all five fixture files retain byte-identical contents and
unchanged modification times. The deliberate write mutant fails that oracle.
The positive what-if control returns one proposal with synthetic transfer size
`3003`, `executable: false` and `filesystem_changed: false`. Elevated Windows
runners instead assert `identity_unavailable` and return at the native identity
boundary; that branch was not exercised locally.

## Gates

| Command | Result |
|---|---|
| `cargo fmt --all --check` | PASS |
| `cargo test --workspace` | PASS: 697 tests, including 74 service tests and all seven new tests |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `py -3 scripts/verify.py` | PASS: 222 Python tests, one Windows symlink-privilege skip; two JS syntax checks, nine JS boundary checks, 80 cross-language fixtures; Rust gates pass |
| `npm.cmd --prefix app run check` | PASS: zero errors and warnings |
| `npm.cmd --prefix app run test` | PASS: 294 tests across 26 files |
| `npm.cmd --prefix app run build` | PASS, including restored build after UI mutation |
| `py -3 scripts/test_app.py --view-shots .loomward/placement-108/ui-captures` | PASS: regular browser checks plus visual captures |
| `py -3 scripts/test_app.py --a11y` | PASS: automated accessibility leg, no page/console errors |
| `py -3 scripts/test_app.py --live-serve` | PASS: existing live legs plus real-service declaration, candidates, simulation, saved list/get and scenario rendering |
| `git diff --check` | PASS |

Desktop and narrow Tiers captures were inspected. The existing mobile table
scrolls within its container. The UI detector reported no findings. Local logs,
mutation scripts and visual captures remain in ignored `.loomward/` state;
none is a public inventory or operational export.

## Limits and remaining risk

Hosted CI, its elevated refusal branch, a Tauri runtime, human assistive-technology
testing, large-catalogue performance and real personal inventory were not
verified. Fresh per-file link observations are deliberately absent, so verified
source relief remains Unknown. Native volume-space queries are scoped but not
forcibly interruptible; caller deadlines and catalogue bounds still apply.
Catalogue observations and stored scenarios describe a point in time. This
work does not implement or claim live disk balancing, moves, copies, deletion,
recall, teacher calls or learning.

Recommended commit messages (driver-owned):

- `Wire catalogue placement reads and saved scenarios`
- `Connect Tiers declarations and saved scenarios to the service`
