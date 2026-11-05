# Baut TaskPulse. Aufruf:  powershell -ExecutionPolicy Bypass -File build.ps1 [-Debug] [-Test]
# Ergebnis: target\release\TaskPulse.exe
param([switch]$Debug, [switch]$Test)
$ErrorActionPreference = "Stop"
# Rust GNU-Toolchain + GNU binutils (dlltool, as) aus MSYS2 unter %USERPROFILE%\.cargo\mingw-binutils.
# Die binutils werden für die raw-dylib-Importe einiger Crates gebraucht.
$env:PATH = "$env:USERPROFILE\.cargo\mingw-binutils\mingw64\bin;$env:USERPROFILE\.cargo\bin;$env:PATH"
Set-Location $PSScriptRoot
if ($Test) { cargo test --release; exit $LASTEXITCODE }
if ($Debug) { cargo build } else { cargo build --release }
exit $LASTEXITCODE
