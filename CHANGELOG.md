# Änderungsprotokoll

Hier steht, was sich in jeder Version geändert hat. Neue Punkte kommen unter „Unveröffentlicht“;
beim Veröffentlichen (veroeffentlichen.ps1) wird daraus die neue Versionsnummer mit Datum.
Derselbe Text erscheint auf GitHub beim Release und in TaskPulse unter Einstellungen → Updates.

## Unveröffentlicht

## 0.1.7 – 2026-11-06

- Benchmark: Neuer Reiter zum Aufzeichnen – Start und Stopp von Hand, ein Messpunkt pro Sekunde. Danach (und schon während der Aufzeichnung) Graphen mit Punkten und Linien für Temperaturen (CPU, GPU, bei AMD je CCD), Auslastung (CPU, GPU, RAM, VRAM), Takt und Leistung in Watt, jeweils mit min/Ø/max. Die Achsen beginnen immer bei 0 (keine negativen Werte) und reichen nach oben knapp über den echten Höchstwert – der Graph wandert nicht und steigt nur, wenn die Werte wirklich steigen. Reihen lassen sich über die Legende ausblenden. Export als Log (Textdatei mit System, Spaltenerklärung, Zusammenfassung und allen Messpunkten – zum Hochladen bei einer KI, die daraus Diagramme oder eine Excel-Tabelle macht), „Log kopieren“ für den KI-Chat und CSV für Excel. Beim Speichern wählst du Ordner und Dateinamen selbst (Windows-Dialog „Speichern unter“, Vorschlag: Spiel und Datum); der zuletzt gewählte Ordner wird gemerkt.
- Benchmark: Bildrate (FPS) des Spiels im Vordergrund mit Frametime-Graph, Ø-FPS, 1 % Low und 0,1 % Low – auch in Log und CSV. Die Kennzahlen gelten für das Programm mit den meisten Bildern (z. B. nur der FurMark-Test, nicht sein Startfenster); weitere Programme im Vordergrund werden getrennt aufgeführt. Messung und Auswertung laufen mit erhöhter Priorität, damit sie auch unter Volllast (CPU-Burner) im Takt bleiben. Gemessen über die Grafik-Ereignisse von Windows (ETW, wie PresentMon/FrameView/CapFrameX) für DirectX 9–12, Vulkan und OpenGL, ohne sich ins Spiel einzuklinken (Anti-Cheat-freundlich). Braucht Adminrechte.
- Benchmark: 0,1 % Low hervorgehoben und bewertet (gleichmäßig / vereinzelte Mikroruckler / deutliche Hänger), wenn sie weit unter der 1 % Low liegt – z. B. 1 % Low 60 FPS, aber 0,1 % Low 15 FPS = spürbares kurzes Einfrieren. Dazu Ruckler-Zählung (Bilder länger als 2,5 × übliche Frametime, mindestens 20 ms) mit eigenem Graphen „Ruckler pro Sekunde“, auch in Log und CSV.
- Benchmark: Erklärung im Reiter, was TaskPulse während eines Spiels liest und was nicht.
- Oberfläche: Bei schmalem Fenster werden die Knöpfe oben rechts kürzer beschriftet, damit nichts überlappt.
- Prozesse: Knopf „Task beenden“ oben rechts in der Liste (wie im Task-Manager), dazu die Taste Entf. Rechtsklick → „Task beenden“ geht weiterhin.
- Leistung → CPU: Logische Prozessoren als breites Raster, gruppiert nach Aufbau der CPU – bei AMD nach CCD (Zen 2: CCX, X3D-CCD mit „3D V-Cache“ markiert, auch Threadripper/EPYC und mehrere Sockel), bei Intel nach P- und E-Kernen, bei Core-Ultra-Laptop-CPUs zusätzlich nach Low-Power-E-Kernen (LP-E). Je Gruppe Kerne, Threads, L3-Cache, Durchschnitt und Höchstwert. Die Erkennung kommt direkt von Windows, ohne Modellliste.
- Leistung → CPU: Jede Kern-Kachel zeigt den Takt und groß die Auslastung, darunter Watt und Kernspannung (mit Adminrechten und PawnIO; AMD: Watt je Kern und angeforderte VID, Intel: Spannung – Intel meldet keine Watt je Kern).
- ARM-Rechner (z. B. NVIDIA RTX Spark N1X, Snapdragon X): TaskPulse erkennt die ARM-CPU auch in der x64-Emulation und teilt die Kerne korrekt in P- und E-Kerne (beim N1X verschränkt angeordnet), statt fälschlich nach CCDs.
- Leistung → CPU: Temperaturen in Weiß – bei AMD je CCD in der CCD-Überschrift (gelesen wie im Linux-Treiber k10temp, für Zen bis Zen 5), bei Intel je Kern in der Kachel und der heißeste Kern in der P-/E-Überschrift. AMD meldet keine Temperatur je Kern.
- Leistung → CPU: Neuer dritter Reiter „CCD-Auslastung“ (AMD) bzw. „P/E-Kern-Auslastung“ (Intel) mit einer Verlaufsgrafik je Gruppe, Ø-Takt, Watt und Spannung. Hat die CPU nur eine Kernart bzw. ein CCD, erscheint der Reiter nicht.
- Leistung → CPU: Die gewählte Ansicht bleibt gespeichert.
- Leistung → Datenträger: Jeder Datenträger einzeln (Modell, Typ wie NVMe-SSD/SSD/HDD/USB, Kapazität, Laufwerke) mit aktiver Zeit und Übertragungsrate. „Datenträger (alle)“ bleibt und zeigt jetzt auch Lesen/Schreiben als Grafik.
- Leistung → GPU: Kartenhersteller (z. B. ASUS, MSI, Gigabyte), Treiberversion (bei NVIDIA in NVIDIA-Zählung, z. B. 617.42) und PCI-Kennungen.
- Updates: Was neu ist, steht jetzt bei jedem Release auf GitHub, nach einem Update oben im Hauptfenster („Was ist neu?“) und unter Einstellungen → Updates → Änderungsprotokoll.
- Updates: Lokal gebaute Entwicklerversionen tragen eine eigene Nummer (z. B. 0.1.6.1-Dev); das nächste Release auf GitHub ersetzt sie automatisch.
- Fehlerberichte: Bei einem Programmfehler oder Absturz schreibt TaskPulse sofort eine Datei mit Fehler, Codestelle und Aufrufkette (Ordner %APPDATA%\TaskPulse\Fehlerberichte). Abstürze, bei denen das nicht mehr geht, findet TaskPulse beim nächsten Start im Windows-Fehlerprotokoll, erstellt nachträglich einen Bericht und zeigt oben einen Hinweis. Kleinere Probleme (Sensor, Update, FPS) landen im Protokoll TaskPulse.log.
- Fehlerberichte: Einstellungen → Fehlerberichte → „Diagnosebericht erstellen“ fasst System, Sensoren, Einstellungen, Abstürze und Protokoll in einer Datei zum Weitergeben zusammen (auch per „TaskPulse.exe --diagnose“, falls das Fenster nicht mehr startet). Es wird nichts automatisch gesendet.
- Behoben: Mit Adminrechten stürzte TaskPulse beim Beenden ab (kaum sichtbar, aber jedes Mal ein Fehlerbericht in Windows).
- Behoben: Die Einstellungsdatei wird auch gelesen, wenn sie mit BOM gespeichert wurde.

## 0.1.6 – 2026-11-06

- Automatische Updates über GitHub, abschaltbar unter Einstellungen → Updates („Jetzt nach Updates suchen“ geht auch von Hand).
- Projekt auf GitHub; jede Version wird dort automatisch gebaut und als TaskPulse-Setup.exe veröffentlicht.
- Eigenes Programmsymbol (Logo) für EXE, Verknüpfungen, Fenster und Installer.
- Taskleiste: „MEM“ heißt jetzt „CPU RAM“, „GPU MEM“ heißt „GPU VRAM“.
- CPU RAM lässt sich wie GPU VRAM wahlweise in % oder in GB anzeigen.

## 0.1.5 – 2026-11-05

- Installer (TaskPulse-Setup.exe) mit Verknüpfungen, Autostart, „immer mit Adminrechten“ und Eintrag unter „Apps“ zum Deinstallieren.
- Unterstützung für Intel-CPUs (Temperatur und Watt über PawnIO) und für Grafikkarten von AMD und Intel (über Windows-Schnittstellen); Auswahl der Grafikkarte in den Einstellungen.
- Trennstrich in der Taskleiste zwischen Internet- und PC-Werten.
- CPU- und GPU-Watt in der Taskleiste.
- Einstellung „TaskPulse immer mit Administratorrechten starten“ (ohne UAC-Abfrage).
- Größere Schrift im ganzen Programm (einstellbar), GPU-Grafiken auf der Leistungsseite 2 × 3, besser lesbare Prozessliste, deutlicher Sortierpfeil.
- Erste Version: Prozesse mit GPU, GPU-Engine, VRAM und VRAM-Anteil, Leistungsseite, Taskleisten-Anzeige.
