# Live Alerts response proof implementation plan

> **For agentic workers:** Use superpowers:executing-plans for this bounded repair.

**Goal:** Preserve the real-service Alerts refresh assertions without a later
Chromium response-body lookup that already failed in hosted CI.

**Architecture:** A narrowly scoped Playwright route forwards the actual
`alerts.list` request, captures its API response, and fulfills the browser request
with that unchanged upstream response. The browser still drives the interaction
and renders the service result. Existing request, settings, revision, draft,
focus, error-recovery, and responsive-layout assertions remain.

**Tech Stack:** Python and the repository's installed Playwright API.

**Spec:** Existing `live_alerts` contract in `scripts/test_app.py`; failed run
[41440982808, job 125262486832](https://github.com/Chris0Jeky/Loomward/actions/runs/41440982808/job/125262486832).

## Constraints and review focus

- Forward the original request and actual response, including status and headers.
- Capture only `alerts.list`; release the route before synthetic error injection.
- Reject missing or duplicate captures; never substitute an empty successful reply.
- Dispose API responses and remove routes on normal and exceptional exits.
- Retain all persisted-state and browser interaction assertions without retries.

## Work

- [x] Read the exact-head failure: `Response.json` fails with
  `Network.getResponseBody: No data found for resource with given identifier`.
  The log does not establish a navigation or the mechanism of body loss.
- [x] Replace the later body lookup with a real-service pass-through capture.
- [ ] Run Python syntax and applicable local checks; report missing native/browser
  tooling as unverified.
- [ ] Publish a draft PR; run the unchanged mock and full live HTTP browser gates
  on its exact head, retain the original failure, and obtain fresh review.

The original Windows incident #1129 is separate and remains unexplained while
its job logs are unavailable. This repair makes no claim about that incident.
