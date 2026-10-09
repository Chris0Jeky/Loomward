# Optional wrapper; no execution-policy changes, elevation, winget or global installs.
param([switch]$CreateVenv, [switch]$Telemetry)
$ErrorActionPreference = 'Stop'
$Arguments = @((Join-Path $PSScriptRoot 'bootstrap.py'))
if ($CreateVenv) { $Arguments += '--create-venv' }
if ($Telemetry) { $Arguments += '--telemetry' }
if (Get-Command py -ErrorAction SilentlyContinue) { & py -3 @Arguments }
elseif (Get-Command python -ErrorAction SilentlyContinue) { & python @Arguments }
else { throw 'Install Python 3.11 or later, then retry. No system settings were changed.' }
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
