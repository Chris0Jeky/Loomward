# Real-folder enumeration stress test

Measured on Windows 11, 10 October 2026 local time, without elevation. This extends the
[synthetic enumeration spike](enumeration-spike.md) as a bounded LW-064 experiment; it does
not deliver a production scanner, snapshot, catalogue, deletion or movement capability.
The receipt is [real-folder-stress.json](../../evidence/v3/real-folder-stress.json), with
[native regression controls](../../evidence/v3/real-folder-stress-checks.json).

## Method and privacy

Four owner-authorised roots are identified publicly only as `real-A` through `real-D`. The first
three are on the C: NVMe; the fourth is on the E: HDD. Actual root paths and the reproduction
plan stay in the primary checkout's gitignored state. Each native child appends a Unix timestamp,
absolute root and reason to the private `scan-roots.log`, flushes it with `sync_all`, and only
then inspects root ancestors or traverses. The Python child appends an ISO timestamp/root/reason
and calls `fsync` before its scan. No real file contents, snapshots, names, path hashes or per-file
diagnostics are persisted to this public repository. No privileges, cache policy, process policy
or external services were changed.

The manifest-free `cross-check` runs `std-single`, `std-parallel`, `find` and `handle` sequentially,
each in a fresh process. Parallel strategies use eight workers; handle queries use 64 KiB buffers.
All share the existing bounded directory queue, no-follow metadata checks and directory pins
without write/delete sharing. Each compares the same five counters: files, descendant directories,
logical bytes, skipped reparse/offline/recall entries, and denied directories. The skip counter
retains the synthetic walker's rejection-group semantics; it is not a reparse-tag breakdown.
Denied directories are counted when opening or enumerating them fails with permission denied;
they are still included in the parent-observed directory count. Other errors fail visibly.
Reparse roots/ancestors are rejected; descendant links are counted as skips before traversal.
There is no per-file content open and no file list in native measurements.

Logical bytes are per directory entry, not allocated or uniquely owned bytes: hard links count
once per name. Skips and denied directories remain visible even when strategies agree. Agreement
does not establish full coverage, an immutable snapshot or safe effects. Holding directory pins
prevents directory replacement while held, but does not freeze file sizes or make this a production
coexistence benchmark. Timers cover traversal and directory opens; child startup, scope logging
and ancestor preflight are excluded. Peak RSS is each fresh process's lifetime PeakWorkingSetSize,
including preflight, not filesystem cache usage or private commit. Host workloads and antivirus
were not isolated, and fixed strategy order can favour later queries.

## Cache caveats

There is **no true cold-cache measurement**. No standby flush, reboot, elevation or no-buffering
claim was attempted. An initial, cold-ish traversal means first traversal of a root by this session;
its previous use since boot is unknown. Only the first `std-single` run can carry even that weak
meaning. The other strategies in its sweep inherit caches populated by preceding scans. Five
additional complete sweeps are labelled `warm-uncontrolled`.

The first `real-A` traversal was a preliminary diagnostic build with broader directory sharing;
its source and executable hashes are retained separately. Final timings use the restored original
directory pins. Thus `real-A`'s final-build initial sweep is already session-warm; the preliminary
first-touch number is contextual evidence, not a controlled comparison of the final implementation.
The other roots had not yet been traversed by this session before their final-build initial sweep.
Synthetic generation and exact manifest validation populate caches before timing, on both devices.

## Results

Counts below use the last final-build std-single observation. Entries include files, descendant directories and skips; denied directories are already included in directories. GiB is rounded, while the JSON keeps integer bytes.

| Root | Storage | Entries | Files | Dirs | Logical GiB | Skip group | Denied dirs | Exact sweeps |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| real-A | NVMe | 323,788 | 291,935 | 31,837 | 68.23 | 16 | 2 | 5/6 |
| real-B | NVMe | 311,449 | 260,395 | 51,036 | 79.66 | 18 | 2 | 6/6 |
| real-C | NVMe | 200,575 | 138,616 | 61,959 | 16.60 | 0 | 0 | 6/6 |
| real-D | HDD | 14,824 | 12,639 | 2,185 | 20.49 | 0 | 0 | 6/6 |

Each warm median and range uses five runs; RSS is the maximum of their five process peaks. Initial times are the initial sweep, whose later strategies inherit preceding cache; real-A uses the separately hashed preliminary build described above.

### real-A

| Strategy | Initial s | Warm median s (min-max) | Warm files/s | Peak RSS MiB |
|---|---:|---:|---:|---:|
| std-single | 38.573 | 14.440 (14.152-14.769) | 20,217 | 5.34 |
| std-parallel | 4.387 | 4.097 (3.771-4.567) | 71,259 | 6.70 |
| find | 0.742 | 0.593 (0.562-0.680) | 492,427 | 9.18 |
| handle | 0.590 | 0.404 (0.400-0.422) | 723,126 | 9.09 |

### real-B

| Strategy | Initial s | Warm median s (min-max) | Warm files/s | Peak RSS MiB |
|---|---:|---:|---:|---:|
| std-single | 34.399 | 15.016 (14.522-15.089) | 17,341 | 5.41 |
| std-parallel | 3.766 | 3.389 (3.294-3.515) | 76,838 | 6.70 |
| find | 0.932 | 0.954 (0.868-1.009) | 273,082 | 9.46 |
| handle | 0.713 | 0.680 (0.631-0.962) | 382,851 | 9.63 |

### real-C

| Strategy | Initial s | Warm median s (min-max) | Warm files/s | Peak RSS MiB |
|---|---:|---:|---:|---:|
| std-single | 26.404 | 12.215 (11.130-12.818) | 11,348 | 5.14 |
| std-parallel | 3.960 | 3.734 (3.466-4.440) | 37,126 | 6.33 |
| find | 1.257 | 1.179 (1.138-1.594) | 117,549 | 8.38 |
| handle | 0.957 | 0.930 (0.866-1.255) | 149,074 | 8.59 |

### real-D

| Strategy | Initial s | Warm median s (min-max) | Warm files/s | Peak RSS MiB |
|---|---:|---:|---:|---:|
| std-single | 4.110 | 0.287 (0.284-0.311) | 43,990 | 5.08 |
| std-parallel | 0.081 | 0.081 (0.078-0.086) | 156,886 | 5.89 |
| find | 0.086 | 0.035 (0.034-0.036) | 361,383 | 6.03 |
| handle | 0.031 | 0.033 (0.031-0.034) | 388,188 | 5.98 |

## Agreement and the live-root finding

**23 of 24 final-build sweeps agree exactly.** Every sweep of real-B, real-C and real-D agrees on all five counters. Real-A's five warm sweeps include one disagreement; its file, directory, skip and denied counts are stable in every sweep.

In real-A final-build warm pass 1, std-single observed **73,256,569,896 bytes**; the three later strategies observed **73,256,570,790 bytes**, a difference of **894 bytes**. All four subsequently agreed on the latter total. The preliminary first-touch sweep similarly grew from 73,256,569,002 to 73,256,569,896 bytes. These failures are retained verbatim and return a nonzero cross-check status; they are not converted to a PASS.

A separate metadata-only temporal probe found one existing entry whose size and modification time changed, with a net size change of 1,788 bytes over its 180-second wait; added/removed files: 0/0. Both probe traversals reported two errors, so these observations also retain partial coverage. No names or content were exported. This is direct evidence of live metadata changes; identifying the writer and attributing the earlier 894-byte transitions to a specific file remain unverified. Sequential walkers over a live tree do not have a common snapshot instant, so transient disagreement is meaningful negative evidence, not proof that a Windows API permanently miscounts.

## Python reference context

The unchanged Python scanner on the smallest real root, **real-D**, matched files, directories, logical bytes and skips: **0.760 s**, **16,621 files/s**, **36.14 MiB** peak RSS. It examined 14,824 entries, hit no cap and reported zero errors/skips. Its public receipt omits all file records and diagnostic paths. The Python scanner builds, hashes path references for, sorts and retains inventory records; native walkers aggregate only. This is application context, not equal-work speedup proof.

## Identical 1M tree: NVMe versus HDD

Both trees use mixed profile, seed 42, 1,000,000 files, 6,325 descendant directories and 121,256,337,157,388 logical bytes. Names, layout and sizes match the same deterministic oracle; zero-filled sparse files do not model content throughput, fragmentation or physical reclaimability. All 40 included timed runs matched the manifest. Four additional original HDD trial-3 results also matched but are retained separately as excluded_overlap_runs: that sweep overlapped the temporal probe's second traversal and was replaced once after the probe and original batch finished. The included HDD trials are 1, 2, 4, 5 and replacement 6. Both devices use NTFS; G: is the Sabrent Rocket Q NVMe, E: the ST2000DM006 SATA HDD. Five warm-uncontrolled runs per strategy/device use fresh processes, eight parallel workers and 64 KiB buffers.

| Strategy | NVMe median s | HDD median s | HDD/NVMe wall ratio | NVMe files/s | HDD files/s | NVMe / HDD max RSS MiB |
|---|---:|---:|---:|---:|---:|---:|
| std-single | 45.128 | 33.060 | 0.73x | 22,159 | 30,248 | 6.50 / 6.54 |
| std-parallel | 10.335 | 8.192 | 0.79x | 96,758 | 122,073 | 7.62 / 7.82 |
| find | 0.283 | 0.143 | 0.50x | 3,529,635 | 6,991,159 | 7.90 / 8.32 |
| handle | 0.202 | 0.131 | 0.65x | 4,957,900 | 7,640,610 | 7.45 / 7.82 |

These ratios describe warm metadata enumeration on this host, not raw device speed. OS caches can conceal HDD seek costs. The differing initial real-D std-single time and warm repeats are compatible with cache effects but do not isolate their cause. No cache eviction, uncached content read or true cold-HDD run was performed.

### Marked trees left in place

| Synthetic root | Created this lane | Generation s | File allocation bytes | Generation free-space delta bytes |
|---|---|---:|---:|---:|
| `G:\loomward-lab\scale\mixed-1000000` | reused | 76.633 | 7,246,288 | 1,304,076,288 |
| `E:\loomward-lab\scale\mixed-1000000` | yes | 221.316 | 10,549,984 | 1,071,075,328 |

Each tree retains its exact ownership marker and complete deterministic manifest. E: generation uses the same create-new, sparse-before-extension and 20 GB free-space/allocation guards as G:. Volume deltas include metadata and concurrent unrelated activity; they are not precise per-tree allocated space. The new synthetic regression control was cleaned with the guarded G: destroy command. E: cleanup authority was not expanded.

## Verification and handoff

- `cargo fmt --all --check`: PASS.
- `cargo test --workspace`: PASS, 64 tests on Windows.
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS.
- `py -3 scripts/verify.py`: PASS, 212 Python tests (one privilege skip), JS syntax, 9 JS boundary assertions, 80 parity fixtures, Rust gates.
- `py -3 crates/loomward-lab/check_windows.py`: PASS, 20 existing native regression cases.
- `py -3 crates/loomward-lab/check_real_windows.py --root-log <private-log>`: PASS, 8 new native controls.
- `git diff --check`, JSON parsing, receipt completeness, source/binary hashes, private-log audit and local report links: PASS.

Measurement source and executable hashes, final handoff hashes, raw aggregates and every warm trial are in the JSON. After timing, two cfg(test) root examples were replaced with fictional paths; operational Rust code did not change. Rebuilding changed the executable hash, so performance trials retain their original binary identity rather than claiming the rebuilt binary was timed. The handoff binary independently passes all eight native controls, another four-strategy real-D cross-check and both 1M synthetic handle/oracle checks; these confirmations are outside the timing tables. The real-root public receipt contains no chosen root paths, names, path hashes or per-file data. Actual roots were logged before each scan; the recorded-line audit is aggregate-only. There were no commits, pushes, PRs, hosted CI runs, releases or packages in this lane.

Status and checkpoint files are coordinator-owned and updated separately. This is bounded LW-064 evidence, not closure of its broader fixture/platform gate. `HUMAN_TODO.md` is driver-owned and unchanged: q-4 records the owner grant as closed; q-1, q-2 and q-5 remain open. The driver owns review, commit and integration.

Not measured: true cold caches, writer identity, production coexistence, hostile concurrent filesystem changes, hard-link deduplication, physical reclaimability, content/copy throughput, alternate filesystems and a persistent native catalogue. Strategy agreement alone establishes none of those properties.
