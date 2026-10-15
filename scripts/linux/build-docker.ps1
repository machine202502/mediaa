<#
.SYNOPSIS
  Builds the Linux version of MediaA on Windows using Docker Desktop.

.EXAMPLE
  .\scripts\linux\build-docker.ps1              # -> dist\linux\mediaa
  .\scripts\linux\build-docker.ps1 -Test        # run the test suite on Linux
  .\scripts\linux\build-docker.ps1 -Jobs 4      # fewer parallel compile jobs (less RAM)

  The first run builds FFmpeg with vcpkg (~15-30 min); later runs reuse the Docker volumes
  mediaa-work / mediaa-cargo-registry / mediaa-vcpkg-cache.
#>
param([switch]$Test, [int]$Jobs = 8)
# Native tools (docker, cargo, vcpkg) write progress to stderr; with 'Stop' PowerShell 5.1 would
# turn that into errors when output is redirected. Failures are checked via $LASTEXITCODE.
$ErrorActionPreference = 'Continue'

$root = (Resolve-Path "$PSScriptRoot\..\..").Path
$image = 'mediaa-linux-builder'
$name = 'mediaa-linux-build'
$commit = (Get-Content "$root\scripts\vcpkg-commit.txt" -Raw).Trim()

docker build -t $image --build-arg "VCPKG_COMMIT=$commit" -f "$PSScriptRoot\Dockerfile" "$PSScriptRoot"
if ($LASTEXITCODE -ne 0) { throw 'docker build failed' }

$mode = if ($Test) { '--test' } else { 'build' }
docker rm -f $name 2>$null | Out-Null
# Detached + `logs -f`: a dropped client connection does not abort the build.
docker run -d --name $name `
  -v "${root}:/src" `
  -v mediaa-work:/work `
  -v mediaa-cargo-registry:/root/.cargo/registry `
  -v mediaa-vcpkg-cache:/root/.cache/vcpkg `
  -e MEDIAA_WORK=/work `
  -e VCPKG_MAX_CONCURRENCY=$Jobs `
  -e CARGO_BUILD_JOBS=$Jobs `
  $image bash /src/scripts/linux/build.sh $mode | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'docker run failed' }

docker logs -f $name
$code = [int](docker wait $name)
docker rm $name | Out-Null
if ($code -ne 0) { throw "Linux build failed (exit $code)" }
if (-not $Test) { Write-Host "Linux binary: $root\dist\linux\mediaa" }
