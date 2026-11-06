# Neue Version veröffentlichen: Versionsnummer setzen, Änderungsprotokoll abschließen, committen, Tag pushen.
# GitHub baut dann automatisch TaskPulse-Setup.exe und hängt sie mit den Änderungen an das Release –
# installierte TaskPulse-Versionen finden das Update von selbst.
#
# Vorher in CHANGELOG.md unter "## Unveröffentlicht" eintragen, was sich geändert hat.
# Aufruf:  powershell -ExecutionPolicy Bypass -File veroeffentlichen.ps1 0.1.7
param(
  [Parameter(Mandatory = $true)][string]$Version,
  [string]$Nachricht = ""
)
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Version bitte als 1.2.3 angeben" }
if (-not (git remote)) { throw "Noch kein GitHub-Repository verbunden" }
if (git tag --list "v$Version") { throw "Version $Version gibt es schon" }

$utf8 = New-Object System.Text.UTF8Encoding($false)
$heute = Get-Date -Format "yyyy-MM-dd"
# Dieser Ordner und – falls vorhanden – der Entwicklungsordner daneben bleiben gleich
$ordner = @($PSScriptRoot)
$dev = Join-Path $PSScriptRoot "..\TaskPulse"
if (Test-Path (Join-Path $dev "Cargo.toml")) { $ordner += (Resolve-Path $dev).Path }

# Änderungsprotokoll prüfen
$log = [IO.File]::ReadAllText("$PSScriptRoot\CHANGELOG.md", $utf8)
$m = [regex]::Match($log, '(?ms)^## Unveröffentlicht[^\r\n]*\r?\n(.*?)(?=^## |\z)')
if (-not $m.Success -or -not $m.Groups[1].Value.Trim()) {
  throw "In CHANGELOG.md steht unter '## Unveröffentlicht' nichts – bitte erst eintragen, was sich geändert hat."
}
$punkte = $m.Groups[1].Value.Trim()

foreach ($o in $ordner) {
  $toml = [IO.File]::ReadAllText("$o\Cargo.toml", $utf8)
  $alt = [regex]::Match($toml, '(?m)^version = "([^"]+)"').Groups[1].Value
  # Nur die erste Zeile "version = ..." (die des Pakets), nicht die der Bibliotheken
  $toml = ([regex]'(?m)^version = "[^"]+"').Replace($toml, "version = `"$Version`"", 1)
  [IO.File]::WriteAllText("$o\Cargo.toml", $toml, $utf8)
  if (Test-Path "$o\Cargo.lock") {
    $lock = [IO.File]::ReadAllText("$o\Cargo.lock", $utf8)
    $lock = [regex]::Replace($lock, '(name = "taskpulse"\r?\nversion = ")[^"]+"', "`${1}$Version`"")
    [IO.File]::WriteAllText("$o\Cargo.lock", $lock, $utf8)
  }
  if (Test-Path "$o\CHANGELOG.md") {
    $c = [IO.File]::ReadAllText("$o\CHANGELOG.md", $utf8)
    # "## Unveröffentlicht" wird zur neuen Version; darüber ein leerer Abschnitt für die nächsten Änderungen
    $c = ([regex]'(?m)^## Unveröffentlicht[^\r\n]*').Replace($c, "## Unveröffentlicht`n`n## $Version – $heute", 1)
    [IO.File]::WriteAllText("$o\CHANGELOG.md", $c, $utf8)
  }
}

if ($Nachricht -eq "") { $Nachricht = "TaskPulse $Version`n`n$punkte" }
git add -A
git commit -m $Nachricht
git tag -a "v$Version" -m "TaskPulse $Version"
git push
git push origin "v$Version"
# Update-Bericht "TaskPulse <Version> Update.pdf" im Ordner PDF (nur auf dem Entwicklungsrechner)
$bericht = Join-Path $PSScriptRoot "..\Test\update_pdf_erzeugen.py"
if (Test-Path $bericht) {
  try { python $bericht $Version } catch { "Update-Bericht konnte nicht erzeugt werden: $_" }
}
"Version $alt → $Version hochgeladen. Den Bau siehst du auf GitHub unter 'Actions', das fertige Release (mit den Änderungen) unter 'Releases'."
