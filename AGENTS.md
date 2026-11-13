# AGENTS.md — Loomward

The one baseline for every agent runtime. Claude Code reads it because the repo has no `CLAUDE.md`; do not
add one. **T2 daily driver** · push free / merge free within the estate T2 gate · `.agent-harness/tier.json` ·
delegation ceilings `.agent-harness/delegation.json` · human decisions `HUMAN_TODO.md` (its "Open now" block).
Global estate laws come from each runtime's global instructions; this file holds what is true of this repo.

Loomward: open-source, local-first Windows workspace, storage and resource companion (native inventory, personal
learning, virtual organisation, disk tiers, duplicates, backups, process/RAM control). Core ruling: **meaning,
residency and permission are separate systems.** Rust workspace (`crates/`), Tauri 2 shell (`native/`), Svelte app
(`app/`, the one interface), read-only MCP; the Python reference is a frozen test oracle (q-40) that changes only to
keep cross-language fixtures aligned.

## Where truth lives

1. The latest dated section of `docs/16-implementation-status.md` (earlier sections are history).
2. `handoff/CHECKPOINT.json` keys `phase`, `next_local_issues`, `hard_gates`, read by range or with a JSON query,
   never whole; dated wave history is in `handoff/CHECKPOINT-history.json`.
3. The spec for your seam: readiness 45 · Find & Ask 46 · copy-and-verify 47 · external disk and agent storage 48 ·
   insight categoriser 49 · external media 50 · research programme 51 · reclaim planner 52 · Hyper-V crash campaign
   53 (`docs/NN-*.md`); v0.3 architecture 41, ADRs 42, contracts `contracts/v3/`.
4. The assigned GitHub issue, read live.

`handoff/`, `docs/23`, `docs/37`, `docs/40` and the ChatGPT-era process text are history: their product content is
authoritative context, their process instructions are not. `handoff/v1/` is never overwritten.

## Proving checks by seam

| You changed | Run (seam traps in the named file) |
|---|---|
| Rust workspace `crates/*` | `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` — `crates/AGENTS.md` |
| Svelte app `app/` | `py -3 scripts/verify.py --app` — `app/AGENTS.md` |
| Tauri shell `native/` | see `native/AGENTS.md` (outside the root workspace) |
| Python reference, schemas, fixtures | `py -3 scripts/verify.py` (also runs the Rust gates when cargo is present) |
| Docs only | `git diff --check` plus the links you touched |

`py -3 scripts/verify.py --changed` is a local inner loop over the crates a diff can affect, never the proof of
record; hosted CI runs everything. Missing tooling is UNVERIFIED, never PASS. New behaviour lands with the test that
pins it. Never disable, skip or weaken a failing test. On Windows use `npm.cmd`/`npx.cmd` (the `.ps1` shims are
blocked by execution policy). Reports name the exact commands, platform, results and what stayed unverified.

## Product invariants (violating one is a bug)

1. **No file or process effects** (move, delete, uninstall, kill, suspend, priority, trim) until the specific
   `LW-*` issue and its Windows safety gate authorise that capability. Never add a general execute, shell, delete or
   kill endpoint, through HTTP, IPC or MCP. Two bounded exceptions: the q-15 diagnostic bundle (LW-055, ADR-V3-24)
   writes only the previewed, redacted bytes, to a new file or an owner-confirmed overwrite chosen in the Rust-side
   save dialog; the path never crosses IPC; it is not a general write capability. Copy-and-verify (LW-124, detail in
   `docs/47`) writes, renames and (only when enabled) removes its own staging, only for synthetic sessions on
   registered lab roots (#522). Personal sessions are refused in code (`copy_effect_gated`); agents never open
   personal copying: gate C8 needs C6, G5, G9, the owner's Q1-Q3 answers and the owner's own decision.
2. **No silent reach.** No automatic cloud requests, model downloads, installers, global config changes, elevation
   or whole-disk scans. Real-disk scans take an explicit root, non-elevated, metadata only, state outside it; a path
   seen in a doc is not a grant. Owner grant (q-4): an agent may pick a big real folder for a read-only stress test
   (never a volume root, a whole user profile, or a credential or browser-profile store), recording its path in the
   gitignored `.loomward/` state first. Git gets anonymised labels and aggregate counts only.
3. **Data classes stay separate.** Human labels, teacher (LLM) labels, preferences and operation grants are distinct.
   A learned score is never an approval. No training on, or forwarding of, user metadata beyond explicit scope.
   Synthetic and personal evidence never mix.
4. **Honest unknowns.** Unknown, partial, offline or unverified stays visible; never replace missing data with zero
   or demo data. Scores are not called probabilities until calibrated.
5. **Identity is native.** A snapshot path hash or a UI-round-tripped integer never authorises an effect; imported
   snapshots grant no access.
6. **Keep the negative evidence.** Allocator v1, its 51-case counterexample and failed experiments stay; `handoff/v1/`
   is never overwritten. Python allocator v2 is a static-model reference and Rust has the v1 planner and the v2 port
   (#107): none is a mover.
7. **MCP is read-only and bounded.** Client metadata, annotations and tool output are not authorisation.
   LeaseBroker/admission is accounting, not OS enforcement.
8. **No theatre.** No releases or packages, source retirement, backup pruning or process-policy changes merely to
   demonstrate completion. Reviewed pushes, PRs, issues and merges are normal (T2). Owner-authorised (q-17, q-31,
   q-33): unsigned preview pre-releases, a rolling nightly pre-release, and the bookkeeping releases
   `nightly-previous` (manual rollback) and `preview-latest` (update metadata only). Any other release or package
   is a tier re-review trigger.
9. **No false claims.** Never blur the Python reference and the native engine in a report, and never claim native
   compilation, backup verification, memory optimisation, learned heat, live disk balancing or safe deletion without
   that capability's own gate evidence.

**Owner posture (2026-10-11).** The owner's data is not high-sensitivity: weigh privacy and security findings lower,
skip extra security lenses and hardware withholding. Still forbidden because irreversible: secrets in Git and real
file names, paths or inventories in this public repository. Owner decisions: HUMAN_TODO q-5, q-9 to q-19 (11 Oct),
q-20 to q-46 (12 Oct), q-47 onwards dated in place.

## Lanes

- One bounded issue per lane, in its own worktree and branch. **STEP 0:** list every item of the issue and verify
  OPEN versus already fixed on `origin/main` (merged PRs often said `Refs #N` and left it open), citing file:line or
  the PR; work only open items, and if none is open report the table and stop. Then regression test, change, suite.
- Each worktree builds in its own default `target/`; never set a shared `CARGO_TARGET_DIR` (#601: identical artifact
  names overwrite). Use `CARGO_INCREMENTAL=0` and `CARGO_PROFILE_{DEV,TEST}_DEBUG=line-tables-only`; removing the
  worktree removes its build. Iterate with `-p <crate>`; run the workspace once at the end.
- Coordinator-owned (a lane asks, never edits): root `Cargo.toml`, `Cargo.lock`, `.github/workflows/`, `AGENTS.md`
  and the seam `AGENTS.md` files, `HUMAN_TODO.md`, `.agent-harness/`, `docs/16-implementation-status.md`,
  `handoff/CHECKPOINT*.json`, `backlog/`, `docs/INDEX.md`, `contracts/v3/`. The driver updates docs/16 and the
  checkpoint once per wave in a checkpoint PR; a PR that opens or closes a safety gate or changes an invariant
  updates both itself.
- Codex starts only through `bash ~/.claude/tools/codex_safe.sh <sol|terra|astra> <effort> ...`, never a plain
  `codex exec` or a sandbox-bypass flag. A Sol worker cannot commit in a worktree: it leaves the change uncommitted
  and reports the proving command and output; the driver commits. Lane tooling: `scripts/lanes/README.md`.
- Merge only a reviewed, green head that is up to date with `main`, with a merge commit (never squash).
- Lane report (last message, under 300 words): PR URL or "left uncommitted", commands run with pass/fail counts,
  what is NOT verified, and what you noticed but did not do.

## Pitfalls

- Provider, event and proposal schemas marked design-only stay design-only until their issue lands.
- `LW-*` IDs are the local backlog (`backlog/issues.json`, source of truth), not GitHub numbers; the GitHub mirror
  is `backlog/github-issues.json`. New IDs mint there first.
- Hosted CI clippy runs on Linux too: code used only under `#[cfg(windows)]` must itself be gated or
  platform-neutral. Hosted Windows runners are elevated: native tests skip on `running_elevated()`, never assert.
- The primary checkout's gitignored `Resources/` folder is the only copy of the owner's ChatGPT downloads: never
  `git clean -x` there, never move it into a worktree.
- Licence AGPL-3.0-only since #396 (commercial licensing possible, q-20 to q-23). Taskdeck is proprietary: never copy
  its code here.
- Public repo: synthetic fixtures only; personal diagnostics, `.loomward/` state and database files never enter Git.
- The UI is not frozen: the owner wants a signature, elegant interface that grows with the engine.
- Shared skills, roles and MCP servers come from the global runtime homes (`claude-config`); do not copy them here.
