param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [string[]]$Arguments = @(),
    [string]$Instance = "",
    [string]$Library = "",
    [switch]$Wait,
    [int]$TimeoutSec = 60,
    [hashtable]$Env = @{}
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
$Exe = (Resolve-Path -LiteralPath $Exe).Path
if ([string]::IsNullOrWhiteSpace($Instance)) {
    $Instance = "bg-" + [guid]::NewGuid().ToString("N")
}
if ([string]::IsNullOrWhiteSpace($Library)) {
    $Library = Join-Path ([IO.Path]::GetTempPath()) ("mediaa-bg-" + [guid]::NewGuid().ToString("N"))
}
New-Item -ItemType Directory -Force $Library | Out-Null

$info = [Diagnostics.ProcessStartInfo]::new()
$info.FileName = $Exe
$info.Arguments = ($Arguments -join " ")
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
Set-Env $info "MEDIAA_DEV_BACKGROUND" "1"
Set-Env $info "MEDIAA_DEV_AGENT" "1"
Set-Env $info "MEDIAA_INSTANCE_ID" $Instance
Set-Env $info "MEDIAA_DEV_LIBRARY" $Library
foreach ($key in $Env.Keys) {
    Set-Env $info $key ([string]$Env[$key])
}
$process = [Diagnostics.Process]::Start($info)
"pid=$($process.Id) instance=$Instance library=$Library"
if ($Wait) {
    if (-not $process.WaitForExit($TimeoutSec * 1000)) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        throw "process did not finish within $TimeoutSec s"
    }
    "exit=$($process.ExitCode)"
}
