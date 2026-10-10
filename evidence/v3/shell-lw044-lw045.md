# LW-044 / LW-045: read-only desktop contexts and shortcut health

Measured 2026-10-10 on Windows 11, Rust/Cargo 1.97.1, on the lane diff over base `2e35b0c`
before it was committed (branch `feat/shell-known-folders-shortcuts`). Commands below run from
the worktree root.

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
  tracker blocks. Unicode is decoded strictly; ANSI bytes retain their unknown
  producer code page, and only ASCII ANSI paths can be checked. Unknown ExtraData
  signatures remain visible and prevent a health claim.
- Health is evidence for the recorded LinkInfo path, never shell target resolution.
  It distinguishes present, missing, unavailable volume (including serial mismatch),
  network-unreachable unknown, ambiguous and unknown. Network, environment-dependent,
  relative and device paths never reach the target probe. Missing metadata rechecks
  the volume so a disconnect is not reported as a missing file.
- The explicit-path Windows reader reuses the existing fail-closed non-elevated root
  check, then retains ancestor/final metadata handles for the read. Those attribute-only handles do
  not deny writes or renames, and each component is reopened by full path, so an ancestor swapped
  between steps is not prevented (tracked in #248). Opens use no-recall flags and no-follow on the
  final component, refuse reparse/offline/recall
  attributes and non-disk objects, and check the byte cap before reading. Metadata
  health never reads target contents. Sharing/access/provider failures remain unknown.
- Bounds: 1 MiB file; 32,768 units per text field (ANSI bytes or UTF-16 code units);
  4,096 IDList records including the terminal; 64 ExtraData blocks. Offsets, nested
  sizes, terminators and truncation are checked before slicing or allocating.
- `Cargo.lock`: only the new local workspace crate entry. No new third-party crate.
  Existing `windows-sys` supplies known-folder/allocator and filesystem APIs;
  existing `loomward-windows` supplies admission and attribute refusal;
  existing `tempfile` supplies disposable test directories.

## Verified

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
unresponsive filesystem driver. Strict sharing/reparse/encoding refusal can reduce
coverage; unknown is intentional. Health is a point-in-time observation, not effect
authority, deletion evidence or a guarantee about what Windows would launch.

Reference oracle: Microsoft's [MS-SHLLINK structure index](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-shllink/3892236a-ef98-4f09-b184-871689b50977)
and [known-folder flags](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/ne-shlobj_core-known_folder_flag).
