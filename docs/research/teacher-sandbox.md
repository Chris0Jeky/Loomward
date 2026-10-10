# L15b: the unregistered AppContainer launch gate

Measured on **10 October 2026**, Windows x64 **10.0.26300**, Rust/Cargo **1.97.1**,
from `spike/l15b-teacher-sandbox`, base/head
`35f020850e282f223aa37bc29475f9b9b1158f2a`, with uncommitted spike changes.
This is LW-111 research, not a production teacher or personal-data activation.

## Verdict

**The permitted no-profile launch failed before the probe executed.**
`DeriveAppContainerSidFromAppContainerName` succeeded for a fresh name, but
`GetAppContainerFolderPath` returned **HRESULT `0x80070002`** and `CreateProcessW`
with `SECURITY_CAPABILITIES`, zero capabilities and no new Job Object returned
**Win32 `2` (`ERROR_FILE_NOT_FOUND`)**. An ordinary launch of the same copied
executable, cwd, environment and three inherited handles succeeded and exited 0;
its token reported `appcontainer=false`.

The missing profile is the supported explanation, **not a demonstrated repair**:
registration was forbidden, so this experiment cannot show that registration fixes
the failure. The no-registration route is blocked on this host. It establishes
**none** of C-FS, C-NET, C-TOOLS or contained process-tree enforcement. Personal
teacher use must remain refused with `confinement_not_enforced`.

Microsoft's [launch procedure](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer)
creates a profile before launching; its
[folder lookup API](https://learn.microsoft.com/en-us/windows/win32/api/userenv/nf-userenv-getappcontainerfolderpath)
is a read-only diagnostic here. Neither documentation nor a process-creation
failure substitutes for the cooperative enforcement canaries.

## Method and rerun

The runnable example is
[teacher_sandbox.rs](../../crates/loomward-lab/examples/teacher_sandbox.rs), with
[Windows implementation](../../crates/loomward-lab/examples/teacher_sandbox/windows.rs).
The raw [receipt](../../evidence/v3/teacher-sandbox.json) records the launch failure,
positive controls, explicit blocked cases and cleanup. A successful `--run` exit
means the measurement and cleanup completed; it does **not** mean confinement passed.

```powershell
cargo run -p loomward-lab --example teacher_sandbox -- --run evidence/v3/teacher-sandbox.json
cargo test -p loomward-lab --example teacher_sandbox
```

`--probe <private-config.json>` is the in-container role. `--child` is an internal
process-inheritance control. The launcher creates these arguments and the config;
they are not an input interface for untrusted teacher data. The probe refuses a
non-AppContainer token. Every role checks `TokenElevation` and refuses an elevated
token before file, ACL or network operations. The refusal branch was not exercised
with an elevated token; this lane never elevated.

The launcher creates one private temporary directory under the user profile. Its
protected DACL gives the invoking user and SYSTEM access. It creates a non-granted
canary directory and a granted lab directory; only the latter receives the freshly
derived container SID ACE. Low-integrity labels and ACL changes address only these
disposable directories. Executable copies, synthetic canaries, per-role stdio files
and an empty `CODEX_HOME` live there. The SID is derived; **no profile is registered**.
No firewall, registry, policy, owner-file ACL or global configuration is changed.

Documents is resolved using `SHGetFolderPathW(CSIDL_PERSONAL)` without creating it.
The listing operation only opens a directory iterator: it does not consume, retain
or publish names. Synthetic canaries contain a fixed synthetic string. The launcher
resolves one IPv4 address of `api.openai.com` and starts a loopback TCP listener.
Each network control calls `TcpStream::connect_timeout` only, with a four-second
limit, then drops the stream: **no HTTP, TLS, prompt or application payload**. The
only endpoints are `1.1.1.1:443`, that resolved API address and the loopback listener.
DNS resolution happens in the launcher, so this would test IP connectivity, not
in-container DNS or TLS/provider reachability.

The ordinary process control uses `CreateProcessW` with the same stdio-only
`PROC_THREAD_ATTRIBUTE_HANDLE_LIST` and explicit environment (`SystemRoot`, `TEMP`,
`TMP`, empty request-local `CODEX_HOME`), but omits `SECURITY_CAPABILITIES`. It reports
identity through inherited stdout. An earlier diagnostic's direct child report-file
write returned error **5**; inherited stdout worked. That diagnostic is not a
filesystem-confinement result, and no ACL was widened to make it pass.

The intended contained matrix is no capabilities / `internetClient`, each without
and with a new Job Object. On the first AppContainer launch failure the launcher
stops the remaining matrix and Codex runs. It never falls back to an ordinary
teacher launch, registers a profile, or borrows an existing application's identity.

## Results

`—` means no access attempt executed, rather than a zero error code or a denial.
Successful controls have a null error code in the JSON.

| Probe | Capability set / job | Outcome | Exact error |
|---|---|---|---|
| Ordinary copied probe launch and identity | Not contained; no new job | PASS: exited 0, `appcontainer=false`, inherited host job membership true | None |
| SID derivation | Fresh name, no registration | PASS: SID returned | No failing HRESULT |
| Profile folder lookup | Derived SID | Missing profile lookup | `0x80070002` |
| AppContainer launch | None; no new job | FAILED before process creation | Win32 2 |
| AppContainer launch | `internetClient`; no new job | Blocked after first launch failure | — |
| AppContainer launch | None; new job | Blocked after first launch failure | — |
| AppContainer launch | `internetClient`; new job | Blocked after first launch failure | — |
| Non-granted synthetic canary read | None / `internetClient` | Both blocked; no contained read | — |
| Granted lab canary read | None / `internetClient` | Both blocked; no contained positive control | — |
| Documents listing | None / `internetClient` | Both blocked | — |
| TCP connect `1.1.1.1:443` | None / `internetClient` | Both blocked; no contained connect | — |
| TCP connect API address `162.159.140.245:443` | None / `internetClient` | Both blocked | — |
| TCP connect loopback `127.0.0.1:9699` | None / `internetClient` | Both blocked | — |
| Child spawn, inherited container SID | All contained cases | Blocked | — |
| Job UI restrictions, no breakaway, kill-on-close | Both capability sets | Blocked | — |
| Native Codex version/startup/auth/network | None | Blocked before native launch | — |
| #116 model/tool canaries | Contained Codex | **Blocked-by-design**: no authenticated contained runner | — |
| Launcher reads both canaries, opens Documents iterator | Not contained | All successful positive controls | None |
| Launcher TCP connects to all three endpoints | Not contained | All successful positive controls | None |
| Disposable-root cleanup | Launcher | PASS: close succeeded; root absent; subsequent prefix inventory empty | None |

The API address and loopback port are this receipt's resolved/generated values, not
future endpoint allowlists. Earlier diagnostic runs also failed AppContainer launch
with error 2 and removed their roots; the committed evidence candidate is the final
measurement. No canary directory, executable copy or temporary ACL remains.

The installed native binary was resolved from the platform package path specified
in [the prior spike](teacher-confinement.md), without executing the npm wrapper. Its
SHA-256 matches the pin:
`9e7c59c05cc1ce5677b1f94e835b2ac038ca3be14504e78d558eacdb0ea3f55d`.
The executable was not started inside a container. There is consequently **no
observed Codex auth or network failure** to report; calling error 2 an auth denial
would be false. No owner credentials were read, copied, granted or supplied. The
future Codex diagnostic in the example has zero capabilities, empty `CODEX_HOME`,
and only the fixed synthetic prompt on stdin. Its short flag set is an unauthenticated
startup diagnostic, not a replacement for #116's hardened runner profile.

## Answers and concrete L15 recommendation

1. **Filesystem:** unverified. The canary and Documents operations never ran inside
   the boundary. An unregistered launch failure cannot establish a filesystem
   allowlist. Standard AppContainer isolation also does not imply that every
   broadly accessible Windows resource is denied; test LPAC and the actual token,
   ACLs and inherited handles before claiming a strict read allowlist.
2. **Network:** unverified for both capability sets, including loopback.
   Microsoft documents `internetClient` as general outbound Internet/public-network
   access, not an endpoint allowlist. This supports the design concern but is **not
   confirmation from this host**. An owner-managed q-6 egress rule or a broker still
   needs its own non-provider and loopback enforcement tests.
   [Capability documentation](https://learn.microsoft.com/windows/uwp/devices-sensors/enable-device-capabilities).
3. **Processes:** unverified inside AppContainer. The example prepares atomic
   `PROC_THREAD_ATTRIBUTE_JOB_LIST` attachment, kill-on-close, neither breakaway flag,
   and all eight UI restriction bits. It would inspect the parent/child container
   SID, query UI restrictions, attempt explicit breakaway and observe a lingering
   child's exit after job close. These paths did not execute. The ordinary control
   already has host-job membership; UI limits may conflict with nested jobs, so a
   standalone permitted host must be tested too. Do not remove required restrictions
   or enable breakaway to get a pass.
   [Nested job limitations](https://learn.microsoft.com/windows/win32/procthread/nested-jobs),
   [Job Object behavior](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects).
4. **Codex:** discovery and pin verification passed; contained startup did not run.
   Empty request-local auth cannot establish subscription inference. A broker or a
   separately authorised credential arrangement remains a design decision.
5. **#116 canaries:** blocked-by-design. Model refusal, no logged tools, and an
   unauthenticated startup cannot satisfy LW-111.

**Recommend the inference broker path.** Keep credentials and provider networking
in a trusted, narrow broker; the worker gets only a bounded, fixed-format inference
request/response channel, no provider credential and no general URL, shell or file
operation. A loopback TCP broker is not proven reachable here; prefer an explicitly
ACL-scoped inherited IPC channel, then test that exact channel and its validation.
A supported inference-only provider interface and its account/access arrangement
require an owner decision; subscription CLI access is not API entitlement.

For a future `loomward-windows/src/confine.rs`, the smallest useful production
contract is fail-closed launch preparation: pinned binary, explicit capability set,
owned request directory ACLs, minimal environment, exactly three inherited stdio
handles, and atomic job attachment. Return an explicit `profile_unavailable` or
`confinement_unverified` result with the original Win32/HRESULT cause. Never silently
run outside the boundary. Standard AppContainer versus LPAC must be a proven profile
decision; this spike only prepares standard AppContainer. Registering/provisioning
a profile is a **new owner decision**, not implied by q-5 or q-6. Do not turn this
example into automatic profile provisioning.

For L15, retain the existing synthetic-only route and personal-mode rejection.
Bind the full pinned argv/environment/binary/capability/IPC profile to its digest.
Do not advertise confined personal inference until C-FS, C-CTX, C-TOOLS and C-NET
all pass under the same worker and process tree. OS isolation limits effects of
tools; it does not establish that the CLI has removed its tool definitions.

Owner action: q-6 remains separate and cannot fix the missing launch profile by
itself. First choose broker architecture versus separately authorised profile
provisioning; if choosing direct provider egress, the owner's endpoint rule then
needs its own measured deny/allow controls. This lane requests no elevation and
makes no global changes.

## Proving and handoff

Changed: the example and Windows module, lab-only Windows feature flags and existing
SHA-256 dependency wiring, this report and the raw receipt. No commit or push.

Verified: ordinary native launch, SID derivation/profile lookup, exact launch error,
connect-only positive controls, disposable cleanup, argument quoting and capability
builder tests. All requested proving checks passed on Windows:

- `cargo fmt --all --check`
- `cargo test --workspace`: 195 tests passed, no failures
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test -p loomward-lab --example teacher_sandbox`: four tests passed
- `git diff --check` (new files additionally checked with `git diff --no-index --check`)

NOT verified: contained file/network/process behavior, job composition/UI enforcement,
Codex startup/auth/inference, #116 canaries, LPAC, endpoint filtering, elevated-refusal
execution, production runner integration, UI/Tauri or hosted CI.

Residual risk: **LW-111 remains blocked**, and the code paths beyond the failed launch
are compiled/tested builders, not Windows confinement evidence. Recommended single
commit: `Measure the unregistered teacher AppContainer launch gate`.
