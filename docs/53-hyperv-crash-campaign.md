# Hyper-V crash campaign for the copy gate (G5)

Status: **design, proposed 2026-10-25** by the research lane (LW-133, [docs/51](51-research-programme.md) section 4.1).
It runs nothing yet: building the VM waits on the owner's answer to `loomward-hyperv-lab-setup` (HUMAN_TODO q-49).
Lab data only, never the owner's files and never `H:`. It opens no gate by itself: G5 is one input to C8
(docs/47 section 11).

## 1. What it proves, and what it does not

The C6 matrix (`evidence/v3/copy-faults.md`) kills the copy executor's process at every journal boundary. A process
kill keeps the operating system's cache, so it proves crash consistency, not power loss. The owner chose a Hyper-V VM
hard-stopped mid-copy as the G5 method (q-47 Q2). `Stop-VM -TurnOff` removes the guest's power: the guest's cache
manager, its NTFS metadata in memory and every unflushed write are lost.

It proves, for the guest's storage stack: the journal and staging invariants hold when the whole machine dies; no
receipt claims more than the bytes that survive; recovery after a real reboot (not a new process on a live system)
reaches a legal state.

It does not prove that a USB bridge or HDD honours a flush when its own power goes: the host stays powered and its
VHDX file sits behind the host's cache. Receipts never say "power-loss qualified" on this evidence (docs/51 section
4.1; physical qualification is LW-134).

## 2. Setup (one-off, after the owner's answer)

| Item | Choice |
|---|---|
| VM | Generation 2, `LoomwardCrashLab`, 2 vCPU, 4 GB static memory, checkpoints **off** (a checkpoint would make a hard-off recoverable by the host) |
| Storage | system VHDX and a separate 8 GB **lab VHDX** (NTFS, the copy source and destination), both under `E:\loomward-lab\hyperv\` |
| Guest OS | the owner's ISO or Microsoft's Windows evaluation image (q-49 option a or b) |
| Guest tooling | the test build of `loomward-windows` with `copy-primitives`, copied in by PowerShell Direct; no network adapter |
| Host privilege | the owner's account in "Hyper-V Administrators" (q-49); nothing else elevated |
| Guest write cache | the lab VHDX attached with the default write caching; recorded in the evidence, because it changes what a hard-off loses |

The host never touches `C:` for VM storage (it is about 20 GB from Loomward's reserve) and refuses to start if `E:`
has less than 80 GB free.

## 3. One cycle

1. **Start** the VM. The host waits for a guest heartbeat (Hyper-V integration services) with a 5-minute timeout.
2. **Arm.** Through PowerShell Direct the host starts the guest runner with the scenario and a boundary `N` (C6's
   numbering, CP-01 to CP-22, or a random time offset). The runner sets `LOOMWARD_COPY_PAUSE_AT=N` (new, test builds
   only, next to C6's `LOOMWARD_COPY_CRASH_AT`): at the Nth boundary it flushes nothing the executor would not,
   writes `armed N label` to the lab volume's marker file through an **unbuffered, write-through** handle, and blocks.
3. **Cut.** The host polls the marker over PowerShell Direct (read only), and on `armed` calls
   `Stop-VM -TurnOff -Force`. Random-time cycles skip the marker and cut after a random delay inside the copy.
4. **Recover.** Start the VM; the guest runner opens `state.db`, runs `recover`, then the C6 oracle (all seven
   checks), then Abandon if the operation is open, then the oracle again.
5. **Record** one JSON line on the host: cycle, scenario, boundary, label at the cut, state after recovery and after
   Abandon, oracle result, guest `chkdsk /scan` result for the lab volume, timings. The lab VHDX is reset from a
   pristine copy every 50 cycles so drift never masks a fault.

Any oracle failure stops the campaign at once and keeps the VM and lab VHDX for diagnosis.

## 4. Campaign size

| Tier | Runs | Purpose |
|---|---|---|
| G5 floor (docs/47 section 10) | at least 20, covering the create-batch, publish and cleanup windows | the gate |
| Boundary sweep | every CP-01 to CP-22 boundary of the Run and Abandon scenarios once (about 120) | coverage |
| Target | about 2,000 including random-time cuts | the "crash-qualified" label (docs/51 section 4.1) |

A cycle is about 1-2 minutes (boot, copy, cut, boot, recover). The floor takes about an hour; the target about three
to four days unattended. Zero failures in N runs bounds the per-cycle failure rate below about 3/N at 95%, but the
value is coverage of fault points, not that number.

## 5. Evidence and acceptance

`evidence/v3/copy-power-cut-hyperv.md`: host and guest builds, commit, VM settings (memory, checkpoints off, cache
mode), the lab tree, the JSON summary (cycles per boundary class, outcomes, oracle failures: zero), and the statement
"Hyper-V hard-off: guest power loss; host-backed storage; not a USB power-loss qualification". Acceptance: zero oracle
failures, every CP class cut at least once, at least 20 cuts inside the create-batch, publish and cleanup windows,
`chkdsk /scan` clean after every cycle or the cycle reported.

## 6. Lanes

| Lane | Delivers | Effect | Acceptance |
|---|---|---|---|
| H1 | this design | none | review |
| H2 | the pause hook `LOOMWARD_COPY_PAUSE_AT` in `effects/exec.rs` beside the C6 kill hook (test builds only) and the guest runner (a test-binary mode: run, recover, oracle, report JSON) | lab only | C6's harness runs the pause mode in-process: pause at N, then `TerminateProcess` from the parent, same oracle; second review lens (copy engine) |
| H3 | the host orchestrator (`scripts/hyperv_campaign.py`): plan, preflight (Hyper-V module, VM absent or ours, `E:` free space, never `C:` or `H:`), a dry-run that prints every cmdlet it would run, the cycle loop, JSONL output | none in dry-run | unit tests with a fake runner: refuses `C:`/`H:` paths, refuses an existing foreign VM, stops on the first oracle failure |
| H4 | VM build and the campaign itself | lab VM only | section 5, after q-49 |
