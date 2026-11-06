//! Seite "Benchmark": Temperaturen, Takt, Auslastung und Leistung im Sekundentakt aufzeichnen
//! und als Graphen (Punkte mit Linien verbunden) anzeigen. Start und Stopp von Hand.

use super::{Frame, TaskPulseApp};
use crate::format;
use eframe::egui::{self, Color32, RichText};
use crate::sensors::fps;
use egui_plot::{Legend, Line, Plot, PlotPoints, Points};
use std::time::Instant;

/// Ein Messpunkt.
#[derive(Clone, Default)]
pub struct Sample {
    /// Sekunden seit dem Start
    pub t: f64,
    pub cpu_load: Option<f64>,
    pub cpu_temp: Option<f64>,
    pub cpu_ghz: Option<f64>,
    pub cpu_w: Option<f64>,
    pub gpu_load: Option<f64>,
    pub gpu_temp: Option<f64>,
    pub gpu_ghz: Option<f64>,
    pub gpu_w: Option<f64>,
    pub ram: Option<f64>,
    pub vram: Option<f64>,
    /// AMD: Temperatur je CCD
    pub ccd: Vec<f64>,
    /// Bildrate des Spiels in dieser Sekunde
    pub fps: Option<f64>,
    /// Durchschnittliche und längste Frametime dieser Sekunde, ms
    pub ft_avg: Option<f64>,
    pub ft_max: Option<f64>,
    /// Ruckler in dieser Sekunde (Bilder > 2,5 × übliche Frametime und ≥ 20 ms)
    pub stutters: Option<f64>,
}

impl Sample {
    fn from_frame(f: &Frame, t: f64) -> Sample {
        let b = &f.basic;
        Sample {
            t,
            cpu_load: b.cpu.load_pct,
            cpu_temp: b.cpu_temp_c,
            cpu_ghz: b.cpu_freq_mhz.map(|m| m / 1000.0),
            cpu_w: b.cpu_power_w,
            gpu_load: f.gpu.total_pct.or(b.gpu.load_pct),
            gpu_temp: b.gpu.temp_c,
            gpu_ghz: b.gpu.clock_mhz.map(|m| m / 1000.0),
            gpu_w: b.gpu.power_w,
            ram: b.mem.used_pct,
            vram: match (f.vram_used, f.vram_total) {
                (Some(u), Some(t)) if t > 0.0 => Some(u / t * 100.0),
                _ => None,
            },
            ccd: f.ccd_temps.clone(),
            ..Default::default()
        }
    }
}

/// Angaben zum System, beim Start der Aufzeichnung festgehalten (für das Log).
#[derive(Clone, Default)]
pub struct SystemInfo {
    pub started: String,
    pub cpu: String,
    pub gpu: String,
    pub ram_gb: f64,
    pub admin: bool,
}

/// Spiel bzw. Programm, dessen Bildrate gemessen wird.
#[derive(Clone, Default)]
pub struct Game {
    pub pid: u32,
    pub name: String,
}

#[derive(Default)]
pub struct BenchState {
    pub recording: bool,
    start: Option<Instant>,
    info: SystemInfo,
    pub samples: Vec<Sample>,
    last_seq: u64,
    last_t: f64,
    message: Option<String>,
    /// laufende FPS-Messung (ETW)
    fps: Option<fps::FpsTrace>,
    pub fps_error: Option<String>,
    /// Spiel, das gerade gemessen wird; alle Spiele dieser Aufzeichnung
    pub game: Option<Game>,
    pub games: Vec<String>,
    /// Alle Frametimes der Aufzeichnung (ms) – für 1 % / 0,1 % Low
    pub frametimes: Vec<f64>,
    /// Frametimes je Programm – die Kennzahlen gelten für das Programm mit den meisten Bildern
    /// (sonst würden z. B. das Startfenster eines Benchmarks mit 6 FPS die Lows verfälschen)
    pub game_frames: Vec<(String, Vec<f64>)>,
    /// laufender „Speichern unter“-Dialog
    save_job: Option<SaveJob>,
}

/// Das Programm mit den meisten Bildern (= das eigentlich gemessene Spiel) und seine Frametimes.
pub fn main_game(frames: &[(String, Vec<f64>)]) -> Option<(&str, &[f64])> {
    frames.iter().max_by_key(|(_, v)| v.len()).map(|(n, v)| (n.as_str(), v.as_slice()))
}

impl BenchState {
    fn begin(&mut self) {
        self.samples.clear();
        self.frametimes.clear();
        self.game_frames.clear();
        self.games.clear();
        self.game = None;
        self.start = Some(Instant::now());
        self.last_t = 0.0;
        self.last_seq = 0;
        self.recording = true;
        self.message = None;
        self.fps = None;
        match fps::FpsTrace::start() {
            Ok(t) => {
                self.fps = Some(t);
                self.fps_error = None;
            }
            Err(e) => {
                if crate::winutil::is_elevated() {
                    crate::diag::log(format!("FPS-Messung nicht gestartet: {e}"));
                }
                self.fps_error = Some(e)
            }
        }
    }

    pub fn end(&mut self) {
        self.recording = false;
        if let Some(t) = self.fps.take() {
            t.stop();
        }
    }
}

/// Fenster im Vordergrund → Prozess-ID
fn foreground_pid() -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    let mut pid = 0u32;
    unsafe {
        let h = GetForegroundWindow();
        if !h.is_invalid() {
            GetWindowThreadProcessId(h, Some(&mut pid));
        }
    }
    pid
}

/// Welches Programm wird gemessen? Das Fenster im Vordergrund, wenn es Bilder ausgibt, sonst das
/// bisherige Spiel, sonst das Programm mit der höchsten Bildrate.
pub fn pick_game(snap: &[fps::FpsSample], foreground: u32, current: Option<u32>) -> Option<usize> {
    let pos = |pid: u32| snap.iter().position(|x| x.pid == pid);
    pos(foreground).or_else(|| current.and_then(pos)).or(if snap.is_empty() { None } else { Some(0) })
}

/// Höchstens 12 Stunden im Sekundentakt.
const MAX_SAMPLES: usize = 12 * 3600;

const C_CPU: Color32 = Color32::from_rgb(17, 125, 187);
const C_GPU: Color32 = Color32::from_rgb(10, 160, 130);
const C_RAM: Color32 = Color32::from_rgb(160, 60, 200);
const C_VRAM: Color32 = Color32::from_rgb(230, 150, 40);
const C_FPS: Color32 = Color32::from_rgb(120, 200, 80);
const C_FT_MAX: Color32 = Color32::from_rgb(230, 80, 80);
const C_CCD: [Color32; 4] = [
    Color32::from_rgb(90, 170, 255),
    Color32::from_rgb(240, 110, 90),
    Color32::from_rgb(250, 200, 70),
    Color32::from_rgb(140, 220, 120),
];

impl BenchState {
    /// `--benchmark-start`: Aufzeichnung gleich beim Öffnen starten (Tests, Bildschirmfotos).
    pub fn from_args() -> BenchState {
        let mut st = BenchState::default();
        if std::env::args().any(|a| a == "--benchmark-start") {
            st.begin();
        }
        st
    }
}

fn system_info(f: &Frame, admin: bool) -> SystemInfo {
    SystemInfo {
        started: now_text(),
        cpu: crate::sensors::cputemp::cpu_info().brand,
        gpu: f.basic.gpu.name.clone().or_else(|| f.adapters.first().map(|a| a.name.clone())).unwrap_or_default(),
        ram_gb: f.basic.mem.total_bytes / 1073741824.0,
        admin,
    }
}

/// Wird bei jedem Bild aufgerufen: neue Messung übernehmen (etwa jede Sekunde).
pub fn record(app: &mut TaskPulseApp) {
    let st = &mut app.bench;
    if !st.recording || app.frame.seq == st.last_seq {
        return;
    }
    if st.samples.is_empty() {
        st.info = system_info(&app.frame, app.admin);
    }
    st.last_seq = app.frame.seq;
    let t = st.start.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
    // Sekundenabstand: höchstens ein Punkt je ~0,9 s
    if !st.samples.is_empty() && t - st.last_t < 0.9 {
        return;
    }
    st.last_t = t;
    // Auf 0,1 s genau: unter Volllast kommen Messungen nicht exakt im Sekundentakt – gerundet auf
    // ganze Sekunden entstanden sonst doppelte bzw. fehlende Sekunden
    let mut sample = Sample::from_frame(&app.frame, (t * 10.0).round() / 10.0);
    if st.fps.is_some() {
        // eigene Prozesse und den Desktop-Fenstermanager nicht als Spiel werten
        let mut exclude: Vec<u32> = app.frame.procs.iter().filter(|p| p.image.eq_ignore_ascii_case("dwm.exe") || p.image.eq_ignore_ascii_case("TaskPulse.exe")).map(|p| p.pid).collect();
        exclude.push(std::process::id());
        let snap = fps::snapshot(&exclude);
        if let Some(i) = pick_game(&snap, foreground_pid(), st.game.as_ref().map(|g| g.pid)) {
            let g = &snap[i];
            let name = app.frame.procs.iter().find(|p| p.pid == g.pid).map(|p| if p.display.is_empty() { p.image.clone() } else { p.display.clone() }).unwrap_or_else(|| format!("PID {}", g.pid));
            if !st.games.contains(&name) {
                st.games.push(name.clone());
            }
            st.game = Some(Game { pid: g.pid, name });
            sample.fps = Some(g.fps);
            if !g.frametimes_ms.is_empty() {
                sample.ft_avg = Some(g.frametimes_ms.iter().sum::<f64>() / g.frametimes_ms.len() as f64);
                sample.ft_max = g.frametimes_ms.iter().copied().reduce(f64::max);
                st.frametimes.extend_from_slice(&g.frametimes_ms);
                let gname = st.game.as_ref().map(|x| x.name.clone()).unwrap_or_default();
                let pos = match st.game_frames.iter().position(|(n, _)| *n == gname) {
                    Some(p) => p,
                    None => {
                        st.game_frames.push((gname, Vec::new()));
                        st.game_frames.len() - 1
                    }
                };
                st.game_frames[pos].1.extend_from_slice(&g.frametimes_ms);
                // Grenze aus den bisherigen Bildern dieses Programms (übliche Frametime × 2,5, mindestens 20 ms)
                if let Some((_, limit)) = fps::stutters(&st.game_frames[pos].1) {
                    sample.stutters = Some(g.frametimes_ms.iter().filter(|x| **x > limit).count() as f64);
                }
            }
        } else if !st.samples.is_empty() && st.samples.len() % 10 == 5 && fps::event_count() == 0 {
            // Nach einigen Sekunden noch kein einziges Grafik-Ereignis: protokollieren (einmalig je 10 s)
            if st.samples.len() == 5 {
                crate::diag::log("FPS: nach 5 s noch keine Grafik-Ereignisse empfangen");
            }
        }
    }
    // Nur für Bildschirmfotos ohne Adminrechte: TASKPULSE_DEMO_FPS=1 setzt Beispiel-Frametimes ein
    // (~60 FPS, alle 12 s ein Hänger von 66 ms)
    if st.fps.is_none() && std::env::var_os("TASKPULSE_DEMO_FPS").is_some() {
        let sec = st.samples.len();
        let mut ft: Vec<f64> = (0..60).map(|i| 16.0 + ((i * 7 + sec * 3) % 5) as f64 * 0.4).collect();
        if sec % 12 == 5 {
            ft[30] = 66.0;
            ft.truncate(57);
        }
        st.frametimes.extend_from_slice(&ft);
        if st.game_frames.is_empty() {
            st.game_frames.push(("Beispielspiel".into(), Vec::new()));
        }
        st.game_frames[0].1.extend_from_slice(&ft);
        sample.fps = Some(ft.len() as f64);
        sample.ft_avg = Some(ft.iter().sum::<f64>() / ft.len() as f64);
        sample.ft_max = ft.iter().copied().reduce(f64::max);
        sample.stutters = fps::stutters(&st.frametimes).map(|(_, l)| ft.iter().filter(|x| **x > l).count() as f64);
        st.game = Some(Game { pid: 0, name: "Beispielspiel".into() });
        if st.games.is_empty() {
            st.games.push("Beispielspiel".into());
        }
    }
    st.samples.push(sample);
    if st.samples.len() >= MAX_SAMPLES {
        st.end();
        st.message = Some("Aufzeichnung nach 12 Stunden automatisch beendet.".into());
    }
}

fn mmss(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// (min, Ø, max) einer Reihe
pub fn stats(v: &[f64]) -> Option<(f64, f64, f64)> {
    if v.is_empty() {
        return None;
    }
    let min = v.iter().copied().fold(f64::INFINITY, f64::min);
    let max = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Some((min, v.iter().sum::<f64>() / v.len() as f64, max))
}

struct Series {
    name: String,
    color: Color32,
    points: Vec<[f64; 2]>,
}

fn series(samples: &[Sample], name: &str, color: Color32, get: impl Fn(&Sample) -> Option<f64>) -> Option<Series> {
    let points: Vec<[f64; 2]> = samples.iter().filter_map(|s| get(s).filter(|v| v.is_finite()).map(|v| [s.t, v])).collect();
    (!points.is_empty()).then(|| Series { name: name.to_string(), color, points })
}

/// Ein Diagramm mit mehreren Reihen: Punkte, mit Linien verbunden.
fn chart(ui: &mut egui::Ui, id: &str, title: &str, unit: &'static str, decimals: usize, list: Vec<Series>, y_max: Option<f64>) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong().size(17.0));
    if list.is_empty() {
        ui.label(RichText::new("Keine Werte (fehlt der Sensor oder die Adminrechte?)").weak());
        return;
    }
    // Kennzahlen über dem Diagramm
    ui.horizontal_wrapped(|ui| {
        for s in &list {
            let vals: Vec<f64> = s.points.iter().map(|p| p[1]).collect();
            if let Some((mn, avg, mx)) = stats(&vals) {
                ui.colored_label(s.color, RichText::new("●").size(15.0));
                ui.label(format!(
                    "{}: min {} · Ø {} · max {} {unit}",
                    s.name,
                    format::de(mn, decimals),
                    format::de(avg, decimals),
                    format::de(mx, decimals)
                ));
                ui.add_space(10.0);
            }
        }
    });
    let n = list.iter().map(|s| s.points.len()).max().unwrap_or(0);
    // Bei sehr vielen Punkten nur die Linie, sonst Punkte + Linie
    let radius: f32 = if n > 600 { 0.0 } else if n > 200 { 1.8 } else { 3.0 };
    // Feste Achsen: unten immer 0 (keine negativen Werte), oben knapp über dem echten Höchstwert;
    // die Zeitachse läuft von 0 bis zur letzten Messung. So wandert der Graph nicht und zeigt
    // nur das, was wirklich gemessen wurde.
    let (x_max, top) = axis_range(&list, y_max);
    let plot = Plot::new(id)
        .height(230.0)
        .legend(Legend::default())
        .allow_scroll(false)
        .allow_zoom(false)
        .allow_drag(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .auto_bounds(egui::Vec2b::new(false, false))
        .x_axis_formatter(|m, _r| if m.value < 0.0 { String::new() } else { mmss(m.value) })
        .y_axis_formatter(move |m, _r| {
            if m.value < 0.0 {
                String::new()
            } else {
                format!("{} {unit}", format::de(m.value, if decimals > 0 && m.step_size < 1.0 { decimals } else { 0 }))
            }
        })
        .label_formatter(move |name, v| {
            if name.is_empty() || v.y < 0.0 {
                String::new()
            } else {
                format!("{name}\n{} · {} {unit}", mmss(v.x), format::de(v.y, decimals))
            }
        });
    plot.show(ui, |p| {
        p.set_plot_bounds(egui_plot::PlotBounds::from_min_max([0.0, 0.0], [x_max, top]));
        for s in list {
            p.line(Line::new(PlotPoints::from(s.points.clone())).name(&s.name).color(s.color).width(2.0_f32));
            if radius > 0.0 {
                p.points(Points::new(PlotPoints::from(s.points)).name(&s.name).color(s.color).radius(radius).filled(true));
            }
        }
    });
}

/// Achsenbereich eines Diagramms: (Ende der Zeitachse, Obergrenze der Werte).
/// Obergrenze = fester Wert (z. B. 100 %) oder 10 % über dem höchsten gemessenen Wert –
/// sie steigt also nur, wenn die Werte wirklich steigen. Untergrenze ist immer 0.
fn axis_range(list: &[Series], fixed: Option<f64>) -> (f64, f64) {
    let x_max = list.iter().flat_map(|s| s.points.iter().map(|p| p[0])).fold(0.0, f64::max).max(10.0);
    let peak = list.iter().flat_map(|s| s.points.iter().map(|p| p[1])).filter(|v| v.is_finite()).fold(0.0, f64::max);
    let top = match fixed {
        Some(f) => f.max(peak),
        None if peak > 0.0 => nice_ceiling(peak * 1.1),
        None => 1.0,
    };
    (x_max, top)
}

/// Aufrunden auf eine „glatte“ Zahl (1; 1,2; 1,5; 2; 2,5; 3; 4; 5; 6; 8 × 10^n), damit die Achse ruhig bleibt.
fn nice_ceiling(v: f64) -> f64 {
    if v <= 0.0 {
        return 1.0;
    }
    let mag = 10f64.powf(v.log10().floor());
    [1.0, 1.2, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0].iter().map(|f| f * mag).find(|c| *c >= v - 1e-9).unwrap_or(10.0 * mag)
}

/// Spalten der Exporte: (Name, Beschreibung, Wert)
type Col = (&'static str, &'static str, fn(&Sample) -> Option<f64>);
const COLS: [Col; 14] = [
    ("fps", "Bildrate des Spiels in Bildern pro Sekunde (braucht Adminrechte)", |s| s.fps),
    ("frametime_ms", "Durchschnittliche Zeit pro Bild in dieser Sekunde, ms", |s| s.ft_avg),
    ("frametime_max_ms", "Längstes Bild dieser Sekunde, ms", |s| s.ft_max),
    ("ruckler", "Ruckler in dieser Sekunde: Bilder, die mehr als 2,5-mal so lange dauern wie üblich (mindestens 20 ms)", |s| s.stutters),
    ("cpu_last_prozent", "CPU-Auslastung in %", |s| s.cpu_load),
    ("cpu_temp_c", "CPU-Temperatur in °C (Tctl/Package, braucht Adminrechte)", |s| s.cpu_temp),
    ("cpu_takt_ghz", "CPU-Takt in GHz (höchster Kern bzw. Durchschnitt laut Einstellung)", |s| s.cpu_ghz),
    ("cpu_watt", "CPU-Leistungsaufnahme in Watt (Package, braucht Adminrechte)", |s| s.cpu_w),
    ("gpu_last_prozent", "GPU-Auslastung in %", |s| s.gpu_load),
    ("gpu_temp_c", "GPU-Temperatur in °C", |s| s.gpu_temp),
    ("gpu_takt_ghz", "GPU-Kerntakt in GHz", |s| s.gpu_ghz),
    ("gpu_watt", "GPU-Leistungsaufnahme in Watt", |s| s.gpu_w),
    ("ram_prozent", "Belegter Arbeitsspeicher in %", |s| s.ram),
    ("vram_prozent", "Belegter Grafikspeicher (VRAM) in %", |s| s.vram),
];

/// Log zum Weitergeben (z. B. an eine KI, die daraus Diagramme oder eine Excel-Tabelle macht):
/// Kopf mit System, Spaltenerklärung, Zusammenfassung und alle Messpunkte als CSV
/// (Komma als Trennzeichen, Punkt als Dezimalzeichen, leere Zelle = kein Wert).
#[cfg(test)]
pub fn log_text(samples: &[Sample], info: &SystemInfo) -> String {
    log_text_full(samples, info, &[])
}

pub fn log_text_full(samples: &[Sample], info: &SystemInfo, game_frames: &[(String, Vec<f64>)]) -> String {
    let ccds = samples.iter().map(|s| s.ccd.len()).max().unwrap_or(0);
    let dur = samples.last().map(|s| s.t).unwrap_or(0.0);
    let mut o = String::new();
    o += "# TaskPulse Benchmark-Log\n\n";
    o += &format!("- Programm: TaskPulse {}\n", crate::updater::CURRENT);
    o += &format!("- Start der Aufzeichnung: {}\n", info.started);
    o += &format!("- Dauer: {} ({} Messpunkte, Abstand etwa 1 Sekunde)\n", mmss(dur), samples.len());
    o += &format!("- CPU: {}\n", info.cpu);
    o += &format!("- GPU: {}\n", info.gpu);
    o += &format!("- Arbeitsspeicher: {:.1} GB\n", info.ram_gb);
    o += &format!(
        "- Adminrechte: {}\n\n",
        if info.admin { "ja" } else { "nein (CPU-Temperatur, CCD-Temperaturen, CPU-Watt und FPS fehlen deshalb)" }
    );
    if let Some((main, frametimes)) = main_game(game_frames) {
        o += &format!("- Gemessenes Spiel/Programm (FPS): {main}\n");
        if let Some((avg, l1, l01)) = fps::lows(frametimes) {
            o += &format!("- Bildrate: Ø {avg:.1} FPS · 1 % Low {l1:.1} FPS · 0,1 % Low {l01:.1} FPS ({} Bilder)\n", frametimes.len());
            o += &format!("- Gleichmäßigkeit: {} (0,1 % Low = {:.0} % der 1 % Low)\n", fps::smoothness(l1, l01).0, if l1 > 0.0 { l01 / l1 * 100.0 } else { 0.0 });
            if let Some((n, limit)) = fps::stutters(frametimes) {
                o += &format!("- Ruckler: {n} Bilder länger als {limit:.1} ms (2,5 × übliche Frametime, mindestens 20 ms)\n");
            }
        }
        for (name, ft) in game_frames.iter().filter(|(n, _)| n != main) {
            if let Some((avg, _, _)) = fps::lows(ft) {
                o += &format!("- Außerdem im Vordergrund: {name} (Ø {avg:.1} FPS, {} Bilder – nicht in den Kennzahlen oben)\n", ft.len());
            }
        }
    }
    o += "\n";
    o += "## Hinweis zur Auswertung\n\n";
    o += "FPS und Frametimes stammen aus den Windows-Grafik-Ereignissen (ETW, wie PresentMon); TaskPulse klinkt sich nicht ins Spiel ein. \
          „1 % Low“ ist die Bildrate der langsamsten 1 % aller Bilder, „0,1 % Low“ die der langsamsten 0,1 %. \
          Liegt die 0,1 % Low weit unter der 1 % Low (z. B. 60 zu 15 FPS), gibt es seltene, aber deutlich spürbare Hänger (Mikroruckler, kurzes Einfrieren).\n\n";
    o += "Die Messwerte stehen unten im Abschnitt „Daten“ als CSV: Komma als Trennzeichen, Punkt als Dezimalzeichen, \
          etwa eine Zeile pro Sekunde, leere Zelle = Wert nicht verfügbar. Die Spalte zeit_s zählt die Sekunden ab Start (auf 0,1 s genau). \
          Die Spalte fps gilt für das Programm im Vordergrund; die Kennzahlen oben nur für das gemessene Spiel.\n\n";
    o += "Beispiel-Auftrag an eine KI: „Erstelle aus den CSV-Daten Liniendiagramme über die Zeit: (1) Temperaturen von CPU, GPU und CCDs, \
          (2) Auslastung von CPU und GPU, (3) Takt, (4) Leistung in Watt. Nenne Auffälligkeiten (Drosselung, Spitzen) und \
          gib die Daten als Excel-Tabelle aus.“\n\n";
    o += "### Spalten\n\n| Spalte | Bedeutung |\n|---|---|\n| zeit_s | Sekunden seit Start |\n";
    for (n, d, _) in COLS.iter() {
        o += &format!("| {n} | {d} |\n");
    }
    for i in 0..ccds {
        o += &format!("| ccd{}_temp_c | Temperatur von CCD {} in °C (AMD, braucht Adminrechte) |\n", i + 1, i + 1);
    }
    o += "\n## Zusammenfassung\n\n| Messwert | min | Durchschnitt | max |\n|---|---|---|---|\n";
    let mut sum_row = |name: &str, vals: Vec<f64>| {
        match stats(&vals) {
            Some((a, b, c)) => o += &format!("| {name} | {a:.2} | {b:.2} | {c:.2} |\n"),
            None => o += &format!("| {name} | – | – | – |\n"),
        }
    };
    for (n, _, f) in COLS.iter() {
        sum_row(n, samples.iter().filter_map(f).collect());
    }
    for i in 0..ccds {
        sum_row(&format!("ccd{}_temp_c", i + 1), samples.iter().filter_map(|s| s.ccd.get(i).copied()).collect());
    }
    o += "\n## Daten\n\n```csv\nzeit_s";
    for (n, _, _) in COLS.iter() {
        o += &format!(",{n}");
    }
    for i in 0..ccds {
        o += &format!(",ccd{}_temp_c", i + 1);
    }
    o += "\n";
    let v = |x: Option<f64>| x.filter(|x| x.is_finite()).map(|x| format!("{x:.2}")).unwrap_or_default();
    for s in samples {
        o += &format!("{:.1}", s.t);
        for (_, _, f) in COLS.iter() {
            o += &format!(",{}", v(f(s)));
        }
        for i in 0..ccds {
            o += &format!(",{}", v(s.ccd.get(i).copied()));
        }
        o += "\n";
    }
    o += "```\n";
    o
}

fn documents() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE").map(|p| std::path::PathBuf::from(p).join("Documents"))
}

/// Ergebnis eines „Speichern unter“-Vorgangs: Ok(Pfad), Err(Meldung) oder abgebrochen (None)
type SaveJob = std::sync::Arc<std::sync::Mutex<Option<Option<Result<std::path::PathBuf, String>>>>>;

/// Vorgeschlagener Dateiname, z. B. "TaskPulse-Benchmark-Cyberpunk 2077-2026-11-06_14-03-22.txt"
pub fn suggested_name(game: Option<&str>, ext: &str) -> String {
    let game: String = game
        .unwrap_or("")
        .chars()
        .filter(|c| !r#"\/:*?"<>|"#.contains(*c))
        .collect::<String>()
        .trim()
        .chars()
        .take(40)
        .collect();
    if game.is_empty() {
        format!("TaskPulse-Benchmark-{}.{ext}", file_stamp())
    } else {
        format!("TaskPulse-Benchmark-{game}-{}.{ext}", file_stamp())
    }
}

/// Öffnet „Speichern unter“ in einem eigenen Thread und schreibt die Datei dort hin.
fn save_as(content: String, name: String, start: Option<std::path::PathBuf>, csv: bool, ctx: egui::Context) -> SaveJob {
    let job: SaveJob = Default::default();
    let j = job.clone();
    std::thread::spawn(move || {
        let (title, filters, ext): (&str, Vec<(&str, &str)>, &str) = if csv {
            ("Benchmark als CSV speichern", vec![("CSV für Excel (*.csv)", "*.csv"), ("Alle Dateien (*.*)", "*.*")], "csv")
        } else {
            ("Benchmark-Log speichern", vec![("Textdatei (*.txt)", "*.txt"), ("Markdown (*.md)", "*.md"), ("Alle Dateien (*.*)", "*.*")], "txt")
        };
        let r = crate::winutil::save_file_dialog(title, &name, start.as_deref(), &filters, ext).map(|p| std::fs::write(&p, &content).map(|_| p.clone()).map_err(|e| e.to_string()));
        *j.lock().unwrap() = Some(r);
        ctx.request_repaint();
    });
    job
}

/// Inhalt der CSV-Datei (für Excel: Semikolon, Dezimalkomma, UTF-8 mit BOM).
fn csv_text(samples: &[Sample]) -> String {
    let ccds = samples.iter().map(|s| s.ccd.len()).max().unwrap_or(0);
    let mut out = String::from("Zeit (s);FPS;Frametime ms;Frametime max ms;Ruckler;CPU-Last %;CPU °C;CPU GHz;CPU W;GPU-Last %;GPU °C;GPU GHz;GPU W;RAM %;VRAM %");
    for i in 0..ccds {
        out += &format!(";CCD {} °C", i + 1);
    }
    out.push('\n');
    let f = |v: Option<f64>| v.map(|x| format!("{x:.2}").replace('.', ",")).unwrap_or_default();
    for s in samples {
        let mut row = vec![
            format!("{:.1}", s.t).replace('.', ","),
            f(s.fps),
            f(s.ft_avg),
            f(s.ft_max),
            f(s.stutters),
            f(s.cpu_load),
            f(s.cpu_temp),
            f(s.cpu_ghz),
            f(s.cpu_w),
            f(s.gpu_load),
            f(s.gpu_temp),
            f(s.gpu_ghz),
            f(s.gpu_w),
            f(s.ram),
            f(s.vram),
        ];
        for i in 0..ccds {
            row.push(f(s.ccd.get(i).copied()));
        }
        out += &row.join(";");
        out.push('\n');
    }
    // UTF-8 mit BOM, damit Excel Umlaute und °C richtig anzeigt
    format!("\u{feff}{out}")
}

fn local_time() -> windows::Win32::Foundation::SYSTEMTIME {
    unsafe { windows::Win32::System::SystemInformation::GetLocalTime() }
}

/// Für Dateinamen: "2026-11-06_14-03-22"
fn file_stamp() -> String {
    let t = local_time();
    format!("{:04}-{:02}-{:02}_{:02}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// Zum Lesen: "06.11.2026 14:03:22"
fn now_text() -> String {
    let t = local_time();
    format!("{:02}.{:02}.{:04} {:02}:{:02}:{:02}", t.wDay, t.wMonth, t.wYear, t.wHour, t.wMinute, t.wSecond)
}

pub fn show(app: &mut TaskPulseApp, ui: &mut egui::Ui) {
    let st = &mut app.bench;
    // Ergebnis eines „Speichern unter“ abholen; gewählten Ordner merken
    if let Some(job) = &st.save_job {
        let r = job.lock().unwrap().take();
        if let Some(r) = r {
            st.save_job = None;
            st.message = Some(match r {
                Some(Ok(p)) => {
                    if let Some(dir) = p.parent() {
                        app.cfg.bench_dir = dir.to_string_lossy().to_string();
                    }
                    crate::winutil::open_in_explorer(&p.to_string_lossy());
                    format!("Gespeichert: {}", p.display())
                }
                Some(Err(e)) => format!("Speichern fehlgeschlagen: {e}"),
                None => "Speichern abgebrochen.".into(),
            });
        }
    }
    let start_dir = Some(app.cfg.bench_dir.clone()).filter(|d| !d.is_empty()).map(std::path::PathBuf::from).or_else(documents);
    ui.horizontal(|ui| {
        ui.heading("Benchmark");
        ui.add_space(12.0);
        if st.recording {
            let btn = egui::Button::new(RichText::new("■  Aufzeichnung stoppen").strong().color(Color32::WHITE)).fill(Color32::from_rgb(196, 43, 28));
            if ui.add(btn).clicked() {
                st.end();
                st.message = Some(format!("Aufzeichnung beendet: {} Messpunkte über {}.", st.samples.len(), mmss(st.samples.last().map(|s| s.t).unwrap_or(0.0))));
            }
            let el = st.start.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
            ui.label(RichText::new(format!("● läuft seit {}  ·  {} Messpunkte", mmss(el), st.samples.len())).color(Color32::from_rgb(220, 70, 60)).strong());
            if let (Some(g), Some(f)) = (&st.game, st.samples.last().and_then(|s| s.fps)) {
                ui.label(RichText::new(format!("{}: {:.0} FPS", g.name, f)).strong().size(16.0));
            } else if st.fps.is_some() && st.samples.len() >= 5 {
                ui.label(RichText::new("FPS: noch kein Spiel erkannt – Spiel bzw. Benchmark im Vordergrund?").weak());
            }
        } else {
            let btn = egui::Button::new(RichText::new("▶  Aufzeichnung starten").strong().color(Color32::WHITE)).fill(Color32::from_rgb(15, 118, 110));
            if ui.add(btn).on_hover_text("Zeichnet jede Sekunde Auslastung, Temperaturen, Takt und Leistung auf. Startet eine neue Aufzeichnung.").clicked() {
                st.begin();
            }
            if !st.samples.is_empty() {
                let busy = st.save_job.is_some();
                if ui
                    .add_enabled(!busy, egui::Button::new("Log speichern …"))
                    .on_hover_text("Ordner und Dateinamen selbst wählen. Textdatei mit System, Spaltenerklärung, Zusammenfassung und allen Messpunkten – zum Hochladen bei einer KI oder für Excel.")
                    .clicked()
                {
                    let content = log_text_full(&st.samples, &st.info, &st.game_frames);
                    let name = suggested_name(st.games.first().map(|s| s.as_str()), "txt");
                    st.save_job = Some(save_as(content, name, start_dir.clone(), false, ui.ctx().clone()));
                    st.message = Some("Speicherort wählen …".into());
                }
                if ui.button("Log kopieren").on_hover_text("Das Log in die Zwischenablage kopieren, z. B. zum Einfügen in einen KI-Chat.").clicked() {
                    ui.ctx().copy_text(log_text_full(&st.samples, &st.info, &st.game_frames));
                    st.message = Some("Log in die Zwischenablage kopiert.".into());
                }
                if ui.add_enabled(!busy, egui::Button::new("Als CSV speichern …")).on_hover_text("Ordner und Dateinamen selbst wählen. Alle Messpunkte als Tabelle für Excel.").clicked() {
                    let name = suggested_name(st.games.first().map(|s| s.as_str()), "csv");
                    st.save_job = Some(save_as(csv_text(&st.samples), name, start_dir.clone(), true, ui.ctx().clone()));
                    st.message = Some("Speicherort wählen …".into());
                }
                if ui.button("Verwerfen").clicked() {
                    st.samples.clear();
                    st.message = None;
                }
            }
        }
    });
    if let Some(m) = &st.message {
        ui.label(RichText::new(m).weak());
    }
    if !app.admin {
        ui.label(RichText::new("Ohne Adminrechte fehlen FPS, CPU-Temperatur und CPU-Watt.").small().color(Color32::from_rgb(200, 140, 0)));
    } else if let Some(e) = &st.fps_error {
        ui.label(RichText::new(format!("FPS: {e}")).small().color(Color32::from_rgb(200, 140, 0)));
    }
    egui::CollapsingHeader::new("Ist der Benchmark Anti-Cheat-sicher?").id_salt("anticheat").show(ui, |ui| {
        for t in [
            "FPS: TaskPulse liest nur die Grafik-Ereignisse mit, die Windows für jedes Bild ohnehin protokolliert (ETW) – genau wie Intel PresentMon, NVIDIA FrameView und CapFrameX.",
            "Kein Einklinken: keine DLL im Spiel, kein Hook, kein Overlay, kein Lesen von Spielspeicher, keine Eingaben.",
            "Auf das Spiel selbst greift TaskPulse nicht zu – nur einmalig mit dem kleinsten Leserecht auf den Dateipfad (wie der Windows-Task-Manager).",
            "Temperatur, Watt und Spannung der CPU kommen über den signierten Treiber PawnIO. Er liest nur die freigegebenen Sensor-Register. Wer bei einem Spiel ganz sicher gehen will: Einstellungen → CPU-Sensoren → PawnIO ausschalten (dann fehlen nur CPU-Temperatur und -Watt).",
        ] {
            ui.label(RichText::new(format!("• {t}")).small());
        }
    });
    ui.separator();

    let samples = &st.samples;
    if samples.is_empty() {
        ui.add_space(20.0);
        ui.label(
            RichText::new(
                "„Aufzeichnung starten“ klicken, dann z. B. ein Spiel oder einen Belastungstest laufen lassen und danach stoppen.\n\
                 Aufgezeichnet wird jede Sekunde: die Bildrate (FPS) des Spiels im Vordergrund, Auslastung von CPU, GPU, Arbeitsspeicher und VRAM, Temperaturen (CPU, GPU, bei AMD je CCD), \
                 Takt und Leistung in Watt. Die Graphen erscheinen schon während der Aufzeichnung.",
            )
            .size(15.0),
        );
        return;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.set_max_width(ui.available_width());
        let ccds = samples.iter().map(|s| s.ccd.len()).max().unwrap_or(0);
        let mut temps: Vec<Series> = [series(samples, "CPU", C_CPU, |s| s.cpu_temp), series(samples, "GPU", C_GPU, |s| s.gpu_temp)].into_iter().flatten().collect();
        for i in 0..ccds {
            if let Some(s) = series(samples, &format!("CCD {}", i + 1), C_CCD[i % C_CCD.len()], move |x| x.ccd.get(i).copied()) {
                temps.push(s);
            }
        }
        if samples.iter().any(|s| s.fps.is_some()) {
            let main = main_game(&st.game_frames);
            let main_ft: &[f64] = main.map(|m| m.1).unwrap_or(&[]);
            if let Some((avg, l1, l01)) = fps::lows(main_ft) {
                ui.add_space(4.0);
                let (verdict, level) = fps::smoothness(l1, l01);
                let col = [Color32::from_rgb(60, 180, 110), Color32::from_rgb(230, 170, 40), Color32::from_rgb(230, 80, 70)][level as usize];
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("Ø {avg:.0} FPS")).strong().size(20.0));
                    ui.label(RichText::new(format!("1 % Low {l1:.0} FPS")).strong().size(18.0));
                    ui.label(RichText::new(format!("0,1 % Low {l01:.0} FPS")).strong().size(18.0).color(col)).on_hover_text(
                        "Bildrate der langsamsten 0,1 % aller Bilder. Liegt sie weit unter der 1 % Low, gibt es seltene, \
                         aber deutlich spürbare Hänger – z. B. 1 % Low 60 FPS, 0,1 % Low 15 FPS = kurzes Einfrieren.",
                    );
                    ui.label(RichText::new(format!("· {verdict}")).strong().color(col));
                    if let Some((n, limit)) = fps::stutters(main_ft) {
                        ui.label(RichText::new(format!("· {n} Ruckler (Bilder > {limit:.0} ms)")).strong().color(if n == 0 { Color32::from_rgb(60, 180, 110) } else { col }));
                    }
                    if let Some((name, _)) = main {
                        ui.label(RichText::new(format!("· {name}")).strong());
                    }
                    let others: Vec<&str> = st.game_frames.iter().map(|(n, _)| n.as_str()).filter(|n| Some(*n) != main.map(|m| m.0)).collect();
                    if !others.is_empty() {
                        ui.label(RichText::new(format!("(ohne {})", others.join(", "))).weak())
                            .on_hover_text("Diese Programme waren zwischendurch im Vordergrund. Ihre Bilder zählen nicht zu Ø, 1 % und 0,1 % Low.");
                    }
                });
            }
            chart(ui, "bench_fps", "Bildrate", "FPS", 0, [series(samples, "FPS", C_FPS, |s| s.fps)].into_iter().flatten().collect(), None);
            chart(
                ui,
                "bench_ft",
                "Frametimes",
                "ms",
                1,
                [series(samples, "Ø", C_FPS, |s| s.ft_avg), series(samples, "längstes Bild", C_FT_MAX, |s| s.ft_max)].into_iter().flatten().collect(),
                None,
            );
            if samples.iter().any(|s| s.stutters.unwrap_or(0.0) > 0.0) {
                chart(ui, "bench_stutter", "Ruckler pro Sekunde", "", 0, [series(samples, "Ruckler", C_FT_MAX, |s| s.stutters)].into_iter().flatten().collect(), None);
            }
        }
        chart(ui, "bench_temp", "Temperaturen", "°C", 1, temps, None);
        chart(
            ui,
            "bench_load",
            "Auslastung",
            "%",
            0,
            [
                series(samples, "CPU", C_CPU, |s| s.cpu_load),
                series(samples, "GPU", C_GPU, |s| s.gpu_load),
                series(samples, "RAM", C_RAM, |s| s.ram),
                series(samples, "VRAM", C_VRAM, |s| s.vram),
            ]
            .into_iter()
            .flatten()
            .collect(),
            Some(100.0),
        );
        chart(
            ui,
            "bench_clock",
            "Takt",
            "GHz",
            2,
            [series(samples, "CPU", C_CPU, |s| s.cpu_ghz), series(samples, "GPU", C_GPU, |s| s.gpu_ghz)].into_iter().flatten().collect(),
            None,
        );
        chart(
            ui,
            "bench_power",
            "Leistung",
            "W",
            1,
            [series(samples, "CPU", C_CPU, |s| s.cpu_w), series(samples, "GPU", C_GPU, |s| s.gpu_w)].into_iter().flatten().collect(),
            None,
        );
        ui.add_space(4.0);
        ui.label(RichText::new("Maus über einen Punkt: genauer Wert · Legende anklicken: Reihe ein-/ausblenden · Achsen beginnen immer bei 0").small().weak());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_and_time() {
        assert_eq!(stats(&[2.0, 4.0, 9.0]), Some((2.0, 5.0, 9.0)));
        assert_eq!(stats(&[]), None);
        assert_eq!(mmss(0.0), "0:00");
        assert_eq!(mmss(75.4), "1:15");
        assert_eq!(mmss(3725.0), "1:02:05");
    }

    #[test]
    fn log_is_ai_readable() {
        let s = vec![
            Sample { t: 0.0, cpu_load: Some(12.5), gpu_temp: Some(40.0), ccd: vec![45.25, 41.0], ..Default::default() },
            Sample { t: 1.0, cpu_load: Some(20.0), gpu_temp: Some(42.0), ccd: vec![46.0, 41.5], ..Default::default() },
        ];
        let info = SystemInfo { started: "06.11.2026 14:00:00".into(), cpu: "AMD Ryzen 9 5950X".into(), gpu: "RTX 5080".into(), ram_gb: 31.9, admin: true };
        let t = log_text(&s, &info);
        assert!(t.contains("- CPU: AMD Ryzen 9 5950X"));
        assert!(t.contains("zeit_s,fps,frametime_ms,frametime_max_ms,ruckler,cpu_last_prozent,cpu_temp_c,"));
        assert!(t.contains(",ccd1_temp_c,ccd2_temp_c\n"));
        // fehlende Werte = leere Zellen, Punkt als Dezimalzeichen
        assert!(t.contains("\n0.0,,,,,12.50,,,,,40.00,,,,,45.25,41.00\n"));
        assert!(t.contains("| cpu_last_prozent | 12.50 | 16.25 | 20.00 |"));
        assert!(t.contains("| cpu_temp_c | – | – | – |"));
        assert!(t.trim_end().ends_with("```"));
    }

    #[test]
    fn axes_start_at_zero_and_follow_real_peak() {
        let s = |pts: Vec<[f64; 2]>| Series { name: "FPS".into(), color: C_FPS, points: pts };
        // 60 FPS über 30 s → 60 × 1,1 = 66 → Achse bis 80
        let (x, top) = axis_range(&[s(vec![[0.0, 58.0], [30.0, 60.0]])], None);
        assert_eq!(x, 30.0);
        assert_eq!(top, 80.0);
        // 144 FPS → 158,4 → 200; nur 0-Werte → 1 (keine negative Achse)
        assert_eq!(axis_range(&[s(vec![[0.0, 144.0]])], None).1, 200.0);
        assert_eq!(axis_range(&[s(vec![[0.0, 0.0], [5.0, 0.0]])], None), (10.0, 1.0));
        // Prozent: fest bis 100
        assert_eq!(axis_range(&[s(vec![[0.0, 40.0]])], Some(100.0)).1, 100.0);
        assert_eq!(nice_ceiling(66.0), 80.0);
        assert_eq!(nice_ceiling(21.0), 25.0);
        assert_eq!(nice_ceiling(3.3), 4.0);
        assert_eq!(nice_ceiling(396.0), 400.0);
    }

    #[test]
    fn file_names() {
        let n = suggested_name(Some("Cyberpunk 2077: Phantom/Liberty"), "txt");
        assert!(n.starts_with("TaskPulse-Benchmark-Cyberpunk 2077 PhantomLiberty-"), "{n}");
        assert!(n.ends_with(".txt"));
        assert!(suggested_name(None, "csv").starts_with("TaskPulse-Benchmark-20"));
        assert!(!suggested_name(Some("a<b>c?"), "txt").contains(['<', '>', '?']));
    }

    #[test]
    fn lows_only_for_main_program() {
        // FurMark-Startfenster (6 FPS) + eigentlicher Test (750 FPS): Kennzahlen nur vom Test
        let frames = vec![("FurMark(GUI)".to_string(), vec![125.0; 60]), ("FurMark CLI".to_string(), vec![1.33; 90_000])];
        let (name, ft) = main_game(&frames).unwrap();
        assert_eq!(name, "FurMark CLI");
        let (avg, l1, _) = fps::lows(ft).unwrap();
        assert!(avg > 700.0 && l1 > 700.0);
        let log = log_text_full(&[], &SystemInfo::default(), &frames);
        assert!(log.contains("- Gemessenes Spiel/Programm (FPS): FurMark CLI"));
        assert!(log.contains("Außerdem im Vordergrund: FurMark(GUI)"));
    }

    #[test]
    fn game_choice() {
        let s = |pid, fps| fps::FpsSample { pid, fps, frametimes_ms: vec![] };
        let snap = vec![s(10, 240.0), s(20, 60.0)];
        // Vordergrund gewinnt, auch mit weniger FPS
        assert_eq!(pick_game(&snap, 20, None), Some(1));
        // sonst bisheriges Spiel, sonst höchste Bildrate
        assert_eq!(pick_game(&snap, 99, Some(20)), Some(1));
        assert_eq!(pick_game(&snap, 99, None), Some(0));
        assert_eq!(pick_game(&[], 99, None), None);
    }

    #[test]
    fn series_skip_missing_values() {
        let s = vec![
            Sample { t: 0.0, cpu_temp: Some(40.0), ..Default::default() },
            Sample { t: 1.0, cpu_temp: None, ..Default::default() },
            Sample { t: 2.0, cpu_temp: Some(42.5), ..Default::default() },
        ];
        let x = series(&s, "CPU", C_CPU, |s| s.cpu_temp).unwrap();
        assert_eq!(x.points, vec![[0.0, 40.0], [2.0, 42.5]]);
        assert!(series(&s, "GPU", C_GPU, |s| s.gpu_temp).is_none());
    }
}
