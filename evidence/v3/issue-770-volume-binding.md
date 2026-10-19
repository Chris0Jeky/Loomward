# #770: deep-path volume binding

Measured on KRASPYON, Windows 11, non-elevated, 2026-10-19. Base:
`291021d2ee07495c353cd5ebbe71e9d21f98c31e` (also `origin/main` at investigation).
Only disposable synthetic directories were used. No soak processes or data were touched.

## Step 0

| Requested item | State on origin/main | Evidence |
| --- | --- | --- |
| Diagnose the reported 534 | OPEN; original report misattributes its line | At #732's merge `a1925d95`, `native.rs:1522` calls `std::fs::create_dir`, not `volume_device` (line 1526). The reported `Os { code: 534, ... }` is an `io::Error`; this function would instead unwrap `SourceError::Io(534)`. |
| Bind the volume despite an unrepresentable full path | OPEN | `volume_device` queried only the complete descendant GUID spelling and propagated any API failure. |
| Exercise boundaries independently of the temp prefix | OPEN | The old fixture targeted 32,740 units of its input path, without measuring the actual GUID or NT spelling. |

The original test passed alone in this session, with `running_elevated() == false`.
Its original 534 was **not reproduced**, so neither the precise creation API that
produced it nor the reason that particular run failed is established. Rust's
[Windows directory builder](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/fs/windows.rs)
converts the path and calls `CreateDirectoryW`; a line-1522 panic cannot establish
a failure of `GetFinalPathNameByHandleW`.

## Discriminating measurements

Direct calls to `GetFinalPathNameByHandleW` with capacities 256, 32,768 and 65,536
showed that a larger caller buffer **does** allow GUID spellings longer than
32,767 units. For an input DOS spelling of 32,740 units, the GUID spelling was
32,782 units and the NT spelling 32,757. Buffer-growth arithmetic was not the
failure in this run.

The revised fixture measures the actual base spellings, then appends enough
synthetic name units to hit these exact lengths (excluding NUL):

| Full NT units | Full GUID units | Direct GUID query, 65,536-unit buffer |
| ---: | ---: | --- |
| 32,741 | 32,766 | Success |
| 32,742 | 32,767 | Success |
| 32,743 | 32,768 | Success |
| 32,766 | 32,791 | Success |
| 32,767 | 32,792 | Success |
| 32,768 | 32,793 | Returns zero; `GetLastError()` = 161 (`ERROR_BAD_PATHNAME`) |

At the last boundary all tested volume flags (DOS, GUID, NT, NONE, and
GUID with `FILE_NAME_OPENED`) failed, even with the largest buffer. The
distinguishing limit is the full NT object name, consistent with the byte counts
in [`UNICODE_STRING`](https://learn.microsoft.com/en-us/windows/win32/api/ntdef/ns-ntdef-_unicode_string).
The GUID output itself is not constrained to that length. These are API-level
observations; Windows internals were not traced in a debugger.

#732's [hosted Windows native job](https://github.com/Chris0Jeky/Loomward/actions/runs/39193994358/job/117872656727)
reported this test as `ok` alongside the other
elevation-guarded tests. The fixture explicitly returns early on an elevated
token; the repository's `canonical_tempdir` documentation records hosted Windows
runners as elevated. Thus a green hosted job does not prove this boundary ran.
Its captured log contains no token measurement (successful-test stderr is
suppressed), so an actual skip in that particular job remains an inference.
A shorter CI temp prefix is not established as the cause.

## Product behavior and regression

Normal handles retain the existing growing GUID-path query. For path-size errors
534, 161 or 206, the fallback queries `FileVolumeNameInformation` on the original
pinned handle, opens that NT device's short root via `GLOBALROOT`, and queries the
GUID of that root. It does not infer identity from a drive letter or compare
potentially colliding volume serials. A failed or malformed query refuses with an
error; it does not substitute another volume. This Windows 10+ native information
class is described as reserved for system use in Microsoft's
[enumeration documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/wdm/ne-wdm-_file_information_class).
It worked on this Windows 11 host; unavailable implementations remain errors.

The boundary regression failed before the fallback (`Io(161)` at the 32,768-unit
NT spelling), then passed with the same expected volume device at every boundary.
Fixture creation now uses `NtCreateFile` with a short child name relative to its
synthetic parent handle, avoiding the original absolute-path creation seam. The
non-elevated-token guard remains. All changes are confined to the Windows module.

## Verification

Commands ran from `C:/lw/wt/w12-770` with `CARGO_BUILD_JOBS=4` and
`CARGO_TARGET_DIR=C:/lw/target/lane-w12-770`:

| Command | Result |
| --- | --- |
| `cargo test --manifest-path C:/lw/wt/w12-770/Cargo.toml -p loomward-windows --all-features --lib volume_binding_handles_guid -- --nocapture` | Final fixture: 1 passed; six measured boundary cases. Earlier boundary regression: 1 failed with `Io(161)` before fallback, then 1 passed with fallback. |
| `cargo test --manifest-path C:/lw/wt/w12-770/Cargo.toml -p loomward-windows --all-features` | 251 passed (243 library + 8 property tests), 0 failed, 2 ignored. |
| `cargo clippy --manifest-path C:/lw/wt/w12-770/Cargo.toml --workspace --all-targets --all-features -- -D warnings` | Passed, exit 0. |
| `cargo fmt --manifest-path C:/lw/wt/w12-770/Cargo.toml --all --check` | Passed, exit 0. |
| `cargo test --manifest-path C:/lw/wt/w12-770/Cargo.toml --workspace --no-fail-fast` | 1,734 passed, 1 failed, 4 ignored; exit 101. Service failure detailed below. Windows library: 243 passed, 2 ignored. |
| `cargo test --manifest-path C:/lw/wt/w12-770/Cargo.toml -p loomward-service --lib tests::f3_duplicates_expired_request_releases_the_refresh_fence -- --exact --nocapture` | Isolated rerun: 1 passed, 405 filtered out; exit 0. |
| `C:/lw/target/lane-w12-770/debug/deps/loomward_service-07feddb16a65281c.exe --exact tests::f3_duplicates_expired_request_releases_the_refresh_fence --nocapture` | Exact binary from the failed workspace run: 1 passed alone, 405 filtered out; exit 0. |
| `git -C C:/lw/wt/w12-770 diff --check` | Passed, exit 0. |

The full run's only failure was the unchanged service test
`f3_duplicates_expired_request_releases_the_refresh_fence`, at
`crates/loomward-service/src/tests.rs:5463`: the returned code did not match
`Busy | DeadlineExceeded`. Its request uses a 300 ms deadline. Both the isolated
Cargo rerun and the **same workspace test executable** passed alone, consistent
with the lane brief's warning about existing service timing failures under load.
The full run remains failed; the exact unexpected error code was not printed by
that assertion, and its causal explanation was not independently established.
No service assertions or code were changed. The two ignored Windows laboratory
tests are `durable_stop_20k_lab` and `throughput_lab`.

Original 534 reproduction, hosted CI for this uncommitted diff, other Windows
versions, and other-volume hardware were not verified. `HUMAN_TODO.md` was read
and left unchanged; this repair does not require an owner decision or change a
safety gate. The original 534 diagnosis stays open in #770: the exact next
diagnostic is to capture the full original failing panic plus the failing
creation call's input length and immediate error in that environment.
The separate `normalized_path` containment check is unchanged; successful volume
binding does not prove that an entire scan at these path lengths can proceed.
