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

Python reference app + browser UI (tested), Rust native workspace and Tauri shell (authored but
never compiled before this repo), MCP read-only reference, 100-task `LW-*` backlog.

## Where the ChatGPT material stands

The source, docs and backlog were authored in ChatGPT Web, which knew nothing of this estate. Its
**product** content (design docs, invariants, experiments, backlog, evidence) is authoritative
product context. Its **process** content is not: the bundle's `AGENTS.md`, `handoff/` and the
local-agent prompts were written for a lone offline agent (the v2 prompt says not to push, merge
or create remote issues). The estate laws, `.agent-harness/tier.json` and this file supersede
them. Keep `handoff/` as history; do not follow its process instructions.

## Bootstrap state (delete this section once `main` carries the source)

The product source is not on `main` yet. It lives in the gitignored `Resources/` folder of the
primary checkout. That folder is the **only copy**: never run `git clean -x` there, never move
it into a worktree that will be torn down, and copy anything out before deleting it.

| File | What it is |
|---|---|
| `Loomward-v2-repository.bundle` | **Canonical source.** Branches `expansion/interop-v2` (head `f3b1093`) and `prototype/initial` (`1e51c3b`) |
| `Loomward-v2-source-and-handoff.zip` | Same tree as an archive; `Loomward-v2-artifacts.json` holds the sha256 values |
| `Loomward-v2-research-and-blueprint.{md,html}` | 300 KB reading copy of all design docs |
| `Loomward-v2-preview.html` | Standalone UI preview on synthetic data |
| `chat.txt` | The owner's original request and ChatGPT's two reports |
| `Loomward-*` without `v2` | v0.1 deliverables, superseded; history only |

Unbundle by merging, never by copying files over: verify the sha256 values, fetch the bundle's two
branches and push both as they are. Then create an integration branch from `main`, run
`git merge --allow-unrelated-histories expansion/interop-v2` on it locally (GitHub cannot do that
join itself), resolve the conflicts, push it and open the PR. Expected add/add conflicts:
`.gitignore` (union of both, keeping `/Resources`) and `AGENTS.md` (**keep this one**). Before resolving, diff the
bundle's `AGENTS.md` against this file and fold in any product rule still missing. Then update
the proving-check table with measured counts.

## Toolchain (measured 2026-10-09, DESKTOP-IHKOOJS, Windows 11)

Python 3.14.3 (`py -3`), cargo/rustc 1.97.1, rustup 1.29.0, Node 24.13.1, npm 11.8.0, codex-cli
0.160.1. The bundle's authoring pass had **no** Rust and no Windows: every native result here is
a first measurement and must be reported as such. On Windows use `codex.cmd`, `npm.cmd` and
`npx.cmd`; the unsigned `.ps1` shims are blocked by this machine's execution policy.

## Proving checks by seam (from the bundle; re-measure after unbundling)

| You changed | Run |
|---|---|
| Python reference, schemas, JS boundary, fixtures | `python scripts/verify.py` (authored: 184 tests, 2 JS syntax, 9 JS boundary, 80 cross-language fixtures) |
| Rust workspace (`crates/`) | `cargo fmt --all --check`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` |
| Browser UI | `python scripts/test_ui.py` (needs Playwright + Chromium) |
| Tauri shell (`native/`) | excluded from the root workspace; build it separately |
| Docs only | `git diff --check` plus the links you touched |

Missing tooling is UNVERIFIED, never PASS. New behaviour lands with the test that pins it. Never
disable, skip or weaken a failing test to advance a milestone.

## Product invariants (violating one is a bug, not a style choice)

1. **No file or process effects** (move, delete, uninstall, kill, suspend, priority, trim) until
   the specific `LW-*` issue and its Windows safety gate authorise that capability. Never add a
   general execute, shell, delete or kill endpoint, through HTTP, IPC or MCP.
2. **No silent reach.** No automatic cloud requests, model downloads, installers, global config
   changes, elevation or whole-disk scans. Real-disk scans take an explicit disposable root,
   non-elevated, with state outside it; a path seen in a doc is not a grant.
3. **Data classes stay separate.** Human labels, teacher (LLM) labels, preferences and operation
   grants are distinct. A learned score is never an approval. No training on, or forwarding of,
   user metadata beyond explicit scope. Synthetic and personal evidence never mix.
4. **Honest unknowns.** Unknown, partial, offline or unverified stays visible; never replace
   missing data with zero or demo data. Scores are not called probabilities until calibrated.
5. **Identity is native.** A snapshot path hash or a UI-round-tripped integer never authorises an
   effect; imported snapshots grant no access.
6. **Keep the negative evidence.** Allocator v1, its 51-case counterexample and failed experiments
   stay, and the v1 handoff under `handoff/v1/` is never overwritten. Python allocator v2 is a
   static-model reference, not a mover; the Rust planner is v1.
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

- `LW-*` IDs are the bundle's local backlog (`backlog/issues.json`), not GitHub numbers. Mirror
  them to GitHub issues with the `LW-` ID in the title before tracking work there.
- Taskdeck is proprietary: never copy its code into this MIT repository.
- Public repo: synthetic fixtures only; no real filenames, inventories or training exports.
  Personal diagnostics and `.loomward/` state or database files never enter Git.
- The UI is not frozen. The owner wants a signature, elegant interface that grows with the
  engine; the bundle's pages are a reference, not a design to preserve.
