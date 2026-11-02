param(
    [string]$Exe = "",
    [int]$Runs = 10,
    [int]$TimeoutSec = 15,
    [string]$OutDir = "",
    [hashtable]$Env = @{},
    [switch]$WarmDaemon,
    [switch]$Foreground
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
if ([string]::IsNullOrWhiteSpace($Exe)) {
    $Exe = Join-Path $root "target\debug\mediaa-agent.exe"
}
$Exe = (Resolve-Path -LiteralPath $Exe).Path
$ProcessName = [IO.Path]::GetFileName($Exe)
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path $root "target\perf"
}
New-Item -ItemType Directory -Force $OutDir | Out-Null
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class PickerProbe {
    public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
}
"@

function Find-Window([string]$title) {
    $script:found = [IntPtr]::Zero
    $cb = [PickerProbe+EnumWindowsProc]{
        param($hwnd, $unused)
        $sb = [Text.StringBuilder]::new(256)
        [void][PickerProbe]::GetWindowText($hwnd, $sb, 256)
        if ($sb.ToString() -eq $title -and [PickerProbe]::IsWindowVisible($hwnd)) {
            $script:found = $hwnd
            return $false
        }
        return $true
    }
    [void][PickerProbe]::EnumWindows($cb, [IntPtr]::Zero)
    return $script:found
}

function Set-Env($info, [string]$name, [string]$value) {
    if ($info.PSObject.Properties.Name -contains "Environment") {
        $info.Environment[$name] = $value
    } else {
        $info.EnvironmentVariables[$name] = $value
    }
}

function Get-Percentile($values, [double]$percent) {
    $sorted = $values | Sort-Object
    $index = [math]::Ceiling(($percent / 100.0) * $sorted.Count) - 1
    if ($index -lt 0) { $index = 0 }
    if ($index -ge $sorted.Count) { $index = $sorted.Count - 1 }
    return [double]$sorted[$index]
}

if (Get-Process mediaa -ErrorAction SilentlyContinue) {
    throw "close running MediaA instances before measuring picker latency"
}
$pickerSamples = New-Object System.Collections.Generic.List[double]
$maskSamples = New-Object System.Collections.Generic.List[double]
$instance = "picker-" + [guid]::NewGuid().ToString("N")
if (-not $Foreground) { $Env["MEDIAA_DEV_BACKGROUND"] = "1" }
$Env["MEDIAA_DEV_AGENT"] = "1"
$sharedLibrary = Join-Path ([IO.Path]::GetTempPath()) ("mediaa-picker-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $sharedLibrary | Out-Null
if ($WarmDaemon) {
    $daemonInfo = [Diagnostics.ProcessStartInfo]::new()
    $daemonInfo.FileName = $Exe
    $daemonInfo.Arguments = "--daemon"
    $daemonInfo.UseShellExecute = $false
    $daemonInfo.CreateNoWindow = $true
    Set-Env $daemonInfo "MEDIAA_INSTANCE_ID" $instance
    Set-Env $daemonInfo "MEDIAA_DEV_LIBRARY" $sharedLibrary
    foreach ($key in $Env.Keys) { Set-Env $daemonInfo $key ([string]$Env[$key]) }
    [void][Diagnostics.Process]::Start($daemonInfo)
    Start-Sleep -Seconds 2
}
"picker latency: $Exe, runs $Runs"
for ($i = 1; $i -le $Runs; $i++) {
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $Exe
    $info.Arguments = "--area"
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    Set-Env $info "MEDIAA_INSTANCE_ID" $instance
    Set-Env $info "MEDIAA_DEV_LIBRARY" $sharedLibrary
    foreach ($key in $Env.Keys) { Set-Env $info $key ([string]$Env[$key]) }
    $started = [DateTime]::UtcNow
    [void][Diagnostics.Process]::Start($info)
    $deadline = $started.AddSeconds($TimeoutSec)
    $pickerMs = -1.0
    $maskMs = -1.0
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($maskMs -lt 0 -and (Find-Window "MediaA picker mask") -ne [IntPtr]::Zero) {
            $maskMs = ([DateTime]::UtcNow - $started).TotalMilliseconds
        }
        if ((Find-Window "MediaA area picker") -ne [IntPtr]::Zero) {
            $pickerMs = ([DateTime]::UtcNow - $started).TotalMilliseconds
            break
        }
        Start-Sleep -Milliseconds 15
    }
    if ($WarmDaemon) {
        Get-CimInstance Win32_Process -Filter "Name='$ProcessName'" | Where-Object { $_.CommandLine -match "--ui" } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    } else {
        Get-Process mediaa -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 400
    if ($pickerMs -ge 0) { $pickerSamples.Add($pickerMs) }
    if ($maskMs -ge 0) { $maskSamples.Add($maskMs) }
    "{0,2}/{1}: picker {2,6:N0} ms, mask {3,6:N0} ms" -f $i, $Runs, $pickerMs, $maskMs
}

function Format-Stats($values) {
    if ($values.Count -eq 0) { return "| min | - |`n| p50 | - |`n| p95 | - |`n| max | - |" }
    return @"
| min | $([math]::Round(($values | Measure-Object -Minimum).Minimum)) |
| p50 | $([math]::Round((Get-Percentile $values 50))) |
| p95 | $([math]::Round((Get-Percentile $values 95))) |
| max | $([math]::Round(($values | Measure-Object -Maximum).Maximum)) |
"@
}

if ($WarmDaemon) {
    Get-CimInstance Win32_Process -Filter "Name='$ProcessName'" | Where-Object { $_.CommandLine -match "--daemon" } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    Remove-Item -LiteralPath $sharedLibrary -Recurse -Force -ErrorAction SilentlyContinue
}

if ($pickerSamples.Count -eq 0) {
    throw "the region overlay never became visible"
}
$report = @"
# Region picker latency

- exe: ``$Exe``
- runs: $($pickerSamples.Count) / $Runs
- extra env: $(($Env.Keys | ForEach-Object { "$_=$($Env[$_])" }) -join ", ")
- mode: $(if ($Foreground) { "foreground (windows may appear)" } else { "background (off-screen, no activation, no mask)" })

## Picker overlay visible

| metric | ms |
|---|---:|
$(Format-Stats $pickerSamples)

## Native mask visible

| metric | ms |
|---|---:|
$(Format-Stats $maskSamples)
"@
$path = Join-Path $OutDir "picker-latency-$stamp.md"
$report | Set-Content -Path $path -Encoding UTF8
$pickerSamples | ForEach-Object { [pscustomobject]@{ picker_ms = [math]::Round($_); mask_ms = "" } } | Export-Csv -Path (Join-Path $OutDir "picker-latency-$stamp.csv") -NoTypeInformation -Encoding UTF8
""
$report
"report: $path"
