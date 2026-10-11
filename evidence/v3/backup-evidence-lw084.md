# LW-084: backup observations without a recovery shortcut

Date: 2026-10-11. Starting revision: `c030528d9db1eacd5511966be455b590d1ca5b1b`,
branch `feat/backup-evidence-084`. This is a worker slice (committed by the driver), not a provider
connection, Windows restore qualification, release or permission to retire a source.
All fixture names, identities, content versions and observations are synthetic.

## Changed

1. Added [loomward-backup](../../crates/loomward-backup/src/lib.rs), a pure Rust
   normaliser: `normalise_backup_receipt(raw: &[u8], source: &BackupSource) ->
   Result<RecoveryEvidence, ReceiptError>`. It returns evidence on valid input and a
   named error on malformed input. A small new crate follows the ownership already
   proposed by LW-035/LW-036 and keeps backup parsing outside the unrelated teacher
   runner and shared protocol. The workspace glob discovers it; the lockfile adds
   only this crate, with already installed `serde` and `serde_json` dependencies.
2. Parses one provider's supplied snapshot array and optional integrity-summary
   bytes. No process, filesystem, environment, clock, credential or network APIs
   appear in the implementation. Provider roots, hostnames, usernames and unused
   statistics are discarded rather than copied into recovery evidence.
3. Returns a [proposed drill](../../crates/loomward-backup/src/lib.rs) as data: exact
   requested relative paths and SHA-256 versions, `new-disposable-target`, required
   metadata comparisons and proof requirements. That target is an unresolved label;
   nothing creates, opens, writes, restores or removes it.
4. Added [synthetic regression tests](../../crates/loomward-backup/tests/receipts.rs)
   and an [offline mutation check](../../crates/loomward-backup/tests/mutations.py).
   No HTTP/IPC/MCP routes, commands, DTOs or shared schemas changed. The proposed
   interface below is for the next wiring wave.

## Provider choice and limits

Restic was already the candidate in `docs/09-backups.md`. Its documented
`snapshots --json` format is a single array with snapshot IDs, timestamps and roots;
`check --json` has a final integrity summary. The project intends JSON output to
remain backwards compatible, while permitting added fields. This gives the parser
a documented machine boundary without interpreting console prose. Those formats
were checked against the official 0.19.1 documentation, not a running tool. Neither
format supplies a per-file content comparison or a Windows metadata fidelity test;
those observations must be supplied separately, bound to the repository and snapshot.
Unknown provider fields are ignored only after strict JSON validation. No claim is
made about older provider versions or actual captured provider output.
[Official restic scripting formats](https://restic.readthedocs.io/en/stable/075_scripting.html#snapshots),
[integrity summary](https://restic.readthedocs.io/en/stable/075_scripting.html#check).

The existing Python reference has no backup adapter or restore verifier to port.
`docs/09-backups.md` explicitly describes its unconfigured Protection surface.
`contracts/v2/INDEX.json` marks the provider descriptor design-only; it does not
install a provider or confer authority. LW-035 execution, LW-036 native restore
fidelity and LW-073 general provider conformance remain prerequisites for connected
support, not work completed by this parser.

## Evidence semantics

The three observations remain separate:

| Field | What the supplied data establishes |
|---|---|
| `snapshot_taken` | The selected full snapshot ID appears in the parsed array. This says nothing about current per-file versions. |
| `integrity_checked` | A successful, repository/snapshot-bound check summary reports no errors, broken packs or repair/prune warnings. |
| `integrity_read_all_data` | The check observation additionally records a full-data check; structure-only checks remain visible but cannot satisfy recovery eligibility. |
| `restore_drill_succeeded` | The bound, complete restore observation covers every requested version and required metadata class. This is scoped to the requested manifest. |

`restored_versions` retains the successfully bound observation's content versions.
`restore_coverage` is the intersection of requested versions, snapshot manifest
versions and restored versions; it is content coverage, not a claim of metadata
coverage. Required metadata is tested separately for each requested file.
Paths are relative, scope-projected names, never native identity or effect operands.
Versions are canonical lowercase 64-digit SHA-256 observations, not a green job
counter. The supplied snapshot manifest is explicitly bound by the observation
owner to `repository_id` and `snapshot_id`; snapshot root names do not invent it.

Topology is an explicit supplied observation: independent target, same volume,
same physical device, or unknown. The normaliser never infers physical independence
from a pathname, drive letter or repository URL. Only independent target qualifies.
Credential evidence contains only a validity deadline, never a password or key;
missing validity and a deadline at/before `now` both refuse recovery eligibility.

Observation ages are computed from the supplied observation times and `now`, in
seconds on the same observer clock. The configured maximum age is explicit;
exactly the maximum remains fresh. Snapshot/check/restore ages are separate.
Future observations retain unknown age (`None`) and a refusal. The provider's
snapshot-time string is a bounded, uninterpreted display label, not the age oracle
and not a claim of RFC3339 date validation. Missing check/restore observations keep
their ages unknown. Historical positive observations remain visible when stale,
while `verified_recovery` becomes false.

`verified_recovery` means **the supplied, scoped evidence meets every predicate**.
It does not authenticate receipts, measure content, detect metadata classes,
inspect devices, test credentials, or prove recovery on this host. The future
observation owner must establish provenance, snapshot-manifest binding, independent
content comparison, topology and required metadata from its own allowed sources.
An untrusted client must never construct those claims and promote itself. This
predicate is never permission and never a source-retirement gate.

## Refusals and parser budgets

| Condition | Result |
|---|---|
| Green job, missing or different requested file version | `VersionNotCovered`; no verified recovery |
| Same volume, same device, or unknown topology | `TargetNotIndependent`; no verified recovery |
| Expired or unknown credential validity | `CredentialsUnavailable`; no verified recovery |
| Missing requested restored paths or incomplete restore receipt | `PartialRestore`; no verified recovery |
| Missing comparison of any required metadata class | `UnsupportedMetadata`; no verified recovery |
| Snapshot absent | `SnapshotMissing`; no verified recovery |
| Check absent, unbound, failed, warning-bearing or structure-only | `IntegrityNotChecked`; no verified recovery |
| Restore absent, unbound or nonzero exit | `RestoreNotSucceeded`; no verified recovery |
| Observation too old or in the future | `StaleObservation` / `InvalidObservationTime`; no verified recovery |
| Oversized byte document | `InputTooLarge` |
| Malformed JSON, duplicate member at any depth, invalid UTF-8, out-of-range number or excessive nesting | `InvalidJson` |
| Wrong parsed shape/type, invalid ID, traversal in a provider root, or excess snapshot count | `InvalidReceipt` |
| Invalid/empty requested scope, duplicate path, ambiguous Windows name, invalid version or excess supplied observations | `InvalidSource` |

Each snapshot/check document is capped at 262,144 bytes. The default serde_json
recursion limit stays enabled (128). Each snapshot list, root list and file manifest
is capped at 256 records, each path at 1,024 bytes, and each metadata list at seven
classes. Numeric parsing rejects unsigned values above signed 64-bit maximum and
non-finite/out-of-range floats, including numbers hidden in ignored extensions.
Observation seconds/deadlines are bounded at 253,402,300,799. Duplicate JSON members
are rejected before lossy map construction, including escaped spellings of the
same key. Complete consumption is required; trailing documents are refused.

Relative manifest paths reject traversal, absolute paths, backslashes, controls,
empty components, Windows reserved names, streams and trailing dot/space aliases;
duplicate paths are checked without case. Provider roots may be absolute labels
but traversal components are rejected. These are lexical checks, not native path
validation; no path is passed to an executor.

## Proposed restore drill (data only)

The successful synthetic control requests `fixture/document.txt` and
`fixture/notes.txt`, each at the fixture's explicit SHA-256 version. The proposal
names a new disposable target, not the source or backup repository, and requires
repository/snapshot binding, independently observed restored content SHA-256,
per-file required metadata comparison, a complete manifest and fresh observations.
The synthetic control requires ACL comparison; the unsupported-metadata test adds
alternate streams and refuses recovery until those comparisons exist.

No drill executes here. A future native fixture must establish actual content and
Windows metadata fidelity, including refused/unsupported cases, missing keys,
corruption, locks, partial reads and capacity failures under LW-036's own gate.
This proposal cannot grant source retirement, retention changes or backup pruning.

## Interface delta for next wave (not wired)

Proposed fixed command: `backup.evidence.normalise`, a read-only normalisation of
supplied bytes and existing observations. It must not discover, connect, invoke a
provider or restore anything.

Proposed request DTO: provider format identifier (`restic-snapshots-json`), bounded
encoded snapshot octets and an owner-held scoped observation reference. Preserve
raw bytes through decoding; a wire adapter must not first turn them into a JSON
map and lose duplicate keys. Resolve the reference to the internal `BackupSource`
only after session/scope checks. Its optional integrity observation also holds
bounded raw check-summary bytes. Do not accept client claims of topology, credential
validity, full-data check arguments, snapshot manifests or compared restore metadata
as trusted observations. No repository/config/credential path or execute flag.

Proposed response DTO: repository/snapshot evidence IDs; separate snapshot,
integrity and scoped restore flags; required/restored/covered path-version manifests;
topology; nullable snapshot/check/restore ages; the scoped recovery predicate;
named refusals; and a data-only drill proposal. A consumer must display unknowns,
coverage and scope and must not reduce this DTO to an unconditional green shield.

Transport refusals: unknown provider format, malformed/oversized encoded payload,
missing/stale/unavailable source observation reference, denied scope, duplicate or
malformed provider JSON, invalid manifest, unsupported metadata and unavailable
native proof. Map parser errors distinctly from well-formed insufficient evidence.
Do not treat successful normalisation as permission. No effects command or
source-retirement integration is proposed.

## Verified

Platform/toolchain: Windows, Python 3.14.3, cargo/rustc 1.97.1, Node 24.13.1.
No hardware, personal inventories or provider repositories were observed for this
slice. Proving logs remain gitignored under `.loomward/lw084/`; they are not public
fixtures. The initial suite at the starting revision passed before edits.

The refusal-only implementation compiled: `cargo test -p loomward-backup --offline`
returned **101**, with **15 failed / 1 passed**. The one initially passing test
checked invalid source cases; its discriminatory proof is the source-validation
mutation below. The implemented suite passed **16/16**, with no skipped tests.

`py -3 crates/loomward-backup/tests/mutations.py` returned **0**: all **19** final
mutants compiled and failed the targeted test with exit **101**; every one of the
16 new tests killed at least one mutant. The script restored the exact original
source bytes, then reran all 16 tests successfully. It refuses a survivor, build
error or a moved mutation seam and restores the source in `finally`.

| Mutation removed/bypassed | Targeted regression |
|---|---|
| Matching version refusal | `green_job_without_matching_version_is_not_recovery` |
| Independent target refusal | `same_volume_is_not_recovery` |
| Credential validity refusal | `expired_or_missing_credentials_are_not_recovery` |
| Restore completeness refusal | `partial_restore_is_not_recovery` |
| Required metadata refusal | `unsupported_metadata_is_not_recovery` |
| Integrity refusal | `snapshot_and_integrity_are_distinct_from_restore` |
| Staleness and future-time refusals (two mutants) | `stale_and_future_observations_are_not_recovery` |
| Missing snapshot and binding (two mutants) | `missing_snapshot_and_unbound_or_failed_observations_are_not_recovery` |
| Same-device/unknown topology check | `same_device_and_unknown_topology_are_not_recovery` |
| Always reject the successful control | `complete_restore_is_scoped_and_proposal_is_only_data` |
| Ignore integrity summary errors | `integrity_summary_errors_and_structure_only_check_do_not_verify_recovery` |
| Accept duplicate keys and huge numbers (two mutants) | `malformed_wrong_types_huge_numbers_and_duplicate_keys_are_rejected` |
| Remove byte budget | `byte_and_depth_budgets_are_enforced` |
| Silently repair invalid UTF-8 | `invalid_utf8_is_rejected_without_lossy_conversion` |
| Skip source validation | `source_limits_empty_scope_and_duplicate_paths_are_refused` |
| Bypass relative path validation | `traversal_and_windows_ambiguous_names_are_refused` |

An initial narrower traversal mutant survived because trailing-dot rejection also
rejects `..`. It was replaced by a shared path-validation bypass; that mutant was
killed. This was a mutation-selection correction, not a weakened regression test.

All commands ran at this worktree's root against the unchanged starting HEAD plus
this uncommitted slice. Cargo network access was disabled for proving runs.
The tested normaliser SHA-256 is
`227787a404465f51513f88648baf64e95410e01b7287777201fbafee3ea41e86`.

| Required command | Exit | Observed output |
|---|---|---|
| `cargo fmt --all --check` | 0 | No formatting findings |
| `cargo test --workspace` | 0 | 797 passed across workspace tests and doctests; no failed or ignored tests |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 0 | Finished successfully; no lint findings |
| `py -3 scripts/verify.py` | 0 | 227 Python tests, one existing symlink-privilege skip; both JS syntax checks, nine JS boundary assertions, 80 cross-language fixtures, Rust format/tests/lint all PASS |
| `git diff --check` | 0 | No whitespace findings; new untracked files additionally checked with `git diff --no-index --check` |

Raw gate logs: `.loomward/lw084/fmt.log`, `workspace.log`, `clippy-all.log` and
`verify.log`. Red/green and mutation logs are in the same ignored directory.

## NOT verified

No backup tool was invoked; no real provider output, repository, credentials,
Windows file fidelity, restore, live disk topology, credential access or provider
conformance kit was verified. No UI or native shell integration exists for this
module. The two provider byte formats are synthetic document-conformance fixtures,
not recordings from a deployed restic version. No source-retirement safety claim.

## Residual risk and driver handoff

The remaining risk is provenance and applicability of supplied observations: fake
or incomplete caller-owned context cannot establish real recovery. The next bounded
step is a scoped observation owner and reviewed DTO wiring, followed by the separate
native restore/fidelity gate. A complete fixture is evidence for the normaliser,
not evidence that any user's data is recoverable.

Recommended commit: `feat(backup): normalise scoped restic recovery evidence`.
No commit or push was made. Declined execution, provider discovery, shared wire
changes and live restore testing because this lane is read-only evidence only.
