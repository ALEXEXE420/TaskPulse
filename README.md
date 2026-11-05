<p align="center"><img src="docs/logo.png" width="96" alt="TaskPulse-Logo"></p>

# TaskPulse

Ein Task-Manager für Windows 10/11, geschrieben in Rust. Er zeigt GPU-Werte pro Prozess und hat eine dauerhafte Anzeige direkt in der Taskleiste.

![Taskleisten-Anzeige](docs/taskleiste_dunkel.png)

### ⬇ [TaskPulse-Setup.exe herunterladen](https://github.com/ALEXEXE420/TaskPulse/releases/latest/download/TaskPulse-Setup.exe)

Windows 10 (2004 oder neuer) und Windows 11. Ältere Versionen und Änderungen findest du unter [Releases](https://github.com/ALEXEXE420/TaskPulse/releases).

## Funktionen

- **Prozesse:** CPU, Arbeitsspeicher, Datenträger, **GPU-Auslastung, GPU-Engine, VRAM und VRAM-Anteil je Prozess**, gruppiert wie im Windows-Task-Manager, mit Farbskala.
- **Leistung:** Verlaufsgrafiken für CPU (auch je Kern), Arbeitsspeicher, Datenträger, Netzwerk und GPU (alle Engines).
- **Taskleisten-Anzeige**, die auch ohne Hauptfenster läuft:
  - Upload und Download, auf Wunsch mit Auslastung der Leitung in %
  - CPU-Auslastung, -Temperatur, -Takt und -Watt
  - CPU RAM
  - GPU-Auslastung, -Temperatur, -Takt und -Watt
  - GPU VRAM
  - Die Felder sind frei anzuordnen, die Anzeige lässt sich links oder rechts platzieren und verschieben, auch auf dem zweiten Monitor.
- **Hersteller:**
  - Grafikkarten von NVIDIA laufen über NVML (mit Watt), AMD und Intel über die Windows-Schnittstellen.
  - Für CPU-Temperatur und -Watt bei AMD (Ryzen) und Intel wird der Treiber [PawnIO](https://pawnio.eu/) gebraucht.
- **Weitere Funktionen:** Autostart, „immer mit Adminrechten“ ohne UAC-Abfrage, Installer und automatische Updates.

![Leistung – GPU](docs/leistung_gpu.png)

## Installation

1. **[TaskPulse-Setup.exe herunterladen](https://github.com/ALEXEXE420/TaskPulse/releases/latest/download/TaskPulse-Setup.exe)** und starten.
2. Optionen wählen, dann **Installieren** klicken. TaskPulse wird nach `C:\Program Files\TaskPulse` installiert.
3. Optional [PawnIO](https://pawnio.eu/) installieren. Erst dann zeigt TaskPulse CPU-Temperatur und CPU-Watt an.

Die Datei ist nicht digital signiert, deshalb warnt Windows SmartScreen beim ersten Start. Klicke dort auf „Weitere Informationen“ und dann auf „Trotzdem ausführen“.

Deinstallieren geht über *Einstellungen → Apps → TaskPulse*.

## Updates

TaskPulse sucht beim Start und danach alle 12 Stunden auf GitHub nach einem neuen Release.

- **Wenn ein neues Release da ist:**
  - Ist das Hauptfenster zu, installiert die Taskleisten-Anzeige das Update selbst und startet TaskPulse neu.
  - Ist das Hauptfenster offen, erscheint oben ein Hinweis mit **Jetzt installieren**.
- **Einstellungen:** Unter *Einstellungen → Updates* kannst du die automatische Suche abschalten oder von Hand nach Updates suchen.
- **Adminrechte:** Läuft TaskPulse ohne Adminrechte, fragt Windows beim Update einmal nach, weil `Program Files` geschützt ist.
- **Daten:** Das ist der einzige Internetzugriff von TaskPulse. Es wird keine Telemetrie gesendet.

## Selbst bauen

Du brauchst [Rust](https://rustup.rs/) (stable).

```powershell
cargo build --release
# Ergebnis: target\release\TaskPulse.exe
```

Mit der MSVC-Toolchain (Visual Studio Build Tools) reicht dieser Befehl. Für die GNU-Toolchain gibt es `build.ps1`, das die MinGW-binutils in den Pfad einträgt.

Hat die Datei „Setup“ im Namen, also zum Beispiel `TaskPulse-Setup.exe`, startet dieselbe EXE als Installer.

## Neue Version veröffentlichen

```powershell
powershell -ExecutionPolicy Bypass -File veroeffentlichen.ps1 0.1.7 "Kurz: was ist neu"
```

Das Skript setzt die Version in `Cargo.toml`, macht einen Commit und lädt den Tag `v0.1.7` hoch. Danach baut GitHub Actions (`.github/workflows/release.yml`) die Datei `TaskPulse-Setup.exe` und veröffentlicht sie als Release. Bereits installierte Versionen finden das Update dann von selbst.

Von Hand geht es so: In `Cargo.toml` die `version` erhöhen, committen und einen Tag `v<Version>` pushen. Tag und Version müssen übereinstimmen, sonst bricht der Bau ab.

## Projektaufbau

| Ordner | Inhalt |
|---|---|
| `src/sensors` | Messwerte: CPU (PDH, PawnIO), GPU (NVML, D3DKMT, Leistungsindikatoren), Prozesse, Netzwerk, Datenträger |
| `src/taskbar` | Taskleisten-Anzeige (Win32, Direct2D/DirectWrite) |
| `src/ui` | Hauptfenster (egui) |
| `src/installer.rs`, `src/updater.rs` | Installer und Updates über GitHub-Releases |
| `resources/icon` | Programmsymbol |
| `resources/pawnio` | PawnIO-Module für AMD und Intel (LGPL, siehe `COPYING`) |

## Lizenz

Für TaskPulse selbst ist noch keine Lizenz festgelegt. Die Dateien in `resources/pawnio` stammen aus [PawnIO.Modules](https://github.com/namazso/PawnIO.Modules) und stehen unter der LGPL 2.1 (`resources/pawnio/COPYING`).
