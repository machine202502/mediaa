$ErrorActionPreference = 'Continue'
$env:CARGO_TARGET_DIR = 'C:\shared-data\reps\.old\gifcap2\target'
. "$PSScriptRoot\scripts\windows\env.ps1"
cargo build -p mediaa
exit $LASTEXITCODE
