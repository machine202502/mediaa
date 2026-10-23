. "$PSScriptRoot\env.ps1"
cargo test --workspace
if ($LASTEXITCODE -ne 0) { throw 'tests failed' }
