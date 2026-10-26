param(
    [string]$Exe = "",
    [int]$TimeoutSec = 20
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if ([string]::IsNullOrWhiteSpace($Exe)) {
    $Exe = Join-Path $root "target\debug\mediaa.exe"
}
$Exe = (Resolve-Path -LiteralPath $Exe).Path
$instance = "mediaa-overlay-smoke-$([guid]::NewGuid().ToString('N'))"
$library = Join-Path ([IO.Path]::GetTempPath()) $instance
New-Item -ItemType Directory -Path $library | Out-Null

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class MediaAOverlaySmoke {
    public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hwnd, uint msg, IntPtr wParam, IntPtr lParam);
}
"@

$psi = [Diagnostics.ProcessStartInfo]::new()
$psi.FileName = $Exe
$psi.Arguments = "--area"
$psi.UseShellExecute = $false
$psi.CreateNoWindow = $true
$psi.Environment["MEDIAA_DEV_LIBRARY"] = $library
$psi.Environment["MEDIAA_INSTANCE_ID"] = $instance
$process = [Diagnostics.Process]::Start($psi)

function Find-Overlay([uint32]$processId) {
    $script:found = [IntPtr]::Zero
    $callback = [MediaAOverlaySmoke+EnumWindowsProc]{
        param($hwnd, $unused)
        [uint32]$windowPid = 0
        [void][MediaAOverlaySmoke]::GetWindowThreadProcessId($hwnd, [ref]$windowPid)
        if ($windowPid -eq $processId) {
            $text = [Text.StringBuilder]::new(256)
            [void][MediaAOverlaySmoke]::GetWindowText($hwnd, $text, $text.Capacity)
            if ($text.ToString() -eq "MediaA area picker") {
                $script:found = $hwnd
                return $false
            }
        }
        return $true
    }
    [void][MediaAOverlaySmoke]::EnumWindows($callback, [IntPtr]::Zero)
    return $script:found
}

try {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSec)
    $overlay = [IntPtr]::Zero
    while ([DateTime]::UtcNow -lt $deadline -and $overlay -eq [IntPtr]::Zero) {
        if ($process.HasExited) {
            throw "MediaA exited before opening the region overlay: $($process.ExitCode)"
        }
        $overlay = Find-Overlay ([uint32]$process.Id)
        Start-Sleep -Milliseconds 100
    }
    if ($overlay -eq [IntPtr]::Zero) {
        throw "Region overlay window was not created"
    }
    while (-not [MediaAOverlaySmoke]::IsWindowVisible($overlay) -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 100
    }
    if (-not [MediaAOverlaySmoke]::IsWindowVisible($overlay)) {
        throw "Region overlay did not become visible"
    }

    [void][MediaAOverlaySmoke]::PostMessage($overlay, 0x0100, [IntPtr]0x09, [IntPtr]::Zero)
    [void][MediaAOverlaySmoke]::PostMessage($overlay, 0x0101, [IntPtr]0x09, [IntPtr]::Zero)
    Start-Sleep -Seconds 2
    if (-not [MediaAOverlaySmoke]::IsWindowVisible($overlay)) {
        throw "Region overlay disappeared after Tab"
    }

    [void][MediaAOverlaySmoke]::PostMessage($overlay, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 500
    "Region overlay smoke passed"
}
finally {
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
    }
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    $resolvedLibrary = [IO.Path]::GetFullPath($library)
    if ($resolvedLibrary.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $resolvedLibrary -Recurse -Force -ErrorAction SilentlyContinue
    }
}
