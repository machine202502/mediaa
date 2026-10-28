$ErrorActionPreference = 'Continue'
$Root = (Resolve-Path "$PSScriptRoot\..\..").Path
$Triplet = 'x64-windows-static-release'
$VcpkgCommit = (Get-Content "$Root\scripts\vcpkg-commit.txt" -Raw).Trim()

function Find-Vcpkg {
    foreach ($c in @($env:VCPKG_ROOT, 'C:\vcpkg', "$Root\.tools\vcpkg")) {
        if ($c -and (Test-Path "$c\vcpkg.exe")) { return $c }
    }
    $dest = "$Root\.tools\vcpkg"
    Write-Host "== cloning vcpkg into $dest =="
    git clone https://github.com/microsoft/vcpkg $dest
    git -C $dest checkout $VcpkgCommit
    & "$dest\bootstrap-vcpkg.bat" -disableMetrics
    return $dest
}

function Find-LibClang {
    if ($env:LIBCLANG_PATH -and (Test-Path "$env:LIBCLANG_PATH\libclang.dll")) { return $env:LIBCLANG_PATH }
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $vs = & $vswhere -latest -products * -property installationPath
        if ($vs -and (Test-Path "$vs\VC\Tools\Llvm\x64\bin\libclang.dll")) { return "$vs\VC\Tools\Llvm\x64\bin" }
    }
    if (Test-Path "$env:ProgramFiles\LLVM\bin\libclang.dll") { return "$env:ProgramFiles\LLVM\bin" }
    throw 'libclang.dll not found: install the VS component "C++ Clang Compiler for Windows" or LLVM, or set LIBCLANG_PATH.'
}

if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
    throw 'cmake not found: install CMake and add it to PATH (llama.cpp is built with CMake).'
}
if (-not $env:VULKAN_SDK -or -not (Test-Path "$env:VULKAN_SDK\Lib\vulkan-1.lib")) {
    throw 'Vulkan SDK not found: install it and set VULKAN_SDK (llama.cpp Vulkan backend).'
}

$env:VCPKG_ROOT = Find-Vcpkg
$env:LIBCLANG_PATH = Find-LibClang
$env:FFMPEG_DIR = "$Root\vcpkg_installed\$Triplet"
$env:TMP = "$Root\target\tmp"
$env:TEMP = $env:TMP
New-Item -ItemType Directory -Force $env:TMP | Out-Null

if (-not (Test-Path "$env:FFMPEG_DIR\lib\avcodec.lib")) {
    Write-Host "== vcpkg: building static FFmpeg ($Triplet) =="
    & "$env:VCPKG_ROOT\vcpkg.exe" install --triplet $Triplet "--x-manifest-root=$Root" "--x-install-root=$Root\vcpkg_installed"
    if ($LASTEXITCODE -ne 0) { throw 'vcpkg install failed' }
}
Set-Location $Root
