# G5 Hyper-V power-cut campaign: first runs (interim)

Date: 2026-10-26 to 2026-10-27. Host: **DESKTOP-IHKOOJS**, Windows 11 Pro 10.0.26300, not elevated (the owner's
account is in Hyper-V Administrators, q-49). Design: [docs/53](../../docs/53-hyperv-crash-campaign.md); method chosen
by the owner as G5's (q-47 Q2); refs LW-133, #900, #901, #930.

> Hyper-V hard-off: guest RAM lost; host-backed storage keeps acknowledged writes; not a device flush-ordering or USB
> power-loss qualification.

**Verdict: G5 is not earned.** Floor credits: 4 of the 20 required, in 2 of the 4 required windows (create-batch, write;
none yet in publish or cleanup). Zero oracle failures, zero dirty `chkdsk`. The data-flush negative control has
**not been caught** in 10 cycles that cut a running copy, so under docs/53 section 4 this evidence covers journal and
staging consistency under guest-RAM loss only, not flush omission, and docs/47's G5 text goes back to the owner before
C8. C8 stays shut.

## Builds and VM (read back)

| Item | Value |
|---|---|
| Orchestrator | `scripts/hyperv_campaign.py` at `06daa51` (#930) for the launch-anchored runs; earlier heads of #900 and #901 for cycles 1-32 |
| Guest binary (genuine) | `loomward-windows` cargo test executable, `copy-primitives`, release, `+crt-static`, from `38dea551` (#900 round 2); SHA-256 `decf4160...3442862` |
| Guest binary (mutant) | the same commit with one line changed: `StagedFile`'s `self.file.sync_all()?` (the data flush before the `verified` record) removed; never committed; SHA-256 `2e817abc...e69867` |
| Guest image | Windows Server 2025 Standard Evaluation, Desktop Experience; ISO SHA-256 `D0EF4502...8579B23D` (observed; Microsoft publishes none) |
| VM | `LoomwardCrashLab`, Generation 2, configuration version 12.0, 2 vCPU, 4 GB static memory, checkpoints disabled (type and automatic), no network adapter; every path under `E:\loomward-lab\hyperv\` |
| Disks | SCSI 0:0 system, 0:1 lab VHDX (8 GB NTFS, `MaximumIOPS` 400), 0:2 signal VHDX; pristine copies hashed in `state\pristine.json` (outside Git) |
| Lab-disk write cache | enabled in the guest, read back before and after every cycle (`reported_value` 2) |
| Tree | `small` (cycles 1-47); `large` (negative 6, cycles 48-50) |

Full SHA-256 values: genuine `decf41606c8a29a05f0feae86db1679d6ffbac441e5839ee3e7754dab3442862`, mutant
`2e817abc93fb98f8b83d00aafa09c85a4da3e8416117fe7c759f260ca7e69867`, ISO
`D0EF4502E350E3C6C53C15B1B3020D38A5DED011BF04998E950720AC8579B23D`.

## Floor cycles (genuine build, in-flight)

| Cycle | Scenario | Last durable record | Window credited | Delay (ms) | Cut latency (ms) | Dirty pages | `chkdsk /scan` | Oracle |
|---|---|---|---|---|---|---|---|---|
| 17 | Run | identities | create-batch | 1,559 | 10,239 | 90 | clean | pass |
| 18 | Run | identities | create-batch | 152 | 8,161 | 185 | clean | pass |
| 19 | Run | planned | write | 3,931 | 5,708 | 94 | clean | pass |
| 21 | Run | identities | create-batch | 2,122 | 6,763 | 222 | clean | pass |

All four recovered from `copying` and ended `cancelled` after Abandon. Five other genuine cycles (13-16, 22) were cut
after the copy committed; they pass and are excluded from the floor, as section 3 requires. **Cut latency** is from the
last progress poll to `Stop-VM` returning: 5.7-10.2 s on this host under load, against the design's 0-2 s random delay,
so the delay does little to spread cuts; the windows reached are the slow early ones.

## Negative control (data-flush mutant)

| Run | Anchor | Tree, window | Cycles that cut a running copy | Caught | Other outcomes |
|---|---|---|---|---|---|
| neg 1-2 | progress poll | small, 0-2.5 s | 1 (cycle 12, `planned`) | 0 | 9 cut after commit (13-15, 25-32): excluded |
| neg 5 | launch | small, 0-1.5 s | 6 (36-39, 41, 43: `staging_intent`, `planned`, `create_batch`) | 0 | 6 `pre_copy` (40, 42, 44-47): cut before the first durable record, no evidence either way |
| neg 6 | launch | large, 0-4 s | 3 (48-50: `planned`, `planned`, `identities`) | 0 | stopped at cycle 51 by the lab-disk reset bug (below) |

Why the progress anchor missed: the mutant skips `FlushFileBuffers`, so its whole act finishes inside one PowerShell
Direct round trip (1-2 s) and every poll-anchored cut landed after commit. The launch anchor (#930) cuts a fixed delay
after the launch returns, with no poll, and does land inside the copy.

Why the launch-anchored cuts still miss: the mutant can only be observed after an item's `verified` record is durable
while its bytes are not. In all 10 cuts the last durable record was `planned` or earlier, so no item had a durable
`verified` record and losing its bytes was legal; the oracle was right to pass them. The large tree made planning
longer, not the write window easier to hit. A second, unmeasured effect would remain even then: `Stop-VM -TurnOff`
took 0.5-3.7 s from call to return in negative 5 and the guest's lazy writer flushes about once a second, so an
unflushed small item is likely on the virtual disk before the VM dies.

## Infrastructure failures (no cut; not counted either way)

Seventeen cycle rows (1-11, the first 21, 23, 24, 33-35): PowerShell Direct unavailable or timing out at boot (1, 2, 23, 24), a guest that failed to
boot (3), a cache read-back timeout (4), `CldFlt` not attached (7, fixed by attaching it every boot), a stale
`prepare.json` (8) and an existing cycle directory (the first cycle 21 row: a second, concurrent orchestrator refused
to reuse it), the guest's free-space query (5, 9-11, fixed in #901), an elevated token (6, a failure by design), and
the host lacking 4 GB for the VM (33, 34, while the owner's foreground work held the RAM). Cycle 35 cut before the
first durable record and stopped the campaign (`copy_did_not_run`); #930 now records that as `pre_copy`. After cycle
50 the large tree left `L:` under 2 GB and the first live lab-disk reset failed before replacing either disk (PowerShell
passed `$null` to `File.Replace` as an empty path; fixed in #945).

## What this does and does not show

- Shown: four genuine cuts with dirty guest pages, during create-batch and write, all recovering through a real reboot
  to a legal state with the oracle passing and a clean `chkdsk /scan`; no receipt claimed more than what verified.
- Not shown: the publish and cleanup windows; the floor of 20; the second mutant (verified record before the bytes);
  any flush-omission detection. The 5-10 s cut latency and a write window shorter than it are the obstacles.
- Next: cut closer to the act (a guest-side trigger on the signal disk, or `Stop-VM` issued from a pre-opened
  session) and a tree whose write phase outlasts that latency at the 400-IOPS cap (many medium files), so cuts land
  after durable `verified` records; then the floor, then both mutants for 50 cycles each, as docs/53 section 4 asks.

Raw records: `E:\loomward-lab\hyperv\results\campaign.jsonl` and per-cycle folders (outside Git: they hold lab paths
only, but stay with the lab).
