# Hyper-V crash campaign for the copy gate (G5)

Status: **design, proposed 2026-10-25** by the research lane (LW-133, [docs/51](51-research-programme.md) section 4.1).
It runs nothing yet: building the VM waits on the owner's answer to `loomward-hyperv-lab-setup` (HUMAN_TODO q-49).
Lab data only, never the owner's files and never `H:`. It opens no gate by itself: G5 is one input to C8
(docs/47 section 11). No agent elevates: the Hyper-V feature is already enabled (`vmms` running, measured
2026-10-25) and the only privileged step, membership of "Hyper-V Administrators", is the owner's.

## 1. What it proves, and what it does not

The C6 matrix (`evidence/v3/copy-faults.md`) kills the copy executor's process at every journal boundary. A process
kill keeps the operating system's cache, so it proves crash consistency, not power loss. The owner chose a Hyper-V VM
hard-stopped mid-copy as the G5 method (q-47 Q2). `Stop-VM -TurnOff` kills the VM worker: the guest's RAM is lost,
including its cache manager, dirty pages and NTFS metadata held in memory.

It proves, for the guest: the journal and staging invariants hold when the whole machine dies with dirty data in
guest RAM; recovery after a real reboot (not a new process on a live system) reaches a legal state; no receipt claims
more than what verifies after the reboot.

It does **not** prove flush ordering or barrier correctness at the device. The host stays powered, so every write the
guest already handed to its virtual disk survives in the host's cache, flushed or not, and a VHDX never reorders or
drops acknowledged writes the way a real disk's volatile cache can. Nor does it say anything about a USB bridge or HDD
losing its own power. Receipts never say "power-loss qualified" on this evidence (docs/51 section 4.1; physical
qualification is LW-134). The evidence file states this scope verbatim (section 5).

## 2. Setup (one-off, after the owner's answer)

| Item | Choice |
|---|---|
| VM | Generation 2, `LoomwardCrashLab`, 2 vCPU, 4 GB static memory, no network adapter; built by `scripts/hyperv_build_vm.ps1` without elevation |
| Every VM path on `E:` | `New-VM -Path`, `-NewVHDPath`, `Set-VM -SnapshotFileLocation`, `-SmartPagingFilePath` all under `E:\loomward-lab\hyperv\`; the orchestrator reads back every path the VM reports (configuration, runtime state, VHDX, checkpoints) and refuses anything outside that prefix after resolving junctions and mount points (an allowlist, not a `C:`/`H:` denylist); the host defaults (`Get-VMHost`) are never relied on |
| Checkpoints | `-CheckpointType Disabled`, `AutomaticCheckpointsEnabled $false`, so no differencing disk sits under the guest |
| Disks | system VHDX; an 8 GB **lab VHDX** (NTFS; copy source, destination and `state.db`); a separate 1 GB **signal VHDX** (its own device, never written by the copy) |
| Lab disk cache | write caching enabled in the guest, pinned and read back each boot, recorded per cycle |
| Guest accounts | a **standard** local user runs the runner; a separate guest administrator PowerShell Direct session does only `chkdsk /scan` and the write-cache read-back, and is never used for the runner; the runner and the orchestrator treat an elevated token (the copy primitives skip under one, `exec_crash_tests.rs`) as a cycle **failure**, never a skip |
| Guest tooling | the `loomward-windows` **cargo test executable** built with `copy-primitives` (the hooks are `#[cfg(test)]`), in release mode with a static C runtime (`-C target-feature=+crt-static`), copied in through PowerShell Direct and checked by SHA-256 on both sides |
| Guest image | Windows Server 2025 Standard Evaluation, **Desktop Experience** (image index 2), from Microsoft's evaluation ISO downloaded to `E:` (owner go-ahead 2026-10-26, q-49); no published hash, so the observed SHA-256 is recorded. Server Core does not work: it lacks `cldapi.dll`, which the copy engine imports |
| Guest credentials | random passwords in `E:\loomward-lab\hyperv\state\guest.json` (outside Git, ACL limited to the owner's account and SYSTEM); never printed |
| Cloud Files filter | Windows Server leaves `CldFlt` running but attached to no volume, and the copy engine's sync-root query then fails with `ERROR_INVALID_FUNCTION`; the admin session attaches it to `L:` after every boot (`fltmc attach CldFlt L:`) |
| Lab folder ACL | `L:\lab` gets a protected, auto-inherited DACL (`icacls /inheritance:d`, SDDL `D:PAI`): under a fresh volume root's legacy DACL every child ACE reads as explicit and C1 reports a security-descriptor difference on every node (#899) |
| Session identity | PowerShell Direct on this host was measured handing a new session the identity of the **previous** session to the VM, whatever credential was supplied; every session therefore checks `WindowsIdentity.GetCurrent()` and reconnects until it is the requested account |

The orchestrator refuses to start if `E:` has less than 80 GB free. The act starts inside the same PowerShell Direct
session that polls and cuts: a process started through PowerShell Direct ends when its session closes, so a separate
launch session would kill the act before the hard-off.

## 3. One cycle

**Floor cycles (in-flight, no pause; these alone count toward the G5 floor of 20).**

1. Start the VM; wait for the integration-services heartbeat (5-minute timeout; a guest that fails to boot stops the
   campaign and is kept for diagnosis).
2. Through PowerShell Direct, start the runner on the Run scenario, or on the **Abandon** scenario with staging removal
   on (a copy stopped after its first verified batch, then abandoned; the cut lands during cleanup). Before arming it
   writes a **pre-arm snapshot** to the **signal disk**: identity, signature and SHA-256 of every source object and of
   the sentinel entries pre-seeded in the destination parent; the host copies it to the orchestrator's state outside
   Git. Each cycle copies into a fresh destination parent, removed by the orchestrator through the reset below.
3. The runner reports progress on the signal disk only (never the lab disk), so no signal write can flush the lab
   volume. Once the copy has passed its first journal record, the host waits a random 0-2,000 ms and calls
   `Stop-VM -TurnOff -Force`. The lab disk's IOPS are capped (`Set-VMHardDiskDrive -MaximumIOPS`) and the tree holds
   thousands of small files plus large ones, so the create-batch, write, publish and cleanup windows are seconds wide.
4. The guest's dirty-cache size at the last progress report (`\Cache\Dirty Pages`) is recorded; a cycle with none
   is not credited to the floor.

**Coverage cycles (pause at boundary N; never counted toward the floor).** The runner sets
`LOOMWARD_COPY_PAUSE_AT=N` (new, beside C6's `LOOMWARD_COPY_CRASH_AT`, test builds only), signals `armed N label` on the
signal disk and blocks. These mostly re-prove C6 with boot-time recovery added: while the executor is blocked the
guest's lazy writer keeps flushing, so a pause cut is closer to a kill followed by a partial shutdown than to a power
cut.

**Recovery (every cycle).** Boot. Before `recover`, the runner records a destination baseline. Then: `recover`; the
oracle; Abandon if the operation is open; the oracle again. The oracle is C6's seven checks adapted to a reboot:
check 2 compares the source against the pre-arm snapshot (not an in-memory copy); the sentinel entries must match the
pre-arm snapshot exactly (never the post-boot baseline, so a deleted or replaced sentinel always fails); checks 4 and
6 compare Loomward-created entries against the post-boot baseline, because those may legitimately be lost at the cut; check 5 allows an
entry to be absent only when the journal never recorded it as durable, and requires every Loomward removal to be
identity-bound as before. A cycle passes only if it shows a copy actually ran (a journal with at least one record
after `preparing`).

**Record.** One JSON line on the host: cycle kind, scenario, Q1 policy, cut delay or boundary, last durable journal
record after reboot, states after recovery and after Abandon, oracle result, dirty pages, `chkdsk /scan` result for the
lab volume, timings. The window a floor cycle is credited to is the one its **last durable journal record** names;
cycles cut after the copy completed are excluded from the floor. The lab and signal VHDXs are reset from pristine copies every 50 cycles, or earlier when the lab volume falls below
2 GB free.

Any oracle failure, elevated token, or dirty `chkdsk` stops the campaign and keeps the VM and disks for diagnosis.

## 4. Negative control (before any run counts)

The campaign must be shown able to fail, as C6's oracle was: a mutant test build with the data flush removed before
the `verified` record, and one with the verified record written before the bytes, each run for 50 floor cycles. If the
campaign catches them, G5 covers flush omission under guest-RAM loss; if it does not, the evidence says G5 covers only
journal and staging consistency under guest-RAM loss, and docs/47's G5 text is revisited with the owner before C8.

## 5. Campaign size, evidence and acceptance

| Tier | Runs | Purpose |
|---|---|---|
| G5 floor (docs/47 section 10) | at least 20 in-flight cuts at random times, credited to at least the create-batch, write, publish and cleanup windows | the gate |
| Boundary sweep (coverage) | every CP-01 to CP-22 boundary of the Run, Abandon and Confirm scenarios under both Q1 policies (planned: `scripts/hyperv_campaign.py` runs only `run`, `abandon` and `mixed` today; Confirm is not built) | coverage |
| Target | about 2,000 cycles, mostly in-flight | the "crash-qualified" label (docs/51 section 4.1) |

A cycle is about 1-2 minutes. The floor takes about an hour; the target three to four days unattended.

`evidence/v3/copy-power-cut-hyperv.md`: host and guest builds, commit, VM settings read back (paths, memory,
checkpoints, lab-disk cache), the tree, the negative-control result, per-window floor credits, outcomes, dirty-page
distribution, oracle failures (zero), and the statement: "Hyper-V hard-off: guest RAM lost; host-backed storage keeps
acknowledged writes; not a device flush-ordering or USB power-loss qualification." Acceptance: negative control run
and reported; zero oracle failures; every floor window credited at least once; `chkdsk /scan` clean on every cycle (a
dirty result fails G5 until explained).

## 6. Lanes

| Lane | Delivers | Effect | Acceptance |
|---|---|---|---|
| H1 | this design | none | review |
| H2 | the pause hook `LOOMWARD_COPY_PAUSE_AT` beside the C6 kill hook (test builds only), progress and snapshot output to a caller-chosen signal path, and the guest runner mode (run, recover, adapted oracle, JSON) | lab only | C6's harness drives pause mode in-process with the same oracle; the elevated-token case fails; second review lens (copy engine) |
| H3 | the host orchestrator (`scripts/hyperv_campaign.py`): plan, preflight (Hyper-V module, VM absent or ours, `E:` free space, every VM path inside the allowlist), a dry-run printing every cmdlet, the cycle loop, JSONL | none in dry-run | unit tests with a fake runner: refuses paths outside the allowlist, refuses a foreign VM, stops on the first oracle failure, elevated token or dirty `chkdsk`, never credits a post-completion cut to the floor |
| H4 | VM build, negative control, then the campaign | lab VM only | section 5, after q-49 |
