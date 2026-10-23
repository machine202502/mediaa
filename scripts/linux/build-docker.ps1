param([switch]$Test, [int]$Jobs = 8)
$ErrorActionPreference = 'Continue'

$root = (Resolve-Path "$PSScriptRoot\..\..").Path
$image = 'mediaa-linux-builder'
$name = 'mediaa-linux-build'
$commit = (Get-Content "$root\scripts\vcpkg-commit.txt" -Raw).Trim()

docker build -t $image --build-arg "VCPKG_COMMIT=$commit" -f "$PSScriptRoot\Dockerfile" "$PSScriptRoot"
if ($LASTEXITCODE -ne 0) { throw 'docker build failed' }

$mode = if ($Test) { '--test' } else { 'build' }
docker rm -f $name 2>$null | Out-Null
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
