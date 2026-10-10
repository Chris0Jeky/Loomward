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
Generation never overwrites an existing root. It reserves 20 GB (decimal) free space, limits a tree to
3M files, sparsifies before extending files, and refuses completion if payload allocation or the
observed generation volume delta exceeds 20 GB (decimal). Volume deltas include concurrent unrelated
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
The private plan is a list of `{ "label": "real-A", "path": "<explicit-root>", "storage": "NVMe" }`
objects, labelled consecutively, with three or four roots. Keep it outside the public worktree.
The Python scanner keeps per-file records in memory but exports aggregate context only; its
200k examination cap remains in force. Generation/validation primes synthetic caches; no cold
claim is made for those trees. Existing deterministic manifests are checked before reuse.

`check_real_windows.py --root-log <private-log>` tests manifest-free agreement, cyclic-junction
rejection, an actual denied directory and scope/log guards using one newly marked disposable G:
tree; its ACL change and cleanup touch only that control. Real experiment results and limitations:
[real-folder-stress.md](../../docs/research/real-folder-stress.md).
