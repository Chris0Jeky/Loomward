# Windows NTFS enumeration spike

9 October 2026. Sol lane L, base `1ee166d597b61cf935a84499351009f0e316857a`, Windows 11,
non-elevated user token, 20 logical processors, owner-described G: NVMe, measured NTFS.
This is a synthetic metadata enumeration experiment, not a whole-disk scan, a WinDirStat
comparison, an engine integration or an effects capability. All generated files stay beneath
`G:\loomward-lab\scale`. No E: experiment, cache eviction, elevation, global setting,
installation, commit, push or branch operation occurred.

## Reproduce

```powershell
cargo build -p loomward-lab --release --offline
py -3 crates/loomward-lab/check_windows.py
py -3 crates/loomward-lab/measure.py --sizes 100000 1000000 3000000 --passes 2 --threads 8
```

The final main comparison uses one fresh-process warm trial per strategy/scale, plus three
trials per tuning configuration. The earlier repeated exploratory run is retained in
[enumeration-spike-exploratory.json](../../evidence/v3/enumeration-spike-exploratory.json);
it predates the ancestor-pin change and was deliberately interrupted before a redundant final
std repeat. Its numbers are not the final comparison. The trees were generated during that
exploratory stage and revalidated by the final binary; generation timings therefore belong
to that earlier generator build.

The final raw receipt is [enumeration-spike.json](../../evidence/v3/enumeration-spike.json).
It records build/source hashes, each fresh-process measurement, exact expected/observed totals,
worker counts, buffer sizes, CPU seconds and peak working-set bytes. The native regression
receipt is [enumeration-spike-checks.json](../../evidence/v3/enumeration-spike-checks.json).
The crate [README](../../crates/loomward-lab/README.md) describes generation/cleanup guards.

## Method and correctness

Three profiles: `dev` (64-file source/package buckets and tiny files), `media` (128-file
buckets and 1-16 GiB files), and `mixed` (256-file buckets, 70% tiny documents, 20% photos,
8% archives and 2% 1-8 GiB model files). Seed 42 determines all names, directory shapes and
logical sizes. Names contain Unicode and case variants; every 64th bucket has a 40-component
path, beyond MAX_PATH. All file content is sparse zeros, not realistic contents. Small NTFS
resident files can report nonzero allocation even when marked sparse.

Each tree has an ownership marker and independent planned totals in a manifest. `data/` is
its only scan scope; its root and sibling control files are excluded from counts. Generation
sets FSCTL_SET_SPARSE before extending EOF, then checks the completed tree with the handle
walker. The four walkers must match file count, descendant-directory count and logical bytes
exactly; skips or errors cannot produce a PASS. Buffer-chain decoding checks offsets, alignment,
lengths and names; portable tests and a Windows ABI-layout test pin that representation.

(a) `std::fs::read_dir` plus `symlink_metadata`, one worker; the no-follow form of metadata is
intentional. Rust documents this query as GetFileInformationByHandle on Windows,
while its stable MetadataExt interface does not expose file_index (a nightly API):
[metadata query](https://doc.rust-lang.org/std/fs/fn.symlink_metadata.html),
[Windows metadata extensions](https://doc.rust-lang.org/std/os/windows/fs/trait.MetadataExt.html).
(b) The same with std threads and a shared stealable directory queue. (c)
FindFirstFileExW with FindExInfoBasic and FIND_FIRST_EX_LARGE_FETCH, parallel. (d)
GetFileInformationByHandleEx on directory handles with FileIdExtdDirectoryInfo and a 64 KiB
aligned buffer, parallel. This selects the documented Win32 option in the brief; it does not
benchmark NtQueryDirectoryFileEx separately. All parallel main trials use eight workers.

Directory handles are opened with BACKUP_SEMANTICS and OPEN_REPARSE_POINT; no elevation or
backup privilege is enabled. Checked directory and ancestor handles are retained without
write/delete sharing while needed. Reparse, offline and recall entries are skipped before
traversal; an actual synthetic junction was tested for every walker. Unsupported APIs, denied
access, corrupt manifests and incomplete trees fail visibly. This lab is for trusted disposable
static trees, not an authority boundary against same-user malware or a consistent live snapshot.
The lab deliberately restricts directory sharing while holding pins; a production observer must
measure coexistence with live writers and report incomplete coverage rather than require these
locks or silently claim a stable snapshot.

Wall and CPU timers cover traversal including directory opens, but exclude startup and manifest
validation. RSS is the process's lifetime PeakWorkingSetSize, so it includes manifest validation;
it is not private commit or filesystem cache usage. GetProcessTimes CPU is summed across workers
and has coarse quantisation; >100% denotes more than one core. The Python comparison includes
its full per-file records, hashing of path references, sorting and coverage construction; native
walkers only aggregate totals, so it supplies application context, not an equal-work benchmark.

Cache is **uncontrolled throughout**. Generation and exact validation populate caches before
the first timing. Repeat timings use fresh processes but retain OS caches. No reboot or cache
purge was authorised or attempted, and a slower first trial does not establish a cold trial.
Antivirus/filter activity and other host work were neither disabled nor isolated.

## Results

| Files | Strategy | Wall s | Files/s | CPU s | Peak RSS MiB | Exact |
|---:|---|---:|---:|---:|---:|---|
| 100,000 | std-single | 7.907881 | 12,646 | 6.000000 | 4.42 | yes |
| 100,000 | std-parallel | 1.051150 | 95,134 | 7.968750 | 5.61 | yes |
| 100,000 | find | 0.053663 | 1,863,485 | 0.312500 | 5.40 | yes |
| 100,000 | handle | 0.035621 | 2,807,341 | 0.296875 | 5.30 | yes |
| 100,000 | python-reference | 10.895755 | 9,178 | 10.765625 | 148.66 | yes |
| 1,000,000 | std-single | 63.756362 | 15,685 | 54.640625 | 5.80 | yes |
| 1,000,000 | std-parallel | 11.866468 | 84,271 | 89.765625 | 6.86 | yes |
| 1,000,000 | find | 0.426782 | 2,343,115 | 2.578125 | 7.23 | yes |
| 1,000,000 | handle | 0.252248 | 3,964,346 | 1.843750 | 6.83 | yes |
| 3,000,000 | std-single | 226.666814 | 13,235 | 183.093750 | 8.88 | yes |
| 3,000,000 | std-parallel | 42.762114 | 70,156 | 265.937500 | 8.88 | yes |
| 3,000,000 | find | 1.069853 | 2,804,123 | 7.187500 | 19.60 | yes |
| 3,000,000 | handle | 0.637366 | 4,706,870 | 4.406250 | 20.90 | yes |

All rows are **warm/uncontrolled** final-build trials. Parallel strategies use eight workers;
handle rows in this main table use 64 KiB. CPU is aggregate process time, not elapsed time.
The Python reference is unchanged and is measured only at 100k (its 200k examination cap permits
all 100k files and 664 directories). All 43 final/tuning runs matched the manifest exactly.

### Handle tuning at 1M (three trials/configuration)

| Workers | Buffer KiB | Median wall s | Files/s at median | Maximum peak RSS MiB |
|---:|---:|---:|---:|---:|
| 1 | 64 | 0.956466 | 1,045,515 | 5.80 |
| 2 | 64 | 0.553059 | 1,808,125 | 5.80 |
| 4 | 64 | 0.305378 | 3,274,632 | 5.80 |
| 8 | 4 | 0.257401 | 3,884,992 | 6.53 |
| 8 | 16 | 0.206046 | 4,853,288 | 7.04 |
| 8 | 64 | 0.240997 | 4,149,438 | 6.88 |
| 8 | 256 | 0.371277 | 2,693,407 | 7.87 |
| 16 | 64 | 0.222470 | 4,494,980 | 8.56 |

### Paired buffer confirmation at 3M (three alternating trials each)

| Workers | Buffer KiB | Median wall s | Files/s at median | Median CPU s | Maximum peak RSS MiB |
|---:|---:|---:|---:|---:|---:|
| 8 | 16 | 0.558059 | 5,375,772 | 3.843750 | 9.23 |
| 8 | 64 | 0.597656 | 5,019,614 | 4.328125 | 20.77 |

### Trees left for reuse

| Root under G:\loomward-lab\scale | Payload files | Descendant dirs | Logical bytes | Reported file allocation bytes | Generation free-space delta bytes |
|---|---:|---:|---:|---:|---:|
| mixed-100000 | 100,000 | 664 | 11,969,937,673,064 | 733,464 | 74,928,128 |
| mixed-1000000 | 1,000,000 | 6,325 | 121,256,337,157,388 | 7,246,288 | 1,304,076,288 |
| mixed-3000000 | 3,000,000 | 18,895 | 362,174,108,598,017 | 21,684,072 | 4,704,530,432 |
| smoke-dev | 257 | 44 | 2,081,936 | 1,848 | 385,024 |
| smoke-media | 257 | 42 | 2,200,096,997,376 | 0 | 921,600 |
| smoke-mixed | 257 | 41 | 19,296,309,352 | 1,720 | 659,456 |

Reported payload allocation totals **29,667,392 bytes**;
observed generation free-space deltas total **6,085,500,928 bytes**
(about 6.09 GB, including metadata and unrelated activity). Both observations are below the
20 GB task budget. Allocation sums are not a complete physical-space oracle. Scale generation
took 8.631 / 76.633 / 288.093 seconds for 100k / 1M / 3M respectively, including validation.
The marked cleanup control was removed successfully; no test junction or busy marker remains.

## API information cost

| Strategy | Logical size / attributes | File ID | Allocation size | Reparse tag |
|---|---|---|---|---|
| std read_dir + symlink_metadata | Separate metadata query per entry | Not reported by this baseline | Not reported by this baseline | Attribute-based rejection; tag not reported |
| FindFirstFileExW Basic/LARGE_FETCH | In directory record | Extra handle/query | Extra handle/query | dwReserved0 for reparse entries |
| GetFileInformationByHandleEx Extd | In directory record | 128-bit ID in record | In record | In record |
| FileFullDirectoryInfo (documented, not timed) | In directory record | Extra query | In record | Conditional EaSize tag (spec; not measured) |

The [Extd structure](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_extd_dir_info)
defines IDs, EOF, allocation and reparse tags. [Full directory info](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_full_dir_info)
lacks IDs and a dedicated tag field. The [FileFullDirectoryInformation specification](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/e8d926d1-3a22-4654-be9c-58317a85540b) assigns EaSize to the reparse tag when that attribute is set; this variant was not measured here. [Find data](https://learn.microsoft.com/en-us/windows/win32/api/minwinbase/ns-minwinbase-win32_find_dataw)
has logical size, attributes and a conditional reparse tag, but no ID/allocation fields.
[Large fetch](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-findfirstfileexw)
requests a larger internal query buffer; Basic omits short-name retrieval. [The handle API](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getfileinformationbyhandleex)
requires explicit buffer sizes and driver support for the selected class. [Sparse operations](https://learn.microsoft.com/en-us/windows/win32/fileio/sparse-file-operations)
require explicitly marking a file sparse before relying on holes.

All three implemented approaches can skip reparse points; none confers effect authority. ID
observations still need volume identity and freshness checks before any future operation.
Allocation sums are per-entry observations, including resident-file reporting, not physical
reclaimability, stream accounting, hard-link deduplication or total volume usage.

## Recommendation and bounds

Use **GetFileInformationByHandleEx(FileIdExtdDirectoryInfo), eight workers, 16 KiB per directory
query** as the starting candidate for this shape. It was fastest among the four tested approaches,
returned nonzero 128-bit IDs for every generated payload file and observed allocation without a
per-file open. The 64 KiB comparison reached 4.71M files/s at 3M; paired tuning preferred 16 KiB
(5.38M files/s at its median, maximum observed RSS 9.23 MiB). One worker did not exploit the NVMe
well, sixteen workers did not beat the eight-worker/16 KiB setting at 1M, and 256 KiB cost more
on these mostly 256-file leaf directories. These are starting measurements, not universal thread
or buffer optima. Benchmark wide directories and HDD latency before adapting concurrency or
increasing buffers. On unsupported filesystems/classes, a future FindExInfoBasic fallback should
surface unknown IDs/allocation rather than fabricate zero; no fallback was silently timed here.

The lab's shared directory frontier is capped at 1024 jobs. Queue overflow is walked inline
rather than stored in an unbounded private frontier; depth beyond 128 fails. There is no retained
file list. The worst-case buffer term is W * (D+1) * B, because inline recursion may retain parent
buffers, plus bounded queued paths and shared ancestor pins. At the recommended eight workers/16 KiB this term
is at most 16.125 MiB for D=128, or 5.125 MiB at this generator's D=40 (the 64 KiB comparison
uses 64.5 / 20.5 MiB for this term). Add O(Q * P) queued path storage and O((Q + W*D) * D)
shared ancestor-pin bookkeeping as a conservative bound, where P is the maximum encoded path
length. Actual measured RSS is in the tables. Queue paths are limited by Windows path limits; ancestor pins and worker stacks add
memory and OS handles. This is a parameterised bound, not a promised constant RSS for arbitrary
trees. Cleanup deliberately preflights/pins all directories and retains file paths, so its memory
is O(tree entries), unlike the scan. It is a lab cleanup tool, not a production bulk deleter.

For the future catalogue, start by streaming **1024 records per batch**, at most eight queued
batches, through a single database writer with backpressure. Keep UTF-16 names/IDs and integer
byte counts until serialization, and do not reopen every file to recover fields Extd already
provides. Place scan state outside the payload root. Measure 1024/4096-row transaction batches,
WAL/index costs, cancellation and partial coverage through the actual catalogue before choosing
a production batch size. The spike measures enumeration only: it cannot substantiate a database
write rate or turn the existing reference catalogue into a persistent native index.

## Verification and remaining scope

- `cargo fmt --all --check`: PASS.
- `cargo test --workspace`: PASS, 19 core + 5 lab tests on Windows.
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS.
- `py -3 crates/loomward-lab/check_windows.py`: PASS, 20 native regression cases.
- `cargo check -p loomward-lab --tests --target x86_64-linux-android --offline`: PASS non-Windows
  compilation using an already installed target; this is not Ubuntu execution.
- Initial unchanged baseline `py -3 scripts/verify.py`: PASS (186 Python tests, one privilege skip,
  JavaScript gates and then-existing Rust gate); no Python reference or UI source was changed.
- Required proving-check tail: [enumeration-spike-gate.txt](../../evidence/v3/enumeration-spike-gate.txt).
- Final Rust source/executable hashes match the measured receipt; all local report/README links
  resolve. The driver still owns independent review, commit and integration.

This is a bounded contribution to LW-064, not completion of its broader fixture gate: ADS,
compressed files, ACL variants, locks, removable/cloud simulations and hostile concurrent swaps
remain unmeasured. Linux/Ubuntu execution, HDD/SMB/ReFS, real user data, true cold caches, wide
single-directory extremes, fragmentation, catalogue writes and WinDirStat remain unverified.
The generated sparse files are not representative of content hashing, preview or copy throughput.
`HUMAN_TODO.md` remains driver-owned and unchanged; its q-1 through q-4 are still open. This
explicit synthetic-root grant does not supply q-4's future non-synthetic scan authority.
