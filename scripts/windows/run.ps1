<#
.SYNOPSIS
  Runs MediaA from source for testing (debug build, console logs visible). -Release for an optimized run.
#>
param([switch]$Release)
. "$PSScriptRoot\env.ps1"
if ($Release) { cargo run --release -p mediaa } else { cargo run -p mediaa }
