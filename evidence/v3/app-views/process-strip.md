# Observatory process strip — issue #142

Windows 11, 10 October 2026. Branch `feat/observatory-process-strip`, tested over
`f910c89372e827d42791b25a9de30b87720663fc` plus the uncommitted working diff.
No commit or push was made. Screenshots contain only synthetic mock processes.

## Changed

- Five process rows ranked by machine-normalised CPU or private commit, using the existing
  byte parser for exact sorting; unknown readings follow measured zero.
- Every row shows its own `observed_at` age. Retained, paused, denied, partial-coverage and
  unavailable observations stay explicit. A local clock ages rows without any transport call.
- Ordinary explanation links open Companion by opaque process reference, including a
  missing-reference error. Focus moves to the explanation and late responses cannot replace
  a newer explanation. No process effect controls were added.
- Companion's lease and list reads are shared helpers. The baseline Observatory had snapshot
  polling but no sampler lease: it now uses one lease for gauges and processes, renewed with
  the same ID every 20 s at 1 Hz. Process list reads are limited to five rows every 15 s,
  with an extra stored-list read on a lens switch. Pausing or leaving releases the lease,
  including a subscribe response arriving after unmount. Native sampler code is unchanged.
- The strip holds row updates while a link has focus. Its status region persists; labels,
  underlines and text carry meaning in forced colours, and controls meet 24 px targets.
- The mock keeps enumeration timestamps for rows, lists and explanations, including across
  newer system snapshots and paused reads. Initial rates are unknown; later synthetic
  observations require a live process lease.

## Verified

| Command | Final observed output |
|---|---|
| `npm.cmd --prefix app ci` | 66 packages installed; 0 vulnerabilities |
| `npm.cmd --prefix app run check` | 346 files; 0 errors, 0 warnings; viz checks pass |
| `npm.cmd --prefix app run test` | 23 test files; 187 tests passed |
| `npm.cmd --prefix app run build` | Vite production build succeeded |
| `py -3 scripts/test_app.py` | 148 PASS messages; `ALL PASS` |
| `py -3 scripts/test_app.py --a11y` | 117 PASS messages; `ALL PASS (a11y)` |
| `py -3 scripts/test_app.py --live-serve` | 12 PASS messages; `ALL PASS (live serve)` |
| `git diff --check` | Exit 0, no output |

The app runner's accessibility leg is a separate `--a11y` invocation, so both were run.
Model/feed Vitest coverage includes sorting, unknowns, ages, partial/unavailable coverage,
one lease, bounded read cadence, renewal, expiration recovery and late-subscribe cleanup.
Playwright coverage in `scripts/test_app_views.py` and `scripts/test_app_a11y.py` exercises
lens switching, initial unknown CPU, retained ages, focus holding/release, pause/resume,
Companion navigation, missing refs, contrast, keyboard names/order, 320 px and forced colours.
Polling predicates use `wait_until`, not CSP-blocked `page.wait_for_function`.
Browser checks report no page errors. Existing Companion regression tests remain green.

Final browser logs are local, gitignored `.loomward/process-strip-app.log`,
`.loomward/process-strip-a11y.log`, `.loomward/process-strip-live.log` and
`.loomward/process-strip-capture.log`.

## Screenshots

- [Observatory dark theme](process-strip-observatory.png)
- [Woven atlas theme](process-strip-woven-atlas.png)
- [Light-token probe](process-strip-light-token-probe.png)
- [320 px layout](process-strip-320.png)

Both shipped themes are dark. The light image is a constructed-stylesheet palette probe
of the built strip, not a shipped light theme. All four images were visually inspected;
the final dark capture was also inspected after the last timing fix.

## NOT verified

Real NVDA/Narrator announcements require human testing. The mock proves strip behaviour;
the live-server gate proves the real HTTP/CSP boundary, not live process-counter accuracy
or Tauri/WebView2 behaviour. The ten-minute native P12 CPU comparison was not measured.

## Residual risk

Unknown readings do not establish which unobserved processes belong in a global top five;
the UI exposes denied counts and never substitutes zero. A disappeared process can make
its explanation unavailable, which is shown as an error. Native P12 overhead remains
unmeasured; tests establish one lease and bounded reads, not a hardware CPU percentage.
The optional Impeccable detector reported only an existing Companion accent-border warning,
outside the changed styles; it reported no strip findings.

Recommended single commit: `Add Observatory process strip with observed ages`.
