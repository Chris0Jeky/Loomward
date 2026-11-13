# Run a Git Bash command outside the agent session's job object via WMI.
param([string]$Cmd)
$bash = 'C:\Program Files\Git\bin\bash.exe'
$cl = '"' + $bash + '" -lc "' + $Cmd.Replace('"','\"') + '"'
$lwRoot = if ($env:LW_HOME) { $env:LW_HOME } else { 'C:\lw' }
$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{CommandLine=$cl; CurrentDirectory=$lwRoot}
"pid $($r.ProcessId) rc $($r.ReturnValue)"
