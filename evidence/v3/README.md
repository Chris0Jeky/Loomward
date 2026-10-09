# v0.3 evidence: first Windows and native measurements

9 October 2026, DESKTOP-IHKOOJS, Windows 11 Pro 10.0.26300, standard (non-elevated) user.
Python 3.14.3, Node 24.13.1, cargo/rustc 1.97.1. Every result here is the first run of its kind:
the v0.1 and v0.2 authoring passes had neither Windows nor a Rust toolchain.

## Provenance of the import

The ChatGPT deliverables arrived in the owner's local `Resources/` folder (gitignored, not
published). Every sha256 in `Loomward-v2-artifacts.json` and `Loomward-artifacts.json` matched
the files present. `git bundle verify` passed on both bundles (complete history). The v2 bundle's
`expansion/interop-v2` tree (f3b1093) matches the v2 source ZIP file-for-file, 346 files, once
CRLF conversion is ignored. Both bundle branches were pushed unchanged and joined to `main` with
`git merge --allow-unrelated-histories`. The four PNG screenshots and `extracted-tests.txt` listed
in the v2 manifest were never downloaded, so they are absent. `chatgpt-session-reports.txt` is the
owner's original request and ChatGPT's two session reports, kept verbatim as provenance.

## Before any change (bundle as authored, f3b1093 merged)

| Gate | Result |
|---|---|
| Python suite | 184 ran, **3 failed**, 1 skipped (`test_symlink_not_followed`: symlink privilege unavailable to a standard user) |
| JavaScript | both syntax checks pass; 9 boundary assertions and 80 parity fixtures pass |
| Rust compile + `cargo test --workspace` | **compiles; 19/19 pass** (20 written; `symlink_is_not_followed` is `#[cfg(unix)]`) |
| `cargo fmt --all --check` | **fails** (formatting only) |
| `cargo clippy --workspace --all-targets -D warnings` | **fails**: one `needless_return` in the Windows reparse check |
| Chromium bridged UI | 14/14 pass |

The three Python failures had one root cause. Windows `DirEntry.stat()` reports `st_ino`,
`st_dev` and `st_nlink` as 0, so the scanner recorded every file as identity 0 with link count 0.
Duplicate inspection then treated every file as hard-linked and skipped it. A zero stood in for an
unknown, which breaks the honest-unknowns invariant. A second Windows quirk surfaced while fixing
it: on Python 3.12+ `os.stat()` reports creation time as `st_ctime` but `os.fstat()` reports change
time, so the stability signature could never match an open handle; `st_birthtime_ns` agrees.

## After the fixes (`windows-verify.txt`, `scripts/verify.py --ui`)

185 Python tests (one new regression) pass with the same legitimate skip; JavaScript passes;
rustfmt, 19 Rust tests and clippy pass; 14/14 Chromium checks pass. `Cargo.lock` was resolved for
the first time (34 packages, all crates.io) and reviewed.

## Still not established

The Chromium harness bridges `fetch` through Python, so this is still not WebView2 or direct
browser-network evidence. The Tauri shell (`native/`) was not built. No native Windows identity,
volume or USN code exists yet (LW-003/004/007). No real-disk scan was run: no scan root has been
granted (HUMAN_TODO q-4). No hosted CI result exists until the workflow runs on GitHub.
