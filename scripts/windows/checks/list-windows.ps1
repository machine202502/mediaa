param(
    [string]$TitleLike = "",
    [int]$ProcessId = 0,
    [switch]$All,
    [switch]$Json
)

$ErrorActionPreference = "Stop"

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class MediaaWindows {
    public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    public static string List(string titleLike, int processId, bool all) {
        var rows = new StringBuilder();
        EnumWindows((h, l) => {
            var sb = new StringBuilder(512);
            GetWindowText(h, sb, 512);
            string title = sb.ToString();
            if (title.Length == 0) return true;
            uint wp; GetWindowThreadProcessId(h, out wp);
            if (processId != 0 && (int)wp != processId) return true;
            if (titleLike.Length > 0 && title.IndexOf(titleLike, StringComparison.OrdinalIgnoreCase) < 0) return true;
            if (!all && !IsWindowVisible(h)) return true;
            RECT r; GetWindowRect(h, out r);
            rows.AppendLine(string.Format("{0}|{1}|{2}|{3}|{4},{5},{6},{7}", h.ToInt64(), wp, IsWindowVisible(h), title, r.Left, r.Top, r.Right, r.Bottom));
            return true;
        }, IntPtr.Zero);
        return rows.ToString();
    }
}
"@

$rows = [MediaaWindows]::List($TitleLike, $ProcessId, [bool]$All)
$items = @()
foreach ($line in ($rows -split "`n")) {
    if ([string]::IsNullOrWhiteSpace($line)) { continue }
    $parts = $line.Split('|')
    $rect = $parts[4].Split(',')
    $items += [pscustomobject]@{
        Handle  = [long]$parts[0]
        Hex     = ('0x{0:X}' -f [long]$parts[0])
        Pid     = [int]$parts[1]
        Visible = ($parts[2] -eq 'True')
        Title   = $parts[3]
        Left    = [int]$rect[0]
        Top     = [int]$rect[1]
        Right   = [int]$rect[2]
        Bottom  = [int]$rect[3]
    }
}
if ($Json) {
    $items | ConvertTo-Json -Depth 3
} else {
    $items | Format-Table Handle, Hex, Pid, Visible, Left, Top, Right, Bottom, Title -AutoSize
}
