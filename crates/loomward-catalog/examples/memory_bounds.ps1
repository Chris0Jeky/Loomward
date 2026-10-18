param(
    [Parameter(Mandatory=$true)][string]$Executable,
    [Parameter(Mandatory=$true)][string]$Output,
    [int]$Rows = 1000000
)
$ErrorActionPreference = 'Stop'
$results = @()
foreach ($case in @('rollup', 'listing')) {
    $stdout = [System.IO.Path]::GetTempFileName()
    $stderr = [System.IO.Path]::GetTempFileName()
    try {
        $process = Start-Process -FilePath $Executable -ArgumentList @($case, $Rows) -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
        $null = $process.Handle
        $peak = 0L
        while (-not $process.HasExited) {
            $process.Refresh()
            $peak = [Math]::Max($peak, $process.PeakWorkingSet64)
            Start-Sleep -Milliseconds 100
        }
        $process.WaitForExit()
        $results += [ordered]@{
            case = $case
            rows = $Rows
            peak_rss_bytes = $peak
            limit_bytes = 400MB
            within_limit = ($peak -le 400MB)
            exit_code = $process.ExitCode
            result = [System.IO.File]::ReadAllText($stdout)
            progress = [System.IO.File]::ReadAllText($stderr)
        }
        $results[-1] | ConvertTo-Json -Compress | Write-Output
    } finally {
        Remove-Item -LiteralPath $stdout, $stderr
    }
}
$results | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $Output
if ($results | Where-Object { $_.exit_code -ne 0 -or -not $_.within_limit }) {
    throw 'Catalogue memory bound failed; see the recorded measurements.'
}
