param(
    [long]$Handle = 0,
    [string]$Title = "",
    [int]$ProcessId = 0,
    [string]$Out = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
if ([string]::IsNullOrWhiteSpace($Out)) {
    $dir = Join-Path $root "target\screenshots"
    New-Item -ItemType Directory -Force $dir | Out-Null
    $Out = Join-Path $dir ("window-" + (Get-Date -Format "yyyyMMdd-HHmmss") + ".png")
}
$Out = [IO.Path]::GetFullPath($Out)
New-Item -ItemType Directory -Force (Split-Path -Parent $Out) | Out-Null

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Text;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
public static class MediaaCapture {
    public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }

    public static long Find(string title, int processId) {
        long found = 0;
        EnumWindows((h, l) => {
            uint wp; GetWindowThreadProcessId(h, out wp);
            if (processId != 0 && (int)wp != processId) return true;
            var sb = new StringBuilder(512);
            GetWindowText(h, sb, 512);
            if (sb.ToString() == title) { found = h.ToInt64(); return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static string Capture(long handle, string outPath) {
        IntPtr hwnd = new IntPtr(handle);
        RECT r;
        if (!GetWindowRect(hwnd, out r)) throw new Exception("GetWindowRect failed");
        int width = r.Right - r.Left;
        int height = r.Bottom - r.Top;
        if (width <= 0 || height <= 0) throw new Exception("window has no visible size");
        using (Bitmap bitmap = new Bitmap(width, height))
        using (Graphics graphics = Graphics.FromImage(bitmap)) {
            IntPtr hdc = graphics.GetHdc();
            bool printed;
            try { printed = PrintWindow(hwnd, hdc, 2); }
            finally { graphics.ReleaseHdc(hdc); }
            if (!printed) throw new Exception("PrintWindow failed");
            int stepX = Math.Max(1, width / 16);
            int stepY = Math.Max(1, height / 16);
            int samples = 0;
            int black = 0;
            for (int x = 0; x < width; x += stepX) {
                for (int y = 0; y < height; y += stepY) {
                    Color c = bitmap.GetPixel(x, y);
                    samples++;
                    if (c.R < 8 && c.G < 8 && c.B < 8) black++;
                }
            }
            if (samples > 0 && black == samples) {
                throw new Exception("window captured as a black frame: it is hidden or protected from capture; show it off-screen (checks/run-background.ps1) and set MEDIAA_DEV_PICKER=1 for the region picker");
            }
            bitmap.Save(outPath, ImageFormat.Png);
            return string.Format("{0}x{1}", width, height);
        }
    }
}
"@ -ReferencedAssemblies System.Drawing

if ($Handle -eq 0) {
    if ([string]::IsNullOrWhiteSpace($Title)) {
        if ($ProcessId -eq 0) { throw "pass -Handle, -Title or -Pid" }
        $process = Get-Process -Id $ProcessId -ErrorAction Stop
        $Handle = $process.MainWindowHandle.ToInt64()
    } else {
        $Handle = [MediaaCapture]::Find($Title, $ProcessId)
    }
}
if ($Handle -eq 0) { throw "window not found" }
$size = [MediaaCapture]::Capture($Handle, $Out)
"window {0} {1} -> {2}" -f $Handle, $size, $Out
