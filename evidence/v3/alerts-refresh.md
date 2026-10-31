# Manual advisory alert refresh

Mission60 adds **Refresh checks** to Alerts using the existing read-only `alerts.list` command. It reevaluates recorded observations; it does not start a scan, change saved thresholds, enable notifications or act on files. The form remains an unsaved draft until Save thresholds succeeds. A failed read clears the old report and leaves an explicit retry control; the draft survives recovery.

Base: `5ec82d2e9f6ab08462b270b33a9e619b1a4875a2`. The independent slice has no PR1118 collection-curation changes; that parked branch and its exhausted proof budget remain separate.

## Direct evidence

- Baseline: 54 Alerts/race tests passed with one concurrent worker.
- The first regression ran the existing view script and sought the actual Refresh checks button handler. It failed because that control was absent; the initial report load succeeded first.
- After implementation, 65 focused tests passed. They cover the actual handler, the full draft versus persisted report settings, repeated announcement requests, one pending read, mutual exclusion with save, failure and explicit recovery, and late success/error/finally after invalidation, client/connection change, disconnect or unmount.
- Python AST checks for both browser scripts, `git diff --check` and the Impeccable detector passed. Local typecheck was deferred because measured RAM did not leave the physical reserve plus its small-check allowance; the hosted app gate must provide that proof.
- Local command from `app`: `npm.cmd test -- --maxWorkers=1 tests/unit/alerts.test.ts tests/unit/view-races.test.ts`.
- The existing mock browser journey now exercises keyboard refresh, full draft retention, unchanged report thresholds, focus and repeated announcements in the same live region. The existing live HTTP journey observes actual request counts, confirms unchanged saved settings/state revision, covers explicit failure/recovery and captures 390/1360px states.

## Pending gate and boundaries

The authored browser journeys are not passing evidence until the existing hosted full gate executes them. Typecheck, syntax, independent review and exact-head hosted results will be recorded in the PR and sealed delivery. No local substantial/GPU proof allocation was used. Physical-device and assistive-technology acceptance, personal-data/provider workflows and actual file effects remain separately bounded by `HUMAN_TODO.md`; no owner UX acceptance is inferred. LW-121's broader monitoring/notification scope stays open.
