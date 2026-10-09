# Decision worksheet for the first Windows session

Nothing here should delay launching the synthetic demo. It specifies the unknowns that prevent safe production assumptions.

| Decision | Safe initial assumption | Information needed before widening scope |
|---|---|---|
| Name/licence/public release | Loomward working name; original code MIT; private repository helper | Confirm name and final release policy |
| OS and privilege | Standard-user Windows 11 target | Actual build, edition, policies and native toolchain |
| Filesystems/devices | Unknown; no live inventory performed here | Volume identity, filesystem, capacity, removable status and backup role |
| Main disk reserve | No universal threshold silently installed | Desired free GiB, growth rate and actual constraints |
| Initial root | User-selected disposable/small folder | Explicit metadata scope |
| Cloud sync | Content excluded where placeholder/reparse state is known | Provider and hydration expectations |
| Sensitive material | Names may still be private; no raw content to model | Excluded scopes and retention preferences |
| Teacher | Off | Exact local endpoint, model ID, metadata grant and budget |
| Process inspection | Off until requested | Own-session visibility desired; no control grant |
| Backups | Unknown/unconfigured | Independent repository, key recovery and restore test |
| Desktop | Companion window only | Known-folder redirection, public desktop, monitors/scaling |
| Organisation | Virtual labels, no movement | Preferred collections, project group examples and exceptions |
| Usage history | Unknown | Consented event sources and observation window |

Record actual findings in a private local diagnostic file under `.loomward/`, not in a public issue or committed fixture. When reporting a problem, minimise private paths and use a synthetic reproducer where possible.
