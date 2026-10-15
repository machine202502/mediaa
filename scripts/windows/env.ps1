<#
  Shared setup for the Windows scripts (dot-source it: `. $PSScriptRoot\env.ps1`).
  - finds or clones vcpkg (pinned commit from scripts/vcpkg-commit.txt) -> $env:VCPKG_ROOT
  - finds libclang (needed by bindgen for FFmpeg headers)       -> $env:LIBCLANG_PATH
  - finds the Vulkan SDK (llama.cpp GPU backend)                 -> $env:VULKAN_SDK
  - builds static FFmpeg into vcpkg_installed\ (first run ~10-20 min, then cached)
#>
# Native tools (docker, cargo, vcpkg) write progress to stderr; with 'Stop' PowerShell 5.1 would
# turn that into errors when output is redirected. Failures are checked via $LASTEXITCODE.
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

$env:VCPKG_ROOT = Find-Vcpkg
$env:LIBCLANG_PATH = Find-LibClang
# The installer sets VULKAN_SDK machine-wide; shells started before it do not see it yet.
if (-not $env:VULKAN_SDK) { $env:VULKAN_SDK = [Environment]::GetEnvironmentVariable('VULKAN_SDK', 'Machine') }
if (-not $env:VULKAN_SDK) { throw 'Vulkan SDK not found: winget install --id KhronosGroup.VulkanSDK -e' }
$env:FFMPEG_DIR = "$Root\vcpkg_installed\$Triplet"

if (-not (Test-Path "$env:FFMPEG_DIR\lib\avcodec.lib")) {
    Write-Host "== vcpkg: building static FFmpeg ($Triplet) =="
    & "$env:VCPKG_ROOT\vcpkg.exe" install --triplet $Triplet "--x-manifest-root=$Root" "--x-install-root=$Root\vcpkg_installed"
    if ($LASTEXITCODE -ne 0) { throw 'vcpkg install failed' }
}
Set-Location $Root
