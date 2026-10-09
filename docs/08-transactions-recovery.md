# Future file transactions and recovery

Status: specification plus a pure Rust transition simulator. **There is no copy, move, delete or undo executor in this package.** The simulator's states are not evidence that any operation occurred. A journal is necessary but insufficient: every stage must have concrete postconditions and filesystem evidence.

## Operation lifecycle
| Stage | Durable record and required evidence | Crash recovery |
|---|---|---|
| Draft | Proposed group, policy/model versions, preview | No effects; may discard |
| Approved | Exact plan digest, session/grant, expiry | Revalidate before using approval |
| Prepared | Fresh object/destination/volume identities, space reservation and supported metadata class | If uncertain, stop and return to review |
| Copying | Unique destination staging name, byte range/checkpoint/provider state | Inspect actual stage; resume only when identity and protocol allow |
| Verified | Destination content and required metadata verified against a stable source observation | Revalidate if source changed after verification |
| Published | Final destination name created without overwriting an unrelated entry; durability evidence | Do not delete source solely because a final path exists |
| Source retained | Original or independent recoverable copy explicitly retained | Show that source disk may still have no space relief |
| Committed | Recorded postconditions, updated logical-location mapping and recovery receipt | Present result with actual measured space change |
| Needs attention | Error, uncertain stage or conflicting changes | Never auto-label complete or blind-retry deletion |

Retirement of retained source data is a separate policy/operation, not an automatic consequence of “committed.” Permanent purge must not be smuggled into cleanup of temporary files. In a future supported immediate-move flow, retirement can occur only with the exact approved recovery conditions, verified copy and up-to-date source checks; the state machine and UI must represent it explicitly before enabling that flow.

## Cross-volume movement is not atomic rename
Treat it as copy, verify, publish, update references and then conditionally retire the source. A supported primitive such as CopyFile2 may perform copying [R13], but it does not provide an entire application-level transaction across independent filesystems and power failures. Persist the intent before effects and the completion evidence after effects. Flush file contents and journal state where the platform supports the required durability semantics; test what is actually guaranteed.

Copy to a unique staging name on the destination volume. Reserve enough intermediate capacity and refuse overwrite-by-default. A stale pre-existing destination is a conflict, not an invitation to merge directory trees. Preserve source-relative group structure and metadata under an explicitly supported copy profile. If a group contains an unsupported object, reject the group rather than producing a partial “successful move.”

Open/verify source identity and version before and after content verification. A hash alone cannot detect metadata loss or permission changes. If the source changes, the verified destination may be an older version: retain both, label the operation uncertain and ask. Reading a live database file without an application-consistent snapshot is not a general backup strategy.

## The quarantine/space paradox
Renaming an original into a quarantine folder **on the same source volume does not free its allocated space**. Keeping both the source and destination may improve recoverability but cannot satisfy immediate source-space pressure. The app must not count staging/quarantine as reclaimed capacity.

There are three explicit outcomes: (a) copy-only with no immediate source benefit; (b) approved move after verified independent protection and safe source retirement; or (c) defer because recovery and capacity requirements cannot both be met. Moving a quarantine to another disk is itself another protected transfer, not a free shortcut. The planner must include intermediate storage and retirement delay in its numbers.

## Undo is a new operation
Undo cannot simply replay stored paths in reverse. Destination contents may have changed, the original path may now belong to another file, the original volume may be absent, ACLs may have changed or the original application may have different state. Generate a new plan with current identities, checks and conflict handling. When in doubt, restore to a new location and retain both versions. Do not overwrite a user's subsequent edits to preserve the appearance of one-click undo.

## Recovery receipt
Record operation/group IDs; manifest digest; original and resulting identities; intended and measured byte quantities; content and metadata verification results; grants; provider/format versions; backup snapshot references; start/end timestamps; policy/model versions; conflicts; retention deadline; undo prerequisites; and a precise outcome (`completed`, `partial`, `cancelled`, `needs_attention`). Avoid storing secret file content or backup credentials in the journal.

For a multi-file operation, the receipt must say whether the whole group is complete. A partial transaction cannot be hidden behind an aggregate success count. Jobs remain resumable only while their input manifests, identities, permissions and provider versions match.

## Fault-injection matrix
Before enabling even user-approved ordinary-file moves, run a disposable Windows fixture for every stage boundary: process crash before/after durable record, system restart, source modification, target full, cable removal, ACL denied, anti-virus sharing violation, destination name race, source junction swap, cloud placeholder transition and changed drive mapping. Verify actual files and metadata, not only mock return values. Test cancellation at multiple byte offsets and ensure recovery never issues an unplanned delete.

The journal is application state, not a replacement for Windows integrity mechanisms or a backup. USN history cannot reconstruct file content for undo [R06]. Do not claim otherwise.
