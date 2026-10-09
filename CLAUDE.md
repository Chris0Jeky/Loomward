# CLAUDE.md — Loomward

**T1 sandbox** · push free / merge free · runtimes Claude + Codex (Muse and Grok as delegates) ·
tier and flags: `.agent-harness/tier.json` · delegation ceilings: `.agent-harness/delegation.json` ·
human decisions: `HUMAN_TODO.md`. Global laws come from `~/.claude/rules/laws.md`; this file is the
canon for every runtime and holds only what is true of *this* repo. `AGENTS.md` is a thin Codex
adapter.

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
product context. Its **process** content is not: `handoff/*`, `LOCAL-AGENT-PROMPT.md` and the
bundle's own `AGENTS.md` say "do not push, merge or create remote issues" and "review inline".
Those are superseded by the global laws, `.agent-harness/tier.json` and this file. Keep
`handoff/` as history; do not follow its process instructions.

## Bootstrap state (delete this section once `main` carries the source)

The product source is not on `main` yet. It lives in the gitignored `Resources/` folder:

| File | What it is |
|---|---|
| `Loomward-v2-repository.bundle` | **Canonical source.** Branches `expansion/interop-v2` (head `f3b1093`) and `prototype/initial` (`1e51c3b`) |
| `Loomward-v2-source-and-handoff.zip` | Same tree as an archive; `Loomward-v2-artifacts.json` holds the sha256 values |
| `Loomward-v2-research-and-blueprint.{md,html}` | 300 KB reading copy of all design docs |
| `Loomward-v2-preview.html` | Standalone UI preview on synthetic data |
| `chat.txt` | The owner's original request and ChatGPT's two reports |
| `Loomward-*` without `v2` | v0.1 deliverables, superseded; history only |

Unbundle by merging, never by copying files over: verify the sha256 values, fetch the bundle's two
branches, push both, then merge `expansion/interop-v2` into `main` with
`--allow-unrelated-histories` through a PR. Expected add/add conflicts: `.gitignore` (union of
both, keeping `/Resources`) and `AGENTS.md` (**keep ours**, the thin adapter; the bundle's product
invariants already live below). Then fold any bundle-only agent guidance worth keeping into this
file and update the proving-check table with measured counts.

## Toolchain (measured 2026-10-09, DESKTOP-IHKOOJS, Windows 11)

Python 3.14.3 (`py -3`), cargo/rustc 1.97.1, rustup 1.29.0, Node 24.13.1, npm 11.8.0, codex-cli
0.160.1. The bundle's authoring pass had **no** Rust and no Windows: every native result here is
a first measurement and must be reported as such.

## Proving checks by seam (from the bundle; re-measure after unbundling)

| You changed | Run |
|---|---|
| Python reference, schemas, JS boundary, fixtures | `python scripts/verify.py` (authored: 184 tests, 2 JS syntax, 9 JS boundary, 80 cross-language fixtures) |
| Rust workspace (`crates/`) | `cargo fmt --all --check`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` |
| Browser UI | `python scripts/test_ui.py` (needs Playwright + Chromium) |
| Tauri shell (`native/`) | excluded from the root workspace; build it separately |
| Docs only | `git diff --check` plus the links you touched |

Missing tooling is UNVERIFIED, never PASS. New behaviour lands with the test that pins it.

## Product invariants (violating one is a bug, not a style choice)

1. **No file or process effects** (move, delete, uninstall, kill, suspend, priority, trim) until
   the specific `LW-*` issue and its Windows safety gate authorise that capability. Never add a
   general execute, shell, delete or kill endpoint, through HTTP, IPC or MCP.
2. **No silent reach.** No automatic cloud requests, model downloads, installers, global config
   changes, elevation or whole-disk scans. Real-disk scans take an explicit disposable root,
   non-elevated, with state outside it; a path seen in a doc is not a grant.
3. **Data classes stay separate.** Human labels, teacher (LLM) labels, preferences and operation
   grants are distinct. A learned score is never an approval. No training on, or forwarding of,
   user metadata beyond explicit scope.
4. **Honest unknowns.** Unknown, partial, offline or unverified stays visible; never replace
   missing data with zero or demo data. Scores are not called probabilities until calibrated.
5. **Identity is native.** A snapshot path hash or a UI-round-tripped integer never authorises an
   effect; imported snapshots grant no access.
6. **Keep the negative evidence.** Allocator v1, its 51-case counterexample and failed experiments
   stay. Python allocator v2 is a static-model reference, not a mover; the Rust planner is v1.
7. **MCP is read-only and bounded.** Client metadata, annotations and tool output are not
   authorisation. LeaseBroker/admission is accounting, not OS enforcement.

## Pitfalls

- `LW-*` IDs are the bundle's local backlog (`backlog/issues.json`), not GitHub numbers. Mirror
  them to GitHub issues with the `LW-` ID in the title before tracking work there.
- Taskdeck is proprietary: never copy its code into this MIT repository.
- Public repo: synthetic fixtures only; no real filenames, inventories or training exports.
- The UI is not frozen. The owner wants a signature, elegant interface that grows with the
  engine; the bundle's pages are a reference, not a design to preserve.
