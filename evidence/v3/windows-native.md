# Windows-native lane W evidence: LW-064, LW-003, LW-004

Measured on 2026-10-09 in a disposable worktree, branch `feat/windows-native`,
base/HEAD `1ee166d597b61cf935a84499351009f0e316857a`. Changes are uncommitted for the driver.
This is the first measurement of these native adapters, not the Python reference scanner.
Platform: Windows 11, OS build 26300, x86_64-pc-windows-msvc, rustc/cargo 1.97.1.
The Windows product-name compatibility API reports Windows 10 Pro; the measured build and
project host identify the Windows 11 environment. No elevation was requested.

## Changed

A workspace member `loomward-windows` supplies handle-derived identity, separate current
verification, and volume/device observations. No production effect API is present. The
laboratory module and binary require the explicit `fixtures` feature (unit tests also compile
it). No new third-party package/version was added: serde, serde_json and dev-only tempfile were
already workspace dependencies; windows-sys 0.61.2 was already locked.

Identity uses CreateFileW (FILE_READ_ATTRIBUTES, all three share modes, backup semantics,
open-reparse-point), FileIdInfo, FileBasicInfo, FileStandardInfo and FileAttributeTagInfo.
FileIdInfo unsupported errors permit BY_HANDLE_FILE_INFORMATION fallback, explicitly labelled
Legacy64 with the original reason. No usable ID yields Unsupported rather than a path hash.
The observed key and ancestor chain distinguish hard links, equal separate files, replacement
and a junction-swapped root even when its new leaf is the same hard-linked object. Exact
serial/byte/timestamp values serialize as decimal text, with 128-bit IDs retained as bytes.

Volume GUIDs, mount paths, filesystem serial/flags, capacity, drive type and storage properties
come from the requested native APIs. Each unreadable group is Unknown with the I/O reason.
Available <= free <= total is validated. Storage descriptors are checked for minimum length
and version/size headers. Bus, seek penalty and TRIM are device hints, not speed measurements
or a physical-device identity. Filesystem case-sensitive search is support, not directory mode.

Lab safety refuses non-descendants, the lab base itself, traversal, ambiguous Win32 root names,
unmarked nonempty roots, reparse ancestors and replaced identities. Pinned directory handles
prevent ancestor renames during fixture work. Cleanup checks unlisted entries and ADS before
any deletion, pins listed normal directories, rechecks identities, and unlinks only manifest
entries plus its own marker/empty root. No recursive delete, shell subprocess or global policy
change exists in the fixture implementation. Creation records entries before their effects;
partial/corrupt markers fail closed and are retained for inspection.

## Verified

The pre-edit `py -3 scripts/verify.py` baseline passed: 186 Python tests (one symlink-privilege
skip), two JS syntax checks, nine JS boundary assertions, 80 cross-language fixtures, the
existing 19 Windows Rust tests, fmt and clippy. No Python, JS, core, CLI or UI behavior was edited.

The initial new-crate tests compiled and failed (0/3) at the explicitly unimplemented fixture,
identity and volume seams. Subsequent tests exposed and pinned real defects before their fixes:
trailing-space roots were accepted; read-attributes-only handles failed to prevent renames;
available bytes could exceed free bytes; repeat creation mishandled long paths; and an unlisted
ADS was lost during cleanup. These cases now pass. No failing test was disabled or weakened.

Final proving commands, executed after the last Rust change:

| Command | Exit |
|---|---:|
| `cargo fmt --all --check` | 0 |
| `cargo test --workspace` | 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 |
| `cargo test --workspace --all-features` | 0 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 0 |
| `cargo clippy -p loomward-windows --all-features --all-targets --target x86_64-linux-android -- -D warnings` | 0 |

The required workspace test run executed **29 tests: 19 core + 10 Windows-adapter tests**,
with no failures or ignored Rust tests. Privilege cases emit named skips within the fixture
class test; they are not counted as successful symlink creation. The all-features run also
compiled the fixture CLI. The Android cross-target check compiles all non-Windows stub/test
code without warnings; it does not execute tests or establish Ubuntu/GNU-host compatibility.

Selected actual output tail (fmt returned no output):

```text
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.45s
    Checking loomward-windows v0.1.0 (<worktree>/crates/loomward-windows; local path redacted)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.05s
```

Native tests cover: cleanup refusing outside/unmarked roots and foreign files; idempotent
creation; ambiguous root aliases; pinned ancestor rename rejection; same-object hard links
versus separate equal files; current Same/Changed/Recreated/Gone results; junction root and
ancestor swaps with an unchanged hard-linked leaf; manifest traversal preserving the external
synthetic file; foreign ADS preserving its content; sparse/compressed attributes; ADS bytes;
read-only/hidden/system flags; >260 UTF-16-unit extended-length paths; Unicode/emoji; real
volume bounds; and explicit unknowns/invalid capacity. The deny-share fixture holds a native
share_mode(0) file handle, verifies another content open is denied, releases it and verifies
content access resumes. The current G: fixtures used FileId128, not fallback.

CLI proof after the last change:

```text
cargo run -p loomward-windows --features fixtures --bin loomward-fixtures -- create --root G:/loomward-lab/fixtures/sol-w-native
cargo run -p loomward-windows --features fixtures --bin loomward-fixtures -- create --root G:/loomward-lab/fixtures/sol-w-native
cargo run -p loomward-windows --features fixtures --bin loomward-fixtures -- destroy --root G:/loomward-lab/fixtures/sol-w-native
```

All three exited 0. The manifest had 21 entries (including attempted privilege-dependent
entries with no identity, retained as named skips). The final disposable root was confirmed
absent with Test-Path. The lab base remains reusable; test temp roots were automatically removed.
Only workspace-local build/receipt files remain in ignored `target/`.

## Skipped cases / NOT verified

- `case-variant-collision`: fixture entry already exists
- `file-symlink`: A required privilege is not held by the client. (os error 1314)
- `directory-symlink`: A required privilege is not held by the client. (os error 1314)
- `cloud-placeholder`: requires an explicitly configured cloud provider; not fabricated
- `removable-media`: requires physical media/disconnect; not fabricated
- `deny-share-lock`: in-test only; CLI cannot retain a lock after exit

The CLI lock skip is a lifetime limitation; the in-test deny-share case passed. No privilege,
Developer Mode, global case-sensitive policy, cloud provider or hardware state was changed.
Ubuntu/GNU execution, legacy-ID fallback on an actually unsupported filesystem, ReFS, network
filesystems, real offline/removable events, cloud placeholders and concurrent hostile writers
remain unverified. Tauri/WebView2, hosted CI, integration into a native catalogue or executor,
and performance/backup/deletion safety gates were outside this lane.

## Real volume inventory

Recorded at 2026-10-09T19:53:31+00:00 by:

```text
cargo run -p loomward-windows --features fixtures --bin loomward-fixtures -- volumes --json
```

The command exited 0 and returned nine volumes. Exact CLI JSON is in
[windows-native-volumes.json](windows-native-volumes.json), with no volume labels, device vendor/
serial strings, file names, content or user inventories. Capacity is a point-in-time snapshot;
free space can change while other work runs. All fields shown below were read without elevation.
Unmounted volumes are visible volumes with no mount paths, not invented empty disks.

| Volume GUID path | Mounts | FS | Total bytes | Free/available bytes | Bus | Seek penalty | TRIM |
|---|---|---|---:|---:|---|---|---|
| `\\?\Volume{redacted-1}\` | `(unmounted)` | FAT32 | 100663296 | 60779520 | nvme | False | True |
| `\\?\Volume{redacted-2}\` | `F:\` | NTFS | 524283904 | 487948288 | nvme | False | True |
| `\\?\Volume{redacted-3}\` | `G:\` | NTFS | 998613946368 | 219926507520 | nvme | False | True |
| `\\?\Volume{redacted-4}\` | `(unmounted)` | NTFS | 567275520 | 86945792 | nvme | False | True |
| `\\?\Volume{redacted-5}\` | `C:\` | NTFS | 1999323557888 | 130351624192 | nvme | False | True |
| `\\?\Volume{redacted-6}\` | `D:\` | NTFS | 524283904 | 485605376 | sata | True | False |
| `\\?\Volume{redacted-7}\` | `E:\` | NTFS | 1999373398016 | 912848498688 | sata | True | False |
| `\\?\Volume{redacted-8}\` | `(unmounted)` | NTFS | 497020928 | 33746944 | sata | True | False |
| `\\?\Volume{redacted-9}\` | `(unmounted)` | NTFS | 950005760 | 84369408 | nvme | False | True |

All observed NTFS volumes report hard-link/ADS/sparse/compression/reparse/case-sensitive-search
support and are writable by filesystem flag; this is not a file-access or operation grant.
FAT32 reports those six capabilities false. All nine report fixed drive type, non-removable
media and current filesystem availability. No benchmark or physical-device enumeration ran.

## Residual risk

Identity is an observation, not an atomic snapshot, grant or executor check. Before/after
ancestor comparisons reject demonstrated swaps but do not solve all concurrent metadata races;
IDs/creation timestamps may eventually be reused. The lab requires a private, quiescent tree and
one writer; its marker is not authentication against same-user malware. File/stream additions
can still race preflight, and cleanup is not transactional. Changed/corrupt entries fail closed,
so interrupted fixtures may need inspection rather than automatic cleanup. Do not export this
lab's mutation primitives into the default production library or IPC/MCP.

## Coordinator-owned status proposal

`docs/40-parallel-work-packs.md` assigns the capability matrix and `handoff/` to the coordinator.
This lane therefore proposes these updates for integration without overwriting those files:

- Append to `docs/16-implementation-status.md`: LW-064 disposable lab and LW-003/LW-004 native
  observation adapters measured on Windows; 10 adapter tests/29 workspace tests pass, fmt/clippy
  pass, symlink creation skipped for privilege, Android compile/lint only, no executor effects.
- Add `native.windows_adapter` in `handoff/CHECKPOINT.json`: identity/volume/lab implemented,
  `windows_tests_run: 10`, `default_library_read_only: true`,
  `fixture_effects_explicit_feature: true`, `ubuntu_runtime_verified: false`,
  `legacy_fallback_runtime_verified: false`, evidence paths for this report/volume JSON.
- Integrate the checkpoint's consolidated native counts only after the driver's combined gate;
  retain the original historical measurements and the actual skip limitations.

[HUMAN_TODO.md](../../HUMAN_TODO.md) was read and remains unchanged: q-1 tier confirmation,
q-2 Muse network confirmation, q-3 product direction and q-4 broader real-scan scope remain open.
The brief authorizes this disposable lab/volume-query lane; it does not infer those decisions.

## Tested source identity

No commit/push/branch creation or external publication occurred. The driver commits these
changes. SHA-256 of the final Rust sources and workspace manifests tested above:

| File | SHA-256 |
|---|---|
| `Cargo.toml` | `60f003629fbe171bc65fa29066bd99d26263b1a0d7a7ace555476903683475b0` |
| `Cargo.lock` | `aeca3acdb9b52ed9a1008906f267a2335136baeb405a6b44d2fad7b40964d9fe` |
| `crates/loomward-windows/src/bin/loomward-fixtures.rs` | `e8c26721a84a79f051effb5ac27e5568343e662136a3b47198d6bebdd933665f` |
| `crates/loomward-windows/src/fixtures.rs` | `de9a322dea91362b0ead657addafaa31a604bf3d3a39b049306210c4d16af3fa` |
| `crates/loomward-windows/src/identity.rs` | `3c8e7821e1d9c893ebe652f0aef3c0f65a797f11c86bcaba4eb197bc53814373` |
| `crates/loomward-windows/src/lib.rs` | `881e1afc7f7f03d0a29a27a287a37a8be607f344590e2d06d8430d6d6796f870` |
| `crates/loomward-windows/src/tests.rs` | `3277d2e7eee967f8ceb9069e8834d272b0a5a5625f6cdb840ba8eba671de2ac1` |
| `crates/loomward-windows/src/volumes.rs` | `6a2c5a613e103be7a561cd48749dbf7bb305d89ee443f4ccf1900fef5a212bc1` |
| `crates/loomward-windows/src/win.rs` | `88c1541b0a30beb92e217eb2084a97bab951dd6cdc46bf066cf8bfc707eafb98` |
| `crates/loomward-windows/Cargo.toml` | `07dae0eaa3c1de1c76a88ad388d4d8259850965e63e30b100633013e46f3d53e` |

Recommended logical commit subjects (the driver owns staging and integration):

1. `Add Windows identity and volume observations` (workspace registration/lock, default library,
   Win32 adapter, identity/volume modules and their portable/native observation tests).
2. `Add scoped Windows fixture laboratory` (opt-in module/bin and lab regression tests).
3. `Record Windows native fixture and volume evidence` (README, this receipt and volume JSON;
   coordinator integrates its owned status/checkpoint proposal).
