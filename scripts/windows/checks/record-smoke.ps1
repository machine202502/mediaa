param(
    [string]$Exe = "",
    [int]$Seconds = 3,
    [string]$Library = "",
    [switch]$Keep
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
if ([string]::IsNullOrWhiteSpace($Exe)) {
    $Exe = Join-Path $root "target\debug\mediaa-agent.exe"
}
$Exe = (Resolve-Path -LiteralPath $Exe).Path
if ([string]::IsNullOrWhiteSpace($Library)) {
    $Library = Join-Path ([IO.Path]::GetTempPath()) ("mediaa-record-" + [guid]::NewGuid().ToString("N"))
}
New-Item -ItemType Directory -Force $Library | Out-Null
$Library = (Resolve-Path -LiteralPath $Library).Path
$instance = "record-" + [guid]::NewGuid().ToString("N")

$info = [Diagnostics.ProcessStartInfo]::new()
$info.FileName = $Exe
$info.Arguments = "--daemon"
$info.UseShellExecute = $false
$info.CreateNoWindow = $true
$info.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden

function Set-Env($start, [string]$name, [string]$value) {
    if ($start.PSObject.Properties.Name -contains "Environment") {
        $start.Environment[$name] = $value
    } else {
        $start.EnvironmentVariables[$name] = $value
    }
}

Set-Env $info "MEDIAA_INSTANCE_ID" $instance
Set-Env $info "MEDIAA_DEV_LIBRARY" $Library
Set-Env $info "MEDIAA_DEV_RECORD" "$Seconds"
Set-Env $info "MEDIAA_DEV_AGENT" "1"

$process = [Diagnostics.Process]::Start($info)
"pid=$($process.Id) instance=$instance library=$Library"
$samples = New-Object System.Collections.ArrayList
$deadline = (Get-Date).AddSeconds($Seconds + 8)
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 200
    $proc = Get-Process -Id $process.Id -ErrorAction SilentlyContinue
    if (-not $proc) {
        break
    }
    [void]$samples.Add([pscustomobject]@{
        Private = $proc.PrivateMemorySize64 / 1MB
        Working = $proc.WorkingSet64 / 1MB
        Cpu = $proc.TotalProcessorTime.TotalMilliseconds
    })
}
$process | Stop-Process -Force -ErrorAction SilentlyContinue

$files = @(Get-ChildItem $Library -Recurse -File | Where-Object {
    $_.FullName -notmatch '\\\.cache\\' -and
    $_.Extension -in @(".gif", ".mp4", ".mkv", ".webp", ".webm")
})
if ($files.Count -eq 0) {
    throw "recording was not produced in $Library"
}

$private = @($samples | ForEach-Object { $_.Private } | Sort-Object)
$working = @($samples | ForEach-Object { $_.Working } | Sort-Object)
$cpu = 0.0
if ($samples.Count -ge 2) {
    $cpu = $samples[$samples.Count - 1].Cpu - $samples[0].Cpu
}
$p50 = $private[[int][math]::Floor($private.Count / 2)]
$p95 = $private[[int][math]::Floor($private.Count * 0.95)]
$max = $private[$private.Count - 1]
$wsMax = $working[$working.Count - 1]

$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$perf = Join-Path $root "target\perf"
New-Item -ItemType Directory -Force $perf | Out-Null
$report = Join-Path $perf "record-smoke-$stamp.md"
$lines = @(
    "# Daemon recording smoke",
    "",
    "- exe: ``$Exe``",
    "- library: ``$Library``",
    "- seconds: $Seconds",
    "- output: ``$($files[0].FullName)`` ($([math]::Round($files[0].Length / 1KB)) KB)",
    "",
    "| metric | value |",
    "|---|---:|",
    "| samples | $($samples.Count) |",
    "| private p50 | $([math]::Round($p50, 1)) MB |",
    "| private p95 | $([math]::Round($p95, 1)) MB |",
    "| private max | $([math]::Round($max, 1)) MB |",
    "| working set max | $([math]::Round($wsMax, 1)) MB |",
    "| cpu total | $([math]::Round($cpu)) ms |"
)
[IO.File]::WriteAllText($report, ($lines -join "`r`n"), (New-Object Text.UTF8Encoding($false)))

"recording: $($files[0].FullName) $([math]::Round($files[0].Length / 1KB)) KB"
"private p50/p95/max: $([math]::Round($p50,1)) / $([math]::Round($p95,1)) / $([math]::Round($max,1)) MB; ws max $([math]::Round($wsMax,1)) MB; cpu $([math]::Round($cpu)) ms"
"report: $report"
if (-not $Keep) {
    Remove-Item -LiteralPath $Library -Recurse -Force
    "library removed (use -Keep to keep it)"
}
