param([switch]$Release)
Get-ChildItem $PSScriptRoot -Filter '*.ps1' -ErrorAction SilentlyContinue |
    Unblock-File -ErrorAction SilentlyContinue
. "$PSScriptRoot\env.ps1"
$profileName = "debug"
$buildArgs = @("build", "-p", "mediaa")
if ($Release) {
    $profileName = "release"
    $buildArgs += "--release"
}

function Reset-FfmpegCache {
    Write-Host "== build failed, resetting the ffmpeg-sys-next cache =="
    & cargo clean -p ffmpeg-sys-next 2>$null
    Get-ChildItem (Join-Path $Root "target\$profileName\build") -Filter 'ffmpeg-sys-next-*' -Directory -ErrorAction SilentlyContinue |
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    Get-ChildItem (Join-Path $Root "target\$profileName\.fingerprint") -Filter 'ffmpeg-sys-next-*' -Directory -ErrorAction SilentlyContinue |
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
}

$log = $null
& cargo @buildArgs
if ($LASTEXITCODE -ne 0) {
    Write-Host "FFMPEG_DIR=$env:FFMPEG_DIR"
    Write-Host "LIBCLANG_PATH=$env:LIBCLANG_PATH"
    Write-Host "VCPKG_ROOT=$env:VCPKG_ROOT"
    Reset-FfmpegCache
    $env:FFMPEG_DIR = Join-Path $Root "vcpkg_installed\$Triplet"
    $log = Join-Path $Root 'target\build-failure.log'
    Write-Host "== retrying with the workspace FFmpeg path, verbose log: $log =="
    & cargo @buildArgs -vv *> $log
}
if ($LASTEXITCODE -ne 0) {
    if ($log -and (Test-Path $log)) {
        Write-Host "== last build diagnostics =="
        Get-Content $log -Tail 120 | Select-String -Pattern 'error|panic|ffmpeg|libclang|pkg-config|not found' -Context 0,2 |
            Select-Object -Last 20
    }
    exit $LASTEXITCODE
}
$built = Join-Path $Root "target\$profileName\mediaa.exe"
$destDir = Join-Path $env:LOCALAPPDATA "mediaa\dev-run"
New-Item -ItemType Directory -Force $destDir | Out-Null
$dest = Join-Path $destDir "mediaa.exe"
Copy-Item $built $dest -Force
& $dest
exit $LASTEXITCODE
