# LW-047: side-effect-free application inventory

2026-10-10. Windows 11, `x86_64-pc-windows-msvc`, rustc/cargo 1.97.1.
Starting source: `cf27a4ec6734026d1bb3a9d0b393c7e294e4eb60`, branch
`feat/app-inventory-lw047`. Results below cover the uncommitted lane diff.

## Changed surface and coverage

- New `loomward-apps` workspace crate, automatically included by the root
  `members = ["crates/*"]`. No protocol, service or UI wiring in this slice.
- Platform-neutral `RegistryReader`, raw values, application evidence, scopes,
  and provider reports. Missing, known and unknown fields remain distinct.
- Windows adapter reads the Uninstall views of HKLM 64-bit, HKLM 32-bit
  (WOW6432Node via redirection), and the current HKCU 64-bit view. Both root and
  entry handles use `KEY_READ` and the explicit `KEY_WOW64_*` flag. Handles close
  on all return paths. Other users' hives are outside coverage.
- Duplicates are retained separately with their provider/view and registry key;
  a matching display name is not an identity. Registry origins classify as
  per-machine or per-user. Portable origin has its own scope; unsupported origins
  stay unknown. Portable discovery is explicitly unavailable, never inferred from
  absence in the registry.
- Publisher, install location, uninstall string and display icon remain opaque
  UTF-16 evidence text. `REG_EXPAND_SZ` keeps its type and literal environment
  references. No command parsing, environment expansion, path resolution, icon
  loading, filesystem existence checks, process launch, WMI or MSI calls.
- `SystemComponent=1` and `ParentKeyName` are retained. Malformed flags, wrong
  registry types, invalid UTF-16 and embedded NULs become unknown evidence;
  missing display names do not cause entries to be filtered out.
- Denied root opens report denied; missing roots report unavailable; other open
  failures, failed subkey reads, malformed fields and truncation report partial,
  with a reason. The adapter caps each view at 2,048 subkeys and each queried
  value at 4,096 bytes; reaching either budget cannot imply complete coverage.
  Enumeration is a live observation, not an atomic snapshot during concurrent
  installer changes.
- Packaged apps report unavailable: the installed `windows-sys` 0.61.2 bindings
  have no WinRT `PackageManager` projection for general enumeration. Their Appx
  family-query APIs need a known family and do not establish overall coverage.
  No new dependency or shell fallback was added.
- Lockfile review: only the local `loomward-apps` package entry was added; it
  reuses the existing `windows-sys`. No third-party package/version changed.

The reviewed API contract is [RegOpenKeyExW](https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-regopenkeyexw),
[RegEnumKeyExW](https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-regenumkeyexw),
and [RegQueryValueExW](https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-regqueryvalueexw).
The text decoder accepts a missing terminator, as the query API permits.
The alternative general package API is [PackageManager.FindPackagesForUser](https://learn.microsoft.com/en-us/uwp/api/windows.management.deployment.packagemanager.findpackagesforuser?view=winrt-26100).

## Test-first and mutation evidence

`cargo test -p loomward-apps` compiled with the implementation stubs, then failed
seven behavior tests at the stubs; the source guard passed. After implementation,
all ten tests passed. The Windows API mapping tests were added with the adapter.
For every final test, a targeted implementation mutation was then applied,
`cargo test -p loomward-apps <test-name>` ran, and the original file bytes were
restored in a `finally` block. Each mutation compiled and produced exactly one
failing test, rather than a compiler/setup failure:

| Test name | Mutation caught |
| --- | --- |
| `scope_and_cross_view_duplicates` | Classify HKCU as per-machine |
| `failed_or_partial_providers_keep_coverage_and_evidence` | Replace reader coverage with available |
| `packaged_and_portable_are_explicitly_unavailable` | Claim unavailable providers are available |
| `opaque_strings_and_expand_sz_are_never_executed_or_expanded` | Read the uninstall evidence from the icon field |
| `malformed_values_are_unknown_and_coverage_is_partial` | Accept REG_DWORD as text |
| `system_and_child_entries_are_retained` | Decode SystemComponent=1 as false |
| `runtime_source_has_no_process_or_installer_api` | Insert a forbidden process API reference into runtime source |
| `real_registry_views_report_coverage_without_recording_host_data` | Omit the HKCU provider report |
| `registry_access_is_read_only_and_views_are_explicit` | Remove the HKCU WOW64 flag |
| `errors_distinguish_denied_missing_and_partial` | Misclassify access denied as unavailable |

Restored `cargo test -p loomward-apps`: **10 passed, 0 failed**. All data assertions
use synthetic fake-reader entries or API constants. The Windows-only smoke test
completed without error and each real registry view reported a provider state.
It asserts only report shape, not complete/successful coverage. No real application
names, counts, registry values, inventory dumps or user paths were recorded.

## Proving commands and outputs

Commands run from the lane's repository root:

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | PASS, exit 0 |
| `cargo test -p loomward-apps` | PASS, 10 passed, 0 failed |
| `cargo test --workspace` | FAIL, exit 1 in the unchanged engine concurrency test described below |
| `cargo test --workspace -- --test-threads=1` | PASS, exit 0; 437 passed, 0 failed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, exit 0; finished dev profile |
| `git diff --check` | PASS, exit 0 |
| `cargo metadata --no-deps --format-version 1` | PASS; new crate is a workspace member |

The first default-parallel workspace run reached
`scan::watch::native_tests::concurrent_scan_start_reserves_root_before_touching_watch`
in `loomward-engine`, then failed at `scan/watch.rs:1120`: cancellation returned
zero jobs where one was asserted. The engine binary reported 100 passed, 1 failed.
`git diff --exit-code HEAD -- crates/loomward-engine crates/loomward-windows`
passed; `cargo tree -p loomward-engine --depth 1` confirms that engine does not
depend on the new crate. This is a failure in an unchanged seam, not an inventory
test failure.

Bounded isolation:
`cargo test -p loomward-engine scan::watch::native_tests::concurrent_scan_start_reserves_root_before_touching_watch -- --exact`
passed the one selected test. An earlier short-name filter with `--exact` ran zero
tests and provides no proving evidence. The cancellation implementation stops the
watch before collecting active jobs; completion racing that collection is a
possible cause, not a confirmed diagnosis. No engine fix or assertion weakening
was included.

Full serialized follow-up: `cargo test --workspace -- --test-threads=1` passed:
**437 passed, 0 failed**, with no ignored tests. This proves the full suite under
serialized scheduling; it does not turn the failed default-parallel run green or
establish the cancellation race's cause. The bounded diagnostic attempts ended
here. The driver can carry the exact failure above to the engine owner separately.

## NOT verified and residual risk

- General Appx inventory, portable discovery, other user accounts and non-Windows
  execution are unverified/unavailable; coverage is expressly limited.
- No UI, protocol, transport or service integration was attempted. No Python,
  browser, Svelte or Tauri gates were run for this isolated Rust crate.
- Real ACL denial, oversized registry values and concurrent installer mutation
  were not induced on the host; synthetic failures and API mapping tests cover
  their classification. The default-parallel engine failure remains unresolved.
- No commit, push, PR, release, installation or application effect was performed.

Recommended commit: `feat(apps): add read-only application inventory`.
