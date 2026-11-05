# Neue Version veröffentlichen: Versionsnummer setzen, committen, Tag pushen.
# GitHub baut dann automatisch TaskPulse-Setup.exe und hängt sie an das Release –
# installierte TaskPulse-Versionen finden das Update von selbst.
#
# Aufruf:  powershell -ExecutionPolicy Bypass -File veroeffentlichen.ps1 0.1.7 "Was ist neu"
param(
  [Parameter(Mandatory = $true)][string]$Version,
  [string]$Nachricht = ""
)
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Version bitte als 1.2.3 angeben" }
if (-not (git remote)) { throw "Noch kein GitHub-Repository verbunden (siehe README, Abschnitt 'Erstes Hochladen')" }
if (git tag --list "v$Version") { throw "Version $Version gibt es schon" }

$utf8 = New-Object System.Text.UTF8Encoding($false)
$toml = [IO.File]::ReadAllText("$PSScriptRoot\Cargo.toml")
$alt = [regex]::Match($toml, '(?m)^version = "([^"]+)"').Groups[1].Value
$toml = [regex]::Replace($toml, '(?m)^version = "[^"]+"', "version = `"$Version`"", 1)
[IO.File]::WriteAllText("$PSScriptRoot\Cargo.toml", $toml, $utf8)
$lock = [IO.File]::ReadAllText("$PSScriptRoot\Cargo.lock")
$lock = [regex]::Replace($lock, '(name = "taskpulse"\r?\nversion = ")[^"]+"', "`${1}$Version`"")
[IO.File]::WriteAllText("$PSScriptRoot\Cargo.lock", $lock, $utf8)

if ($Nachricht -eq "") { $Nachricht = "TaskPulse $Version" }
git add -A
git commit -m $Nachricht
git tag -a "v$Version" -m $Nachricht
git push
git push origin "v$Version"
"Version $Version ($alt → $Version) hochgeladen. Den Bau siehst du auf GitHub unter 'Actions', das fertige Release unter 'Releases'."
