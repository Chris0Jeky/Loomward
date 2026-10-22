# Run the native Windows gates as a real standard user on a disposable hosted runner.
# No UAC, production capability, fixture guard, or machine policy is changed.
[CmdletBinding()]
param([switch]$Child, [string]$Configuration)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'This qualification requires Windows.' }

if ($Child) {
    $config = Get-Content -LiteralPath $Configuration -Raw | ConvertFrom-Json
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if ($identity.User.Value -ne $config.sid -or
        $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'The qualification child is not the expected standard user.'
    }
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class QualificationToken {
    [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("advapi32.dll", SetLastError=true)]
    static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError=true)]
    static extern bool GetTokenInformation(IntPtr token, int kind, out uint value,
        uint length, out uint returned);
    public static bool IsElevated() {
        IntPtr token;
        if (!OpenProcessToken(GetCurrentProcess(), 8, out token))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        try {
            uint value, returned;
            if (!GetTokenInformation(token, 20, out value, 4, out returned))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            if (returned != 4) throw new InvalidOperationException("Invalid elevation result");
            return value != 0;
        } finally { CloseHandle(token); }
    }
}
'@
    if ([QualificationToken]::IsElevated()) { throw 'The qualification child is elevated.' }
    # Read-only Cloud Files probes diagnose whether the existing filter supports each volume.
    # They never classify an API failure as "not synced" or change the actual fixture location.
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class QualificationCloudProbe {
    [StructLayout(LayoutKind.Sequential)]
    struct PlatformInfo { public uint Build, Revision, Integration; }
    [DllImport("cldapi.dll")]
    static extern int CfGetPlatformInfo(out PlatformInfo info);
    [DllImport("cldapi.dll")]
    static extern int CfGetSyncRootInfoByHandle(IntPtr file, int kind, out long info,
        uint length, out uint returned);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern IntPtr CreateFileW(string path, uint access, uint share, IntPtr security,
        uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    public static string Platform() {
        PlatformInfo info;
        int result = CfGetPlatformInfo(out info);
        return String.Format("CfGetPlatformInfo HRESULT=0x{0:x8}; build={1}; revision={2}; integration={3}",
            result, info.Build, info.Revision, info.Integration);
    }
    public static string SyncRoot(string path) {
        // FILE_READ_ATTRIBUTES, share read/write/delete, OPEN_EXISTING, backup/reparse semantics.
        IntPtr file = CreateFileW(path, 0x80, 7, IntPtr.Zero, 3, 0x02200000, IntPtr.Zero);
        if (file == new IntPtr(-1))
            return "CreateFileW error=" + Marshal.GetLastWin32Error();
        try {
            long info;
            uint returned;
            int result = CfGetSyncRootInfoByHandle(file, 0, out info, 8, out returned);
            return String.Format("CfGetSyncRootInfoByHandle HRESULT=0x{0:x8}; returned={1}",
                result, returned);
        } finally { CloseHandle(file); }
    }
}
'@
    $childProfile = [Environment]::GetFolderPath([Environment+SpecialFolder]::UserProfile)
    if (-not $childProfile -or -not (Test-Path -LiteralPath $childProfile -PathType Container)) {
        throw 'The standard-user profile was not loaded.'
    }
    # Start-Process -UseNewEnvironment supplies Machine scope only. Set the real child's
    # profile fields and task-owned writable paths, never the runner administrator's home.
    $env:USERPROFILE = $childProfile
    $identityParts = $identity.Name -split '\\', 2
    if ($identityParts.Count -ne 2) { throw 'The Windows identity has no account domain.' }
    $env:USERDOMAIN = $identityParts[0]
    $env:USERNAME = $identityParts[1]
    $env:APPDATA = [Environment]::GetFolderPath([Environment+SpecialFolder]::ApplicationData)
    $env:LOCALAPPDATA = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
    $env:TEMP = $config.temp
    $env:TMP = $config.temp
    $env:CARGO_HOME = $config.cargoHome
    $env:CARGO_TARGET_DIR = $config.target
    $env:RUSTC = $config.rustc
    $env:PATH = (Split-Path $config.cargo) + ';' + $env:PATH
    # Source remains owned by checkout's account. This exception is confined to this process
    # tree and this one known checkout; no global Git configuration is written.
    $env:GIT_CONFIG_COUNT = '1'
    $env:GIT_CONFIG_KEY_0 = 'safe.directory'
    $env:GIT_CONFIG_VALUE_0 = $config.workspace
    $drive = [IO.DriveInfo]::new([IO.Path]::GetPathRoot($config.temp))
    if ($drive.DriveFormat -ne 'NTFS') { throw 'The synthetic fixture volume is not NTFS.' }
    Set-Location -LiteralPath $config.workspace
    Write-Host "Qualification identity: $($identity.Name); SID: $($identity.User.Value)"
    Write-Host 'Administrator membership: false; TokenElevation: 0; fixture filesystem: NTFS'
    Write-Host "Windows: $([Environment]::OSVersion.VersionString)"
    Write-Host '::group::Read-only Cloud Files prerequisite diagnostics'
    Write-Host ([QualificationCloudProbe]::Platform())
    Write-Host "Fixture temp $($config.temp): $([QualificationCloudProbe]::SyncRoot($config.temp))"
    $profileProbe = Join-Path $childProfile ('LoomwardCloudProbe-' + [Guid]::NewGuid().ToString('N'))
    $createdProfileProbe = $false
    try {
        New-Item -ItemType Directory -Path $profileProbe | Out-Null
        $createdProfileProbe = $true
        Write-Host "Profile temp ${profileProbe}: $([QualificationCloudProbe]::SyncRoot($profileProbe))"
    } finally {
        if ($createdProfileProbe) { Remove-Item -LiteralPath $profileProbe }
    }
    Write-Host '::endgroup::'
    & $config.rustc --version
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    & $config.cargo --version
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    function Invoke-QualificationCargo {
        param([string[]]$CargoArguments, [int]$MinimumPassed = 0, [switch]$NoSetupSkip)
        Write-Host "::group::stable cargo $($CargoArguments -join ' ')"
        & $config.cargo @CargoArguments 2>&1 | Tee-Object -Variable cargoOutput
        $cargoExit = $LASTEXITCODE
        Write-Host '::endgroup::'
        if ($cargoExit -ne 0) { exit $cargoExit }
        $outputText = $cargoOutput -join "`n"
        if ($NoSetupSkip -and $outputText -match '(?i)\bskipped:') {
            throw 'A required native fixture refused setup.'
        }
        if ($MinimumPassed -gt 0) {
            $results = [regex]::Matches($outputText,
                'test result: ok\. (\d+) passed; 0 failed; 0 ignored;')
            $qualifying = @($results | Where-Object { [int]$_.Groups[1].Value -ge $MinimumPassed })
            if ($qualifying.Count -eq 0) { throw 'The required focused tests did not execute.' }
        }
    }

    # The resolved binaries are the stable toolchain installed by the workflow, copied below.
    # Preserve the existing tests and add exact native witnesses; do not turn setup refusal green.
    Invoke-QualificationCargo -CargoArguments @('test', '--locked', '-p', 'loomward-windows', '--all-features', 'pending_fence_tests') -MinimumPassed 18
    Invoke-QualificationCargo -CargoArguments @('test', '--locked', '-p', 'loomward-service', 'copy::pending_fence_tests') -MinimumPassed 5
    foreach ($test in @(
        'another_store_committing_a_pending_rename_keeps_the_annotation',
        'a_pending_fence_during_rename_commits_annotated',
        'a_process_dying_with_a_pending_fence_reopens_without_effects'
    )) {
        Invoke-QualificationCargo -CargoArguments @('test', '--locked', '-p', 'loomward-windows', '--all-features', "effects::exec::tests::$test", '--', '--exact', '--nocapture', '--test-threads=1') -MinimumPassed 1 -NoSetupSkip
    }
    Invoke-QualificationCargo -CargoArguments @('test', '--locked', '-p', 'loomward-service', '--lib', 'tests::volumes_list_observes_space_through_a_second_root_when_the_first_is_unobservable', '--', '--exact', '--nocapture') -MinimumPassed 1 -NoSetupSkip
    Invoke-QualificationCargo -CargoArguments @('test', '--locked', '-p', 'loomward-engine', 'publication_tests') -MinimumPassed 2
    Invoke-QualificationCargo -CargoArguments @('test', '--workspace')
    Invoke-QualificationCargo -CargoArguments @('test', '--locked', '-p', 'loomward-windows', '--all-features')
    exit 0
}

if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows' -or
    $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or
    $env:GITHUB_REPOSITORY -ne 'Chris0Jeky/Loomward') {
    throw 'Account setup is restricted to this repository on disposable GitHub-hosted Windows.'
}
$workspace = (Resolve-Path -LiteralPath $env:GITHUB_WORKSPACE).Path
$runnerTemp = (Resolve-Path -LiteralPath $env:RUNNER_TEMP).Path
if ($workspace.StartsWith('\\') -or $runnerTemp.StartsWith('\\')) {
    throw 'Qualification paths must be local to the hosted VM.'
}
$parentIdentity = [Security.Principal.WindowsIdentity]::GetCurrent()
$parentPrincipal = [Security.Principal.WindowsPrincipal]::new($parentIdentity)
if (-not $parentPrincipal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'The hosted bootstrap must already have account-management rights; it never elevates.'
}
Write-Host '::group::Installed Cloud Files filter diagnostics (read only)'
$cloudDriver = Get-CimInstance Win32_SystemDriver -Filter "Name='CldFlt'" -OperationTimeoutSec 10
if ($null -eq $cloudDriver) { Write-Host 'CldFlt system-driver registration: absent' }
else { $cloudDriver | Select-Object Name, State, StartMode, PathName | Format-List | Out-Host }
& fltmc.exe instances -f CldFlt
Write-Host "fltmc instances exit code: $LASTEXITCODE"
Write-Host '::endgroup::'
$rustc = (& rustup which --toolchain stable rustc).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve the installed stable compiler.' }
$toolchain = Split-Path (Split-Path $rustc)
$accountName = 'lwci' + [Guid]::NewGuid().ToString('N').Substring(0, 12)
$work = Join-Path $runnerTemp $accountName
$target = Join-Path $workspace 'target'
$createdUser = $null
$createdWork = $false
$process = $null
$grantedPaths = [Collections.Generic.List[string]]::new()
$childExit = 1
$cleanupFailed = $false

function Set-QualificationGrant {
    param([string]$Path, [string]$Rights)
    & icacls.exe $Path /grant "*$($createdUser.SID.Value):$Rights" /Q | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Cannot grant the qualification account access to $Path" }
    $grantedPaths.Add($Path)
}

try {
    New-Item -ItemType Directory -Path $work | Out-Null
    $createdWork = $true
    foreach ($name in @('run', 'run/temp', 'run/cargo', 'toolchain')) {
        New-Item -ItemType Directory -Path (Join-Path $work $name) | Out-Null
    }
    New-Item -ItemType Directory -Force -Path $target | Out-Null
    # Copy public build inputs. Never grant access to the parent account's profile, rustup
    # configuration, Cargo credentials, Git credentials, or other cached application data.
    Copy-Item -Path (Join-Path $toolchain '*') -Destination (Join-Path $work 'toolchain') -Recurse
    $cargoSource = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
    $registry = Join-Path $cargoSource 'registry'
    if (Test-Path -LiteralPath $registry -PathType Container) {
        Copy-Item -LiteralPath $registry -Destination (Join-Path $work 'run/cargo') -Recurse
    }
    $password = ConvertTo-SecureString ('Lw1!' + [Guid]::NewGuid().ToString('N')) -AsPlainText -Force
    $createdUser = New-LocalUser -Name $accountName -Password $password -UserMayNotChangePassword -AccountExpires (Get-Date).AddHours(2) -Description 'Disposable Loomward native CI fixture'
    Add-LocalGroupMember -SID 'S-1-5-32-545' -Member $createdUser
    if (@(Get-LocalGroupMember -SID 'S-1-5-32-544' | Where-Object { $_.SID -eq $createdUser.SID }).Count -ne 0) {
        throw 'The qualification account unexpectedly belongs to Administrators.'
    }
    Set-QualificationGrant -Path $workspace -Rights '(OI)(CI)RX'
    Set-QualificationGrant -Path $target -Rights '(OI)(CI)M'
    Set-QualificationGrant -Path $work -Rights '(OI)(CI)RX'
    Set-QualificationGrant -Path (Join-Path $work 'run') -Rights '(OI)(CI)M'
    $childScript = Join-Path $work 'qualification.ps1'
    Copy-Item -LiteralPath $PSCommandPath -Destination $childScript
    $configurationPath = Join-Path $work 'configuration.json'
    @{
        sid = $createdUser.SID.Value
        workspace = $workspace
        target = $target
        temp = Join-Path $work 'run/temp'
        cargoHome = Join-Path $work 'run/cargo'
        rustc = Join-Path $work 'toolchain/bin/rustc.exe'
        cargo = Join-Path $work 'toolchain/bin/cargo.exe'
    } | ConvertTo-Json | Set-Content -LiteralPath $configurationPath -Encoding utf8
    $stdout = Join-Path $work 'stdout.log'
    $stderr = Join-Path $work 'stderr.log'
    $credential = [PSCredential]::new("$env:COMPUTERNAME\$accountName", $password)
    $process = Start-Process -FilePath (Join-Path $PSHOME 'pwsh.exe') -Credential $credential -LoadUserProfile -UseNewEnvironment -NoNewWindow -PassThru -WorkingDirectory $work -ArgumentList "-NoProfile -NonInteractive -File `"$childScript`" -Child -Configuration `"$configurationPath`"" -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $deadline = [DateTime]::UtcNow.AddMinutes(35)
    while (-not $process.WaitForExit(1000)) {
        if ([DateTime]::UtcNow -ge $deadline) {
            $process.Kill($true)
            if (-not $process.WaitForExit(10000)) { throw 'The timed-out process tree did not stop.' }
            throw 'Standard-user native qualification exceeded its 35-minute bound.'
        }
    }
    $childExit = $process.ExitCode
    if ($childExit -isnot [int]) { throw 'The qualification child did not provide an integer exit code.' }
    Write-Host "Standard-user qualification child exit code: $childExit"
} finally {
    if ($null -ne $process -and -not $process.HasExited) {
        try {
            $process.Kill($true)
            if (-not $process.WaitForExit(10000)) { throw 'The owned process tree did not stop.' }
        } catch { Write-Warning "Qualification process cleanup failed: $_"; $cleanupFailed = $true }
    }
    if ($null -ne $process) { $process.Dispose() }
    if ($createdWork) {
        foreach ($name in @('stdout.log', 'stderr.log')) {
            $log = Join-Path $work $name
            if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log }
        }
    }
    if ($null -ne $createdUser) {
        # Remove only this generated SID's profile/account and this invocation's access rules.
        try {
            # Process exit does not prove that Windows has finished releasing the profile.
            $profileCleanupDeadline = [DateTime]::UtcNow.AddSeconds(30)
            $lastProfileRemovalError = $null
            while ($true) {
                $secondsLeft = [int][Math]::Ceiling(($profileCleanupDeadline - [DateTime]::UtcNow).TotalSeconds)
                if ($secondsLeft -le 0) {
                    if ($null -ne $lastProfileRemovalError) { throw $lastProfileRemovalError }
                    throw 'The temporary profile did not become removable within 30 seconds.'
                }
                $temporaryProfile = Get-CimInstance Win32_UserProfile -Filter "SID='$($createdUser.SID.Value)'" -OperationTimeoutSec $secondsLeft
                if ($null -eq $temporaryProfile) { break }
                if (-not $temporaryProfile.Loaded) {
                    $secondsLeft = [int][Math]::Ceiling(($profileCleanupDeadline - [DateTime]::UtcNow).TotalSeconds)
                    if ($secondsLeft -le 0) { continue }
                    try {
                        $temporaryProfile | Remove-CimInstance -OperationTimeoutSec $secondsLeft
                        break
                    } catch { $lastProfileRemovalError = $_ }
                }
                Start-Sleep -Milliseconds 500
            }
        } catch { Write-Warning "Temporary profile cleanup failed: $_"; $cleanupFailed = $true }
        try { Remove-LocalUser -SID $createdUser.SID }
        catch { Write-Warning "Temporary account cleanup failed: $_"; $cleanupFailed = $true }
        foreach ($path in $grantedPaths) {
            & icacls.exe $path /remove:g "*$($createdUser.SID.Value)" /Q | Out-Null
            if ($LASTEXITCODE -ne 0) { $cleanupFailed = $true }
        }
    }
    if ($createdWork -and (Test-Path -LiteralPath $work)) {
        try { Remove-Item -LiteralPath $work -Recurse -Force }
        catch { Write-Warning "Temporary fixture cleanup failed: $_"; $cleanupFailed = $true }
    }
}
if ($cleanupFailed) { throw 'Qualification cleanup did not complete.' }
exit $childExit
