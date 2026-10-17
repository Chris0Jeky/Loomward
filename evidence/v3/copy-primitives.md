# C2 native copy primitives: first measurements

Date: 2026-10-17. Host: **KRASPYON**, Windows 11 Home 10.0.26100,
NTFS C:. These are first measurements on **one host and one filesystem**,
using only disposable directories created by the tests. They are not a
copy-and-verify safety-gate pass, a power-loss test, or a personal-file effect.
Refs #415; design: [doc 47](../../docs/47-copy-and-verify.md).
Starting commit: `89df38682480f480873d50d2f65f1f10c6d752e5`.

## Changed

`loomward-windows::effects` is Windows-only and requires the new, non-default
`copy-primitives` feature. It implies the existing `content-read` feature so the
source reader can reuse `duplicate_reader`'s pinned ancestors, metadata-first
checks, deny-write handle, bounded/cancellable reads, full signature and final
reopen-by-name check. The existing handle identity query is shared, not replaced.
No other Cargo manifest enables `copy-primitives`; no service command, HTTP route,
desktop IPC handler, or MCP tool calls this module. AGENTS.md invariant 1 remains
closed. No dependencies, application transports, authority files or status
checkpoints changed. `HUMAN_TODO.md` q-32/q-47 and doc 47's owner questions remain
with the driver; this lane makes no owner decision.

The primitives provide verified destination-parent pins; create-new staging,
directories, files and named streams; separate identity capture and preallocation;
1 MiB buffered writes with SHA-256 and a final flush; basic metadata with a second
flush; aligned, non-cached ID-based read-back and deny-write holds; no-replace
same-parent publish; and identity-bound cleanup of files and empty directories.
Failed/partial writes remain staged and cannot be appended as a new copy.

Cleanup first validates the original staging name and identity. It then validates
the recorded object by ID and independently by its manifest name through pinned
parents. A name-opened delete handle denies competing write/delete opens. Files
must have exactly one link; a replacement, changed creation time, reparse parent,
renamed staging folder, nonempty directory, unrecorded entry, or published staging
is retained. There is no recursive/path-based delete primitive.

These are low-level primitives: the later journal/executor must durably record
identities before preallocation/writes, withdraw publish intent before cleanup,
and fence publish with consent and grant epochs. Creation-time preservation
changes `ObjectKey.creation_time`: journal the planned metadata timestamp before
setting it and use the resulting identity afterwards, including crash recovery.
This lane does not implement that journal, consent, recovery, destination
cloud/protected-root policy, retry scheduling, or post-publish tree confirmation.

## Hypotheses measured

| Hypothesis | Result on this host | Evidence |
|---|---|---|
| H1: relative named `$DATA` stream opens/creates | PASS on NTFS. `RootDirectory` is the existing file handle; the name is `:<name>:$DATA`. Create-new collisions refuse. | `h1_relative_named_stream_open_measurement`, `named_stream_create_new_write_and_noncached_verify`, `named_stream_source_keeps_bytes_and_deny_write_holds`; source and stream holds reject writers; more than 64 named streams refuses. |
| H2: held children versus ancestor publish | PASS for the hold-preserving path. The folder publishes while its child has Loomward's non-cached `FILE_SHARE_READ | FILE_SHARE_DELETE` hold. A separate test process holding a child without delete sharing blocks publish with Win32 error **5**; dropping that process's handle permits publish. | `create_new_and_publish_never_replace`, `h2_child_handle_without_delete_share_blocks_publish` (subprocess handshake, not a timing delay). Existing final names and their bytes remain unchanged. |
| H3: flushed bytes read back below the system cache | PASS for sizes **0, 1, 511, 512, 4095, 4096, 4097, 1,048,577 bytes**, plus a named stream. Byte counts and SHA-256 agree, including the short final sector. | `noncached_partial_sector_sizes`, `named_stream_create_new_write_and_noncached_verify`. `IOCTL_STORAGE_QUERY_PROPERTY` / `StorageAccessAlignmentProperty` reports **512-byte** alignment; offsets, buffers and requests use that alignment. A deliberately changed byte produces a different verification digest. |
| H4: creation/last-write times survive flush, close and ID reopen | PASS for synthetic 100 ns timestamps `133000000000000000` and `133000123456789000`. Read-only/hidden/archive bits also survive. Directory and staging-root metadata survive the corresponding operations. | `metadata_survives_reopen_and_readonly_cleanup`, `bottom_up_directory_metadata_and_identity_bound_cleanup`, `staging_root_metadata_remains_bound_through_publish`. Access time is best effort; change time is not preserved. |
| H5: NULL security descriptor inherits destination ACL | PASS for this temp-directory parent: the new file has **6 inherited ACEs, zero explicit ACEs**. | `h5_null_descriptor_inherits_all_aces`, querying the new handle's DACL and every ACE header. This is not evidence for all ACL configurations. |

Additional cleanup measurement (H6): the read-only staged file is removed using
`FileDispositionInfoEx` with DELETE, POSIX_SEMANTICS and IGNORE_READONLY_ATTRIBUTE
on its verified name-opened handle. The code retains a clear-read-only fallback
only for an unsupported disposition flag; that fallback is UNVERIFIED here.

### API details exposed by measurement

- A pinned directory as `OpenFileById`'s volume hint returns error **50** on
  this host, even with no flags and a 64-bit NTFS descriptor. A **zero-access
  volume handle** succeeds with NO_BUFFERING, OPEN_NO_RECALL and
  OPEN_REPARSE_POINT together. It supplies ID lookup and geometry, never raw
  disk reads or writes. Every reopened object's full volume serial, ID,
  creation time and reparse tag must still match the recorded identity, and
  the manifest name is separately checked under the pinned destination.
- NTFS uses the 64-bit descriptor when the extended descriptor is unsupported
  and the upper ID bytes are zero. The full identity is revalidated afterwards;
  IDs are not truncated silently and no path-based reopen fallback exists.
- Directory creates cannot combine FILE_DIRECTORY_FILE and FILE_OPEN_NO_RECALL.
  Only create-new directory calls omit NO_RECALL; existing-object opens retain
  it and validate the returned directory handle. No existing object is adopted.
- POSIX disposition on an ID-only file handle returned error **87** here,
  including after clearing read-only. Cleanup therefore validates by ID but
  disposes the matching, delete-exclusive **name-opened** handle. Link count and
  disposition are checked/issued on that same handle.
- The rename primitive allocates an aligned, NUL-terminated FILE_RENAME_INFO
  buffer and keeps ReplaceIfExists false. The Win32 relative-name call returns
  **123** on this host, including with the correctly sized/terminated buffer.
  The measured publish successes and refusals use the pinned-parent
  `NtSetInformationFile(FileRenameInformation)` fallback with that same buffer,
  never an absolute-path fallback or a replacement rename.

The non-cached read proves readability below the Windows system cache. It does
**not** establish persistence through the device's cache or power loss.

## Verification

Every cargo invocation sets `CARGO_BUILD_JOBS=4` and
`CARGO_TARGET_DIR=C:\lw\target\effects-c2`. Commands are run against this
worktree's root manifest; no primary checkout is used. Raw local logs are kept
under the gitignored `.loomward/` directory, never in this public evidence.

- Regression-first run: the new tests failed to compile before the primitives
  existed. Subsequent native failures exposed the directory flag, ID-hint and
  disposition details above; the tests were not skipped or weakened.
- `cargo test -p loomward-windows --all-features effects::tests -- --nocapture`:
  **22 passed, 0 failed** before adding the final named-stream-writer regression.
  That new test was independently observed failing before its refusal mapping
  was fixed. A geometry regression also pins refusal of missing/non-power-of-two
  or incompatible physical-sector information instead of silently substituting
  the logical sector size. The final full crate run passes **all 24 effects tests**.
- `cargo fmt --all --check`: PASS.
- `cargo clippy -p loomward-windows --all-targets --all-features -- -D warnings`:
  PASS.
- `cargo test -p loomward-windows --all-features`: **123 passed, 1 failed**.
  The failure is the pre-declared #292 host failure
  `enumerate::tests::reopen_follows_identity_after_rename_and_rejects_changed_volume_or_root`
  (`Io(50)`), outside this lane. Its code and test are unchanged.
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS.
- `cargo test --workspace --no-fail-fast`: build failed before tests ran.
  rustc compiling unchanged `loomward-core` reported
  `memory allocation of 1458192 bytes failed` and aborted with
  `0xc0000409 / STATUS_STACK_BUFFER_OVERRUN`. This is a host resource failure,
  separate from the pre-declared #292 runtime test failures. Available memory
  subsequently measured 4,447 MiB. A single-job retry (`-j 1`, same target and
  test/profile settings) completed: **983 passed, 10 failed, 0 ignored** across
  all test targets and doc tests. No tests, profiles or deadlines were weakened.
  Five failures match the supplied #292 list:
  - `scan::tests::followups::native_relist_containment_refuses_moved_out_directory`
  - `scan::tests::followups::native_relist_containment_allows_move_within_root`
  - `scan::tests::followups::native_relist_containment_follows_renamed_root_handle`
  - `scan::tests::native_dirty_epoch_relists_by_verified_identity`
  - `enumerate::tests::reopen_follows_identity_after_rename_and_rejects_changed_volume_or_root`
  The additional service failures are recorded below, not classified as #292.
- `git diff --check`: PASS after the final source and evidence edits.
- Initial implementation handoff: subsequently published as [PR #503](https://github.com/Chris0Jeky/Loomward/pull/503).
  Hosted CI was not run by this Sol lane.

### Additional service failures, outside C2

The workspace service target ran **175 passed, 5 failed**. Four failures
reported `DeadlineExceeded`; the fifth expected a `catalog_rev_changed` detail
but got no reason. The failing tests were:

- `tests::collections_tests::collections_members_keep_missing_revoked_and_bind_cursors`
- `tests::collections_tests::collections_list_resolves_references_once_for_all_collections`
- `tests::collections_tests::collections_hydration_interrupt_reaches_running_inspection`
- `tests::meaning_tests::meaning_is_bounded_read_only_advice_with_explicit_unknowns`
- `tests::n4_recovery_replays_the_retained_outcome_and_never_overwrites_a_later_choice`

A crate-only Cargo replay began rebuilding a different dependency feature graph
and was deliberately stopped before it ran tests. Only that identified build
process and its compiler child were stopped; no other lane was interrupted.
The exact already-built workspace service binary was then run instead:

`C:/lw/target/effects-c2/debug/deps/loomward_service-2cd7cfb8d03ab454.exe --test-threads=1`

Result: **179 passed, 1 failed**. All five original failures passed, but
`inventory::tests::wrong_dataset_and_timed_out_providers_never_publish_fallback`
failed at `src/inventory/tests.rs:166`: its 20 ms request returned the expected
deadline error before the provider was entered, so `slow.calls` was **0**, not
the expected **1**. Replaying only that test with `--exact` and
`--test-threads=1` also failed (**0 passed, 1 failed, 179 filtered out**).

These are separate observations of deadline/scheduling-sensitive service tests,
not a green workspace result or a claim that every service failure is
pre-existing. Service code and tests are unchanged, and the C2 feature is absent
from this compiled workspace service path. The bounded diagnosis ends here;
the provider fixture remains a separate driver follow-up. Raw receipts are in
`.loomward/c2-workspace-tests-serial.log`,
`.loomward/c2-service-existing-binary-serial.log` and
`.loomward/c2-service-provider-isolated.log`.

## PR #503 fix round (2026-10-17)

Applied to committed head `868d700`, in the same KRASPYON/NTFS laboratory.
Changes remain uncommitted for the driver; this lane does not push or merge.

### Reachability pin

`duplicate_reader::tests::copy_primitives_are_unwired_outside_windows` walks the
workspace, including `native/`. Only this crate's manifest may name
`copy-primitives`, and Rust sources outside this crate must not mention
`effects::` or `loomward_windows::effects`. Build outputs, dependency caches,
Git internals and private laboratory state are excluded from the walk.

Mutation proof: temporarily adding a `copy-primitives` comment to the root
`Cargo.toml` made
`cargo test -p loomward-windows --all-features copy_primitives_are_unwired_outside_windows -- --nocapture`
fail **0 passed, 1 failed**, naming that manifest. Restoring its original bytes
made the same command pass **1 passed, 0 failed**. The probe is not retained.

### M1: effective directory rename guards

Before the fix,
`cargo test -p loomward-windows --all-features rename_is_blocked -- --nocapture`
failed **0 passed, 3 failed**: another thread successfully renamed the staging
folder while `guard()` was held, the destination parent while `Destination`
was held, and a nested staged directory while its parent pin was held.

The three opens now request `FILE_LIST_DIRECTORY` as well as attributes, and
continue to omit `FILE_SHARE_DELETE`. All three regressions pass in the focused
effects suite: the competing thread gets **32 (ERROR_SHARING_VIOLATION)**;
dropping the relevant guard/pin permits the rename. No timing delay is used.
This measures these three handles; it does not establish an ancestor-wide
rename lock for attribute-only handles held by other modules.

### M3: refuse inherited or subsequently changed attribute classes

`FSCTL_SET_COMPRESSION` successfully set the compressed bit on the tests' own
temporary directories and files on this volume. Before the fix,
`cargo test -p loomward-windows --all-features compressed_ -- --nocapture`
failed **0 passed, 4 failed**: a compressed destination parent permitted
staging, a compressed staging root permitted child creation, a compressed
staged file verified, and `BasicMetadata::read` masked away compression.

Those four regressions now pass. The destination parent is checked before
creating staging; the newly created staging directory, traversed staged
directories, new files, ID-based reopens, stream verification and metadata
reads check full attributes before returning success or applying the preserved
mask. `UnsupportedAttribute` is a typed payload inside `io::Error`
(`InvalidInput`, display code `unsupported_attribute`) carrying the refused
bits. The same mask covers compressed, encrypted, integrity-stream, sparse,
system and temporary attributes. The unit regression
`unsupported_staged_attributes_have_a_typed_refusal` covers each flag and
checks the downcastable payload, while preserved/normal/directory bits pass.
Real encrypted, integrity-stream and sparse-object setup is **UNVERIFIED**;
their attribute-classification seam is tested. Refused staging is retained;
this adds no cleanup adoption or personal-file capability.

`cargo test -p loomward-windows --all-features effects::tests -- --nocapture`:
**32 passed, 0 failed** after both fixes, including all previous effects tests.
Local red/green receipts are `.loomward/fix-reachability-{red,green}.log`,
`.loomward/fix-rename-red.log`, `.loomward/fix-attributes-red.log` and
`.loomward/fix-effects-green.log`.

### Fix-round proving checks

All commands below use the root `--manifest-path`, `CARGO_BUILD_JOBS=4` and
`CARGO_TARGET_DIR=C:\lw\target\effects-c2`, as above.

- `cargo fmt --all --check`: PASS.
- `cargo check -p loomward-windows --features copy-primitives`: PASS,
  **without** `fixtures` or `--all-features`.
- `cargo clippy -p loomward-windows --all-targets --all-features -- -D warnings`:
  PASS.
- `cargo clippy -p loomward-windows --all-targets -- -D warnings`: PASS.
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS.
- `cargo test -p loomward-windows --all-features`: **147 passed, 0 failed**
  (139 library tests and 8 decoder integration tests). The previously recorded
  #292 Windows failure did not reproduce on this head in this run.
- `cargo test --workspace --no-fail-fast`: **1,124 passed, 2 failed, 2 existing
  ignored**, exit 101, including integration and doc tests. The supplied five
  #292 host failures did not reproduce. Failed targets were the catalogue and
  service libraries, detailed below; the copy feature is not enabled in this
  workspace run. Raw receipt: `.loomward/fix-test-workspace.log`.
- One `gh-pulse.ps1 -Repo Chris0Jeky/Loomward -Numbers 503 -Json` snapshot:
  PR #503 is non-draft, head `868d700`, CI rollup SUCCESS, no pending/failed
  checks, two unresolved review threads. That hosted result covers the committed
  head, **not these uncommitted fixes**; the driver owns push and thread closure.

The workspace run reported an additional failure outside the supplied #292
list: `writer::tests::end_run_checkpoint_does_not_wait_for_a_reader_and_restores_write_timeout`
in unchanged `loomward-catalog/src/writer.rs:2395`. Its one-second
`recv_timeout` returned `Timeout` rather than the `EndRun` receipt. The
catalogue library target reported **54 passed, 1 failed**. This is not labelled
a known baseline failure or a green workspace result. Catalogue code and its
test deadlines are unchanged.

One narrow replay using the exact already-built workspace binary:
`C:/lw/target/effects-c2/debug/deps/loomward_catalog-a5f487d0e8a3d9a9.exe writer::tests::end_run_checkpoint_does_not_wait_for_a_reader_and_restores_write_timeout --exact --nocapture`
passed **1 passed, 0 failed** (15.98 s total). The test and its one-second
receipt deadline were not changed. This records a successful isolated replay;
it does not erase the workspace failure or establish its root cause. Receipt:
`.loomward/fix-catalog-isolated.log`.

The service library reported **189 passed, 1 failed**:
`inventory::tests::wrong_dataset_and_timed_out_providers_never_publish_fallback`
at `src/inventory/tests.rs:166`, expected provider calls **1**, observed **0**.
This matches the separately recorded initial implementation observation above:
the request can expire before entering its slow provider. It is not one of the
supplied #292 failures. Service code/tests remain unchanged; the provider fixture
is still a driver follow-up, and the workspace run remains failed.

Final `git diff --check`: PASS. The working tree contains only the reachability
test, effects implementation/tests and this evidence update; no commit or push
was made. `HUMAN_TODO.md` q-32/q-47 is unchanged and remains with the driver.

The compression test uses Microsoft's documented
[FSCTL_SET_COMPRESSION](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_set_compression)
on test-owned objects only. Access and sharing constants follow
[NtCreateFile](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntcreatefile);
the rename outcomes above are host measurements.

## NOT verified and residual risk

ReFS and other hosts; files above 4 GiB; 200,000 simultaneous holds; every ACL
inheritance shape; the unsupported-read-only-flag fallback; mapped-section H7;
SHA-256-versus-storage throughput; cloud/provider recall behavior; the full
refusal/fault/crash matrix and post-publish confirmation; real power cuts and
proven restore: **UNVERIFIED**. Large-file and handle-limit stress runs were not
attempted on this shared 16 GB host. G5/G9 remain closed, along with personal
copy capability. No physical durability or complete operation receipt is claimed.

## API references

The implementation and measurements were checked against Microsoft's
[OpenFileById](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-openfilebyid),
[FILE_ID_DESCRIPTOR](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_descriptor),
[file buffering](https://learn.microsoft.com/en-us/windows/win32/fileio/file-buffering),
[FILE_RENAME_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_rename_info),
and [extended disposition](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddk/ns-ntddk-_file_disposition_information_ex)
references. Documented intent and this host's observed behavior are kept distinct.
