<#
.SYNOPSIS
  Release build of MediaA for Windows -> dist\windows\mediaa.exe
  (a single file; the app links FFmpeg and the C runtime statically).
#>
. "$PSScriptRoot\env.ps1"
cargo build --release -p mediaa
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
New-Item -ItemType Directory -Force "$Root\dist\windows" | Out-Null
Copy-Item "$Root\target\release\mediaa.exe" "$Root\dist\windows\mediaa.exe" -Force
$size = [math]::Round((Get-Item "$Root\dist\windows\mediaa.exe").Length / 1MB, 1)
Write-Host "done: dist\windows\mediaa.exe ($size MB)"
