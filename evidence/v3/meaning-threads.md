# Meaning threads: engine to Woven Atlas

Synthetic smoke on Windows, 2026-10-11. Base: `0b279351a9ad41d68ab8b7c317f2eb56800a8ad5`.
Branch: `feat/meaning-threads`; changes are uncommitted for the driver.
This proves wiring and bounded behavior, not real-user accuracy or calibration.

## Changed

- Engine `learning/meaning.rs` reads the current stored candidate once and projects student advice.
  Service `meaning.rs` resolves current session IDs and reuses the bounded learning features.
  Available durable file identities only; exclusions and non-files are not eligible. No content,
  reference creation, feedback write, training, promotion, teacher call or operation grant occurs.
- Contract `MeaningThread` gains `no_model`, `abstained`, `not_eligible` and optional `suggestion`,
  reusing the existing ranked scores/reasons DTO and its constant false authority/calibration flags.
  Schema, Rust DTO, command result example and generated TypeScript agree. Personal reads refuse
  before feature extraction with `capability_unavailable` / `learning_personal_gated`, the same
  named gate exposed by learning status. The semantics table records this choice.
- A 6,000-ID request is contract-bounded; a 1,000-node work cap returns a request-order prefix and
  honest `budget_hit`. Deadline expiry refuses the read; changed revisions refuse stale projections.
  HTTP retains its existing 64 KiB ingress limit; Tauri/direct service retains the DTO's ID limit.
  UI projections cap their current sealed-ID batch at 1,000 and visibly report an unknown suffix.
- Atlas uses drawn cells as inference candidates, including resize changes. Meaning is horizontal
  weft in the existing cloth and palette; relative score controls eight-thread dye density.
  Abstention is undyed; no-model gets a quiet explanation. Requests and late replies are cancelled
  or discarded on navigation, toggle-off and destruction. A visible-region text list and both
  Atlas/Explorer inspectors show the same suggestion states, ranked relative scores and reasons.
  The text list is rendered when opened; an arriving meaning response preserves active zoom.
- Deterministic invented mock advice and service/browser/unit regressions exercise the seam.
  Existing learning edits are two visibility changes to reuse feature extraction and the gate;
  test helpers gain sibling visibility. Retraction, events, jobs and duplicates are untouched.

## Verification

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | PASS |
| `cargo test --workspace` | PASS: 765 tests, 0 failures |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `py -3 scripts/verify.py` | PASS: 224 Python tests, one existing symlink privilege skip; two JS syntax checks, nine boundary assertions, 80 parity fixtures, Rust gates |
| `npm.cmd --prefix app run check` | PASS: zero errors/warnings |
| `npm.cmd --prefix app run test` | PASS: 323 tests across 32 files |
| `npm.cmd --prefix app run build` | PASS |
| `py -3 scripts/test_app.py` | PASS: all browser checks, no console/page errors, requests stayed on loopback |
| `py -3 scripts/test_app.py --a11y` | PASS: WCAG browser checks, keyboard, contrast, reflow and forced-colour patterns |
| `py -3 scripts/test_app.py --live-serve` | PASS: real HTTP/service/student and app, synthetic feedback/refit/threads, personal meaning refusal |
| `git diff --check` | PASS |

Raw gate logs are ignored under `.loomward/meaning-final-*.log`; result manifests are
`.loomward/meaning-rust-results.json` and `.loomward/meaning-app-results.json`.
The mechanical design detector reported no findings. Desktop/mobile synthetic screenshots are
`.loomward/meaning-weft-desktop.png` and `.loomward/meaning-weft-mobile.png`.

## Mutation proof

Every new named regression was observed failing under a targeted mutation, then restored and
included in the green suites. Initial service tests also failed against the unimplemented command.

| Regression / oracle | Removed or broken behavior | Observed |
|---|---|---|
| `meaning_is_bounded_read_only_advice_with_explicit_unknowns` | Treat student abstention as suggested | FAIL (101); restored |
| `meaning_work_budget_is_an_honest_prefix` | Raise the cap to 2,000 | FAIL (101); restored |
| Mock meaning unit test | Remove the mock command | FAIL (1); restored |
| Relative-score intensity unit test | Force every intensity to 0.5 | FAIL (1); restored |
| Lazy projection unit test | Remove abort on navigation | FAIL (1); restored |
| Lazy projection unit test, HTTP budget assertion | Send 6,000 IDs rather than cap at 1,000 | FAIL (1); restored |
| Existing contract example oracle, updated student example | Remove schema suggestion field | FAIL (101); restored |
| `meaning_weft` browser check | Remove the relative-score legend warning | FAIL; restored |
| Live before-fit check | Remove service command handler | FAIL at no-model assertion; restored |
| Live after-fit check | Hide the stored candidate only from meaning | FAIL at ranked-advice assertion; restored |

Mutation receipts are `.loomward/meaning-mutation-*.log`. The service regression covers positive
suggestions after distinct synthetic human labels, neutral unfamiliar-feature abstention, model
identity, read-only revisions, no-model, non-file eligibility, stale/duplicate IDs, deadline refusal
and personal gating. The live leg records synthetic human feedback, refits through the product's
real HTTP/service/student path and obtains ranked advice with honest low-support abstention.
The browser check toggles Meaning, reads neutral abstention/reasons/scores in the text list, checks
both themes and forced colours, checks narrow reflow, and reads Explorer's meaning inspector.

## Failures resolved

An initial zoom measurement failed after adding a hidden text projection with repeated linear ID
lookups. Rendering that list on open, using a set for visible membership and preserving active zoom
on late meaning updates restored the existing frame-pacing gate without changing its threshold.
A single-label synthetic fit correctly scored its winner at 0.5 and abstained; the positive-control
fixture now includes contrasting human-supported classes. No student threshold was changed.

## NOT verified and residual risk

No personal learning, real-user accuracy, calibration, model promotion, teacher disclosure,
file/process effects, Tauri/WebView2 integration, hosted CI or assistive-technology session was tested.
The existing symlink privilege skip remains. Very small cells retain the renderer's simplified
level of detail; exact relative scores remain in text. Requests beyond 1,000 nodes omit a suffix
with `budget_hit`, and deadline expiry returns a refusal. These are explicit limits, not accuracy claims.

Recommended commits: `Implement bounded student meaning threads` and
`Render lazy meaning weft in Atlas and Explorer`. No commit or push was made by this lane.
