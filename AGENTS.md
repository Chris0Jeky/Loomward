# Agent operating contract

Read `handoff/START-HERE.md`, `handoff/CHECKPOINT.json`, `docs/37-expansion-verification.md`, `docs/23-expansion-overview.md`, the relevant spec and the assigned backlog issue before editing. The repository contains a working Python reference and **uncompiled native sources**. Never blur that distinction in reports.

## Non-negotiable invariants
- No file/process mutations until an explicit later issue and Windows safety gate authorise implementing that capability. Do not add a general execute, shell, delete or kill endpoint.
- No automatic cloud requests, model downloads, system installers, global configuration changes, elevation, or whole-disk scans.
- No training on user data or teacher metadata requests beyond the user's explicit scope. Keep synthetic and personal evidence separate.
- Unknown/partial/offline/unverified states remain visible. Do not replace missing data with zero or demo data.
- A learned score is not an approval. Human labels, teacher labels, preferences and operation grants are separate data classes.
- A snapshot path hash is not a native execution identity. Do not authorize effects using UI-round-tripped integers or imported snapshots.
- No public publishing, source retirement, backup pruning or process-policy changes merely to demonstrate completion.

## Development workflow
Work on a branch. Implement one bounded issue at a time, write the regression first, execute it, then implement and run the complete applicable suite. Preserve exact evidence and update status/checkpoint files. Keep ownership of shared schema/protocol changes explicit when working in parallel. Do not disable failing tests to advance a milestone.

Run `python scripts/verify.py`. Missing cargo is UNVERIFIED, not PASS. On the first native host, run `cargo fmt --all`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, resolve/review lockfiles and establish Windows CI. The authoring pass did not run these commands successfully. Browser checks use `scripts/test_ui.py` and need Playwright/Chromium.

Use disposable fixtures for native filesystem experiments. Keep personal diagnostics/database files out of Git. Do not publish training exports or real filenames in issues. Treat `backlog/issues.json` as the local issue manifest; LW IDs are not GitHub issue numbers.

## Completion reporting
State what changed, exact tests and platform, what remains unverified, and whether any external action occurred. Preserve failed experiments that explain a design ruling. No false claims of native compilation, backup verification, memory optimisation, learned heat, live disk balancing or safe deletion. Those are future work unless their specific gates have new evidence.

## v0.2 continuation
Use `docs/40-parallel-work-packs.md` for lane ownership. The native Rust foundation remains uncompiled and its planner is the v1 baseline. New snapshot/MCP/scheduler modules are reference implementations, not OS authority. New provider/event/proposal schemas are design-only where marked. Do not overwrite the historical handoff under `handoff/v1/`.
