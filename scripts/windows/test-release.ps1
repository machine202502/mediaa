. "$PSScriptRoot\env.ps1"
cargo test --release --workspace
if ($LASTEXITCODE -ne 0) { throw 'release tests failed' }
cargo build --release -p mediaa
if ($LASTEXITCODE -ne 0) { throw 'release build failed' }
$binary = "$Root\target\release\mediaa.exe"
if (-not (Test-Path -LiteralPath $binary)) { throw 'release binary is missing' }
New-Item -ItemType Directory -Force "$Root\dist\windows" | Out-Null
Copy-Item -LiteralPath $binary -Destination "$Root\dist\windows\mediaa.exe" -Force
$size = [math]::Round((Get-Item -LiteralPath "$Root\dist\windows\mediaa.exe").Length / 1MB, 1)
Write-Host "release ready: dist\windows\mediaa.exe ($size MB)"
