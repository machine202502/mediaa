. "$PSScriptRoot\env-fast.ps1"
cargo check --workspace --all-targets --profile dev-check
if ($LASTEXITCODE -ne 0) { throw 'fast check failed' }
cargo test --workspace --profile test-fast
if ($LASTEXITCODE -ne 0) { throw 'fast tests failed' }
