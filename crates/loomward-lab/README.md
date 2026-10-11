# Loomward scale lab

An isolated Windows NTFS experiment, not an engine executor. Uses std threads, the already
locked windows-sys 0.61, and the workspace's serde dependencies; no new package dependency.
The plan and directory-buffer decoder have platform-independent tests; execution here was on Windows. The binary's
filesystem commands require Windows.

```powershell
cargo build -p loomward-lab --release --offline
target/release/loomward-lab.exe generate --root G:\loomward-lab\scale\example --files 100000 --seed 42 --profile mixed --threads 8
target/release/loomward-lab.exe bench --root G:\loomward-lab\scale\example --strategy handle --threads 8 --buffer-kib 64 --cache-label uncontrolled
```

`dev`, `media` and `mixed` generate deterministic UTF-16 names, case variants, fan-out,
40-component paths, long paths, and profile-specific logical sizes. All payloads are zero-filled
sparse files. They model metadata workloads, not realistic contents, fragmented extents or read
bandwidth. Tiny resident NTFS files can still report allocation. Generation checks the exact
file/directory/logical-byte oracle and measured allocation before marking its manifest complete.
Control files are siblings of `data/`; enumeration counts only `data/` descendants, excluding
that directory itself.

Synthetic generation/bench roots must be **direct children of G:\loomward-lab\scale or E:\loomward-lab\scale**, with ASCII letters/digits/hyphens/
underscores and no device names. Relative, UNC, device, ADS and traversal paths are refused.
Generation never overwrites an existing root. It reserves 20 GB (decimal) free space, limits a G: tree to
2M files, except the owner-authorised oversized **L tier (10,000,000 files)**, which is permitted
only under the E: scale lab. G: and every other root keep the 2M cap/refusal. The root grammar,
marker, non-elevated token, never-overwrite check and sparse zero payloads remain mandatory.
It sparsifies before extending files and refuses completion if payload allocation or the
observed generation volume delta exceeds 20 GB (decimal) for <=3M files. For oversized E: trees,
only that ceiling scales proportionally: `ceil(20 GB * files / 3,000,000)`, at most
66,666,666,667 bytes for 10M. The **20 GB free-space reserve never scales down**. These are
metadata/sparse-allocation ceilings, not permission to write payload content. Volume deltas include concurrent unrelated
activity and metadata; they are not precise per-tree allocation measurements.

`destroy --root <marked-root>` remains restricted to G:; it verifies the exact lab marker and preflights/pins all directories
before deleting anything. It refuses any reparse/offline entry, including junctions, and keeps
the marker until payload deletion succeeds. Ancestor handles and the `.busy` file coordinate
commands in this lab. A crashed command can leave `.busy`; inspect ownership before removing
that lab-local file. Markers are ownership labels in trusted disposable trees, not a security
boundary against a malicious process with the same user's access.

Strategies: `std-single`, `std-parallel`, `find` (FindFirstFileExW Basic/LARGE_FETCH), and
`handle` (GetFileInformationByHandleEx FileIdExtdDirectoryInfo). All reject reparse/offline/
recall entries and fail exact verification on skips or errors. The parallel implementation
shares a bounded stealable directory queue (1024 jobs); overflow walks inline at depth <=128.
Queued descendants retain ancestor pins. Only the handle strategy reports allocation and IDs;
null in other results means unknown. IDs are observations and never confer operation authority.

```powershell
py -3 crates/loomward-lab/check_windows.py
py -3 crates/loomward-lab/measure.py --sizes 100000 1000000 3000000 --passes 2 --threads 8
```

The first command creates three small profile trees, exercises pagination and guards, and removes
only its cleanup control. The second leaves all scale trees, starts a fresh process per run,
checks exact totals, compares the unchanged Python scanner at 100k, and saves raw JSON under
`evidence/v3/`. It also repeats a small handle-worker/buffer sweep at 1M. Neither command clears
OS caches or changes global policy. All timing labels are uncontrolled; no cold-cache claim.

Research and measurements: [enumeration-spike.md](../../docs/research/enumeration-spike.md).

## Native scan tiers

`bench scan --tier S|M|L --runs N [--cache-label first-after-generate]` uses marked mixed trees
named `mixed-100000`, `mixed-1000000` (G:) and `mixed-10000000` (E:). Generate L once with
`generate --root <private-E-lab-root> --files 10000000 --profile mixed --seed 42 --threads 8`.
Keep its selected path only in the ignored `.loomward/scan-roots.log`; published evidence uses
the label "10M lab tier (E: scale lab)". Leave this tree for later L17 runs.

Every scan run uses a fresh process. Complete scans must match the exact independent manifest
(files, descendant directories, logical and allocated bytes) and sink totals. L alone may pass
the N6 partial invariant oracle on engine limits: incomplete coverage, limit reported, observed
totals bounded by the manifest, sink agreement, repairing root and no sweep. This is not an
exact subset oracle: scheduling decides which directories fit the bounded arena. Full allocation
is null on partial scans; observed subset allocation remains separately labelled. S/M never
accept partial coverage. Engine caps and its shared 64 MiB accounting remain unchanged.

Results separate wall time through the last drained `ListingDone`, summed enumeration worker
time (includes backpressure), writer staging time, and bottom-up aggregation/final publication
time. Enumeration and staging overlap; do not subtract their timings from wall time.
The sink retains only directory aggregates: there is no SQLite state disk or durable publication.
`PeakPagefileUsage` and `PeakWorkingSetSize` are process-lifetime Windows high-water counters,
including manifest-plan validation and scan setup. p50 uses the median; nearest-rank p95 is null
below 20 runs. All measurements on the concurrent host are shared-host/uncontrolled, cache
warm/uncontrolled; `first-after-generate` is an observation label, never a cold-cache claim.

## Explicit real-root cross-check

`cross-check --root <absolute-local-root> --root-log <absolute-private-log>` needs no marker or
manifest. It refuses volume roots, profile/credential/browser components, namespace aliases and
reparse/offline root ancestors, requires a non-elevated token, and flushes a timestamp/root/reason
line before each traversal. The log must be outside the scanned root. The owner must explicitly
authorise the selected metadata scope; syntactic validation is not a grant.

Each of the four strategies runs in a fresh child process, with one worker for `std-single`,
eight by default for the others, and 64 KiB handle buffers. Output contains only aggregates,
timings, process CPU and lifetime peak working set. Exact comparison includes file count,
descendant-directory count, logical bytes, skipped reparse/offline/recall entries, and denied
directories. A disagreement is printed field by field and returns a nonzero exit code. Other
traversal errors fail visibly rather than masquerading as denied directories. `--strategy` selects
one child measurement; a reported traversal error still needs inspection of its `error` field.
Directory pins retain the synthetic walker sharing restrictions; this experiment is neither a
consistent snapshot nor a production coexistence benchmark. No file contents are read.

`real_stress.py --plan <private-absolute-JSON> --root-log <private-log> --synthetic` reproduces six
sweeps per real root (one initial uncontrolled sweep plus five warm sweeps), the Python comparison
on the smallest root, and five warm runs per strategy on identical marked G:/E: mixed 1M trees.
The private plan is a list of `{ "label": "real-A", "path": "<explicit-root>", "storage": "fast local volume" }`
objects, labelled consecutively, with three or four roots. Keep it outside the public worktree.
The Python scanner keeps per-file records in memory but exports aggregate context only; its
200k examination cap remains in force. Generation/validation primes synthetic caches; no cold
claim is made for those trees. Existing deterministic manifests are checked before reuse.

`check_real_windows.py --root-log <private-log>` tests manifest-free agreement, cyclic-junction
rejection, an actual denied directory and scope/log guards using one newly marked disposable G:
tree; its ACL change and cleanup touch only that control. Real experiment results and limitations:
[real-folder-stress.md](../../docs/research/real-folder-stress.md).
