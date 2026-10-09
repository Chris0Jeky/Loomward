# loomward-windows

Read-only Windows observations for LW-003 and LW-004, plus an explicitly opted-in disposable
laboratory for LW-064. The default library has no fixture/effect API. The crate uses existing
workspace serde/serde_json, existing tempfile for tests, and locked windows-sys 0.61.2.

`identity::observe(path)` opens the final entry with read-attributes access, all share modes,
backup semantics and open-reparse-point semantics. `ObservedIdentity.object` contains the native
volume serial, 128-bit file identifier, creation time, reparse tag and identity quality; those
fields are flattened in JSON. Other fields include links, attributes, logical/allocation bytes,
Windows 100ns timestamps, and ancestor object bindings. Exact large quantities are decimal text.
Legacy 64-bit identifiers are zero-extended and explicitly labelled with the FileIdInfo failure
reason. Missing usable filesystem identity returns an Unsupported I/O error, never a path hash.

`verify_current(path, &observation)` returns Same, Changed, Recreated, Gone or Unsupported.
It checks ancestor bindings as well as the leaf, including a junction swap onto a different
root whose leaf is the same hard-linked object. Access/query failures conservatively return
Unsupported; `observe` retains the actual I/O reason. This is a metadata observation with
before/after ancestor checks, not an atomic snapshot or a future executor's authority boundary.
IDs and creation times can eventually be reused; Same never grants an effect.

`volumes::enumerate()` returns volume GUID paths, mount paths, filesystem flags/serial, validated
capacity and best-effort drive/device properties. Each unreadable group is Unknown with a reason.
Filesystem case-sensitive search support is not a directory's current mode. Seek penalty and
TRIM are device hints, not benchmarks. Enumeration reports currently visible volumes and does
not fabricate rows for previously disconnected devices or persist inventory history.

Non-Windows calls return explicit Unsupported errors. No installers, elevation, scans, device
benchmarks, provider connections, move/delete/kill endpoints or policy changes are introduced.

## Disposable laboratory

Enable `fixtures` explicitly to build the fixture CLI. The public commands accept only a strict
descendant of `G:\loomward-lab\fixtures`; the unit tests use a private scoped helper and tempfile
roots inside this worktree. There is no configurable arbitrary lab base in the CLI.

```text
cargo run -p loomward-windows --features fixtures --bin loomward-fixtures -- create --root G:\loomward-lab\fixtures\example
cargo run -p loomward-windows --features fixtures --bin loomward-fixtures -- destroy --root G:\loomward-lab\fixtures\example
cargo run -p loomward-windows --features fixtures --bin loomward-fixtures -- volumes --json
```

Creation writes `.loomward-fixtures.json`, records each entry before creating it, then captures
its native identity. Repeated creation validates and returns the existing manifest. Roots with
unmarked contents, traversal, Win32 trailing-dot/space aliases or reparse ancestors are refused.
Directory handles pin the root/ancestors against renaming during fixture work. Cleanup also pins
listed ordinary directories, validates identities/types and checks for unlisted files and ADS
before deletion. It unlinks only the listed entries, its marker and the empty disposable root;
it never uses recursive deletion. Missing entries permit retry after partial cleanup. Corrupt
markers or interrupted/replaced entries fail closed and remain for inspection.

Run fixture work in a private, quiescent disposable tree with one writer. The marker is an
ownership record, not authentication against a malicious same-user writer. A concurrent writer
can still change file contents/streams or introduce a conflicting entry after preflight; cleanup
is not transactional. Never use the fixture helper as a production executor.

Ordinary/equal files, hard links, junctions, ADS, sparse/compressed files, attributes, long paths,
Unicode/emoji, case-only names and delete/recreate observations are covered. Failed optional
classes have named recorded skips. The deny-share lock is exercised and released in-test only;
cloud placeholders and physical removable media are recorded as uncreated cases.

Run the required workspace gate and the opt-in binary gate:

```text
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

See [Windows evidence](../../evidence/v3/windows-native.md).
