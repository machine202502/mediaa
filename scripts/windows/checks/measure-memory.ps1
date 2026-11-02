param(
    [string]$Exe = "",
    [string]$Library = "",
    [string[]]$States = @("about", "screenshot", "library", "settings", "record", "tray", "area"),
    [int]$WarmupSec = 8,
    [int]$SampleSec = 10,
    [string]$OutDir = "",
    [string]$Compare = "",
    [hashtable]$Env = @{},
    [double]$ThresholdPercent = 15
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
if ([string]::IsNullOrWhiteSpace($Exe)) {
    $Exe = Join-Path $root "target\debug\mediaa-agent.exe"
}
$Exe = (Resolve-Path -LiteralPath $Exe).Path
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path $root "target\perf"
}
New-Item -ItemType Directory -Force $OutDir | Out-Null
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$csvPath = Join-Path $OutDir "audit-$stamp.csv"
$mdPath = Join-Path $OutDir "audit-$stamp.md"
$jsonPath = Join-Path $OutDir "audit-latest.json"
$jsonStampPath = Join-Path $OutDir "audit-$stamp.json"

$ownLibrary = $false
if ([string]::IsNullOrWhiteSpace($Library)) {
    $Library = Join-Path ([IO.Path]::GetTempPath()) ("mediaa-audit-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $Library | Out-Null
    $ownLibrary = $true
}
$Library = (Resolve-Path -LiteralPath $Library).Path
$instance = "mediaa-audit-" + [guid]::NewGuid().ToString("N")
$appLog = Join-Path $env:LOCALAPPDATA "mediaa\MediaA\data\logs\mediaa.log"

$stateArgs = @{
    about      = @("--page=about")
    screenshot = @("--page=screenshot")
    record     = @("--page=record")
    audio      = @("--page=audio")
    library    = @("--page=library")
    convert    = @("--page=convert")
    settings   = @("--page=settings")
    daemon     = @("--daemon")
    tray       = @("--tray")
    area       = @("--area")
}

function Set-Env($info, [string]$name, [string]$value) {
    if ($info.PSObject.Properties.Name -contains "Environment") {
        $info.Environment[$name] = $value
    } else {
        $info.EnvironmentVariables[$name] = $value
    }
}

function Start-Audited([string]$state) {
    if (-not $stateArgs.ContainsKey($state)) {
        throw "unknown state '$state'; known: $($stateArgs.Keys -join ', ')"
    }
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $Exe
    $info.Arguments = ($stateArgs[$state] -join " ")
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    Set-Env $info "MEDIAA_INSTANCE_ID" $instance
    Set-Env $info "MEDIAA_DEV_AGENT" "1"
    Set-Env $info "MEDIAA_DEV_LIBRARY" $Library
    foreach ($key in $Env.Keys) { Set-Env $info $key ([string]$Env[$key]) }
    [Diagnostics.Process]::Start($info)
}

function Save-LogSlice([long]$offset, [string]$state) {
    if (-not (Test-Path -LiteralPath $appLog)) { return }
    $target = Join-Path $OutDir "audit-$stamp-$state.log"
    $stream = [IO.File]::Open($appLog, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
    try {
        $length = $stream.Length - $offset
        if ($length -le 0) { return }
        $stream.Seek($offset, [IO.SeekOrigin]::Begin) | Out-Null
        $buffer = New-Object byte[] $length
        $read = $stream.Read($buffer, 0, [int]$length)
        [IO.File]::WriteAllBytes($target, $buffer[0..([math]::Max($read - 1, 0))])
    }
    finally {
        $stream.Dispose()
    }
}

function Get-LogLength() {
    if (-not (Test-Path -LiteralPath $appLog)) { return 0 }
    return (Get-Item -LiteralPath $appLog).Length
}

function Get-Percentile($values, [double]$percent) {
    if ($values.Count -eq 0) { return 0 }
    $sorted = $values | Sort-Object
    $index = [math]::Ceiling(($percent / 100.0) * $sorted.Count) - 1
    if ($index -lt 0) { $index = 0 }
    if ($index -ge $sorted.Count) { $index = $sorted.Count - 1 }
    return [double]$sorted[$index]
}

$samples = New-Object System.Collections.Generic.List[object]
$summary = @{}
$started = Get-Date

"audit exe: $Exe"
"audit library: $Library"
"warmup ${WarmupSec}s, sample ${SampleSec}s per state"
""

foreach ($state in $States) {
    $logOffset = Get-LogLength
    $process = Start-Audited $state
    try {
        Start-Sleep -Seconds $WarmupSec
        $process.Refresh()
        if ($process.HasExited) {
            Write-Warning "$state exited before sampling (code $($process.ExitCode))"
            continue
        }
        $private = New-Object System.Collections.Generic.List[double]
        $working = New-Object System.Collections.Generic.List[double]
        $cpu = New-Object System.Collections.Generic.List[double]
        $threads = New-Object System.Collections.Generic.List[int]
        $prev = $process.TotalProcessorTime
        for ($i = 1; $i -le $SampleSec; $i++) {
            Start-Sleep -Seconds 1
            if ($process.HasExited) { break }
            $process.Refresh()
            $now = $process.TotalProcessorTime
            $pct = ($now - $prev).TotalSeconds * 100.0
            $prev = $now
            $private.Add([double]$process.PrivateMemorySize64 / 1MB)
            $working.Add([double]$process.WorkingSet64 / 1MB)
            $cpu.Add([double]$pct)
            $threads.Add([int]$process.Threads.Count)
            $samples.Add([pscustomobject]@{
                state   = $state
                second  = $i
                private = [math]::Round([double]$process.PrivateMemorySize64 / 1MB, 1)
                working = [math]::Round([double]$process.WorkingSet64 / 1MB, 1)
                cpu_pct = [math]::Round([double]$pct, 2)
                threads = [int]$process.Threads.Count
            })
        }
        if ($private.Count -eq 0) {
            Write-Warning "$state produced no samples"
            continue
        }
        $row = [pscustomobject]@{
            state       = $state
            private_p50 = [math]::Round((Get-Percentile $private 50), 1)
            private_p95 = [math]::Round((Get-Percentile $private 95), 1)
            private_max = [math]::Round(($private | Measure-Object -Maximum).Maximum, 1)
            working_p50 = [math]::Round((Get-Percentile $working 50), 1)
            cpu_p50     = [math]::Round((Get-Percentile $cpu 50), 1)
            cpu_p95     = [math]::Round((Get-Percentile $cpu 95), 1)
            cpu_max     = [math]::Round(($cpu | Measure-Object -Maximum).Maximum, 1)
            threads_max = [int]($threads | Measure-Object -Maximum).Maximum
            samples     = $private.Count
        }
        $summary[$state] = $row
        "{0,-11} private p50/p95/max {1,7} / {2,7} / {3,7} MB   working p50 {4,7} MB   cpu p50/p95/max {5,6} / {6,6} / {7,6} %   threads {8}" -f `
            $state, $row.private_p50, $row.private_p95, $row.private_max, $row.working_p50, $row.cpu_p50, $row.cpu_p95, $row.cpu_max, $row.threads_max
    }
    finally {
        if (-not $process.HasExited) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        }
        Start-Sleep -Milliseconds 500
        Save-LogSlice $logOffset $state
    }
}

$samples | Export-Csv -Path $csvPath -NoTypeInformation -Encoding UTF8

$lines = @()
$lines += "# MediaA memory and CPU audit"
$lines += ""
$lines += "- date: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')"
$lines += "- exe: ``$Exe``"
$lines += "- library: ``$Library``"
$lines += "- warmup: ${WarmupSec}s, sample: ${SampleSec}s per state"
$lines += "- extra env: $(($Env.Keys | ForEach-Object { "$_=$($Env[$_])" }) -join ", ")"
$lines += "- cpu is percent of one core; private/working are megabytes"
$lines += ""
$lines += "| state | private p50 | private p95 | private max | working p50 | cpu p50 | cpu p95 | cpu max | threads |"
$lines += "|---|---:|---:|---:|---:|---:|---:|---:|---:|"
foreach ($state in $States) {
    if (-not $summary.ContainsKey($state)) { continue }
    $r = $summary[$state]
    $lines += "| $state | $($r.private_p50) | $($r.private_p95) | $($r.private_max) | $($r.working_p50) | $($r.cpu_p50) | $($r.cpu_p95) | $($r.cpu_max) | $($r.threads_max) |"
}
$lines += ""
$lines += "Raw samples: ``$(Split-Path -Leaf $csvPath)``"
$lines | Set-Content -Path $mdPath -Encoding UTF8

$snapshot = [pscustomobject]@{
    date    = (Get-Date).ToString("o")
    exe     = $Exe
    library = $Library
    warmup  = $WarmupSec
    sample  = $SampleSec
    env     = $Env
    states  = $summary
}
$snapshot | ConvertTo-Json -Depth 6 | Set-Content -Path $jsonPath -Encoding UTF8
$snapshot | ConvertTo-Json -Depth 6 | Set-Content -Path $jsonStampPath -Encoding UTF8

""
"report: $mdPath"
"csv:    $csvPath"
"json:   $jsonStampPath (also audit-latest.json)"
"elapsed: $([math]::Round(((Get-Date) - $started).TotalSeconds, 0))s"

if ($ownLibrary) {
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    $resolved = [IO.Path]::GetFullPath($Library)
    if ($resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($Compare) {
    if (-not (Test-Path -LiteralPath $Compare)) { throw "baseline not found: $Compare" }
    $baseline = Get-Content -LiteralPath $Compare -Raw | ConvertFrom-Json
    ""
    "compare with $Compare"
    "| state | private p50 | delta | cpu p50 | delta |"
    "|---|---:|---:|---:|---:|"
    $failed = $false
    foreach ($state in $States) {
        if (-not $summary.ContainsKey($state)) { continue }
        $old = $baseline.states.$state
        if (-not $old) { continue }
        $new = $summary[$state]
        $memDelta = 0.0
        if ($old.private_p50 -gt 0) { $memDelta = (($new.private_p50 - $old.private_p50) / $old.private_p50) * 100.0 }
        $cpuDelta = 0.0
        if ($old.cpu_p50 -gt 0) { $cpuDelta = (($new.cpu_p50 - $old.cpu_p50) / $old.cpu_p50) * 100.0 }
        "| $state | $($old.private_p50) -> $($new.private_p50) | {0:+0.0;-0.0;0.0}% | $($old.cpu_p50) -> $($new.cpu_p50) | {1:+0.0;-0.0;0.0}% |" -f $memDelta, $cpuDelta
        if ($memDelta -gt $ThresholdPercent) { $failed = $true }
    }
    if ($failed) {
        Write-Error "memory grew by more than $ThresholdPercent% in at least one state"
        exit 1
    }
}
