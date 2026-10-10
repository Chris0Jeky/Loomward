# Loomward

**A local-first Windows workspace, storage and resource companion.** Loomward aims to show where
your disk space goes (a far faster WinDirStat that says plainly what it does not know), learn how
you organise your files, let you arrange them virtually, simulate how data could be balanced
across fast and slow disks, and explain what is using your memory and processes, all without
changing anything on your machine. Its core ruling is that **meaning, residency and permission are
separate systems**: what a file is about, where it lives and whether anything may act on it are
three different questions, and none of them answers another.

## Status

Research prototype, not a file manager or system optimiser.

**Runs today**

- The **Python reference** (`python/loomward/`) and its browser workbench (`ui/`): a synthetic
  demo, a bounded metadata scan of one explicit folder, a small weighted student model, a tier
  simulator and a read-only MCP reference. Its gates pass on Windows (see [Verify](#verify)).
- **Rust crates** (`crates/`): `loomward-core`, `loomward-windows`, `loomward-telemetry`,
  `loomward-lab`, `loomward-learn` (plus `loomward-cli`). The Rust workspace was first compiled
  and tested on 2026-10-09; [Measured results](#measured-results) says which have recorded evidence.
- The **Svelte app shell** (`app/`), on a **mock transport only**: today it ships the Explorer,
  Tiers, Companion and Grants & health views over synthetic data (#119), with Atlas and
  Observatory in review. With no engine it shows "unavailable", never demo data.
- The **woven-atlas / Observatory prototype** (`app/prototype/`): canvas treemap, sunburst and
  gauges on synthetic data.

**Does not exist yet**

- A **native scan engine** and **persistent catalogue**: designed in
  [docs/41](docs/41-v03-architecture.md), in progress in the lanes of
  [docs/43](docs/43-v03-lanes.md), and absent from this tree (no engine, catalogue, protocol,
  service or HTTP crate). The **view service** the app is meant to talk to has only its contract
  ([`contracts/v3/`](contracts/v3/)).
- A **desktop shell**: `native/` is a Tauri 2 scaffold that has never been built; WebView2 is
  unverified.
- **Real-disk scanning by the product.** Native enumeration has been measured on synthetic trees
  and on four owner-authorised real folders (read-only, metadata only, anonymised), but only by
  lab benchmarks. The Python reference can scan a folder you name. The owner has decided that a
  whole volume may become a scan root only when the owner picks it in the native grant dialog
  (ADR-V3-08); agents never pick one.
- The **teacher on real data.** The optional LLM teacher is designed as weak supervision and stays
  synthetic-only. A spike found the Codex CLI cannot be fully confined by its flags, so OS-level
  sandbox work (LW-111) must pass first; whether real metadata may ever leave the machine is an
  open owner decision ([HUMAN_TODO.md](HUMAN_TODO.md), q-5). No teacher runner exists here.

**No file or process effect exists.** Nothing here moves, deletes, renames, writes, kills,
suspends, reprioritises or trims anything on your machine. (The lab tools create and delete
only their own marked, disposable synthetic trees; the Python reference writes its own state
database.) Backup, relocation, uninstall and process control are future work behind separate
safety gates. The Python reference and the native Rust engine are reported separately: a Python
result is never evidence about the native engine, and the reverse.

## The three threads

Loomward keeps three kinds of fact apart; the "Woven atlas" interface draws them as cloth threads:

| Thread | Question | In the atlas |
|---|---|---|
| **Meaning** | What is this part of? (collection, project, label) | Warp: vertical threads dyed by collection |
| **Residency** | Where does it live? (tier: C:, G:, E:) | Weft: metal threads; hollow means online-only; missing means unknown |
| **Permission** | May anything act on it? (pinned, protected) | Selvedge: stitched edge where a hold begins |

A collection can list every file of a project without moving one. A disk policy can keep the
active project on the fast disk without changing what the project contains. A learned score is
never an approval ([app/prototype/README.md](app/prototype/README.md)).

## Repository map

| Path | What it is |
|---|---|
| `python/loomward/`, `scripts/` | Python reference: scanner, student, tier simulator, loopback server, CLI, MCP reference |
| `ui/` | Original browser workbench; to be retired once `app/` reaches parity |
| `app/` | Svelte 5 + Vite + TypeScript shell (views today: Explorer, Tiers, Companion, Grants & health; transports: mock, HTTP, Tauri); `app/prototype/` is the no-build woven-atlas and Observatory prototype |
| `crates/loomward-core` | Pure domain: capability policy, simulated transaction state machine, planner v1 and a v2 port |
| `crates/loomward-windows` | Read-only Windows identity and volume observation; opt-in `fixtures` feature for a disposable lab |
| `crates/loomward-telemetry` | Read-only memory, process and PDH disk sampling (GPU unsupported) |
| `crates/loomward-lab` | Synthetic scale trees, four enumeration strategies, benchmarks |
| `crates/loomward-learn` | Rust port of the weighted Naive Bayes student; Python is the oracle |
| `crates/loomward-cli` | `loomward-native capabilities \| scan ROOT [MAX_ENTRIES] \| plan SCENARIO.json` |
| `native/` | Tauri 2 scaffold, excluded from the Cargo workspace, never built |
| `contracts/`, `schemas/`, `fixtures/` | `contracts/v3/` is the wire contract for the planned view service; interchange schemas; synthetic fixtures with parity corpora |
| `docs/`, `backlog/`, `evidence/`, `handoff/` | Design documents ([index](docs/INDEX.md)); the `LW-*` backlog; recorded measurements; history from the original authoring pass |

## Run it

Prerequisites (measured versions from [AGENTS.md](AGENTS.md)): Python 3.11 or later (3.14.3),
Rust 1.85 or later (1.97.1) with the MSVC toolchain and C++ Build Tools, Node 24.13.1 and npm
11.8.0 for `app/`. In PowerShell use `npm.cmd` and `npx.cmd`; the `.ps1` shims may be blocked.

### Python reference demo

```powershell
py -3 scripts/run.py demo --open
```

Starts a loopback-only server on port 8765 (`--port 0` picks a free one) and opens a synthetic
workbench. It scans nothing and calls no model, but creates a demo state database. Keep the
printed URL private: its `#token=` fragment is the session secret. Stop with Ctrl+C. Scanning a
real folder (`scripts/run.py serve --root <folder>`) is a separate step: use a disposable folder
and read the [operating guide](docs/38-reference-operating-guide.md) first.

### Svelte app in mock mode

```powershell
npm.cmd --prefix app ci
npm.cmd --prefix app run dev
```

Open the address Vite prints and add `?transport=mock` to it. Without that parameter and with no
engine the shell shows "unavailable" by design (`app/src/lib/transport/select.ts`); the mock
answers from synthetic data and reports every engine figure as unknown.

### Prototype

```powershell
py -3 -m http.server -d app/prototype 8040
```

Run from the repository root, then open `http://127.0.0.1:8040/`. Opening `index.html` from disk
does not work (browsers refuse ES modules over `file://`).

### Scale lab and enumeration bench

The lab is Windows-only and writes to disk; read [its README](crates/loomward-lab/README.md) first.

```powershell
cargo build -p loomward-lab --release --offline
target/release/loomward-lab.exe generate --root G:\loomward-lab\scale\example --files 100000 --seed 42 --profile mixed --threads 8
target/release/loomward-lab.exe bench --root G:\loomward-lab\scale\example --strategy handle --threads 8 --buffer-kib 64 --cache-label uncontrolled
```

Rules, as documented: a root must be a direct child of `G:\loomward-lab\scale` or
`E:\loomward-lab\scale`; the generator never overwrites an existing root, reserves 20 GB
(decimal) of free space and stops at 3M files; `destroy` is restricted to `G:` and deletes only a
root carrying the lab's own marker. By owner decision the 10M-entry tier is planned for `E:` only
and `G:` stays capped at 2M entries (ADR-V3-17, [docs/42](docs/42-v03-adrs.md)). `--offline`
needs the dependencies already downloaded. The Windows fixture lab
(`loomward-windows --features fixtures`) is limited to `G:\loomward-lab\fixtures`.

`cross-check` runs the four strategies against an explicit real folder. It needs a private root log
outside that folder, refuses volume roots and profile or credential locations, and exports
aggregates only. See the lab README, "Explicit real-root cross-check", before using it.

### Telemetry snapshot

```powershell
cargo run -p loomward-telemetry -- snapshot --json
cargo run -p loomward-telemetry -- watch --interval-ms 1000 --count 3 --json
```

One compact JSON object per line. Unknown values are `null` with a reason, never zero; rates and
disk throughput are unknown on the first sample. Schema: [crate README](crates/loomward-telemetry/README.md).

## Verify

Run from the repository root. Missing tooling is reported as UNVERIFIED, never as a pass.

| You changed | Run |
|---|---|
| Python reference, schemas, JS boundary, fixtures | `py -3 scripts/verify.py` (also runs the Rust gates when `cargo` is present) |
| Rust workspace | `cargo fmt --all --check`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` |
| Browser workbench (`ui/`) | `py -3 scripts/test_ui.py`, or everything at once: `py -3 scripts/verify.py --ui` |
| Svelte app | `npm.cmd --prefix app run check`, `... run test`, `... run build`, then `py -3 scripts/test_app.py` |
| Prototype | `py -3 app/prototype/check.py` (add `--gpu` for the hardware rasterizer) |

The browser checks need Playwright and a Chromium (`--browser PATH` selects one);
`scripts/test_app.py` expects `app/dist`, so build first. `py -3 scripts/verify.py --app` runs
the four `app/` checks in order (after `npm ci --prefix app`). Docs-only changes: `git diff --check` plus the links you touched.

Counts as [AGENTS.md](AGENTS.md) records them (Windows 11): 212 Python tests with one skip
(symlink creation needs a privilege a standard user lacks), 2 JavaScript syntax checks, 9
JavaScript boundary assertions, 80 cross-language fixtures, 54 Rust tests (core 29, windows 10,
telemetry 10, lab 5; more are landing in open PRs) and 14 Chromium UI checks. Counts move with
every merge; the command output is the authority. The Chromium harness bridges `fetch` through
Python, so it is not WebView2 or direct browser-network evidence.

Hosted CI (`.github/workflows/ci.yml`) has three jobs, each on Ubuntu and Windows: `python`
(Python 3.11 and 3.13, unit tests, JavaScript checks), `native` (rustfmt, tests, clippy) and `app`
(`npm ci`, check, test, build; the Playwright suites `scripts/test_app.py` and `scripts/test_ui.py`
run on Linux only). A local run is never described as hosted CI.

## Measured results

Every figure is a first-of-its-kind, non-elevated run on one Windows 11 machine: a starting
measurement, not a guarantee.

**Native enumeration spike** ([docs/research/enumeration-spike.md](docs/research/enumeration-spike.md),
raw receipt [evidence/v3/enumeration-spike.json](evidence/v3/enumeration-spike.json)). Four ways of
walking synthetic trees on an NVMe NTFS volume (`G:`), 20 logical processors, wall time:

| Strategy | 100,000 files | 3,000,000 files |
|---|---:|---:|
| `std::fs::read_dir`, one worker | 7.91 s | 226.67 s |
| same, 8 threads | 1.05 s | 42.76 s |
| `FindFirstFileExW`, 8 threads | 0.054 s | 1.07 s |
| `GetFileInformationByHandleEx`, 8 threads | 0.036 s | 0.64 s (4.7 M files/s) |
| Python reference scanner | 10.90 s | not measured |

All 43 final and tuning runs matched the generated manifest exactly. The recommended starting
candidate is the handle strategy with eight workers and 16 KiB buffers (5.38 M files/s at the
median of three paired runs at 3M). Caveats that matter: **caches were uncontrolled (warm)**, with
no cold-cache result, reboot or purge, and antivirus not isolated; the files are synthetic, sparse
and zero-filled, and no database was written, so this says nothing about catalogue speed; the
native walkers only add up totals while the Python scanner builds per-file records, so that row is
context, not an equal-work comparison; WinDirStat was not run.

**Allocator.** `loomward-core` carries a Rust port of the v2 allocator. A 222-case parity corpus
matches Python on all 193 accepted cases field for field, and Rust rejects all 29 inputs that
Python rejects ([evidence/v3/planner-v2-rust.md](evidence/v3/planner-v2-rust.md)). On the original
51 synthetic scenarios v2 misses no feasible target; v1 misses 9 (a static model; nothing is
moved). v1 and its counterexample are kept ([docs/22](docs/22-experiment-findings.md)). The
engine does not call the Rust planner yet.

**Real folders** ([report](docs/research/real-folder-stress.md),
[receipt](evidence/v3/real-folder-stress.json)). Four owner-authorised folders, named only
`real-A` to `real-D` (A to C on the C: NVMe, D on the E: HDD), walked read-only, metadata only.
Warm median wall time, five runs each:

| Root | Entries | Logical GiB | `handle` | `find` | `std-single` |
|---|---:|---:|---:|---:|---:|
| real-A | 323,788 | 68.23 | 0.404 s | 0.593 s | 14.44 s |
| real-B | 311,449 | 79.66 | 0.680 s | 0.954 s | 15.02 s |
| real-C | 200,575 | 16.60 | 0.930 s | 1.179 s | 12.22 s |
| real-D | 14,824 | 20.49 | 0.033 s | 0.035 s | 0.287 s |

The four strategies agreed exactly on 23 of 24 final-build sweeps. The exception was a live
folder (real-A) that grew by 894 bytes between two walks; the report keeps it as a failed
comparison, not a pass. Skipped links and denied directories stay counted. There is no true
cold-cache run: the "initial" sweep is only first-touch for the session.

On identical synthetic 1M-file trees (warm, uncontrolled) the `handle` strategy took a median
0.202 s on the NVMe and 0.131 s on the HDD. The HDD was not slower, which is compatible with
cache effects; it is not a measure of device speed.

**Student and telemetry.** `fixtures/learning-v3/` holds 288 student parity cases (230 tokeniser,
58 student scenarios; counted from the files) read by `crates/loomward-learn/tests/parity.rs`,
with Python as the oracle. No passing run is recorded in `evidence/`; run
`cargo test -p loomward-learn`. Scores are uncalibrated. One telemetry snapshot cost 802.9 ms
including first-call set-up, later samples 20.8 and 24.0 ms
([evidence](evidence/v3/telemetry.md)); a small run, not an overhead guarantee.

**Teacher spike** ([docs/research/teacher-confinement.md](docs/research/teacher-confinement.md)).
Synthetic data only: a hardened Codex CLI invocation gave valid output on 10 of 10 batches of 25
items (median 21.8 s) and disclosed no canary, but a collaboration tool stayed callable despite its
flags. Verdict: partially confinable, so the teacher stays synthetic-only until an OS-enforced
boundary is demonstrated.

**Not yet measured:** true cold caches, WinDirStat, WebView2, a persistent catalogue. The targets
in [docs/41](docs/41-v03-architecture.md) section 15 are hypotheses, not results.

## Architecture and development

- **Plan:** [docs/41](docs/41-v03-architecture.md) (architecture), [docs/42](docs/42-v03-adrs.md)
  (decisions), [docs/43](docs/43-v03-lanes.md) (lanes); the wire contract is
  [`contracts/v3/`](contracts/v3/). **State:** the "v0.3 status" section at the end of
  [docs/16](docs/16-implementation-status.md); [docs/INDEX.md](docs/INDEX.md) maps every chapter.
- **Agents:** [AGENTS.md](AGENTS.md) is the one baseline for every agent runtime, with the product
  invariants (no effects, no silent reach, honest unknowns and more). Read it before changing anything.
- **Backlog:** 112 `LW-*` items (`backlog/issues.json`; [backlog/INDEX.md](backlog/INDEX.md) lists
  the first 100). `LW-` IDs are not GitHub numbers; [backlog/github-issues.json](backlog/github-issues.json)
  maps them (LW-001 to LW-100 are #4 to #105, LW-101 to LW-112 are #120 to #131).
- **Contributing:** [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md). Fixtures here
  must be synthetic. Open owner decisions: [HUMAN_TODO.md](HUMAN_TODO.md).

## Licence and stance

MIT ([LICENSE](LICENSE)). No theatre and no false claims: no release, package or "done" status is declared to show progress;
unknown, partial and unverified stay visible; failed experiments are kept. Loomward does not claim
native compilation of its desktop shell, backup verification, memory optimisation, learned heat,
live disk balancing or safe deletion, because none of them has passed its own gate.
