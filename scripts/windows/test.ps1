<#
.SYNOPSIS
  Runs the whole test suite (unit tests + real FFmpeg encode tests).
#>
. "$PSScriptRoot\env.ps1"
cargo test --workspace
if ($LASTEXITCODE -ne 0) { throw 'tests failed' }
