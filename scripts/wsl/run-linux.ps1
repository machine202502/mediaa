param(
    [string]$Distro = 'Ubuntu-24.04',
    [switch]$InstallDeps,
    [switch]$Build
)
$ErrorActionPreference = 'Continue'
$root = (Resolve-Path "$PSScriptRoot\..\..").Path

if ($Build -or -not (Test-Path "$root\dist\linux\mediaa")) {
    & "$root\scripts\linux\build-docker.ps1"
}

$distros = (wsl.exe -l -q) -replace "`0", '' | Where-Object { $_.Trim() }
if ($distros -notcontains $Distro) {
    throw "WSL distro '$Distro' not found. Installed: $($distros -join ', '). Install one with: wsl --install $Distro"
}

function To-WslPath([string]$p) {
    (wsl.exe -d $Distro wslpath -a ($p -replace '\\', '/')).Trim()
}

if ($InstallDeps) {
    $deps = To-WslPath "$root\scripts\linux\install-runtime-deps.sh"
    wsl.exe -d $Distro -u root bash $deps
    if ($LASTEXITCODE -ne 0) { throw 'installing runtime deps failed' }
}

$bin = To-WslPath "$root\dist\linux\mediaa"
wsl.exe -d $Distro bash -lc "mkdir -p ~/.local/bin && cp '$bin' ~/.local/bin/mediaa && chmod +x ~/.local/bin/mediaa && ~/.local/bin/mediaa"
