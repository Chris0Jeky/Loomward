# Safety, permissions and threat model

## Invariants
The user filesystem is not a training playground. No model confidence, temperature setting, schema validator or prompt instruction can substitute for permission and verified effects. The implementation must make dangerous behavior difficult to express, not merely ask an LLM to be careful.

The first prototype enforces this by having no mutation implementation. That is a narrower but stronger property than a destructive function guarded only by `dry_run=True`. Future effects require a separate release and evidence gate. Application-state writes, exports and explicitly requested content reads are real effects and remain visible in the documentation.

## Assets and adversaries
Protect user files, metadata, application integrity, credentials, backup keys, privacy, working-session responsiveness and trustworthy approvals. Treat filenames, document text, shortcut targets, archive entries, extracted metadata, model responses, downloaded policy templates and imported snapshots as untrusted. A compromised local model server is not a trusted operating-system controller. A renderer compromise must not become unrestricted filesystem access.

The reference does not defend against an administrator or same-user malware able to rewrite its code/database. Its unkeyed audit chain can detect certain accidental changes or edits when checked, but a writer can recompute the chain or delete its tail. A real external anchor or stronger integrity design is needed before describing it as a tamper-proof security audit.

## Capability ladder
| Grant | Scope | Default |
|---|---|---|
| Metadata observation | User-selected roots and stated exclusions | Explicit selection |
| Content inspection | File classes, roots, byte/time budget | Off |
| Personal learning | Selected metadata/labels and retention | Explicit local feedback |
| Local teacher metadata | Named endpoint/model, exact fields, limits | Off |
| Usage observation | Stated event sources/time window | Off |
| Process observation | Per-user visibility supported by Windows | Off until requested |
| File movement | Exact approved plan and verified recovery conditions | Not implemented |
| Deletion/uninstall | Separate high-impact approval and policy | Not implemented |
| Resource modification | Exact process instance and reversible policy | Not implemented |
| Privileged operation | Just-in-time narrowly bounded broker request | Not implemented |

A user's approval of a label does not approve relocation. Approving one move does not approve future similar moves. “Always use this collection” is a classification preference, not a perpetual deletion or movement grant. Silence means no permission.

## Filesystem safety requirements
**Reparse points and path races.** Canonicalisation or `starts_with(root)` is insufficient if an ancestor changes between check and use [R09]. Future Windows execution must use carefully selected native handle semantics, revalidate final identity and directory ancestry, and fail closed on a reparse/capability mismatch. Keep a root handle open through validation/execution. There must be dedicated adversarial tests for junction swaps and path retargeting. The portable reference reduces risk but does not claim this guarantee.

**Hard links and duplicates.** Multiple names can refer to one underlying object [R10]. A hash match does not establish that either pathname is dispensable. Equal bytes may carry different ACLs, alternate streams, application roles or backup significance. The prototype excludes known hard-linked files from duplicate content inspection and never estimates guaranteed reclaimability.

**Cloud and removable storage.** A placeholder must not silently become a large download [R11]. A disconnected disk is not an empty disk. Reconnected hardware may appear at the same drive letter with a different identity. Do not infer destination durability from “the path exists.”

**Windows metadata.** Explicitly classify ACL/security descriptors, owner, inheritance, alternate data streams, sparse/compressed allocation, EFS, timestamps, hard-link sets, junctions, reparse tags, extended-length paths, case-sensitive directories and sharing/lock state. Preservation capabilities must be measured for each supported transfer path. Unsupported classes block execution rather than silently becoming plain files.

**Application ownership.** Protected classes include Windows/program directories, app data with unknown ownership, live databases, repositories' internal metadata, package-managed installations, game libraries, active VM/container images, mapped model weights and open working sets. Moving these through a generic file mover is not acceptable simply because the current caller has write permission.

**Names and encodings.** Handle reserved device names, colon/alternate-stream syntax, UNC and device namespaces, Unicode normalisation differences, trailing dots/spaces, bidirectional display controls and case-only renames. Display escaping prevents HTML injection; it does not solve filesystem naming semantics. Content workers must also bound archive expansion, path traversal, nested archives and decompression ratios.

## Teacher boundary
Input is an explicit metadata subset. Output selects an existing virtual label or abstains and gives a brief evidence-based explanation. No free-form destination path, command, code, policy update, grant or tool call is accepted. Treat structured output as syntactic assistance, not as truth [R28]. Validate identity, vocabulary, fields, types, sizes and provenance independently. Render explanations as text.

The current adapter permits only literal loopback HTTP endpoints with an explicit port and the expected chat-completions path. It disables proxy handling and redirects, limits request/response size and requires a consent flag. This restricts the app's connection destination; it cannot prove that an independently configured local server does not forward data elsewhere. Do not promise universal offline privacy on that basis.

## Future execution approval
Approval contains user/session identity, scope, plan digest, risk class, byte ceiling, expiry and required version checks. Before starting and before each consequential step, check the current object, destination, volume identity, available capacity, grants, lock/usage state and protection evidence. A single stale precondition invalidates the affected group and returns to review; never quietly substitute another path.

Batch operation groups are atomic in intent, not magically atomic across volumes. A project partially copied is not a completed project move. Approval records are revocable. Emergency stop prevents new work and cancels cooperative operations at documented safe boundaries; it does not claim to reverse already committed external effects.

## Parser and plugin isolation
No previews by loading arbitrary file-associated DLLs into the engine. Treat media/office/archive extractors as separate attack surfaces. Bound CPU, memory, output size, recursion and wall time; disable networking by default. Exclude credential material and sensitive user scopes before parsing or embedding. Content-derived embeddings can reveal information and need deletion/export controls too.

## Regression classes before any mutation release
Race a directory with a junction; replace a file after preview; change a drive mapping; deny an ACL midway; disconnect a destination; fill it after capacity checking; crash after each journal append; copy ADS and EFS fixtures; retain conflicting destination names; attempt `..`, UNC/device paths and case-only collisions; revoke consent mid-operation; inject a command into a filename/teacher response; reuse a PID; place the app database in a linked or shared directory; and verify that no failure produces a false “completed” receipt.
