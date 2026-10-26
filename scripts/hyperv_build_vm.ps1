<#
.SYNOPSIS
  Builds the LoomwardCrashLab VM for the G5 Hyper-V crash campaign (docs/53, lane H4).

.DESCRIPTION
  Non-elevated: needs only membership of "Hyper-V Administrators" (owner, HUMAN_TODO q-49). VM data lives under
  E:\loomward-lab\hyperv (the allowlist prefix). No VM file lives on C:, but Hyper-V keeps its own
  registration link under C:\ProgramData\Microsoft\Windows\Hyper-V. Lab data only; nothing touches H:.

  Steps (each idempotent where practical):
    unattend  write state\guest.json (random passwords, never printed) and iso\unattend.iso (IMAPI2, built in)
    create    Generation 2 VM, 2 vCPU, 4 GB static, no network adapter, checkpoints disabled, every path on E:
    install   boot the evaluation ISO (keystroke through Msvm_Keyboard for "press any key"), wait for the guest
    disks     attach and format the lab (L:, 8 GB) and signal (S:, 1 GB) disks; cap the lab disk's IOPS
    pristine  shut down cleanly and keep copies of the lab and signal disks for resets

  Usage: powershell -NoProfile -File scripts\hyperv_build_vm.ps1 -Step unattend|create|install|disks|pristine
#>
param(
    [Parameter(Mandatory)][ValidateSet('unattend', 'create', 'install', 'disks', 'pristine')][string]$Step,
    [string]$Iso = 'E:\loomward-lab\hyperv\iso\server2025-eval.iso',
    [int]$LabIops = 400,
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
$Root = 'E:\loomward-lab\hyperv'
$Iso = [IO.Path]::GetFullPath($Iso)
if (-not $Iso.StartsWith($Root + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw 'iso_outside_allowlist'
}
$isoAncestor = $Iso
while ($isoAncestor) {
    if (Test-Path -LiteralPath $isoAncestor) {
        if ((Get-Item -LiteralPath $isoAncestor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw 'iso_reparse_point'
        }
    }
    $isoAncestor = Split-Path -Parent $isoAncestor
}
$Name = 'LoomwardCrashLab'
$State = Join-Path $Root 'state'
$Disks = Join-Path $Root 'disks'
foreach ($d in @($State, $Disks, (Join-Path $Root 'vm'), (Join-Path $Root 'iso'), (Join-Path $Root 'results'))) {
    New-Item -ItemType Directory -Force -Path $d | Out-Null
}
icacls $State /inheritance:r /grant:r "${env:USERNAME}:(OI)(CI)F" 'SYSTEM:(OI)(CI)F' | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'state_acl_failed' }

function New-Password {
    $bytes = New-Object byte[] 18
    [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
    'Lw!' + ([Convert]::ToBase64String($bytes) -replace '[+/=]', 'x') + '9a'
}

function Get-GuestCredential([string]$who) {
    $g = Get-Content (Join-Path $State 'guest.json') -Raw | ConvertFrom-Json
    $user = if ($who -eq 'admin') { 'lwadmin' } else { 'lwrunner' }
    $pw = if ($who -eq 'admin') { $g.admin } else { $g.runner }
    New-Object PSCredential($user, (ConvertTo-SecureString $pw -AsPlainText -Force))
}

function Wait-Heartbeat([int]$seconds) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        $hb = (Get-VMIntegrationService -VMName $Name -Name Heartbeat).PrimaryStatusDescription
        if ($hb -eq 'OK') { return }
        Start-Sleep -Seconds 5
    }
    throw "guest heartbeat not OK after $seconds s"
}

switch ($Step) {
    'unattend' {
        $stage = Join-Path $State 'unattend-stage'
        try {
        $guest = Join-Path $State 'guest.json'
        if (-not (Test-Path $guest)) {
            @{ admin = (New-Password); runner = (New-Password) } | ConvertTo-Json | Set-Content -Encoding utf8 $guest
        }
        icacls $guest /inheritance:r /grant:r "${env:USERNAME}:(OI)(CI)F" 'SYSTEM:(OI)(CI)F' | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'guest_acl_failed' }
        $g = Get-Content $guest -Raw | ConvertFrom-Json
        $xml = @"
<?xml version="1.0" encoding="utf-8"?>
<unattend xmlns="urn:schemas-microsoft-com:unattend" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
  <settings pass="windowsPE">
    <component name="Microsoft-Windows-International-Core-WinPE" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <SetupUILanguage><UILanguage>en-US</UILanguage></SetupUILanguage>
      <InputLocale>en-US</InputLocale><SystemLocale>en-US</SystemLocale><UILanguage>en-US</UILanguage><UserLocale>en-US</UserLocale>
    </component>
    <component name="Microsoft-Windows-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <DiskConfiguration>
        <Disk wcm:action="add">
          <DiskID>0</DiskID><WillWipeDisk>true</WillWipeDisk>
          <CreatePartitions>
            <CreatePartition wcm:action="add"><Order>1</Order><Type>EFI</Type><Size>260</Size></CreatePartition>
            <CreatePartition wcm:action="add"><Order>2</Order><Type>MSR</Type><Size>16</Size></CreatePartition>
            <CreatePartition wcm:action="add"><Order>3</Order><Type>Primary</Type><Extend>true</Extend></CreatePartition>
          </CreatePartitions>
          <ModifyPartitions>
            <ModifyPartition wcm:action="add"><Order>1</Order><PartitionID>1</PartitionID><Format>FAT32</Format><Label>System</Label></ModifyPartition>
            <ModifyPartition wcm:action="add"><Order>2</Order><PartitionID>3</PartitionID><Format>NTFS</Format><Label>Windows</Label><Letter>C</Letter></ModifyPartition>
          </ModifyPartitions>
        </Disk>
      </DiskConfiguration>
      <ImageInstall><OSImage>
        <InstallFrom><MetaData wcm:action="add"><Key>/IMAGE/INDEX</Key><Value>1</Value></MetaData></InstallFrom>
        <InstallTo><DiskID>0</DiskID><PartitionID>3</PartitionID></InstallTo>
      </OSImage></ImageInstall>
      <UserData><AcceptEula>true</AcceptEula></UserData>
    </component>
  </settings>
  <settings pass="specialize">
    <component name="Microsoft-Windows-Shell-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <ComputerName>LWCRASHLAB</ComputerName>
    </component>
  </settings>
  <settings pass="oobeSystem">
    <component name="Microsoft-Windows-Shell-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <UserAccounts>
        <AdministratorPassword><Value>$($g.admin)</Value><PlainText>true</PlainText></AdministratorPassword>
        <LocalAccounts>
          <LocalAccount wcm:action="add"><Name>lwadmin</Name><Group>Administrators</Group><Password><Value>$($g.admin)</Value><PlainText>true</PlainText></Password></LocalAccount>
          <LocalAccount wcm:action="add"><Name>lwrunner</Name><Group>Users</Group><Password><Value>$($g.runner)</Value><PlainText>true</PlainText></Password></LocalAccount>
        </LocalAccounts>
      </UserAccounts>
      <OOBE><HideEULAPage>true</HideEULAPage><SkipMachineOOBE>true</SkipMachineOOBE></OOBE>
    </component>
  </settings>
</unattend>
"@
        New-Item -ItemType Directory -Force -Path $stage | Out-Null
        Set-Content -Path (Join-Path $stage 'autounattend.xml') -Value $xml -Encoding utf8
        # IMAPI2 builds the answer-file ISO without elevation or extra tools.
        $fsi = New-Object -ComObject IMAPI2FS.MsftFileSystemImage
        $fsi.FileSystemsToCreate = 3  # ISO9660 + Joliet
        $fsi.VolumeName = 'LWUNATTEND'
        $fsi.Root.AddTree($stage, $false)
        $image = $fsi.CreateResultImage()
        $out = Join-Path $Root 'iso\unattend.iso'
        if (-not ('Loomward.IsoWriter' -as [type])) {
            Add-Type -TypeDefinition @'
namespace Loomward {
public static class IsoWriter {
    public static void Write(string path, object comStream, int blockSize, int totalBlocks) {
        var input = (System.Runtime.InteropServices.ComTypes.IStream)comStream;
        var buffer = new byte[blockSize];
        var read = System.Runtime.InteropServices.Marshal.AllocHGlobal(4);
        try {
            using (var output = System.IO.File.Create(path)) {
                for (int i = 0; i < totalBlocks; i++) {
                    input.Read(buffer, blockSize, read);
                    output.Write(buffer, 0, System.Runtime.InteropServices.Marshal.ReadInt32(read));
                }
            }
        } finally { System.Runtime.InteropServices.Marshal.FreeHGlobal(read); }
    }
}
}
'@
        }
        [Loomward.IsoWriter]::Write($out, $image.ImageStream, $image.BlockSize, $image.TotalBlocks)
        "unattend.iso written ($((Get-Item $out).Length) bytes); passwords in state\guest.json (not printed)"
        } finally {
            if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
        }
    }
    'create' {
        if (Get-VM -Name $Name -ErrorAction SilentlyContinue) { throw "$Name already exists" }
        $vmPath = Join-Path $Root 'vm'
        New-VM -Name $Name -Generation 2 -MemoryStartupBytes 4GB -Path $vmPath `
            -NewVHDPath (Join-Path $Disks 'system.vhdx') -NewVHDSizeBytes 40GB | Out-Null
        Get-VMNetworkAdapter -VMName $Name | Remove-VMNetworkAdapter
        Set-VM -Name $Name -ProcessorCount 2 -StaticMemory -CheckpointType Disabled -AutomaticCheckpointsEnabled $false `
            -SnapshotFileLocation $vmPath -SmartPagingFilePath $vmPath -AutomaticStopAction TurnOff `
            -AutomaticStartAction Nothing
        Add-VMDvdDrive -VMName $Name -Path $Iso
        Add-VMDvdDrive -VMName $Name -Path (Join-Path $Root 'iso\unattend.iso')
        $boot = Get-VMDvdDrive -VMName $Name | Where-Object Path -eq $Iso
        Set-VMFirmware -VMName $Name -FirstBootDevice $boot
        Enable-VMIntegrationService -VMName $Name -Name 'Guest Service Interface'
        Get-VM $Name | Select-Object Name, Generation, ProcessorCount, MemoryStartup, CheckpointType, Path | Format-List
    }
    'install' {
        Start-VM -Name $Name
        $vm = Get-VM -Name $Name
        $kb = Get-CimInstance -Namespace root\virtualization\v2 -ClassName Msvm_Keyboard |
            Where-Object { $_.SystemName -eq $vm.Id.Guid.ToUpper() -or $_.SystemName -eq $vm.Id.Guid }
        # "Press any key to boot from CD or DVD": tap Enter for the first 20 seconds.
        for ($i = 0; $i -lt 40; $i++) {
            if ($kb) { Invoke-CimMethod -InputObject $kb -MethodName TypeKey -Arguments @{ keyCode = 13 } | Out-Null }
            Start-Sleep -Milliseconds 500
        }
        "installing; waiting for the guest heartbeat (up to 45 min)"
        Wait-Heartbeat 2700
        # Boot from disk from now on, and drop the installation media.
        Set-VMFirmware -VMName $Name -FirstBootDevice (Get-VMHardDiskDrive -VMName $Name | Select-Object -First 1)
        $cred = Get-GuestCredential admin
        $deadline = (Get-Date).AddMinutes(15)
        while ($true) {
            try { Invoke-Command -VMName $Name -Credential $cred -ScriptBlock { $env:COMPUTERNAME } -ErrorAction Stop; break }
            catch { if ((Get-Date) -gt $deadline) { throw }; Start-Sleep 10 }
        }
        Get-VMDvdDrive -VMName $Name | Remove-VMDvdDrive
        Remove-Item -LiteralPath (Join-Path $Root 'iso\unattend.iso') -Force
        "installed"
    }
    'disks' {
        $lab = Join-Path $Disks 'lab.vhdx'; $sig = Join-Path $Disks 'signal.vhdx'
        if (-not (Test-Path $lab)) { New-VHD -Path $lab -SizeBytes 8GB -Dynamic | Out-Null }
        if (-not (Test-Path $sig)) { New-VHD -Path $sig -SizeBytes 1GB -Dynamic | Out-Null }
        if (-not (Get-VMHardDiskDrive -VMName $Name | Where-Object Path -eq $lab)) { Add-VMHardDiskDrive -VMName $Name -Path $lab }
        if (-not (Get-VMHardDiskDrive -VMName $Name | Where-Object Path -eq $sig)) { Add-VMHardDiskDrive -VMName $Name -Path $sig }
        Get-VMHardDiskDrive -VMName $Name | Where-Object Path -eq $lab | Set-VMHardDiskDrive -MaximumIOPS $LabIops
        if ((Get-VM $Name).State -ne 'Running') { Start-VM $Name; Wait-Heartbeat 600 }
        $cred = Get-GuestCredential admin
        Invoke-Command -VMName $Name -Credential $cred -ScriptBlock {
            $raw = Get-Disk | Where-Object PartitionStyle -eq 'RAW' | Sort-Object Size
            if ($raw.Count -eq 2) {
                $raw[0] | Initialize-Disk -PartitionStyle GPT -PassThru | New-Partition -UseMaximumSize -DriveLetter S |
                    Format-Volume -FileSystem NTFS -NewFileSystemLabel LWSIGNAL -Confirm:$false | Out-Null
                $raw[1] | Initialize-Disk -PartitionStyle GPT -PassThru | New-Partition -UseMaximumSize -DriveLetter L |
                    Format-Volume -FileSystem NTFS -NewFileSystemLabel LWLAB -Confirm:$false | Out-Null
            }
            New-Item -ItemType Directory -Force L:\lab, C:\lw | Out-Null
            # The standard runner may write the lab and signal disks and read C:\lw.
            icacls L:\ /grant 'lwrunner:(OI)(CI)M' | Out-Null
            icacls S:\ /grant 'lwrunner:(OI)(CI)M' | Out-Null
            # A fresh volume root's DACL is not auto-inherited, so children get legacy ACEs the copy verifier reads
            # as explicit (#899). Protect L:\lab with an auto-inherited copy of its DACL (D:PAI).
            icacls L:\lab /inheritance:d | Out-Null
            (Get-Acl L:\lab).Sddl -match 'D:PAI'
            Get-Volume -DriveLetter L, S | Select-Object DriveLetter, FileSystemLabel, FileSystem, Size, SizeRemaining
            Get-PhysicalDisk | Get-StorageAdvancedProperty | Select-Object FriendlyName, IsDeviceCacheEnabled
        }
    }
    'pristine' {
        if (-not $Force -and @(Get-ChildItem -LiteralPath $State -Filter 'pristine-*').Count) {
            throw 'pristine_already_exists_use_force'
        }
        Stop-VM -Name $Name   # clean shutdown, not TurnOff
        $deadline = (Get-Date).AddSeconds(300)
        while ((Get-VM -Name $Name).State -ne 'Off') {
            if ((Get-Date) -ge $deadline) { throw 'clean_shutdown_timeout' }
            Start-Sleep -Milliseconds 250
        }
        $files = @(foreach ($disk in @('lab', 'signal', 'system')) {
            $destination = Join-Path $State "pristine-$disk.vhdx"
            Copy-Item -LiteralPath (Join-Path $Disks "$disk.vhdx") -Destination $destination -Force
            @{ name = "pristine-$disk.vhdx"; size = (Get-Item -LiteralPath $destination).Length;
                sha256 = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash }
        })
        @{ files = $files } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $State 'pristine.json') -Encoding utf8
        $files | ForEach-Object { [PSCustomObject]$_ } | Select-Object name, size, sha256
    }
}
