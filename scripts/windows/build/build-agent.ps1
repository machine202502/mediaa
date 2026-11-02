. "$PSScriptRoot\env.ps1"
cargo build -p mediaa-app-agent
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
$exe = Join-Path $Root "target\debug\mediaa-agent.exe"
$size = [math]::Round((Get-Item $exe).Length / 1MB, 1)
Write-Host "done: target\debug\mediaa-agent.exe ($size MB, red icon)"