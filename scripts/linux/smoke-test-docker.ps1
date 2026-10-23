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
