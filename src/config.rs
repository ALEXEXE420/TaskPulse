//! Einstellungen als lokale JSON-Datei (%APPDATA%\TaskPulse\settings.json).
//! Keine Telemetrie, kein Internetzugriff.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

/// Felder der Taskleisten-Anzeige.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Field {
    Up,
    Down,
    CpuLoad,
    CpuTemp,
    CpuFreq,
    Mem,
    GpuTemp,
    GpuLoad,
    GpuFreq,
    GpuMem,
    CpuPower,
    GpuPower,
    GpuHotspot,
    GpuMemTemp,
}

impl Field {
    pub const ALL: [Field; 14] = [
        Field::Up,
        Field::Down,
        Field::CpuLoad,
        Field::CpuTemp,
        Field::CpuFreq,
        Field::Mem,
        Field::GpuTemp,
        Field::GpuLoad,
        Field::GpuFreq,
        Field::GpuMem,
        Field::CpuPower,
        Field::GpuPower,
        Field::GpuHotspot,
        Field::GpuMemTemp,
    ];

    /// Neuere Zusatzfelder, die anfangs ausgeblendet sind (die Anzeige soll nicht von selbst breiter werden)
    pub const OPTIONAL: [Field; 2] = [Field::GpuHotspot, Field::GpuMemTemp];

    pub fn label(self) -> &'static str {
        match self {
            Field::Up => "Upload (↑)",
            Field::Down => "Download (↓)",
            Field::CpuLoad => "CPU-Auslastung",
            Field::CpuTemp => "CPU-Temperatur",
            Field::CpuFreq => "CPU-Takt",
            Field::Mem => "CPU RAM (Arbeitsspeicher)",
            Field::GpuTemp => "GPU-Temperatur",
            Field::GpuLoad => "GPU-Auslastung",
            Field::GpuFreq => "GPU-Takt",
            Field::GpuMem => "GPU VRAM (Grafikspeicher)",
            Field::CpuPower => "CPU-Leistung (Watt)",
            Field::GpuPower => "GPU-Leistung (Watt)",
            Field::GpuHotspot => "GPU Hot-Spot (NVIDIA RTX 50, Admin)",
            Field::GpuMemTemp => "GPU-Speichertemperatur (NVIDIA RTX 30–50, Admin)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CpuFreqMode {
    /// Höchster aktueller Kerntakt (Standard).
    MaxCore,
    /// Durchschnitt aller Kerne.
    Average,
}

/// Ansicht auf Leistung → CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CpuView {
    /// Eine Grafik für die ganze CPU
    #[default]
    Total,
    /// Jeder logische Prozessor einzeln
    Logical,
    /// CCDs (AMD) bzw. P-/E-Kerne (Intel) zusammengefasst
    Groups,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpuMemMode {
    Percent,
    Gigabytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetUnit {
    /// Megabyte pro Sekunde (wie bisherige Anzeige).
    MBps,
    /// Megabit pro Sekunde.
    Mbitps,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartPage {
    Processes,
    Performance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemUnit {
    Auto,
    MB,
    GB,
}

/// Heatmap-Schwelle: ab `start` wird eingefärbt, bei `full` ist die Farbe voll.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Threshold {
    pub start: f64,
    pub full: f64,
}

impl Threshold {
    /// Intensität 0..1 für einen Wert.
    pub fn intensity(&self, v: f64) -> f32 {
        if !v.is_finite() || v <= self.start {
            return 0.0;
        }
        if self.full <= self.start {
            return 1.0;
        }
        (((v - self.start) / (self.full - self.start)).clamp(0.0, 1.0)) as f32
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Heatmap {
    pub enabled: bool,
    /// CPU in %
    pub cpu: Threshold,
    /// Arbeitsspeicher in MB
    pub mem_mb: Threshold,
    /// Datenträger in MB/s
    pub disk_mbps: Threshold,
    /// GPU in %
    pub gpu: Threshold,
    /// VRAM-Anteil in %
    pub vram_pct: Threshold,
}

impl Default for Heatmap {
    fn default() -> Self {
        Heatmap {
            enabled: true,
            cpu: Threshold { start: 0.5, full: 50.0 },
            mem_mb: Threshold { start: 100.0, full: 4000.0 },
            disk_mbps: Threshold { start: 0.1, full: 50.0 },
            gpu: Threshold { start: 0.5, full: 80.0 },
            vram_pct: Threshold { start: 1.0, full: 50.0 },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskbarConfig {
    pub enabled: bool,
    /// Felder in Anzeigereihenfolge (je zwei Felder bilden eine Spalte).
    pub fields: Vec<Field>,
    /// Ausgeblendete Felder.
    pub hidden: Vec<Field>,
    pub side: Side,
    /// Abstand in Pixeln (bei 100 % Skalierung) zur gewählten Seite.
    pub offset_px: i32,
    /// 0 = Haupt-Taskleiste, 1.. = weitere Monitore.
    pub monitor: usize,
    /// Als frei schwebendes Fenster statt in der Taskleiste (Rückfall).
    pub floating: bool,
    /// Position des schwebenden Fensters.
    pub float_x: i32,
    pub float_y: i32,
    pub interval_ms: u32,
    pub font_size_pt: f32,
    pub font_family: String,
    pub net_unit: NetUnit,
    pub gpu_mem_mode: GpuMemMode,
    /// CPU RAM in % oder belegt in GB.
    pub cpu_mem_mode: GpuMemMode,
    /// Leitungsauslastung in % hinter Up/Download.
    pub show_line_usage: bool,
    /// Trennstrich zwischen Internet- und PC-Werten.
    pub separator: bool,
}

impl Default for TaskbarConfig {
    fn default() -> Self {
        TaskbarConfig {
            enabled: true,
            fields: Field::ALL.to_vec(),
            hidden: Field::OPTIONAL.to_vec(),
            side: Side::Right,
            offset_px: 10,
            monitor: 0,
            floating: false,
            float_x: 100,
            float_y: 100,
            interval_ms: 1000,
            font_size_pt: 9.0,
            font_family: "Segoe UI".into(),
            net_unit: NetUnit::MBps,
            gpu_mem_mode: GpuMemMode::Percent,
            cpu_mem_mode: GpuMemMode::Percent,
            show_line_usage: true,
            separator: true,
        }
    }
}

impl TaskbarConfig {
    pub fn visible_fields(&self) -> Vec<Field> {
        self.fields.iter().copied().filter(|f| !self.hidden.contains(f)).collect()
    }

    pub fn interval(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.interval_ms.clamp(250, 5000) as u64)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub taskbar: TaskbarConfig,
    /// Gebuchte Leitung in MBit/s (0 = nicht hinterlegt).
    pub line_down_mbit: f64,
    pub line_up_mbit: f64,
    /// GUID des Internet-Adapters; leer = automatisch.
    pub net_adapter: String,
    pub cpu_freq_mode: CpuFreqMode,
    /// CPU-Temperatur über PawnIO lesen (abschaltbar, falls Antivirus/Anti-Cheat stört).
    pub use_pawnio: bool,
    /// Temperatur der Speicherriegel über den SMBus lesen (nur mit PawnIO/Adminrechten).
    pub ram_sensors: bool,
    pub heatmap: Heatmap,
    pub mem_unit: MemUnit,
    pub decimals: usize,
    /// Spalten der Prozessseite
    pub columns: Columns,
    pub always_on_top: bool,
    pub group_processes: bool,
    pub autostart: bool,
    pub start_page: StartPage,
    /// Grundschriftgröße des Hauptfensters.
    pub ui_font_size: f32,
    /// TaskPulse immer mit Adminrechten starten (über eine geplante Aufgabe, ohne UAC-Abfrage).
    pub always_admin: bool,
    /// LUID der angezeigten Grafikkarte; leer = automatisch (größter Grafikspeicher).
    pub gpu_adapter: String,
    /// Leistung → CPU: gewählte Ansicht (bleibt gespeichert).
    pub cpu_view: CpuView,
    /// Ordner, in dem Benchmark-Logs zuletzt gespeichert wurden (leer = Dokumente).
    pub bench_dir: String,
    /// Updates von GitHub automatisch suchen und installieren.
    pub auto_update: bool,
    /// GitHub-Repository "Besitzer/Name" für Updates; leer = das eingebaute.
    pub update_repo: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Columns {
    pub pid: bool,
    pub cpu: bool,
    pub mem: bool,
    pub disk: bool,
    pub gpu: bool,
    pub gpu_engine: bool,
    pub vram: bool,
    pub vram_share: bool,
    pub shared_gpu_mem: bool,
}

impl Default for Columns {
    fn default() -> Self {
        Columns {
            pid: false,
            cpu: true,
            mem: true,
            disk: true,
            gpu: true,
            gpu_engine: true,
            vram: true,
            vram_share: true,
            shared_gpu_mem: false,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            taskbar: TaskbarConfig::default(),
            line_down_mbit: 0.0,
            line_up_mbit: 0.0,
            net_adapter: String::new(),
            cpu_freq_mode: CpuFreqMode::MaxCore,
            use_pawnio: true,
            ram_sensors: true,
            heatmap: Heatmap::default(),
            mem_unit: MemUnit::Auto,
            decimals: 1,
            columns: Columns::default(),
            always_on_top: false,
            group_processes: true,
            autostart: false,
            start_page: StartPage::Processes,
            ui_font_size: 16.0,
            always_admin: false,
            gpu_adapter: String::new(),
            cpu_view: CpuView::Total,
            bench_dir: String::new(),
            auto_update: true,
            update_repo: String::new(),
        }
    }
}

impl Config {
    pub fn dir() -> PathBuf {
        if let Some(p) = std::env::var_os("TASKPULSE_CONFIG_DIR") {
            return PathBuf::from(p);
        }
        // Windows: %APPDATA%\TaskPulse; Linux: $XDG_CONFIG_HOME/TaskPulse bzw. ~/.config/TaskPulse
        #[cfg(windows)]
        let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        #[cfg(not(windows))]
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("TaskPulse")
    }

    pub fn path() -> PathBuf {
        Self::dir().join("settings.json")
    }

    /// Lädt die Einstellungen; fehlende oder defekte Datei ergibt Standardwerte.
    pub fn load() -> Config {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &std::path::Path) -> Config {
        match std::fs::read_to_string(path) {
            // Von Hand (z. B. mit PowerShell) gespeicherte Dateien beginnen oft mit einem BOM
            Ok(s) => serde_json::from_str::<Config>(s.trim_start_matches('\u{feff}')).map(|c| c.sanitized()).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let r = self.save_to(&Self::path());
        if let Err(e) = &r {
            crate::diag::log(format!("Einstellungen nicht gespeichert: {e}"));
        }
        r
    }

    pub fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // Erst in temporäre Datei schreiben, dann umbenennen: die Taskleisten-Anzeige liest nie halbe Dateien.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }

    pub fn modified() -> Option<SystemTime> {
        std::fs::metadata(Self::path()).and_then(|m| m.modified()).ok()
    }

    /// Repariert unplausible Werte (z. B. von Hand bearbeitete Datei).
    pub fn sanitized(mut self) -> Config {
        let tb = &mut self.taskbar;
        tb.interval_ms = tb.interval_ms.clamp(250, 5000);
        if !(5.0..=24.0).contains(&tb.font_size_pt) {
            tb.font_size_pt = 9.0;
        }
        // Jedes Feld genau einmal
        let mut seen = Vec::new();
        tb.fields.retain(|f| {
            if seen.contains(f) {
                false
            } else {
                seen.push(*f);
                true
            }
        });
        for f in Field::ALL {
            if !tb.fields.contains(&f) {
                tb.fields.push(f);
                // Neu hinzugekommene Zusatzfelder erst einmal ausgeblendet
                if Field::OPTIONAL.contains(&f) && !tb.hidden.contains(&f) {
                    tb.hidden.push(f);
                }
            }
        }
        tb.offset_px = tb.offset_px.max(0);
        if !self.line_down_mbit.is_finite() || self.line_down_mbit < 0.0 {
            self.line_down_mbit = 0.0;
        }
        if !self.line_up_mbit.is_finite() || self.line_up_mbit < 0.0 {
            self.line_up_mbit = 0.0;
        }
        self.decimals = self.decimals.min(3);
        if !(11.0..=26.0).contains(&self.ui_font_size) {
            self.ui_font_size = 16.0;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_file_with_bom_and_keeps_cpu_view() {
        let dir = std::env::temp_dir().join(format!("taskpulse_bom_{}", std::process::id()));
        let p = dir.join("settings.json");
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = Config::default();
        c.cpu_view = CpuView::Groups;
        let json = serde_json::to_string(&c).unwrap();
        std::fs::write(&p, format!("\u{feff}{json}")).unwrap();
        assert_eq!(Config::load_from(&p).cpu_view, CpuView::Groups);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("taskpulse_cfg_test_{}", std::process::id()));
        let p = dir.join("settings.json");
        let mut c = Config::default();
        c.line_down_mbit = 250.0;
        c.taskbar.side = Side::Left;
        c.taskbar.hidden.push(Field::CpuTemp);
        c.save_to(&p).unwrap();
        let l = Config::load_from(&p);
        assert_eq!(c, l);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_fields_use_defaults() {
        let c: Config = serde_json::from_str(r#"{"line_up_mbit": 40}"#).unwrap();
        assert_eq!(c.line_up_mbit, 40.0);
        assert_eq!(c.taskbar.interval_ms, 1000);
        assert_eq!(c.taskbar.fields.len(), Field::ALL.len());
    }

    #[test]
    fn new_fields_start_hidden() {
        // Alte Einstellungen ohne die neuen Felder: werden angehängt, aber ausgeblendet
        let mut c = Config::default();
        c.taskbar.fields.retain(|f| !Field::OPTIONAL.contains(f));
        c.taskbar.hidden.clear();
        let c = c.sanitized();
        assert!(c.taskbar.fields.contains(&Field::GpuHotspot));
        assert!(!c.taskbar.visible_fields().contains(&Field::GpuHotspot));
        assert!(!c.taskbar.visible_fields().contains(&Field::GpuMemTemp));
        assert!(!Config::default().taskbar.visible_fields().contains(&Field::GpuHotspot));
    }

    #[test]
    fn sanitize_repairs_fields() {
        let mut c = Config::default();
        c.taskbar.fields = vec![Field::Mem, Field::Mem, Field::Up];
        c.taskbar.interval_ms = 10;
        let c = c.sanitized();
        assert_eq!(c.taskbar.fields.len(), Field::ALL.len());
        assert_eq!(c.taskbar.fields[0], Field::Mem);
        assert_eq!(c.taskbar.fields[1], Field::Up);
        assert_eq!(c.taskbar.interval_ms, 250);
    }

    #[test]
    fn heatmap_intensity() {
        let t = Threshold { start: 10.0, full: 30.0 };
        assert_eq!(t.intensity(5.0), 0.0);
        assert_eq!(t.intensity(20.0), 0.5);
        assert_eq!(t.intensity(100.0), 1.0);
        assert_eq!(t.intensity(f64::NAN), 0.0);
    }
}
