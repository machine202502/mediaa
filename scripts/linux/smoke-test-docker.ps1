<#
.SYNOPSIS
  Starts the Linux build in a clean ubuntu:24.04 container on a virtual X server (no WSL needed)
  and saves a screenshot to dist\linux\smoke.png.

.EXAMPLE
  .\scripts\linux\smoke-test-docker.ps1
  .\scripts\linux\smoke-test-docker.ps1 -Lang ru_RU.UTF-8
#>
param([string]$Lang = 'en_US.UTF-8')
$ErrorActionPreference = 'Continue'
$root = (Resolve-Path "$PSScriptRoot\..\..").Path
if (-not (Test-Path "$root\dist\linux\mediaa")) { throw 'dist\linux\mediaa not found — run scripts\linux\build-docker.ps1 first' }

docker run --rm `
  -v "${root}:/src:ro" `
  -v "${root}\dist\linux:/dist" `
  -e "LANG=$Lang" `
  ubuntu:24.04 bash /src/scripts/linux/smoke-test.sh
if ($LASTEXITCODE -ne 0) { throw 'smoke test failed' }
