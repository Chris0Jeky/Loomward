"""Hyper-V guest-RAM-loss campaign. Default: print a plan; never contact the host.

Only --execute crosses the PowerShell boundary. This is not device power-loss
qualification. Credentials and pristine images belong to the private lab state.
"""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass, field
import json
import math
import ntpath
import random
import re
import secrets
import subprocess
import sys
import time


LAB_ROOT = r"E:\loomward-lab\hyperv"
VM_NAME = "LoomwardCrashLab"
RESULTS = LAB_ROOT + r"\results"
CAMPAIGN_LOG = RESULTS + r"\campaign.jsonl"
STATE = LAB_ROOT + r"\state"
PRISTINE_LAB = STATE + r"\pristine-lab.vhdx"
PRISTINE_SIGNAL = STATE + r"\pristine-signal.vhdx"
GUEST_LAB = "L:"
GUEST_SIGNAL = "S:"
GUEST_EXE = r"C:\lw\loomward_windows_tests.exe"
RUNNER_ACCOUNT = "lwrunner"
ADMIN_ACCOUNT = "lwadmin"
GIB = 1024**3
LAB_DISK_BYTES = 8 * GIB
SIGNAL_DISK_BYTES = GIB
BOOT_TIMEOUT = 300.0
ACT_TIMEOUT = 300.0
POLL_SECONDS = 0.25
TEST_ARGS = "effects::exec::hyperv_tests::hyperv_guest --exact --test-threads=1 --nocapture"


class CampaignError(RuntimeError):
    """Refusal or failure: leave the VM and disks as they are for diagnosis."""


@dataclass
class Config:
    command: str = "cycle"
    cycles: int = 1
    scenario: str = "run"
    removal: str = "0"
    tree: str = "small"
    seed: int = field(default_factory=lambda: secrets.randbits(32))
    execute: bool = False
    exe: str = GUEST_EXE
    max_delay_ms: int = 2000
    boot_timeout: float = BOOT_TIMEOUT
    connect_timeout: float = 120.0


class PowerShell:
    """The sole system boundary; Campaign does no filesystem or process I/O."""

    def run(self, script: str, timeout: float) -> str:
        try:
            result = subprocess.run(
                ["powershell", "-NoProfile", "-NonInteractive", "-Command", "-"],
                input=script, text=True, encoding="utf-8", capture_output=True,
                timeout=timeout, check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            # TimeoutExpired can include captured guest output; never dump it.
            raise CampaignError("powershell_unavailable_or_timeout") from exc
        if result.returncode:
            raise CampaignError(result.stderr.strip() or "powershell_failed")
        return result.stdout


def quote(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def encoded(value: str) -> str:
    payload = base64.b64encode(value.encode("utf-8")).decode("ascii")
    return f"[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{payload}'))"


def window_for_label(label: str | None) -> str:
    label = (label or "").lower()
    for parts, window in (
        (("item_removed", "staging_removed", "cancelling"), "cleanup"),
        (("create", "identities"), "create_batch"),
        (("written", "verif", "chunk"), "write"),
        (("publish", "rename", "committed"), "publish"),
    ):
        if any(part in label for part in parts):
            return window
    return "other"


def inside_lab(path: str) -> bool:
    if not isinstance(path, str) or not ntpath.isabs(path):
        return False
    path = ntpath.normcase(ntpath.normpath(path))
    root = ntpath.normcase(LAB_ROOT)
    return path == root or path.startswith(root + "\\")


# Resolve-Path alone does not dereference all Windows junctions. Refuse reparse
# ancestors as well, rather than accepting a lexical path to an outside target.
PATH_GUARD = r"""
function Resolve-LabPath([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path)
    $existing = $full
    while (-not (Test-Path -LiteralPath $existing)) {
        $parent = Split-Path -Parent $existing
        if (-not $parent -or $parent -eq $existing) { throw 'path_unresolvable' }
        $existing = $parent
    }
    $resolved = (Resolve-Path -LiteralPath $existing -ErrorAction Stop).ProviderPath
    $resolved = [IO.Path]::GetFullPath($resolved).TrimEnd('\')
    $prefix = 'E:\loomward-lab\hyperv'
    if (-not ($resolved.Equals($prefix, [StringComparison]::OrdinalIgnoreCase) -or
        $resolved.StartsWith($prefix + '\', [StringComparison]::OrdinalIgnoreCase))) {
        throw "path_outside_allowlist: $Path"
    }
    $cursor = $existing
    while ($cursor) {
        $item = Get-Item -LiteralPath $cursor -Force -ErrorAction Stop
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "path_reparse_point: $Path"
        }
        $parent = Split-Path -Parent $cursor
        if ($parent -eq $cursor) { break }
        $cursor = $parent
    }
    # Existing ancestors are canonical and link-free; suffix may be a new file.
    if (-not ($full.Equals($prefix, [StringComparison]::OrdinalIgnoreCase) -or
        $full.StartsWith($prefix + '\', [StringComparison]::OrdinalIgnoreCase))) {
        throw "path_outside_allowlist: $Path"
    }
    return $full
}
"""


# STORAGE_WRITE_CACHE_PROPERTY.WriteCacheEnabled is an enum at byte offset 12:
# 0 unknown, 1 disabled, 2 enabled. Query the physical disk backing L:, not C:.
CACHE_QUERY = r"""
if (-not ('LwCacheQuery' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class LwCacheQuery {
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern SafeFileHandle CreateFile(string name, uint access, uint share,
        IntPtr security, uint creation, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool DeviceIoControl(SafeFileHandle h, uint code, byte[] input,
        uint inputSize, byte[] output, uint outputSize, out uint returned, IntPtr overlapped);
    public static int Read(int number) {
        using (var handle = CreateFile(@"\\.\PhysicalDrive" + number, 0, 3,
                IntPtr.Zero, 3, 0, IntPtr.Zero)) {
            if (handle.IsInvalid) throw new Win32Exception();
            var query = new byte[12]; query[0] = 4;
            var output = new byte[64]; uint returned;
            if (!DeviceIoControl(handle, 0x2d1400, query, 12, output, 64,
                    out returned, IntPtr.Zero)) throw new Win32Exception();
            if (returned < 28) throw new Exception("short_write_cache_property");
            return BitConverter.ToInt32(output, 12);
        }
    }
}
'@
}
$disk = Get-Partition -DriveLetter L -ErrorAction Stop | Get-Disk -ErrorAction Stop
$cacheValue = [LwCacheQuery]::Read($disk.Number)
$cache = @{ enabled = ($cacheValue -eq 2); reported_value = $cacheValue }
"""


class Campaign:
    def __init__(self, ps, rng, clock, sleep, config: Config):
        self.ps, self.rng, self.clock, self.sleep, self.config = ps, rng, clock, sleep, config
        if config.cycles < 1:
            raise CampaignError("cycles_must_be_positive")
        # The only arbitrary CLI string crosses a cmd.exe wrapper. Disallow its
        # expansion/metacharacters, and require an absolute executable path.
        if (not config.exe.isascii()
                or not re.fullmatch(r"[A-Za-z]:\\[^\r\n\x00\"%!?&|<>^*]*\.exe", config.exe)
                or config.scenario not in ("run", "abandon", "mixed")
                or config.removal not in ("0", "1", "mixed")
                or config.tree not in ("small", "large")):
            raise CampaignError("invalid_configuration")
        self.summary = {"cycles": 0, "floor_credits": dict.fromkeys(
            ("create_batch", "write", "publish", "cleanup", "other"), 0),
            "failures": 0, "caught": 0, "dry_run": not config.execute}

    def _call(self, operation, body, default=None, timeout=120, guard=False):
        script = (
            f"# lw-hv:{operation}\n"
            "$ErrorActionPreference = 'Stop'\n"
            "[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)\n"
            "try {\n" + (PATH_GUARD if guard else "") + body +
            "\n} catch { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }\n\n"
        )
        if "New-PSSession" in body:
            # The PowerShell Direct connection window is spent inside the script.
            timeout += self.config.connect_timeout
        if not self.config.execute:
            # State is private even in a printed plan. Passwords never enter Python.
            print(script.replace(STATE, "<redacted-state>").replace(
                "-Credential $credential", "-Credential '<redacted>'"))
            return default if default is not None else {}
        try:
            output = self.ps.run(script, timeout)
        except CampaignError as exc:
            raise CampaignError(f"{operation}:" + str(exc).replace(STATE, "<redacted-state>")) from exc
        try:
            data = json.loads(output)
        except (ValueError, TypeError) as exc:
            raise CampaignError(f"invalid_response:{operation}") from exc
        if not isinstance(data, dict):
            raise CampaignError(f"invalid_response:{operation}")
        runner_text = str(data.get("runner_output", ""))
        if isinstance(data.get("result"), dict):
            runner_text += "\n" + str(data["result"].get("failure", ""))
        if (data.get("elevated_token") is True or re.search(
                r"elevated[-_ ]token|token[^\r\n]*elevated|running as (?:an? )?admin",
                runner_text, re.IGNORECASE)):
            raise CampaignError("elevated_token")
        return data

    def _credential(self, admin=False):
        account = ADMIN_ACCOUNT if admin else RUNNER_ACCOUNT
        key = "admin" if admin else "runner"
        credential_path = STATE + r"\guest.json"
        return f"""
$credentialPath = Resolve-LabPath {quote(credential_path)}
try {{
    $secrets = Get-Content -LiteralPath $credentialPath -Raw | ConvertFrom-Json
    $password = $secrets.{key}
    if (-not ($password -is [string]) -or -not $password) {{ throw 'missing' }}
    $secure = ConvertTo-SecureString $password -AsPlainText -Force
    $credential = [PSCredential]::new({quote(account)}, $secure)
}} catch {{ throw 'guest_credentials_unavailable' }}
$password = $null; $secrets = $null
"""

    def _session(self, admin=False):
        return self._credential(admin=admin) + f"""
$connectionDeadline = (Get-Date).AddSeconds({self.config.connect_timeout:g})
while ($true) {{
    try {{
        $session = New-PSSession -VMName {quote(VM_NAME)} -Credential $credential -ErrorAction Stop
        break
    }} catch {{
        if ((Get-Date) -ge $connectionDeadline) {{ throw 'guest_direct_connection_timeout' }}
        Start-Sleep -Seconds 1
    }}
}}
"""

    def preflight(self):
        body = f"""
$module = [bool](Get-Command Get-VM -ErrorAction SilentlyContinue)
if (-not $module) {{ throw 'hyperv_module_unavailable' }}
$free = (Get-PSDrive -Name E -ErrorAction Stop).Free
$vm = Get-VM -Name {quote(VM_NAME)} -ErrorAction SilentlyContinue
$info = $null
if ($vm) {{
    $paths = @()
    foreach ($path in @($vm.ConfigurationLocation, $vm.SnapshotFileLocation, $vm.SmartPagingFilePath) +
        @(Get-VMHardDiskDrive -VMName {quote(VM_NAME)} | ForEach-Object {{ $_.Path }})) {{
        if (-not $path -or -not (Test-Path -LiteralPath $path)) {{ throw 'vm_path_unresolvable' }}
        $paths += @{{ raw = $path; resolved = (Resolve-LabPath $path) }}
    }}
    $info = @{{ name = $vm.Name; checkpoint_type = [string]$vm.CheckpointType;
        automatic_checkpoints = $vm.AutomaticCheckpointsEnabled;
        network_adapters = @(Get-VMNetworkAdapter -VMName {quote(VM_NAME)}).Count; paths = @($paths) }}
}}
@{{ module = $module; free_bytes = $free; vm = $info }} | ConvertTo-Json -Depth 8 -Compress
"""
        result = self._call("preflight", body, {"module": True, "free_bytes": 100 * GIB,
            "vm": {"name": VM_NAME, "checkpoint_type": "Disabled", "automatic_checkpoints": False,
                   "network_adapters": 0, "paths": [{"resolved": LAB_ROOT}]}}, guard=True)
        if result.get("module") is not True:
            raise CampaignError("hyperv_module_unavailable")
        if result.get("free_bytes", -1) < 80 * GIB:
            raise CampaignError("host_free_space_below_80_GB")
        vm = result.get("vm")
        if vm is not None:
            if vm.get("name") != VM_NAME:
                raise CampaignError("foreign_vm")
            if vm.get("checkpoint_type") != "Disabled" or vm.get("automatic_checkpoints") is not False:
                raise CampaignError("checkpoints_enabled")
            if vm.get("network_adapters") != 0:
                raise CampaignError("network_adapter_present")
            if not vm.get("paths"):
                raise CampaignError("vm_paths_missing")
            for path in vm["paths"]:
                if not inside_lab(path.get("resolved")):
                    raise CampaignError("path_outside_allowlist:" + str(path.get("resolved")))
        return result

    def _boot(self):
        self._call("start", f"""
$vm = Get-VM -Name {quote(VM_NAME)}
if ($vm.State -eq 'Off') {{ Start-VM -Name {quote(VM_NAME)} | Out-Null }}
elseif ($vm.State -ne 'Running') {{ throw 'vm_not_off_or_running' }}
'{{}}'
""")
        deadline = self.clock() + self.config.boot_timeout
        while True:
            status = self._call("boot", f"""
$heartbeat = Get-VMIntegrationService -VMName {quote(VM_NAME)} -Name Heartbeat
@{{ heartbeat = [string]$heartbeat.PrimaryOperationalStatus;
    secondary = [string]$heartbeat.SecondaryOperationalStatus }} | ConvertTo-Json -Compress
""", {"heartbeat": "OkApplicationsHealthy"}, timeout=60)
            if status.get("heartbeat") in ("Ok", "OkApplicationsHealthy"):
                return
            if self.clock() >= deadline:
                raise CampaignError("guest_boot_failed")
            if self.config.execute:
                self.sleep(min(1.0, max(0.0, deadline - self.clock())))

    def _admin(self, health=False):
        check = r"""
$dirtyQuery = (& fsutil dirty query L: 2>&1 | Out-String)
if ($LASTEXITCODE -ne 0) { throw 'dirty_query_failed' }
if ($dirtyQuery -notmatch '\bis (?:NOT )?Dirty\b') { throw 'dirty_query_unrecognised' }
$bootTime = (Get-CimInstance Win32_OperatingSystem).LastBootUpTime
$eventErrors = @()
$events = @(Get-WinEvent -FilterHashtable @{LogName='Application','System'; StartTime=$bootTime} `
    -ErrorAction SilentlyContinue -ErrorVariable eventErrors)
if (@($eventErrors | Where-Object { $_.FullyQualifiedErrorId -notlike 'NoMatchingEventsFound*' }).Count) {
    throw 'filesystem_events_unavailable'
}
$repairs = @($events | Where-Object {
    $_.ProviderName -in @('Wininit', 'Microsoft-Windows-Wininit', 'Chkdsk', 'Ntfs', 'Microsoft-Windows-Ntfs') -and (
        $_.Level -in @(1, 2, 3) -or
        ($_.ProviderName -in @('Ntfs', 'Microsoft-Windows-Ntfs') -and $_.Id -in @(55, 130, 131, 140)) -or
        ($_.Id -eq 98 -and $_.Message -match 'needs.*(repair|chkdsk)|corrupt') -or
        ($_.Id -in @(1001, 26226) -and $_.Message -match 'made corrections|correcting|repairing|repaired')
    )
} | Select-Object TimeCreated, ProviderName, Id, Level, Message)
$filesystem = @{ dirty = [bool]($dirtyQuery -match '\bis Dirty\b'); dirty_query = $dirtyQuery;
    boot_time = $bootTime.ToString('o'); repair_events = @($repairs) }
$chkdsk = $null
# Preserve boot-time repair evidence; /scan must not mask a dirty/auto-repaired volume.
if (-not $filesystem.dirty -and $repairs.Count -eq 0) {
    $summary = (& chkdsk L: /scan 2>&1 | Out-String)
    $code = $LASTEXITCODE
    $chkdsk = @{ code = $code; clean = ($code -eq 0); summary = $summary }
}
""" if health else ""
        response = ("@{ chkdsk = $chkdsk; filesystem = $filesystem; "
                    "write_cache = $cache }") if health else "$cache"
        body = self._session(admin=True) + f"""
try {{
$answer = Invoke-Command -Session $session -ScriptBlock {{
    $ErrorActionPreference = 'Stop'
    # Server does not attach the Cloud Files filter to data volumes; without it the copy engine's
    # sync-root query fails with ERROR_INVALID_FUNCTION. The attach does not survive a reboot.
    & fltmc.exe attach CldFlt L: 2>&1 | Out-Null
    if (-not ((& fltmc.exe instances -f CldFlt 2>&1 | Out-String) -match '(?m)^L:')) {{ throw 'cldflt_not_attached' }}
    {check}
    {CACHE_QUERY}
    {response}
}}
$answer | ConvertTo-Json -Depth 8 -Compress
}} finally {{ Remove-PSSession $session }}
"""
        default = {"chkdsk": {"code": 0, "clean": True, "summary": "dry run"},
                   "filesystem": {"dirty": False, "repair_events": [], "dirty_query": "dry run"},
                   "write_cache": {"enabled": True}} if health else {"enabled": True}
        return self._call("health" if health else "cache", body, default, timeout=600, guard=True)

    def _env(self, cycle, scenario, removal):
        tag = f"c{cycle:05d}"
        return {
            "LOOMWARD_HV_LAB": GUEST_LAB + "\\lab\\" + tag,
            "LOOMWARD_HV_SIGNAL": GUEST_SIGNAL + "\\" + tag,
            "LOOMWARD_HV_SCENARIO": scenario, "LOOMWARD_HV_REMOVAL": str(removal),
            "LOOMWARD_HV_TREE": self.config.tree,
            "LOOMWARD_COPY_CRASH_TRACE": GUEST_SIGNAL + "\\" + tag + "\\trace.tsv",
        }

    def _runner(self, step, cycle, values):
        signal = values["LOOMWARD_HV_SIGNAL"]
        host_dir = RESULTS + f"\\c{cycle:05d}"
        assignments = "\n".join(f"$env:{key} = {quote(value)}" for key, value in values.items())
        guest = f"""
{assignments}
$env:LOOMWARD_HV_STEP = {quote(step)}
# Never inherit boundary-pause/process-crash hooks from a guest account profile.
$env:LOOMWARD_COPY_PAUSE_AT = $null
$env:LOOMWARD_COPY_CRASH_AT = $null
New-Item -ItemType Directory -Path {quote(signal)} -Force | Out-Null
$completed = Test-Path -LiteralPath (Join-Path {quote(signal)} 'act-done.json')
$ErrorActionPreference = 'Continue'
$output = (& {quote(self.config.exe)} {TEST_ARGS} 2>&1 | Out-String)
$code = $LASTEXITCODE
$ErrorActionPreference = 'Stop'
@{{ exit_code = $code; runner_output = $output; act_done = $completed }}
"""
        artifacts = ("prepare.json",) if step == "prepare" else ("result.json", "prepare.json", "trace.tsv")
        copies = "\n".join(
            f"Copy-Item -FromSession $session -LiteralPath {quote(signal + chr(92) + name)} "
            "-Destination $destination -ErrorAction Stop" for name in artifacts)
        final = "$answer | ConvertTo-Json -Depth 10 -Compress"
        if step == "recover":
            final = f"""
$result = Get-Content -LiteralPath (Join-Path $destination 'result.json') -Raw | ConvertFrom-Json
$trace = @(Get-Content -LiteralPath (Join-Path $destination 'trace.tsv'))
$last = if ($trace.Count) {{ ($trace[-1] -split "`t", 2)[-1] }} else {{ $null }}
$free = Invoke-Command -Session $session -ScriptBlock {{ (Get-PSDrive L).Free }}
@{{ result = $result; last_trace_label = $last; free_bytes = $free;
    exit_code = $answer.exit_code; runner_output = $answer.runner_output; act_done = $answer.act_done }} |
    ConvertTo-Json -Depth 10 -Compress
"""
        body = f"""
$destination = Resolve-LabPath {quote(host_dir)}
""" + ("if (Test-Path -LiteralPath $destination) { throw 'cycle_directory_exists' }\n"
        "New-Item -ItemType Directory -Path $destination | Out-Null\n" if step == "prepare" else "") + self._session() + f"""
try {{
    $answer = Invoke-Command -Session $session -ScriptBlock {{
        $ErrorActionPreference = 'Stop'
        {guest}
    }}
    {copies}
    {final}
}} finally {{ Remove-PSSession $session }}
"""
        default = {"exit_code": 0, "runner_output": ""}
        if step == "recover":
            default.update({"result": {"ok": True, "copy_ran": True, "terminal_before_recover": False, "last_record": "item_written",
                "recovered": "dry run", "abandoned": None, "failure": None},
                "last_trace_label": "item_written", "free_bytes": 4 * GIB})
        return self._call(step, body, default, timeout=600, guard=True)

    def _act(self, values):
        """Prepare the act launch; it runs inside the cut's persistent session. A process started through
        PowerShell Direct ends with its session, so launching it in a separate, closed session would kill
        the act before the hard-off and turn the experiment into a process kill."""
        signal = values["LOOMWARD_HV_SIGNAL"]
        wrapper = signal + r"\act.cmd"
        commands = ['@echo off', 'set "LOOMWARD_HV_STEP=act"',
                    'set "LOOMWARD_COPY_PAUSE_AT="', 'set "LOOMWARD_COPY_CRASH_AT="']
        commands += [f'set "{key}={value}"' for key, value in values.items()]
        commands += [f'"{self.config.exe}" {TEST_ARGS} > "{signal}\\act.log" 2>&1']
        cmd = "\r\n".join(commands) + "\r\n"
        self._launch = "# Act wrapper (no secrets):\n" + "\n".join("# " + line for line in commands) + f"""
$started = Invoke-Command -Session $session -ScriptBlock {{
    $ErrorActionPreference = 'Stop'
    [IO.File]::WriteAllText({quote(wrapper)}, {encoded(cmd)}, [Text.Encoding]::ASCII)
    Start-Process -FilePath 'cmd.exe' -ArgumentList {quote('/d /c ""' + wrapper + '""')} -WindowStyle Hidden
    $true
}}
if ($started -ne $true) {{ throw 'act_start_failed' }}
"""

    def _cut(self, values, delay):
        trace = values["LOOMWARD_COPY_CRASH_TRACE"]
        signal = values["LOOMWARD_HV_SIGNAL"]
        act_log, act_done = signal + r"\act.log", signal + r"\act-done.json"
        body = self._session() + rf"""
try {{
{self._launch}
$deadline = (Get-Date).AddSeconds({ACT_TIMEOUT})
while ($true) {{
$poll = Invoke-Command -Session $session -ScriptBlock {{
    $ErrorActionPreference = 'Stop'
    $lines = if (Test-Path -LiteralPath {quote(trace)}) {{ @(Get-Content -LiteralPath {quote(trace)}) }} else {{ @() }}
    $dirty = (Get-Counter '\Cache\Dirty Pages' -ErrorAction Stop).CounterSamples.CookedValue
    $output = if (Test-Path -LiteralPath {quote(act_log)}) {{
        Get-Content -LiteralPath {quote(act_log)} -Raw
    }} else {{ '' }}
    @{{ trace_lines = @($lines).Count; dirty_pages = $dirty;
        act_done = (Test-Path -LiteralPath {quote(act_done)});
        elevated_token = [bool]($output -match 'elevated[-_ ]token|token[^\r\n]*elevated|running as (an? )?admin') }}
}}
$lastPollReturn = Get-Date
if ($poll.elevated_token) {{ throw 'elevated_token' }}
if ($null -eq $poll.dirty_pages -or $poll.dirty_pages -lt 0 -or
    [double]::IsNaN($poll.dirty_pages) -or [double]::IsInfinity($poll.dirty_pages)) {{
    throw 'dirty_pages_unavailable'
}}
if ($poll.trace_lines -gt 2 -or $poll.act_done) {{ break }}
if ((Get-Date) -ge $deadline) {{ throw 'act_progress_timeout' }}
Start-Sleep -Milliseconds {int(POLL_SECONDS * 1000)}
}}
$delay = {delay}
Start-Sleep -Milliseconds $delay
$stopStarted = Get-Date
Stop-VM -Name {quote(VM_NAME)} -TurnOff -Force
$stopReturned = Get-Date
@{{ trace_lines = $poll.trace_lines; dirty_pages = $poll.dirty_pages; act_done = $poll.act_done;
    delay_ms = $delay; cut_latency_ms = ($stopReturned - $lastPollReturn).TotalMilliseconds;
    last_poll_return = $lastPollReturn.ToString('o'); stop_started = $stopStarted.ToString('o');
    stop_returned = $stopReturned.ToString('o') }} | ConvertTo-Json -Compress
}} finally {{ Remove-PSSession $session -ErrorAction SilentlyContinue }}
"""
        result = self._call("cut", body, {"trace_lines": 3, "dirty_pages": 1, "act_done": False,
            "cut_latency_ms": None, "last_poll_return": None, "stop_started": None, "stop_returned": None},
            timeout=120 + ACT_TIMEOUT + 32, guard=True)
        dirty = result.get("dirty_pages")
        if not isinstance(dirty, (int, float)) or isinstance(dirty, bool) or not math.isfinite(dirty) or dirty < 0:
            raise CampaignError("dirty_pages_unavailable")
        lines = result.get("trace_lines")
        if not isinstance(lines, int) or isinstance(lines, bool) or lines < 0 or not isinstance(result.get("act_done"), bool):
            raise CampaignError("invalid_progress_response")
        latency = result.get("cut_latency_ms")
        if self.config.execute and (not isinstance(latency, (int, float)) or isinstance(latency, bool)
                                   or not math.isfinite(latency) or latency < 0):
            raise CampaignError("cut_latency_unavailable")
        return result

    def _append(self, record):
        line = json.dumps(record, ensure_ascii=True, allow_nan=False, separators=(",", ":"))
        self._call("append", f"""
$path = Resolve-LabPath {quote(CAMPAIGN_LOG)}
New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
$line = {encoded(line)}
[IO.File]::AppendAllText($path, $line + [Environment]::NewLine, [Text.UTF8Encoding]::new($false))
'{{}}'
""", guard=True)

    def cycle(self, number):
        start = self.clock()
        scenario = ("run" if number % 2 else "abandon") if self.config.scenario == "mixed" else self.config.scenario
        removal = (0 if number % 2 else 1) if self.config.removal == "mixed" else int(self.config.removal)
        if scenario == "abandon":
            removal = 1
        record = {"cycle": number, "command": self.config.command, "exe": self.config.exe,
            "kind": "negative_control" if self.config.command == "negative" else "floor", "scenario": scenario, "removal": removal,
            "tree": self.config.tree, "seed": self.config.seed, "delay_ms": None, "trace_lines": None,
            "cut_latency_ms": None, "last_poll_return": None, "stop_started": None, "stop_returned": None,
            "last_trace_label": None, "dirty_pages": None, "recovered": None, "abandoned": None,
            "last_record": None, "copy_ran": False, "ok": False, "failure": None,
            "chkdsk": None, "filesystem": None, "write_cache": {}, "floor_credit": None, "timings": {}}
        validated = False
        try:
            if self.preflight()["vm"] is None:
                raise CampaignError("vm_absent")
            validated = True
            self._boot()
            record["timings"]["initial_boot_s"] = self.clock() - start
            record["write_cache"]["before"] = self._admin()
            if record["write_cache"]["before"].get("enabled") is not True:
                raise CampaignError("write_cache_not_enabled")
            values = self._env(number, scenario, removal)
            prepared = self._runner("prepare", number, values)
            if prepared.get("exit_code") != 0:
                raise CampaignError("prepare_failed")
            self._act(values)
            delay = self.rng.randint(0, self.config.max_delay_ms)
            record["delay_ms"] = delay
            final_poll = self._cut(values, delay)
            for key in ("trace_lines", "dirty_pages", "cut_latency_ms", "last_poll_return", "stop_started", "stop_returned"):
                record[key] = final_poll.get(key)
            if final_poll["act_done"]:
                record["kind"] = "post_completion"
            record["timings"]["cut_s"] = self.clock() - start
            reboot = self.clock()
            self._boot()
            record["timings"]["recovery_boot_s"] = self.clock() - reboot
            health = self._admin(health=True)
            record["chkdsk"] = health.get("chkdsk")
            record["filesystem"] = health.get("filesystem")
            record["write_cache"]["after"] = health.get("write_cache", {})
            filesystem = record["filesystem"]
            if (not isinstance(filesystem, dict) or not isinstance(filesystem.get("dirty"), bool)
                    or not isinstance(filesystem.get("repair_events"), list)):
                raise CampaignError("filesystem_evidence_unavailable")
            if filesystem["dirty"] or filesystem["repair_events"]:
                raise CampaignError("filesystem_repaired")
            check = record["chkdsk"]
            if not check or check.get("code") != 0 or check.get("clean") is not True:
                raise CampaignError("chkdsk_dirty_or_failed")
            if record["write_cache"]["after"].get("enabled") is not True:
                raise CampaignError("write_cache_not_enabled")
            recovered = self._runner("recover", number, values)
            # A durable marker or pre-recovery terminal record catches completion
            # during the delay or between the last Direct poll and TurnOff.
            if recovered.get("act_done") is True:
                record["kind"] = "post_completion"
            result = recovered.get("result")
            if not isinstance(result, dict) or not isinstance(result.get("ok"), bool):
                raise CampaignError("invalid_guest_result")
            # The guest reports the replayed state before recovery; a missing field is never guessed.
            terminal = result.get("terminal_before_recover")
            if not isinstance(terminal, bool):
                raise CampaignError("invalid_guest_result")
            record["terminal_before_recover"] = terminal
            if terminal:
                record["kind"] = "post_completion"
            for key in ("ok", "recovered", "abandoned", "last_record", "copy_ran", "failure", "journal_records"):
                record[key] = result.get(key)
            record["last_trace_label"] = recovered.get("last_trace_label")
            if record["copy_ran"] is not True:
                raise CampaignError("copy_did_not_run")
            if record["ok"] is not True and self.config.command != "negative":
                raise CampaignError("oracle_failed")
            if recovered.get("exit_code") != 0 and record["ok"] is True:
                raise CampaignError("recover_failed")
            # A negative-control cycle runs a mutant: never qualification evidence, whatever its outcome.
            if (record["ok"] and record["kind"] == "floor" and record["dirty_pages"] > 0
                    and self.config.command != "negative"):
                record["floor_credit"] = window_for_label(record["last_record"])
            free = recovered.get("free_bytes")
            if not isinstance(free, (int, float)) or not math.isfinite(free) or free < 0:
                raise CampaignError("lab_free_space_unavailable")
            record["timings"]["total_s"] = self.clock() - start
        except CampaignError as exc:
            record["ok"] = False
            record["floor_credit"] = None
            record["failure"] = record["failure"] or str(exc)
            record["timings"]["total_s"] = self.clock() - start
            if validated:
                try:
                    self._append(record)
                except Exception:
                    print("campaign_log_failed: original failure preserved", file=sys.stderr)
                self.summary["cycles"] += 1
                self.summary["failures"] += 1
            raise
        # Do not retry an append that may already have reached disk.
        self._append(record)
        if self.config.execute:
            self.summary["cycles"] += 1
            self.summary["failures"] += int(not record["ok"])
            self.summary["caught"] += int(not record["ok"] and self.config.command == "negative")
            if record["floor_credit"]:
                self.summary["floor_credits"][record["floor_credit"]] += 1
        if number % 50 == 0 or free < 2 * GIB or not self.config.execute:
            if not self.config.execute:
                print("# CONDITIONAL: reset only at a 50-cycle boundary or when L: has under 2 GB free.")
            self._reset()
        return record

    def _reset(self):
        # Unique virtual capacity identifies each attachment without guessing names.
        self.preflight()
        self._call("reset", rf"""
Stop-VM -Name {quote(VM_NAME)} -ErrorAction Stop
$deadline = [DateTime]::UtcNow.AddSeconds(300)
while ((Get-VM -Name {quote(VM_NAME)}).State -ne 'Off') {{
    if ([DateTime]::UtcNow -ge $deadline) {{ throw 'clean_shutdown_timeout' }}
    Start-Sleep -Milliseconds 250
}}
$attached = @(Get-VMHardDiskDrive -VMName {quote(VM_NAME)} | ForEach-Object {{
    Get-VHD -Path (Resolve-LabPath $_.Path)
}})
$manifestPath = Resolve-LabPath {quote(STATE + chr(92) + 'pristine.json')}
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$pairs = @()
foreach ($spec in @(@{{ source = {quote(PRISTINE_LAB)}; size = {LAB_DISK_BYTES} }},
                    @{{ source = {quote(PRISTINE_SIGNAL)}; size = {SIGNAL_DISK_BYTES} }})) {{
    $source = Resolve-LabPath $spec.source
    $image = Get-VHD -Path $source
    if ($image.Size -ne $spec.size) {{ throw 'pristine_disk_size_unexpected' }}
    $candidates = @($attached | Where-Object {{ $_.Size -eq $image.Size }})
    if ($candidates.Count -ne 1) {{ throw 'reset_disk_identity_ambiguous' }}
    $target = Resolve-LabPath $candidates[0].Path
    if ($source -eq $target) {{ throw 'reset_source_is_attached' }}
    $entries = @($manifest.files | Where-Object {{ $_.name -eq (Split-Path -Leaf $source) }})
    if ($entries.Count -ne 1 -or $entries[0].sha256 -notmatch '^[0-9a-fA-F]{{64}}$') {{
        throw 'pristine_manifest_invalid'
    }}
    $pairs += @{{ source = $source; target = $target; expected = $entries[0] }}
}}
if ($pairs[0].target -eq $pairs[1].target) {{ throw 'reset_disk_identity_ambiguous' }}
foreach ($pair in $pairs) {{
    $temporary = Resolve-LabPath ($pair.target + '.tmp')
    Copy-Item -LiteralPath $pair.source -Destination $temporary -Force -ErrorAction Stop
    if ((Get-FileHash -LiteralPath $temporary -Algorithm SHA256).Hash -ne $pair.expected.sha256 -or
        (Get-Item -LiteralPath $temporary).Length -ne $pair.expected.size) {{
        throw 'reset_hash_mismatch'
    }}
    $pair.temporary = $temporary
}}
# Verify both copies before atomically replacing either live disk.
foreach ($pair in $pairs) {{
    [IO.File]::Replace($pair.temporary, $pair.target, $null)
}}
'{{}}'
""", timeout=900, guard=True)

    def run(self):
        if not self.config.execute:
            print("# DRY RUN: nominal path; heartbeat/progress polls repeat until ready or timeout.")
            print("# Conditional reset below also runs whenever guest L: has under 2 GB free.")
        if self.config.command == "preflight":
            self.preflight()
            return self.summary
        if self.preflight()["vm"] is None:
            raise CampaignError("vm_absent")
        next_cycle = self._call("next_cycle", f"""
$root = Resolve-LabPath {quote(RESULTS)}
$highest = 0
if (Test-Path -LiteralPath $root) {{
    foreach ($dir in @(Get-ChildItem -LiteralPath $root -Directory)) {{
        if ($dir.Name -match '^c([0-9]+)$') {{ $highest = [Math]::Max($highest, [int]$Matches[1]) }}
    }}
    $log = Join-Path $root 'campaign.jsonl'
    if (Test-Path -LiteralPath $log) {{
        Resolve-LabPath $log | Out-Null
        foreach ($line in [IO.File]::ReadLines($log)) {{
            $row = $line | ConvertFrom-Json
            $highest = [Math]::Max($highest, [int]$row.cycle)
        }}
    }}
}}
@{{ cycle = ($highest + 1) }} | ConvertTo-Json -Compress
""", {"cycle": 1}, guard=True)["cycle"]
        count = 1 if self.config.command == "cycle" else self.config.cycles
        if not self.config.execute:
            self.summary["planned_cycles"] = count
        failures_before = self.summary["failures"]
        try:
            for number in range(next_cycle, next_cycle + count):
                self.cycle(number)
        except CampaignError:
            if self.summary["failures"] == failures_before:
                self.summary["failures"] += 1
            print(json.dumps(self.summary, sort_keys=True))
            raise
        print(json.dumps(self.summary, sort_keys=True))
        return self.summary


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("preflight", "cycle", "campaign", "negative"))
    parser.add_argument("--cycles", type=int, default=1)
    parser.add_argument("--scenario", choices=("run", "abandon", "mixed"), default="run")
    parser.add_argument("--removal", choices=("0", "1", "mixed"), default="0")
    parser.add_argument("--tree", choices=("small", "large"), default="small")
    parser.add_argument("--seed", type=int, default=None)
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--connect-timeout", type=float, default=120.0,
                        help="seconds to retry PowerShell Direct after the heartbeat is OK")
    parser.add_argument("--boot-timeout", type=float, default=BOOT_TIMEOUT,
                        help="seconds to wait for the guest heartbeat; raise it when the host disk is contended")
    parser.add_argument("--max-delay-ms", type=int, default=2000,
                        help="cut delay is uniform in [0, this] after the first journal record; size it to the act")
    parser.add_argument("--exe", default=None, help="mutant guest test executable (negative only)")
    args = parser.parse_args(argv)
    if args.command == "negative" and args.exe is None:
        parser.error("negative requires --exe")
    if args.command != "negative" and args.exe is not None:
        parser.error("--exe is only valid for negative")
    args.exe = args.exe or GUEST_EXE
    if args.seed is None:
        args.seed = secrets.randbits(32)
    try:
        summary = Campaign(PowerShell(), random.Random(args.seed), time.monotonic, time.sleep, Config(**vars(args))).run()
    except CampaignError as exc:
        print(f"campaign_refused: {exc}", file=sys.stderr)
        return 1
    if args.command == "negative" and args.execute and summary["caught"] == 0:
        print("NOT CAUGHT")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
