# AGENTS.md — Loomward

The one baseline for every agent runtime (Claude Code reads this file because the repo has no
`CLAUDE.md`; Codex, Grok and Muse read it natively). Do not add a `CLAUDE.md` beside it: that
would stop Claude reading this file.

**T2 daily driver** · push free / merge free · tier and flags: `.agent-harness/tier.json` · delegation
ceilings: `.agent-harness/delegation.json` · human decisions: `HUMAN_TODO.md`. Global estate laws
come from each runtime's global instructions; this file holds only what is true of *this* repo.

## What this is

Loomward is an open-source, local-first Windows workspace, storage and resource companion:
native inventory (a far faster WinDirStat), personal learning (a small student model, with an
optional LLM teacher as weak supervision), virtual organisation, disk-tier balancing,
duplicate/unused-file inspection, backups, and understandable process/RAM control. Its core
ruling: **meaning, residency and permission are separate systems.**

Python reference app (a frozen test oracle since q-40; it changes only to keep cross-language fixtures aligned), Rust native workspace (first compiled and
tested here on 2026-10-09; the Tauri 2 desktop shell first built on 2026-10-10, #164), MCP read-only reference, 130-task
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

## Proving checks by seam (measured on Windows 2026-10-09; counts refreshed 2026-10-11, `evidence/v3/`)

| You changed | Run |
|---|---|
| Python reference (frozen oracle), schemas, fixtures | `py -3 scripts/verify.py` (279 tests (1 skip), including the 80 admission-parity fixtures; it also runs the Rust gates when cargo is present) |
| Rust workspace (`crates/*`, glob) | `cargo fmt --all --check`, `cargo test --workspace` (1,442 on Windows at 4b05fd6, 2026-10-17; per-crate split in docs/16 wave 9 is historical), `cargo clippy --workspace --all-targets -- -D warnings`; add `--all-features` for the `loomward-windows` fixture lab |
| Svelte app (`app/`) | `npm.cmd --prefix app run check`, `run test`, `run build`, then `py -3 scripts/test_app.py` (Playwright e2e incl. Atlas and Observatory), or all four via `py -3 scripts/verify.py --app`; add `--live-serve` when you touch `loomward-http` or an app transport |
| Python, Rust and the Svelte app in one go | `py -3 scripts/verify.py --app` (`--app` reports UNVERIFIED, never PASS, when Node, npm or `app/node_modules` is missing) |
| Local loop on a branch (Rust/Python only) | `py -3 scripts/verify.py --changed` tests and lints only the crates the diff can affect and says so; it is a shortcut, never the proof of record — hosted CI runs the whole workspace |
| Tauri shell (`native/`) | excluded from the root workspace: stage the Ask sidecar with `py -3 scripts/build_broker.py`, then `cargo build`, `cargo test` (59 + 1 ignored on 2026-10-17) and `cargo clippy --all-targets -- -D warnings` with `--manifest-path native/Cargo.toml`, then `native/target/debug/loomward-desktop.exe --self-test`; `native/tests/webview2_probe.py` drives the real window |
| Docs only | `git diff --check` plus the links you touched |

Missing tooling is UNVERIFIED, never PASS. New behaviour lands with the test that pins it. Never
disable, skip or weaken a failing test to advance a milestone.

## Product invariants (violating one is a bug, not a style choice)

1. **No file or process effects** (move, delete, uninstall, kill, suspend, priority, trim) until
   the specific `LW-*` issue and its Windows safety gate authorise that capability. Never add a
   general execute, shell, delete or kill endpoint, through HTTP, IPC or MCP.
   The one write that can touch an owner file is the q-15 diagnostic bundle (LW-055, ADR-V3-24): only
   the previewed, redacted bytes, to a new file or an owner-confirmed overwrite chosen in the Rust-side
   save dialog; the path never crosses IPC. It is not a general write capability. Copy-and-verify (LW-124,
   docs/47) writes, renames and (only when enabled) removes its own staging, but only for synthetic sessions on
   registered lab roots (#522). Personal sessions are refused in code (`copy_effect_gated`); agents never open
   personal copying: gate C8 needs C6, G5, G9, the owner's Q1-Q3 answers and the owner's own decision.
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
   normal (T2); unsigned preview pre-releases and a rolling nightly pre-release are owner-authorised (HUMAN_TODO q-17, q-31, q-33),
   with their bookkeeping releases `nightly-previous` (the previous nightly, kept for manual rollback) and
   `preview-latest` (update metadata only); any other release
   or package is a tier re-review trigger.
9. **No false claims.** Never blur the Python reference and the native engine in a report, and
   never claim native compilation, backup verification, memory optimisation, learned heat, live
   disk balancing or safe deletion without that capability's own gate evidence.

**Owner posture (2026-10-11).** The owner's data is not high-sensitivity: weigh privacy and security findings lower,
skip extra security lenses and hardware withholding. Still forbidden because irreversible: secrets in Git and real file
names, paths or inventories in this public repository. Owner decisions of that day are in HUMAN_TODO q-5, q-9 to q-19;
those of 12 October (licence, thesis, channels, nightly, Find & Ask, readiness bar) are q-20 to q-46.

## Working the product (after unbundling)

- Before editing, read `handoff/START-HERE.md`, `handoff/CHECKPOINT.json`,
  `docs/23-expansion-overview.md`, `docs/37-expansion-verification.md`, the relevant spec and the
  assigned `LW-*` issue. Product facts there are current until live evidence says otherwise.
- One bounded issue at a time: regression test first, then the change, then the full applicable
  suite. The session driver updates the status and checkpoint files
  (`docs/16-implementation-status.md`, `handoff/CHECKPOINT.json`) once per wave in a checkpoint PR,
  without erasing history; feature PRs do not edit them, except that a PR which opens or closes a
  safety gate or changes a product invariant updates both files itself, so they never state a stale gate.
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
  they are mirrored as issues #4-#105, #120-#131 and #404-#421 (`backlog/github-issues.json` maps them). New IDs mint there first.
- Licence: AGPL-3.0-only since #396; the owner may also license it commercially (HUMAN_TODO q-20 to q-23).
- Taskdeck is proprietary: never copy its code into this repository.
- Public repo: synthetic fixtures only; no real filenames, inventories or training exports.
  Personal diagnostics and `.loomward/` state or database files never enter Git.
- The UI is not frozen. The owner wants a signature, elegant interface that grows with the
  engine; the legacy `ui/` pages were retired in q-40 and the Svelte app is the one interface.
