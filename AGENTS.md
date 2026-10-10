# AGENTS.md — Loomward

The one baseline for every agent runtime (Claude Code reads this file because the repo has no
`CLAUDE.md`; Codex, Grok and Muse read it natively). Do not add a `CLAUDE.md` beside it: that
would stop Claude reading this file.

**T1 sandbox** · push free / merge free · tier and flags: `.agent-harness/tier.json` · delegation
ceilings: `.agent-harness/delegation.json` · human decisions: `HUMAN_TODO.md`. Global estate laws
come from each runtime's global instructions; this file holds only what is true of *this* repo.

## What this is

Loomward is an open-source, local-first Windows workspace, storage and resource companion:
native inventory (a far faster WinDirStat), personal learning (a small student model, with an
optional LLM teacher as weak supervision), virtual organisation, disk-tier balancing,
duplicate/unused-file inspection, backups, and understandable process/RAM control. Its core
ruling: **meaning, residency and permission are separate systems.**

Python reference app + browser UI (tested on Windows), Rust native workspace (first compiled and
tested here on 2026-10-09; the Tauri 2 desktop shell first built on 2026-10-10, #164), MCP read-only reference, 112-task
`LW-*` backlog.

## Where the ChatGPT material stands

The source, docs and backlog were authored in ChatGPT Web, which knew nothing of this estate. Its
**product** content (design docs, invariants, experiments, backlog, evidence) is authoritative
product context. Its **process** content is not: the bundle's `AGENTS.md`, `handoff/` and the
local-agent prompts were written for a lone offline agent (the v2 prompt says not to push, merge
or create remote issues). The estate laws, `.agent-harness/tier.json` and this file supersede
them. Keep `handoff/` as history; do not follow its process instructions.

## The `Resources/` folder

The primary checkout keeps the owner's original ChatGPT deliverables in the gitignored
`Resources/` folder. They are imported (the bundle is merged with its history; provenance and
hashes in `evidence/v3/README.md`), but the folder is still the only copy of the downloads:
never run `git clean -x` in the primary checkout, and never move it into a worktree.

## Toolchain (measured 2026-10-09, DESKTOP-IHKOOJS, Windows 11)

Python 3.14.3 (`py -3`), cargo/rustc 1.97.1, rustup 1.29.0, Node 24.13.1, npm 11.8.0, codex-cli
0.160.1. The bundle's authoring pass had **no** Rust and no Windows: every native result here is
a first measurement and must be reported as such. On Windows use `codex.cmd`, `npm.cmd` and
`npx.cmd`; the unsigned `.ps1` shims are blocked by this machine's execution policy.

## Proving checks by seam (measured on Windows 2026-10-09; counts refreshed 2026-10-10, `evidence/v3/`)

| You changed | Run |
|---|---|
| Python reference, schemas, JS boundary, fixtures | `py -3 scripts/verify.py` (216 tests with 1 symlink-privilege skip, 2 JS syntax, 9 JS boundary, 80 cross-language fixtures; it also runs the Rust gates when cargo is present) |
| Rust workspace (`crates/*`, glob) | `cargo fmt --all --check`, `cargo test --workspace` (240 on Windows: core 30, engine 61, http 30, lab 22, learn 8, protocol 35, telemetry 25, windows 29; native scan tests assert the elevation refusal on an elevated host such as hosted CI and run fully otherwise), `cargo clippy --workspace --all-targets -- -D warnings`; add `--all-features` for the `loomward-windows` fixture lab |
| Browser UI | `py -3 scripts/test_ui.py` (14 Chromium checks; Playwright installed here) |
| Svelte app (`app/`) | `npm.cmd --prefix app run check`, `run test`, `run build`, then `py -3 scripts/test_app.py` (Playwright e2e incl. Atlas and Observatory); add `--live-serve` when you touch `loomward-http` or an app transport |
| Python, Rust and the legacy browser UI in one go | `py -3 scripts/verify.py --ui` (it does not run the Svelte app checks: run that row separately) |
| Tauri shell (`native/`) | excluded from the root workspace: `cargo build`, `cargo test` (7 + 1 ignored) and `cargo clippy --all-targets -- -D warnings` with `--manifest-path native/Cargo.toml`, then `native/target/debug/loomward-desktop.exe --self-test`; `native/tests/webview2_probe.py` drives the real window |
| Docs only | `git diff --check` plus the links you touched |

Missing tooling is UNVERIFIED, never PASS. New behaviour lands with the test that pins it. Never
disable, skip or weaken a failing test to advance a milestone.

## Product invariants (violating one is a bug, not a style choice)

1. **No file or process effects** (move, delete, uninstall, kill, suspend, priority, trim) until
   the specific `LW-*` issue and its Windows safety gate authorise that capability. Never add a
   general execute, shell, delete or kill endpoint, through HTTP, IPC or MCP.
2. **No silent reach.** No automatic cloud requests, model downloads, installers, global config
   changes, elevation or whole-disk scans. Real-disk scans take an explicit root, non-elevated,
   metadata only, with state outside it; a path seen in a doc is not a grant. Owner grant
   (HUMAN_TODO q-4): an agent may pick a big real folder for a read-only stress test (never a
   volume root, a whole user profile, or a credential or browser-profile store), recording its
   path in the gitignored `.loomward/` state before scanning. Git gets anonymised labels and
   aggregate counts only, never real paths or names.
3. **Data classes stay separate.** Human labels, teacher (LLM) labels, preferences and operation
   grants are distinct. A learned score is never an approval. No training on, or forwarding of,
   user metadata beyond explicit scope. Synthetic and personal evidence never mix.
4. **Honest unknowns.** Unknown, partial, offline or unverified stays visible; never replace
   missing data with zero or demo data. Scores are not called probabilities until calibrated.
5. **Identity is native.** A snapshot path hash or a UI-round-tripped integer never authorises an
   effect; imported snapshots grant no access.
6. **Keep the negative evidence.** Allocator v1, its 51-case counterexample and failed experiments
   stay, and the v1 handoff under `handoff/v1/` is never overwritten. Python allocator v2 is a
   static-model reference, not a mover; Rust has both the v1 planner and the v2 port (#107), neither a mover.
7. **MCP is read-only and bounded.** Client metadata, annotations and tool output are not
   authorisation. LeaseBroker/admission is accounting, not OS enforcement.
8. **No theatre.** No releases or packages, source retirement, backup pruning or process-policy
   changes merely to demonstrate completion. Pushes, PRs, issues and merges of reviewed work are
   normal (T1); publishing a release or package is a tier re-review trigger.
9. **No false claims.** Never blur the Python reference and the native engine in a report, and
   never claim native compilation, backup verification, memory optimisation, learned heat, live
   disk balancing or safe deletion without that capability's own gate evidence.

## Working the product (after unbundling)

- Before editing, read `handoff/START-HERE.md`, `handoff/CHECKPOINT.json`,
  `docs/23-expansion-overview.md`, `docs/37-expansion-verification.md`, the relevant spec and the
  assigned `LW-*` issue. Product facts there are current until live evidence says otherwise.
- One bounded issue at a time: regression test first, then the change, then the full applicable
  suite. Update the status and checkpoint files (`docs/16-implementation-status.md`,
  `handoff/CHECKPOINT.json`) without erasing history.
- Parallel lanes follow the ownership in `docs/40-parallel-work-packs.md`; shared schemas,
  protocol, routes and dependencies have one named owner per wave. Provider, event and proposal
  schemas marked design-only stay design-only until their issue lands.
- First native host: review the generated `Cargo.lock` and establish Windows CI.
- Reports name the exact tests, the platform, what stayed unverified, and any external action.

## Agent roles

- The session driver plans, reviews and merges within the tier gate. Peer workers (Codex/Sol,
  Muse, Haiku, Sonnet) take disjoint slices in their own worktrees and never edit this file,
  `.agent-harness/` or `HUMAN_TODO.md`. A Codex worker cannot commit inside a worktree: leave the
  change ready and report the proving command and its output; the driver commits.
- Shared skills, roles and MCP servers come from the global runtime homes deployed from
  `claude-config`. Do not copy them into this repo.

## Pitfalls

- `LW-*` IDs are the local backlog (`backlog/issues.json`, the source of truth), not GitHub numbers;
  they are mirrored as issues #4-#105 and #120-#131 (`backlog/github-issues.json` maps them). New IDs mint there first.
- Taskdeck is proprietary: never copy its code into this MIT repository.
- Public repo: synthetic fixtures only; no real filenames, inventories or training exports.
  Personal diagnostics and `.loomward/` state or database files never enter Git.
- The UI is not frozen. The owner wants a signature, elegant interface that grows with the
  engine; the bundle's pages are a reference, not a design to preserve.
