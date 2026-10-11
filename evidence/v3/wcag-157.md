# WCAG findings #157: Svelte app regression evidence

11 October 2026. Windows, Chromium 151.0.7922.34 via Python Playwright; synthetic mock
transport only. Branch `fix/app-wcag-157`, starting HEAD `0377b0f5bb9de3a515e6d2e712dc9e5104332411`.
Changes are uncommitted for the driver. No screenshots were generated or committed.

Most original findings were already fixed in the starting revision. This lane preserves those
implementations, strengthens their regression checks, and fixes three remaining CSS lines.

| Finding | Before (audit or this lane's reproduction) | After and proof |
| --- | --- | --- |
| 1.4.10 / 1.4.4 / 2.4.11: obscured focus | Audit: sticky masthead pinned 53% / 99% of the small viewport. Existing tests checked positioning but not focused boxes at those sizes. | Inherited static chrome below 760 px width or 500 px height and measured sticky scroll padding. New focus/masthead rectangle comparisons cover all six views at 640x360, 320x256, 640x800 and 1280x400, including expanded region lists and canvases. Existing sticky checks cover 1280x800, 900x600 and 800x700. |
| 4.1.2 / 2.4.6 / 1.3.1: names and row context | Audit: repeated Inspect, Revoke and checkbox names; first cells were td. Existing row check accepted th without scope. | Inherited escaped target names and row headers retained. Accessibility-tree duplicate-name checks pass; new checks require target-specific root/grant Revoke labels, all nine candidate checkboxes, and actual th[scope=row] in Explorer, Tiers, Companion and Health. |
| 2.4.3 / 4.1.3: focus and results | Audit: Simulate/Keep/Revoke dropped focus; result regions appeared already filled; Explain and routes were silent. Existing result checks did not establish node persistence. | Inherited Simulate focus retention, Keep return to opener, Revoke result focus, Explain heading focus and route heading focus pass. New checks capture empty Tiers/Health status nodes before actions and prove the same nodes contain the results afterward. |
| 1.4.11: control edges and selected state | Audit: ordinary edges below 3:1 (already fixed on main). This lane reproduced selected basis edges at 1.54:1 and hovered Inspect at 1.70:1. | Both basis inset borders now use the existing --control-line; Inspect hover keeps its normal border. Both views and themes measured: selected indicator 3.33:1 / 3.23:1; hovered Inspect 3.67:1 / 3.53:1. Existing resting input/select/button/switch checks remain green. |
| Other original moderate/minor findings | Placeholder labels/contrast, forced colours, list semantics, dangling Inspector label, gauge image roles, canvas announcements/cursor/edges, polling pause, moving rows, tooltip Escape, orbit buttons, target sizes and region text. | Existing targeted checks remain green. No extra implementation changes in this lane; actual screen-reader speech remains unverified. |

## Regression and mutation proof

The new contrast assertions failed on the unchanged source before each CSS fix (1.54:1 selected
basis, then 1.70:1 hovered Inspect). Reverting each new CSS fix also failed its assertion afterward.

24 targeted source mutations were killed, with a green control run before each mutation and a
green run after restoration. The temporary driver rebuilt the development fixture bundle and ran
unchanged sections of `scripts/test_app_a11y.py` through Chromium; no assertion was disabled.
An ambiguous scroll-padding restore in the temporary driver was corrected and that case rerun;
all mutations were restored, including all temporary Health edits.

| Reverted behavior | Mutants killed | Observed failure |
| --- | ---: | --- |
| Static chrome, width condition, height condition, sticky scroll padding | 4 | Focus-box overlap, pinned rail, or sticky focus obscuration |
| Inspect, root Revoke, grant Revoke, checkbox names | 4 | Bare target label or duplicate accessible names |
| Row-header scope in each of the four table views | 4 | First-cell row-header assertion |
| Simulate, Keep, Revoke, Explain and route focus | 5 | BODY/button retained instead of intended target, or heading/status not focused |
| Replace Tiers/Health status nodes when text changes | 2 | Previously captured live-region node disconnected |
| Inspect hover and each view's selected basis border | 3 | Contrast below 3:1 |
| Each theme's resting control-border token | 2 | Contrast below 3:1 |

Commands from the repository worktree root: `py -3 .loomward/wcag157-mutations.py`,
`py -3 .loomward/wcag157-mutations.py 3`, and `py -3 .loomward/wcag157-mutations.py 22`
(temporary driver; green restored cases covered 0-2, 3-21 and 22-23), followed by the full gates below.
The driver is gitignored test tooling, not part of the delivered source change.

## Final gates

| Command | Result |
| --- | --- |
| `npm.cmd --prefix app run check` | PASS: 353 files, 0 errors, 0 warnings; viz check also passed |
| `npm.cmd --prefix app run test` | PASS: 26 files, 293 tests |
| `npm.cmd --prefix app run build` | PASS: production Vite build, 172 modules |
| `py -3 scripts/test_app.py` | PASS: exit 0, 173 PASS checks plus Playwright assertions; 4 timing-helper unit tests passed |
| `py -3 scripts/test_app.py --a11y` | PASS: exit 0, 172 PASS checks plus Playwright assertions; 4 timing-helper unit tests passed |
| `git diff --check` | PASS |

## Declined and NOT verified

- Later-comment Explorer stale-catalogue fallback focus/loading issues: separate refresh lifecycle seam requiring a catalogue-invalidation browser fixture; not covered by the successful normal-route focus checks.
- Later-comment Explorer breadcrumb list/default non-colour cue: LOW findings outside the requested blocker/serious fixes; forced-colour cues are already tested.
- Native disabled for Explorer Load more: declined because its aria-disabled plus busy guard preserves focus; the refresh/loading announcement path remains outside this slice.
- Malformed negative-parent handling, size-basis discrepancies and stale Companion lease cleanup: separate non-accessibility behavior from later comments, outside this CSS/markup/focus slice.
- NVDA/Narrator speech, live-message ordering, inserted-alert speech, badge pronunciation, radiogroup modes and voice control were not tested. CSS viewport equivalents are not physical Windows display-scaling evidence.
- No axe run: the existing --a11y entrypoint uses Playwright accessibility snapshots and targeted assertions, with no axe dependency. No native WebView2, real inventory, backend transport or other-browser proof is claimed.

Recommended commits: `fix(app): preserve contrast for inspect and selected basis controls`;
`test(app): pin WCAG focus geometry and persistent result regions`.
