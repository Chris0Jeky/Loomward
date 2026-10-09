# Resource companion and process control

## A useful interface, not a RAM cleaner
The aim is to explain and manage workloads with understandable controls. It is not to reproduce arbitrary PowerShell execution behind a text box or make a “free RAM” number rise. Windows manages resident pages, caches and paging; taking pages out of a working set does not necessarily reduce an application's committed allocations [R19,R20]. Aggressive trimming can trade a cosmetically smaller resident number for future faults and latency.

Expose physical memory total/available, process resident working set, private committed memory when reliably available, CPU rate with sample interval, disk I/O, process instance identity, ownership, parent/group, executable/publisher information where available, and observation quality. Do not sum process RSS as if shared pages were unique total machine memory. A first sample cannot give a valid delta CPU rate.

The reference `Telemetry` uses optional psutil. It keys samples by PID plus process creation time, computes CPU deltas relative to elapsed monotonic time and total logical CPU capacity, and displays unknown values when unsupported. It lists at most 200 processes and reports observation/truncation state. It cannot change a process. Linux observations and tests do not validate Windows permission or accounting behavior.

## Control the companion itself first
Establish separate budgets for the catalogue engine, UI, parsers, hash workers, embedding inference, teacher client and external model server. Large LLM residency should be optional and short-lived. Batch questions, cache versioned features, coalesce duplicate work and unload only model sessions that Loomward owns or has permission to manage [R29]. A separate shared model server may be serving the user's coding assistant; unloading it blindly would be a product failure.

For Windows workloads launched by Loomward, evaluate Job Objects [R17] and suitable process policies [R18]. Begin with an explicit “launch this worker under a budget” interface. A job's memory limit may cause allocation failure rather than graceful quality degradation, so user-facing limits need warning, headroom, monitoring and cancellation behavior. Do not apply tight hard limits to arbitrary already-running desktop applications.

## Policy ladder
1. **Observe:** explain recent resource changes and show missing coverage.
2. **Advise:** suggest closing unused model sessions or pausing a task, with evidence.
3. **Cooperate:** request lower concurrency, model unload, smaller batches or a normal stop through a provider.
4. **Reversible Windows policy:** adjust a supported priority/power policy for the exact allowed process instance, save the previous state and set expiry.
5. **Managed workload bounds:** enforce CPU/commit budgets for workloads launched into an appropriate job.
6. **Terminate:** last-resort explicit user action with an unsaved-work warning and protected-process exclusions; never an LLM action.

Suspending an arbitrary process is not automatically harmless: it can hold locks or interfere with applications and services. Do not equate “pause” with OS-level suspend in the UI. Prefer an application-level pause when supported.

## Exact-instance targeting
A future action must verify PID plus creation time and, where useful, executable identity and owner/session. PIDs are reusable. Opening a handle, checking rights and revalidating instance identity happen near execution. The UI cannot approve “whatever process later has PID 1234.” Protected/system/security processes, services and another user's processes need explicit exclusions or dedicated modes, not implicit escalation.

Use standard-user visibility first. Access denied is a normal observation result, not a reason to relaunch the entire app as administrator. Any privileged broker must have a tiny typed request set and independent validation. The ability to inspect a process is not permission to terminate it.

## Profiles and explanations
Offer user-understandable profiles such as “foreground work,” “quiet background processing,” “on battery,” and “overnight maintenance.” Each profile expands to visible settings, duration, workload scope and expected trade-offs. Avoid unsupported promises such as “make this application use exactly 2 GB” or “fix all memory leaks.”

An evidence card might say: “The local model server has retained a model for 42 minutes without a Loomward request. Another client may still use it. Ask the server for active sessions, or leave it loaded.” A future leak candidate might say “private committed memory has increased during this observation window,” with a chart and uncertainty. Neither is proof of a bug or safe grounds for killing it.

## Performance fairness
Measure the full companion, including WebView/browser processes and optional model server, not only the Rust binary. Report background CPU, private commit, resident footprint, read/write throughput, page faults where supported, wake-ups, battery impact and foreground latency. The companion should back off under pressure rather than win its own throughput benchmark by degrading the machine.

## Acceptance before modification
Use disposable worker programs with known memory/CPU behavior. Verify exact-instance targeting, expiry/restoration of policies, process exit races, unsupported rights, job-assignment restrictions, cooperative cancellation and memory-limit failures. Assert protected-process denial and that UI/agent inputs cannot inject a raw command line. No process-control milestone is complete until the effects and rollback are measured on Windows.
