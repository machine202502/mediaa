param([switch]$Release)
. "$PSScriptRoot\env.ps1"
if ($Release) { cargo run --release -p mediaa } else { cargo run -p mediaa }
exit $LASTEXITCODE
