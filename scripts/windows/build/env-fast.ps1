. "$PSScriptRoot\env.ps1"
$sysroot = (& rustc --print sysroot).Trim()
$lld = Join-Path $sysroot 'lib\rustlib\x86_64-pc-windows-msvc\bin\rust-lld.exe'
if (-not (Test-Path -LiteralPath $lld)) {
    throw "rust-lld.exe not found at $lld"
}
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = $lld
