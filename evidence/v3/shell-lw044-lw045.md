# LW-044 / LW-045: read-only desktop contexts and shortcut health

Measured 2026-10-10 on Windows 11, Rust/Cargo 1.97.1, on the lane diff over base `2e35b0c`
before it was committed (branch `feat/shell-known-folders-shortcuts`). The #248 follow-up below
records subsequent checks. Commands run from the worktree root.

## Changed

- `crates/loomward-shell/src/known_folders.rs`: five known-folder contexts, per-user
  versus public ownership, filesystem versus virtual locations, visible provider
  failure. The Windows provider uses `SHGetKnownFolderPath` for Desktop, Public
  Desktop, Documents, Downloads and OneDrive. Current and default paths retain
  redirection as data. `KF_FLAG_DONT_VERIFY` prevents existence checks; no create,
  enumeration or recursion is requested. A configured path does **not** establish
  an online provider or an existing folder. An unavailable default query stays `None`.
  `CoTaskMemFree` only frees the API's returned allocation; no COM object or apartment
  is created. Virtual items are represented in the provider model, not enumerated
  through the composite shell desktop.
- `crates/loomward-shell/src/shortcuts.rs`: pure byte parser for the header and flags,
  opaque but structurally bounded IDList, local/network LinkInfo, volume type/serial
  and label, all five separate StringData fields, environment, known-folder and
  tracker blocks. Valid UTF-16 is decoded; unpaired surrogates retain exact `Text::Wide`
  units rather than rejecting the link. ANSI bytes retain their unknown
  producer code page, and only ASCII ANSI paths can be checked. Unknown ExtraData
  signatures remain visible and prevent a health claim.
- Health is evidence for the recorded LinkInfo path, never shell target resolution.
  It distinguishes present, missing, unavailable volume (including serial mismatch),
  network-unreachable unknown, ambiguous and unknown. Network, environment-dependent,
  relative and device paths never reach the target probe. Missing metadata rechecks
  the volume so a disconnect is not reported as a missing file.
- The explicit-path Windows reader reuses the existing fail-closed non-elevated root
  check and relative-open helpers. Only the drive anchor opens by absolute path; every
  child opens relative to its already-open parent with no-follow and no-recall flags.
  An opened ancestor's pathname can change, but that does not redirect subsequent opens.
  Encountered reparse/offline/recall attributes and non-disk objects are refused with a reason.
  The final content handle supplies both the size check and the bounded read; it is never
  reopened by pathname. Content access denies concurrent writers and permits leaf renames;
  attribute-only handles do not promise to prevent writes or renames. Metadata
  health never reads target contents. Sharing/access/provider failures remain unknown.
- Bounds: 1 MiB file; 32,768 units per text field (ANSI bytes or UTF-16 code units);
  4,096 IDList records including the terminal; 64 ExtraData blocks. Offsets, nested
  sizes, terminators and truncation are checked before slicing or allocating.
- `Cargo.lock`: only the new local workspace crate entry. No new third-party crate.
  Existing `windows-sys` supplies known-folder/allocator and filesystem APIs;
  existing `loomward-windows` supplies admission and attribute refusal;
  existing `tempfile` supplies disposable test directories.

## Original #245 verification

| Command | Result |
| --- | --- |
| `cargo test -p loomward-shell --test shell` | PASS: 15 tests, no failures or skips |
| `py -3 crates/loomward-shell/tests/mutate.py` | PASS: 15/15 compiled mutations killed; restored 15-test suite passes |
| `cargo fmt --all --check` | PASS, exit 0 |
| `cargo test --workspace` | PASS: 480 tests, 0 failed, 0 ignored; exit 0 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, exit 0 |
| `git diff --check` | PASS, exit 0 |

Because the driver has not staged the slice, whitespace was also checked with
`git diff --no-index --check -- /dev/null <new-file>` for all seven new files.
No diagnostics were emitted (the no-index comparison returns 1 for added content).

Regression-first proof: the initial compiling stubs produced 10 assertion failures
and one passing source guard. The finished suite has 15 tests. Each final test also
has a compiled failing mutation; compilation failures do not count as kills. The
mutation runner restores source bytes in `finally` and reruns the intact suite.

| Test | Deliberate regression caught |
| --- | --- |
| `header_flags_and_opaque_id_list` | Drop header flags |
| `string_fields_are_separate_unicode_and_inert` | Exchange argument and icon fields |
| `ansi_bytes_are_preserved_without_guessing_code_page` | Corrupt ANSI bytes |
| `malformed_and_oversized_inputs_are_refused` | Remove parser byte cap |
| `local_link_info_and_health_keep_volume_absence_distinct` | Report unavailable volume as missing |
| `network_link_info_is_never_probed` | Lose the network classification |
| `environment_known_folder_tracker_and_unknown_blocks_stay_data` | Corrupt tracker birth IDs |
| `relative_opaque_and_device_targets_are_not_probed` | Lose relative-path ambiguity |
| `known_folders_preserve_redirection_ownership_unicode_and_failures` | Mark public context per-user |
| `source_has_no_execution_com_initialisation_or_library_loading` | Inject a forbidden source token |
| `windows_known_folder_shape_and_disposable_shortcut_read` | Discard reader bytes before parsing |
| `unicode_link_info_paths_volume_and_network_offsets` | Ignore Unicode offsets |
| `hostile_extra_blocks_utf16_and_counts_are_bounded` | Relax ExtraData block-count cap |
| `health_rechecks_volume_and_refuses_resolver_flags` | Lose disconnect reclassification |
| `windows_reader_cap_local_metadata_and_sharing_refusal` | Corrupt native metadata classification |

Fixtures are hand-built synthetic link bytes and fake folder/target providers. The
hostile argument remains an inert field. Windows tests resolve real known folders
without logging paths, asserting only shape, and read only the synthetic files in
their own disposable temp directory. They also exercise local metadata, missing
metadata, sharing refusal, oversized/truncated reads and network-path refusal. The
non-elevated temp-file reads ran; no user's Desktop entries were read. The source
guard covers all production Rust source and rejects process execution, COM
initialisation, dynamic library loading and shell-link interfaces.

## NOT verified

Real OneDrive hydration/offline transitions, removable-media disconnects, drive
replacement and native network failures were not induced. Their classification is
tested with synthetic providers; real network targets are deliberately never probed.
No live shell IDList resolution, icon extraction, repair, application/service/protocol
wiring, package or release is part of this slice. Python/UI suites were not rerun:
those seams are unchanged. Non-Windows execution and a broad real-world link corpus
were not tested; unsupported native calls there report unavailable/unknown.

## Residual risk

OS metadata calls are synchronous and cannot promise a hard deadline against an
unresponsive filesystem driver. Sharing/reparse refusal and unknown path encoding can reduce
coverage; unknown is intentional. Health is a point-in-time observation, not effect
authority, deletion evidence or a guarantee about what Windows would launch.

Reference oracle: Microsoft's [MS-SHLLINK structure index](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-shllink/3892236a-ef98-4f09-b184-871689b50977)
and [known-folder flags](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/ne-shlobj_core-known_folder_flag).

## #248 follow-up: relative opens and guard coverage

Checked on Windows on the uncommitted lane diff over `ee3987763923f553b147e6f4663a31bb0f3d41c7`,
branch `fix/shell-relative-opens-248`. No commit or push was made.

- `loomward-windows::enumerate` exposes two read-only helpers: guarded directory opens
  and validated single-child opens. They reuse `root_handles` / `open_relative_mode`,
  fail closed on elevation, validate child names and attributes, and translate NTSTATUS
  errors to Win32 errors so absent metadata remains Missing rather than Unknown.
- Shortcut reads and target metadata use the same relative walk. Deterministic callbacks
  in tests replace a checked ancestor with a disposable local junction between opens;
  both content and metadata walks retain the original object. A separate test replaces
  the leaf pathname after opening it and verifies the original link bytes are read.
  A junction already present in the path is refused with a reparse reason.
- The guard recursively walks `src/`. It rejects `process` and `Command` identifiers,
  including grouped imports, aliases and whitespace around `::`, plus all forbidden
  execution, COM, icon, shell-parser and dynamic-loader names requested by #248.
- Removable and optical targets return Unknown immediately after `GetDriveTypeW`;
  neither `GetVolumeInformationW` nor target opens run for these classes. This deliberately
  sacrifices their health coverage to avoid media activation and no-disk dialogs.
- Unpaired UTF-16 surrogates are preserved verbatim. `as_str()` returns None for them,
  so health remains Unknown without probing an undecodable path. Valid Unicode and
  unknown-code-page ANSI handling remain distinct.
- A dependency-free deterministic PRNG supplies 12,000 random or mutated link inputs,
  including valid seeds, corrupted offsets/counts and truncations. Parsing never panicked.

| Command | Result |
| --- | --- |
| `cargo test -p loomward-shell` | PASS: 22 tests (18 integration + 4 Windows unit), no failures or ignored tests |
| `py -3 crates/loomward-shell/tests/mutate.py` | PASS: 31/31 compiled mutations killed; restored 22-test suite passes |
| `cargo fmt --all --check` | PASS, exit 0 |
| `rustfmt --edition 2021 --check crates/loomward-shell/tests/support/native.rs` | PASS, exit 0 |
| `cargo test --workspace` | PASS: 523 tests, 0 failed, 0 ignored; exit 0 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, exit 0 |
| `git diff --check` | PASS, exit 0 |

New tests each have a compiled failing mutation. The Unicode regression first failed
with the old strict decoder; the nested-source guard first failed with the old coverage;
the removable/optical test first failed when media probes still ran. Mutation checks
restore the old full-path component walk and final-path reopen, remove no-follow, revert
raw UTF-16 preservation, allow media queries, disable recursion/process detection and
remove each newly forbidden API name individually. The parser property catches an
injected unchecked short-input panic. Compile failures are not mutation kills.

Fixtures contain synthetic link bytes and disposable local junctions only. The native
mutation failures establish that the junction, swap and pinned-read tests actually ran
without taking their elevated-host refusal branch. No network share was contacted.

### NOT verified / residual risk for #248

Physical removable/optical media, live UNC targets, provider hydration and hostile
filesystem drivers were not exercised. The media test verifies the policy's no-probe
branch; the junction tests use local synthetic targets. Non-Windows execution, hosted
CI, shell resolution, repair and release packaging were not run. The random-input test
is bounded regression coverage, not exhaustive fuzzing. Metadata remains point-in-time;
an unresponsive filesystem driver can still block a synchronous call.
