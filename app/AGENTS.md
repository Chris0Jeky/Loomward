# app/ — Svelte app (seam notes; the root `AGENTS.md` governs)

- Gate: `npm.cmd --prefix app run check`, `run test`, `run build`, then `py -3 scripts/test_app.py` (Playwright
  e2e), or all four via `py -3 scripts/verify.py --app`. It reports UNVERIFIED, never PASS, when Node, npm or
  `app/node_modules` is missing.
- Touching `loomward-http` or an app transport: add `py -3 scripts/test_app.py --live-serve` (real `loomward-serve`).
- Every event source heartbeats: 45 s of stream silence shows Unavailable.
