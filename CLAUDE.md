# CLAUDE.md — Loomward

**T1 sandbox** · push free / merge free · runtimes Claude + Codex (+ Muse, Grok as delegates) ·
tier and flags: `.agent-harness/tier.json` · delegation ceilings: `.agent-harness/delegation.json` ·
human decisions: `HUMAN_TODO.md`. Global laws come from `~/.claude/rules/laws.md`; this file holds
only what is true of *this* repo.

@AGENTS.md

## What this is

Loomward is an open-source, local-first Windows workspace, storage and resource companion:
native inventory (a far faster WinDirStat), personal learning (a small student model, with an
optional LLM teacher as weak supervision), virtual organisation, disk-tier balancing,
duplicate/unused-file inspection, backups, and understandable process/RAM control. Its core
ruling: **meaning, residency and permission are separate systems.** A learned score is never an
approval.

Python reference app + browser UI (tested), Rust native workspace and Tauri shell (authored but
never compiled before this repo), MCP read-only reference, 100-task `LW-*` backlog.

## Bootstrap state (until the ChatGPT deliverables are unbundled onto `main`)

The product source is not on `main` yet. It lives in the gitignored `Resources/` folder:

| File | What it is |
|---|---|
| `Loomward-v2-repository.bundle` | **Canonical source.** Branches `expansion/interop-v2` (head `f3b1093`) and `prototype/initial` (`1e51c3b`) |
| `Loomward-v2-source-and-handoff.zip` | Same tree as an archive; `Loomward-v2-artifacts.json` holds its sha256 |
| `Loomward-v2-research-and-blueprint.{md,html}` | 300 KB reading copy of all design docs |
| `Loomward-v2-preview.html` | Standalone UI preview on synthetic data |
| `Loomward-v2-LOCAL-AGENT-PROMPT.md` | ChatGPT's launch prompt for a local agent |
| `chat.txt` | The owner's original request and ChatGPT's two reports |
| `Loomward-*` without `v2` | v0.1 deliverables, superseded; history only |

Unbundle by merging, never by copying files over: fetch the bundle's two branches, verify the
sha256 values in `Loomward-v2-artifacts.json`, push both branches, then merge
`expansion/interop-v2` into `main` with `--allow-unrelated-histories` through a PR. Expected
add/add conflict: `.gitignore` (keep the bundle's lines plus `/Resources`). The bundle's
`AGENTS.md` becomes the product contract this file imports; do not replace it with an estate
adapter. Delete this section once `main` carries the source.

## Toolchain (measured 2026-10-09, DESKTOP-IHKOOJS, Windows 11)

Python 3.14.3 (`py -3`), cargo/rustc 1.97.1, rustup 1.29.0, Node 24.13.1, npm 11.8.0, codex-cli
0.160.1. The bundle's authoring pass had **no** Rust and no Windows: every native result here is
a first measurement and must be reported as such.

## Proving checks by seam (from the bundle's own contract; re-measure after unbundling)

| You changed | Run |
|---|---|
| Python reference, schemas, JS boundary, fixtures | `python scripts/verify.py` (authored: 184 tests, 2 JS syntax, 9 JS boundary, 80 cross-language fixtures) |
| Rust workspace (`crates/`) | `cargo fmt --all --check`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` |
| Browser UI | `python scripts/test_ui.py` (needs Playwright + Chromium) |
| Tauri shell (`native/`) | excluded from the root workspace; build it separately |
| Docs only | `git diff --check` plus links you touched |

Missing tooling is UNVERIFIED, never PASS.

## Pitfalls

- `LW-*` IDs are the bundle's local backlog (`backlog/issues.json`), not GitHub issue numbers.
  Mirror them to GitHub issues before tracking work there, keeping the `LW-` ID in the title.
- The Python allocator v2 is a static-model reference, not a mover; keep v1 and its 51-case
  counterexample. The Rust planner is still v1.
- Real-disk scans use an explicit disposable root, non-elevated, with state outside the root.
  A path seen in a doc is not a grant. No whole-disk scans, deletions, moves or process kills
  until the specific `LW-*` safety gate says so.
- Taskdeck is proprietary: never copy its code into this MIT repository.
- Public repo: synthetic fixtures only; no real filenames, inventories or training exports.
