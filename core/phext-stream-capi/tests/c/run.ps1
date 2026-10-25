# Builds the static library and runs test_capi.c with MSVC. Usage: pwsh core/phext-stream-capi/tests/c/run.ps1
$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot/../../../.."
cargo build --release -p phext-stream-capi --manifest-path "$root/Cargo.toml"
if ($LASTEXITCODE) { exit $LASTEXITCODE }

$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -property installationPath
$vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'
$out = Join-Path $root 'target\c-test'
New-Item -ItemType Directory -Force $out | Out-Null

$src = Join-Path $PSScriptRoot 'test_capi.c'
$inc = Join-Path $root 'core\phext-stream-capi\include'
$lib = Join-Path $root 'target\release\phext_stream_c.lib'
# Rust staticlibs on MSVC need these system libraries
$sys = 'ws2_32.lib userenv.lib ntdll.lib bcrypt.lib advapi32.lib kernel32.lib'
cmd /c "`"$vcvars`" >nul && cl /nologo /W4 /I`"$inc`" `"$src`" /Fo`"$out\\`" /Fe`"$out\test_capi.exe`" /link `"$lib`" $sys"
if ($LASTEXITCODE) { exit $LASTEXITCODE }
& "$out\test_capi.exe"
exit $LASTEXITCODE
