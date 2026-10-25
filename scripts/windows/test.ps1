. "$PSScriptRoot\env-fast.ps1"
cargo fmt --all -- --check
if ($LASTEXITCODE -ne 0) { throw 'formatting check failed' }
cargo clippy --workspace --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'clippy failed' }
cargo test --workspace
if ($LASTEXITCODE -ne 0) { throw 'tests failed' }
if (Get-Command cargo-audit -ErrorAction SilentlyContinue) {
    cargo audit --file "$Root\Cargo.lock" --deny unsound --deny yanked
    if ($LASTEXITCODE -ne 0) { throw 'dependency audit failed' }
} else {
    Write-Warning 'cargo-audit is not installed; dependency audit skipped'
}
