param(
    [string]$Out = "",
    [int]$Monitor = -1
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
if ([string]::IsNullOrWhiteSpace($Out)) {
    $dir = Join-Path $root "target\screenshots"
    New-Item -ItemType Directory -Force $dir | Out-Null
    $Out = Join-Path $dir ("screen-" + (Get-Date -Format "yyyyMMdd-HHmmss") + ".png")
}
$Out = [IO.Path]::GetFullPath($Out)
New-Item -ItemType Directory -Force (Split-Path -Parent $Out) | Out-Null

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class MediaaDpi {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    public static void Apply() { SetProcessDPIAware(); }
}
"@
[MediaaDpi]::Apply()

if ($Monitor -ge 0) {
    $screens = [System.Windows.Forms.Screen]::AllScreens
    if ($Monitor -ge $screens.Count) { throw "monitor $Monitor not found (count $($screens.Count))" }
    $bounds = $screens[$Monitor].Bounds
} else {
    $bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
}

$bitmap = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
try {
    $graphics.CopyFromScreen($bounds.Left, $bounds.Top, 0, 0, $bitmap.Size)
    $bitmap.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
} finally {
    $graphics.Dispose()
    $bitmap.Dispose()
}
"screen {0}x{1} at ({2},{3}) -> {4}" -f $bounds.Width, $bounds.Height, $bounds.Left, $bounds.Top, $Out
