param([switch]$Release)
. "$PSScriptRoot\env.ps1"
$profileName = "debug"
$buildArgs = @("build", "-p", "mediaa")
if ($Release) {
    $profileName = "release"
    $buildArgs += "--release"
}
& cargo @buildArgs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$built = Join-Path $Root "target\$profileName\mediaa.exe"
$destDir = Join-Path $env:LOCALAPPDATA "mediaa\dev-run"
New-Item -ItemType Directory -Force $destDir | Out-Null
$dest = Join-Path $destDir "mediaa.exe"
Copy-Item $built $dest -Force
& $dest
exit $LASTEXITCODE
