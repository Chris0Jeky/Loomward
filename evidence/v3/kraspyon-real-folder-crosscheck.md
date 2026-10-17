# Real-folder read-only cross-check on KRASPYON (17 October 2026)

Host: KRASPYON laptop, Windows 11 Home 10.0.26100, NTFS C:, non-elevated, warm/uncontrolled cache, shared with
about ten agent lanes. Binary: `loomward-lab cross-check` (release) built from `main` at `bb5836f`. Roots were
picked by the session driver under the q-4 grant (big, non-sensitive system and tool folders; never a volume root, a
whole profile, or a credential or browser store), each recorded in the gitignored `.loomward/scan-roots.log` before
scanning. Only anonymised labels and aggregates are recorded here (AGENTS.md invariant 2); no names or paths.

Command: `loomward-lab cross-check --root <private root> --root-log <private log>`. Each strategy runs in a fresh
child process; `std-single` uses one worker, the others eight; metadata only, no file contents read.

| Root | Agreement | Directories | Files | Logical bytes | Denied dirs | Skipped reparse | std-single | std-parallel | find | handle | Peak RSS (max) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| real-1 | exact | 169,775 | 153,872 | 21.7 GB | 0 | 0 | 694.1 s | 76.8 s | 10.2 s | 9.5 s | 11 MiB |
| real-2 | exact | 15,948 | 126,092 | 171.7 GB | 1 | 16 | 79.6 s | 6.1 s | 1.2 s | 0.9 s | 9 MiB |
| real-3 | refused before traversal: profile, credential, browser, or ambiguous real root (the guard worked as designed) | | | | | | | | | | |
| real-4 | exact | 426 | 8,702 | 6.0 GB | 0 | 0 | 5.0 s | 0.3 s | 0.1 s | 0.1 s | 6 MiB |

Reading: all four strategies agree exactly on every root they were allowed to read. The native `handle`
strategy is 50-75x faster than single-threaded `std`, but `std-single` ran first on a cold cache for real-1, so
that ratio overstates the steady-state difference. Not a benchmark claim: one host, uncontrolled cache, shared CPU.
