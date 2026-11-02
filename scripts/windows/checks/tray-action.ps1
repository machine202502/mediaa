param(
    [ValidateSet('show', 'region', 'fullscreen', 'start', 'audio', 'pause', 'resume', 'stop', 'quit', 'primary')]
    [string]$Action = 'show',
    [string]$Exe = "target\debug\mediaa-agent.exe",
    [int]$WaitSec = 8,
    [switch]$Background,
    [switch]$NoUi,
    [switch]$Keep
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
$Exe = Join-Path $root $Exe
if (-not (Test-Path -LiteralPath $Exe)) {
    throw "debug executable not found: $Exe (run cargo build -p mediaa first)"
}
$Exe = (Resolve-Path -LiteralPath $Exe).Path
if ($Exe -notmatch '[\\/]debug[\\/]') {
    throw "tray-action uses debug-only MEDIAA_DEV_TRAY; pass a target\debug\mediaa.exe"
}

$instance = "tray-" + [guid]::NewGuid().ToString("N")
$library = Join-Path ([IO.Path]::GetTempPath()) ("mediaa-tray-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force $library | Out-Null

function Start-Mediaa([string[]]$arguments) {
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $Exe
    $info.Arguments = ($arguments -join " ")
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $info.Environment["MEDIAA_INSTANCE_ID"] = $instance
    $info.Environment["MEDIAA_DEV_LIBRARY"] = $library
    $info.Environment["MEDIAA_DEV_AGENT"] = "1"
    $info.Environment["MEDIAA_DEV_TRAY"] = $Action
    if ($Background) {
        $info.Environment["MEDIAA_DEV_BACKGROUND"] = "1"
    }
    [Diagnostics.Process]::Start($info)
}

function Stop-Mediaa($process) {
    if ($null -eq $process) {
        return
    }
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }
}

$backend = $null
$ui = $null
try {
    $backend = Start-Mediaa @("--backend")
    Start-Sleep -Milliseconds 150
    if (-not $NoUi) {
        $ui = Start-Mediaa @("--ui", "--tray")
    }
    "action=$Action backend=$($backend.Id) ui=$($ui.Id) instance=$instance"
    Start-Sleep -Seconds $WaitSec
    $backend.Refresh()
    if ($null -ne $ui) {
        $ui.Refresh()
    }
    "after ${WaitSec}s: backend_exited=$($backend.HasExited) ui_exited=$($null -eq $ui -or $ui.HasExited)"
    if ($Keep) {
        "kept processes for inspection; stop them by pid"
    }
}
finally {
    if (-not $Keep) {
        Stop-Mediaa $ui
        Stop-Mediaa $backend
        Remove-Item -Recurse -Force $library -ErrorAction SilentlyContinue
    }
}
