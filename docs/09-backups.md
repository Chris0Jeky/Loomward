# Protection and backup architecture

## Model protection as evidence
A second pathname is not necessarily a second copy. A second copy on the same disk is not independent protection against that disk failing. Synchronisation can propagate unwanted changes. A snapshot, successful backup command, integrity check and demonstrated restore are different facts. Show them separately rather than collapsing them into one green shield.

The current Protection screen intentionally says the backup adapter is not configured/implemented. No restic job is run, no VSS snapshot is created and no file is claimed recoverable. This preserves an honest UI contract for the later provider implementation.

## Provider boundary
Define `discover_capabilities`, `preview_backup`, `start_backup`, `poll_job`, `verify_repository`, `preview_restore`, `restore_to_new_location` and `report_snapshot_coverage`. Provider output must include job identity, completion status, repository identity, snapshot identity, scope manifest, excluded/failed objects, supported metadata classes and timestamps. Use explicit argument arrays or a library interface, not LLM-created shell commands.

Evaluate restic as the first provider [R26,R27]. Keep the adapter replaceable. Do not invent a custom encrypted archive format merely to make backup “native.” Credential/key storage is a separate design: use platform-supported protection and keep recovery key/export instructions understandable. Never write keys into a model prompt, ordinary diagnostic report or plaintext operation log.

## Consistency and fidelity
VSS can support coordinated snapshots [R16], but application-consistent backup depends on the application/writer/provider behavior. Define supported cases. For live databases, VM disks and application-managed stores, use dedicated providers or refuse generic copy backup. A byte-for-byte copy of an inconsistent application file is not automatically a usable restore.

For each copy/backup provider, verify whether it preserves required streams, security descriptors, sparse/compressed state, hard links, EFS-related behavior and timestamps. These are capability claims to test, not assumptions inherited from “Windows support.” Classify an unsupported file as blocked or reduced-fidelity and ask before proceeding.

## Health model
`unknown` means insufficient evidence. `configured` means a destination/key is set. `recent_snapshot` means a completed provider snapshot covers a stated manifest at a time. `repository_checked` says which integrity check passed. `restore_tested` gives a sample or complete restore result and when it happened. `at_risk` and `incomplete` carry specific reasons such as disconnected repository, expired retention, failed items or missing recovery credentials.

Coverage should be object/version-based, not solely a folder name. A backup of last month's file is not protection of today's edits. Record freshness and completeness. User-selected exclusions remain visible and must not be treated as protected just because their parent folder appears in a backup job.

## Retention and cleanup
Retention, forget/prune, replica deletion and source-file retirement require separate policy and consent. A storage-pressure event must not make the backup adapter silently delete the last recoverable version. Account for repository growth and intermediate scratch space. A retention change should show which recovery points would become unavailable before it executes.

Plan initial recovery objectives as user choices, not universal promises: for example, a daily snapshot of personal documents and a monthly restore drill. Measure actual last successful backup and restore duration. Protect the catalogue/configuration too, but do not synchronise a live WAL file as though it were a portable snapshot [R22]. Use a database-supported consistent export/backup path.

## Acceptance gate
On a disposable Windows test tree, back up and restore ordinary files plus the supported metadata fixtures to a new directory. Compare content, required metadata, manifest coverage and unavailable/error reporting. Exercise missing key, corrupt/locked repository, network loss and out-of-space conditions. Only then may an operation policy rely on that provider's protection evidence. A logo on the Protection screen is not this gate.
