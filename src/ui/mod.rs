//! Hauptfenster (egui): Prozesse, Leistung, Einstellungen.

mod benchmark;
mod performance;
mod processes;
mod settings;
mod startup;
mod update;

pub use settings::taskbar_running;

use crate::config::Config;
use crate::sensors::gpu_proc::{DxgiAdapter, GpuProc, GpuProcSample};
use crate::sensors::mem::{Disk, DiskSample};
use crate::sensors::procs::{Proc, Procs};
use crate::sensors::{Basic, BasicSensors};
use crate::winutil;
use eframe::egui;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Ein vollständiger Datenstand des Messkerns.
#[derive(Clone, Default)]
pub struct Frame {
    pub seq: u64,
    pub basic: Basic,
    pub disk: DiskSample,
    pub gpu: GpuProcSample,
    pub procs: Vec<Proc>,
    pub adapters: Vec<DxgiAdapter>,
    pub vram_total: Option<f64>,
    /// Watt/Spannung je physischem Kern (nur mit PawnIO)
    pub core_readings: Vec<crate::sensors::cputemp::CoreReading>,
    /// AMD: Temperatur je CCD (Hardware-Reihenfolge)
    pub ccd_temps: Vec<f64>,
    /// Laufende RAM-Timings je Kanal (AMD mit PawnIO)
    pub ram_timings: Vec<crate::sensors::dimm::Timings>,
    /// Temperatur der Speicherriegel (SMBus)
    pub dimm_temps: Vec<crate::sensors::dimm::DimmTemp>,
    /// Spezifikation zum Aufklappen (Grafikkarte alle 5 s, CPU einmal)
    pub gpu_spec: Arc<crate::sensors::spec::Spec>,
    pub cpu_spec: Arc<crate::sensors::spec::Spec>,
    /// PCIe-Verbindung der angezeigten Grafikkarte
    pub gpu_link: Option<crate::sensors::spec::PcieLink>,
    pub vram_used: Option<f64>,
    pub took_ms: f64,
}

struct Collector {
    frame: Arc<Mutex<Option<Frame>>>,
    interval_ms: Arc<AtomicU64>,
    cfg: Arc<Mutex<Config>>,
}

fn start_collector(ctx: egui::Context, cfg: Config) -> Collector {
    let frame = Arc::new(Mutex::new(None));
    let interval_ms = Arc::new(AtomicU64::new(1000));
    let cfg = Arc::new(Mutex::new(cfg));
    {
        let frame = frame.clone();
        let interval_ms = interval_ms.clone();
        let cfg = cfg.clone();
        std::thread::Builder::new()
            .name("messkern".into())
            .spawn(move || {
                let c0 = cfg.lock().unwrap().clone();
                let mut basic = BasicSensors::new(&c0);
                let mut disk = Disk::new();
                let mut gpu = GpuProc::new();
                let mut procs = Procs::new();
                let phys_cores: Vec<Vec<usize>> = crate::sensors::topology::query().cores.into_iter().map(|c| c.lps).collect();
                // Messen auch dann im Takt, wenn andere Programme die CPU voll auslasten (Benchmark mit CPU-Burner)
                #[cfg(windows)]
                unsafe {
                    let _ = windows::Win32::System::Threading::SetThreadPriority(
                        windows::Win32::System::Threading::GetCurrentThread(),
                        windows::Win32::System::Threading::THREAD_PRIORITY_ABOVE_NORMAL,
                    );
                }
                let mut seq = 0u64;
                let mut dimm_temps: Vec<crate::sensors::dimm::DimmTemp> = Vec::new();
                let nv_spec_src = crate::sensors::nvgpu::NvGpu::new();
                let mut gpu_spec: Arc<crate::sensors::spec::Spec> = Arc::default();
                let mut cpu_spec: Option<Arc<crate::sensors::spec::Spec>> = None;
                let mut gpu_link: Option<crate::sensors::spec::PcieLink> = None;
                let mut next: Option<Instant> = None;
                loop {
                    let iv = interval_ms.load(Ordering::Relaxed);
                    if iv == 0 {
                        std::thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                    let t0 = Instant::now();
                    let c = cfg.lock().unwrap().clone();
                    let b = basic.sample(&c);
                    let core_readings = basic.per_core(&phys_cores);
                    let ccd_temps = basic.ccd_temps();
                    let ram_timings = basic.ram_timings();
                    // Riegeltemperatur ändert sich langsam → alle 2 s lesen
                    if seq % 2 == 0 || dimm_temps.is_empty() {
                        dimm_temps = basic.dimm_temps(&c);
                    }
                    // Nur für Bildschirmfotos ohne Adminrechte: TASKPULSE_DEMO_KERNE=1 setzt Beispielwerte ein
                    let (core_readings, ccd_temps) = if core_readings.is_empty() && std::env::var_os("TASKPULSE_DEMO_KERNE").is_some() {
                        let demo = phys_cores
                            .iter()
                            .enumerate()
                            .map(|(i, l)| crate::sensors::cputemp::CoreReading {
                                lps: l.clone(),
                                watts: Some(0.4 + (i % 5) as f64 * 1.3),
                                volts: Some(0.95 + (i % 4) as f64 * 0.09),
                                // AMD meldet keine Temperatur je Kern; Intel schon
                                temp_c: crate::sensors::cputemp::cpu_info().vendor.contains("Intel").then(|| 41.0 + (i % 6) as f64 * 3.0),
                            })
                            .collect();
                        (demo, vec![48.6, 41.9])
                    } else {
                        (core_readings, ccd_temps)
                    };
                    // (Beispielwerte auch für RAM-Timings und -Temperaturen)
                    let ram_timings = if ram_timings.is_empty() && std::env::var_os("TASKPULSE_DEMO_KERNE").is_some() {
                        crate::sensors::dimm::decode_umc(0, true, 48 | (1 << 11), [16 | (36 << 8) | (18 << 16) | (18 << 24), 54 | (18 << 16), 4 | (6 << 8) | (12 << 24), 16, 16 | (4 << 8) | (12 << 16), 24], [560, 0, 0, 0]).into_iter().collect()
                    } else {
                        ram_timings
                    };
                    if dimm_temps.is_empty() && std::env::var_os("TASKPULSE_DEMO_KERNE").is_some() {
                        dimm_temps = (0..crate::sensors::dimm::modules().len() as u8).map(|i| crate::sensors::dimm::DimmTemp { index: i, temp_c: 38.5 + i as f64 * 1.25 }).collect();
                    }
                    let d = disk.as_mut().map(|d| d.sample()).unwrap_or_default();
                    let g = gpu.as_mut().map(|g| g.sample()).unwrap_or_default();
                    let p = procs.sample();
                    let adapters = gpu.as_ref().map(|g| g.adapters.clone()).unwrap_or_default();
                    let vram_total = b.gpu.mem_total.or_else(|| adapters.iter().map(|a| a.dedicated).reduce(f64::max));
                    let vram_used = b.gpu.mem_used.or(g.adapter_dedicated_used);
                    // Spezifikation: Grafikkarte alle 5 s (PCIe-Verbindung ändert sich beim Stromsparen), CPU einmal
                    if seq % 5 == 0 {
                        let sel = adapters.iter().find(|a| a.luid == c.gpu_adapter).or_else(|| crate::sensors::gpu::auto_adapter(&adapters));
                        if let Some(a) = sel {
                            let nv = nv_spec_src.as_ref().filter(|_| a.vendor_id == 0x10DE).map(|n| n.spec());
                            let pnp = crate::sensors::spec::pnp_info(a);
                            gpu_link = crate::sensors::spec::effective_link(&pnp, nv.as_ref());
                            gpu_spec = Arc::new(crate::sensors::spec::gpu_spec(a, nv.as_ref(), &pnp, None, b.gpu.clock_mhz));
                        }
                    }
                    let cpu_spec = cpu_spec.get_or_insert_with(|| Arc::new(crate::sensors::spec::cpu_spec(b.cpu.nominal_mhz))).clone();
                    seq += 1;
                    let took_ms = t0.elapsed().as_secs_f64() * 1000.0;
                    *frame.lock().unwrap() =
                        Some(Frame { seq, basic: b, disk: d, gpu: g, procs: p, adapters, vram_total, vram_used, took_ms, core_readings, ccd_temps, ram_timings, dimm_temps: dimm_temps.clone(), gpu_spec: gpu_spec.clone(), cpu_spec, gpu_link });
                    ctx.request_repaint();
                    // Nächster Messzeitpunkt fest im Takt (sonst summieren sich Verzögerungen,
                    // und im Benchmark fehlt alle ~15 s eine Sekunde)
                    let iv = Duration::from_millis(iv);
                    next = next.map(|n| n + iv).filter(|n| *n > Instant::now()).or(Some(t0 + iv));
                    if let Some(n) = next {
                        std::thread::sleep(n.saturating_duration_since(Instant::now()));
                    }
                }
            })
            .ok();
    }
    Collector { frame, interval_ms, cfg }
}

pub const HISTORY: usize = 60;

#[derive(Default)]
pub struct History {
    pub cpu: VecDeque<f64>,
    pub mem: VecDeque<f64>,
    pub disk: VecDeque<f64>,
    pub net_down: VecDeque<f64>,
    pub net_up: VecDeque<f64>,
    pub gpu: VecDeque<f64>,
    pub vram: VecDeque<f64>,
    pub cores: Vec<VecDeque<f64>>,
    pub engines: Vec<(String, VecDeque<f64>)>,
    /// Lesen/Schreiben aller Datenträger, Bytes/s
    pub disk_read: VecDeque<f64>,
    pub disk_write: VecDeque<f64>,
    /// Jeder Datenträger einzeln
    pub disks: Vec<DiskHist>,
}

#[derive(Default, Clone)]
pub struct DiskHist {
    pub index: u32,
    pub active: VecDeque<f64>,
    pub read: VecDeque<f64>,
    pub write: VecDeque<f64>,
}

fn push(q: &mut VecDeque<f64>, v: f64) {
    q.push_back(if v.is_finite() { v } else { 0.0 });
    while q.len() > HISTORY {
        q.pop_front();
    }
}

impl History {
    fn add(&mut self, f: &Frame) {
        push(&mut self.cpu, f.basic.cpu.load_pct.unwrap_or(0.0));
        push(&mut self.mem, f.basic.mem.used_pct.unwrap_or(0.0));
        push(&mut self.disk, f.disk.active_pct.unwrap_or(0.0));
        push(&mut self.disk_read, f.disk.read_bps.unwrap_or(0.0));
        push(&mut self.disk_write, f.disk.write_bps.unwrap_or(0.0));
        // Datenträger kommen und gehen (USB): nur die aktuellen behalten
        self.disks.retain(|h| f.disk.disks.iter().any(|d| d.index == h.index));
        for d in &f.disk.disks {
            let pos = match self.disks.iter().position(|h| h.index == d.index) {
                Some(p) => p,
                None => {
                    self.disks.push(DiskHist { index: d.index, ..Default::default() });
                    self.disks.len() - 1
                }
            };
            let h = &mut self.disks[pos];
            push(&mut h.active, d.active_pct.unwrap_or(0.0));
            push(&mut h.read, d.read_bps.unwrap_or(0.0));
            push(&mut h.write, d.write_bps.unwrap_or(0.0));
        }
        self.disks.sort_by_key(|h| h.index);
        push(&mut self.net_down, f.basic.net.down_bps.unwrap_or(0.0));
        push(&mut self.net_up, f.basic.net.up_bps.unwrap_or(0.0));
        push(&mut self.gpu, f.gpu.total_pct.or(f.basic.gpu.load_pct).unwrap_or(0.0));
        let vp = match (f.vram_used, f.vram_total) {
            (Some(u), Some(t)) if t > 0.0 => u / t * 100.0,
            _ => 0.0,
        };
        push(&mut self.vram, vp);
        if self.cores.len() != f.basic.cpu.cores_pct.len() {
            self.cores = vec![VecDeque::new(); f.basic.cpu.cores_pct.len()];
        }
        for (q, v) in self.cores.iter_mut().zip(f.basic.cpu.cores_pct.iter()) {
            push(q, *v);
        }
        for (name, v) in &f.gpu.by_engtype {
            if let Some((_, q)) = self.engines.iter_mut().find(|(n, _)| n == name) {
                push(q, *v);
            } else {
                let mut q = VecDeque::new();
                push(&mut q, *v);
                self.engines.push((name.clone(), q));
            }
        }
        self.engines.sort_by(|a, b| a.0.cmp(&b.0));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Processes,
    Performance,
    Startup,
    Benchmark,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Speed {
    High,
    Normal,
    Low,
    Paused,
}

impl Speed {
    fn ms(self) -> u64 {
        match self {
            Speed::High => 500,
            Speed::Normal => 1000,
            Speed::Low => 4000,
            Speed::Paused => 0,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Speed::High => "Hoch (0,5 s)",
            Speed::Normal => "Normal (1 s)",
            Speed::Low => "Niedrig (4 s)",
            Speed::Paused => "Angehalten",
        }
    }
}

pub struct TaskPulseApp {
    tab: Tab,
    cfg: Config,
    saved_cfg: Config,
    dirty_since: Option<Instant>,
    collector: Collector,
    frame: Frame,
    history: History,
    speed: Speed,
    admin: bool,
    status: Option<(String, Instant)>,
    last_signal_check: Instant,
    always_on_top_applied: Option<bool>,
    font_applied: f32,
    pub procs: processes::ProcState,
    pub perf: performance::PerfState,
    pub settings: settings::SettingsState,
    pub update: update::UpdateUi,
    pub bench: benchmark::BenchState,
    pub startup: startup::StartupState,
    /// Absturzbericht aus dem Windows-Protokoll (beim Start im Hintergrund geprüft)
    pub crash_notice: Arc<Mutex<Option<std::path::PathBuf>>>,
    /// Ergebnis „Diagnosebericht erstellen“
    pub diag_message: Option<String>,
}

impl TaskPulseApp {
    fn new(cc: &eframe::CreationContext<'_>, page: Option<Tab>) -> Self {
        setup_fonts(&cc.egui_ctx);
        let cfg = Config::load();
        let collector = start_collector(cc.egui_ctx.clone(), cfg.clone());
        let start = match cfg.start_page {
            crate::config::StartPage::Processes => Tab::Processes,
            crate::config::StartPage::Performance => Tab::Performance,
        };
        TaskPulseApp {
            tab: page.unwrap_or(start),
            saved_cfg: cfg.clone(),
            cfg,
            dirty_since: None,
            collector,
            frame: Frame::default(),
            history: History::default(),
            speed: Speed::Normal,
            admin: winutil::is_elevated(),
            status: None,
            last_signal_check: Instant::now(),
            always_on_top_applied: None,
            font_applied: 0.0,
            procs: processes::ProcState::default(),
            perf: performance::PerfState { item: performance::start_item() },
            settings: settings::SettingsState::default(),
            update: update::UpdateUi::default(),
            bench: benchmark::BenchState::from_args(),
            startup: startup::StartupState::default(),
            crash_notice: {
                let n = Arc::new(Mutex::new(None));
                let n2 = n.clone();
                let ctx = cc.egui_ctx.clone();
                std::thread::spawn(move || {
                    // Abstürze aus dem System-Protokoll, danach alle noch nicht angezeigten Berichte
                    let crash = crate::diag::check_new_crashes();
                    let unseen = crate::diag::unseen_reports();
                    if let Some(p) = crash.or(unseen) {
                        *n2.lock().unwrap() = Some(p);
                        ctx.request_repaint();
                    }
                });
                n
            },
            diag_message: None,
        }
    }

    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), Instant::now()));
    }

    fn poll(&mut self) {
        let new = self.collector.frame.lock().unwrap().take();
        if let Some(f) = new {
            if f.seq != self.frame.seq {
                self.history.add(&f);
                self.frame = f;
            }
        }
    }

    fn persist(&mut self) {
        if self.cfg != self.saved_cfg {
            *self.collector.cfg.lock().unwrap() = self.cfg.clone();
            let since = *self.dirty_since.get_or_insert_with(Instant::now);
            if since.elapsed() > Duration::from_millis(400) {
                match self.cfg.save() {
                    Ok(()) => self.saved_cfg = self.cfg.clone(),
                    Err(e) => self.set_status(format!("Einstellungen nicht gespeichert: {e}")),
                }
                self.dirty_since = None;
            }
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("TaskPulse");
                ui.add_space(12.0);
                ui.selectable_value(&mut self.tab, Tab::Processes, "Prozesse");
                ui.selectable_value(&mut self.tab, Tab::Performance, "Leistung");
                // Beim Öffnen des Reiters neu einlesen (könnte sich außerhalb geändert haben)
                if ui.selectable_value(&mut self.tab, Tab::Startup, "Autostart").clicked() {
                    self.startup.mark_stale();
                }
                let bench_label = if self.bench.recording { "Benchmark ●" } else { "Benchmark" };
                ui.selectable_value(&mut self.tab, Tab::Benchmark, bench_label);
                ui.selectable_value(&mut self.tab, Tab::Settings, "Einstellungen");
                // Wenig Platz (schmales Fenster, große Schrift): kürzere Beschriftungen rechts
                let narrow = ui.available_width() < 880.0;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.cfg.always_on_top, if narrow { "Vordergrund" } else { "Immer im Vordergrund" })
                        .on_hover_text("Hauptfenster immer im Vordergrund");
                    egui::ComboBox::from_id_salt("speed").selected_text(self.speed.label()).show_ui(ui, |ui| {
                        for s in [Speed::High, Speed::Normal, Speed::Low, Speed::Paused] {
                            ui.selectable_value(&mut self.speed, s, s.label());
                        }
                    });
                    if !narrow {
                        ui.label("Aktualisierung:");
                    }
                    // Unter Linux braucht TaskPulse keine Adminrechte – dort kein Hinweis
                    if !self.admin && cfg!(windows) {
                        if ui
                            .button(if narrow { "🛡 Als Admin" } else { "🛡 Mit Adminrechten neu starten" })
                            .on_hover_text("Ohne Adminrechte fehlen CPU-Temperatur, einige Prozessdaten und Aktionen.")
                            .clicked()
                        {
                            let arg = if self.tab == Tab::Settings { "--settings" } else { "" };
                            if winutil::relaunch_elevated(arg) {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                        if !narrow {
                            ui.colored_label(egui::Color32::from_rgb(200, 140, 0), "Eingeschränkt");
                        }
                    }
                });
            });
            ui.add_space(2.0);
        });
    }

    /// Hinweis, wenn TaskPulse zuletzt abgestürzt ist.
    fn crash_banner(&mut self, ctx: &egui::Context) {
        // Bericht aus diesem Lauf (z. B. Fehler in einem Hintergrund-Thread) sofort melden
        if let Some(p) = crate::diag::take_new_report() {
            *self.crash_notice.lock().unwrap() = Some(p);
        }
        let Some(p) = self.crash_notice.lock().unwrap().clone() else { return };
        let file = p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let what = if file.starts_with("Absturz") { "TaskPulse ist zuletzt abgestürzt" } else { "In TaskPulse ist ein Fehler aufgetreten" };
        egui::TopBottomPanel::top("crash_banner").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.colored_label(
                egui::Color32::from_rgb(220, 120, 40),
                egui::RichText::new(format!("⚠ {what} – ein Fehlerbericht wurde automatisch erstellt.")).strong(),
            );
            ui.horizontal_wrapped(|ui| {
                ui.label("Datei:");
                ui.label(egui::RichText::new(p.to_string_lossy()).monospace());
            });
            ui.label(
                egui::RichText::new("Hilf mit, den Fehler zu beheben: „Auf GitHub melden“ öffnet eine vorausgefüllte Meldung – zieh die Datei dort einfach hinein. Der Bericht enthält keine Passwörter oder persönlichen Dateien.")
                    .small()
                    .weak(),
            );
            ui.horizontal(|ui| {
                if let Some(url) = crate::diag::github_issue_url(&p) {
                    if ui.button("Auf GitHub melden").on_hover_text("Öffnet GitHub im Browser – gesendet wird erst, wenn du dort auf „Create“ klickst").clicked() {
                        winutil::shell_open(&url);
                        winutil::open_in_explorer(&p.to_string_lossy());
                    }
                }
                if ui.button("Ordner öffnen").clicked() {
                    winutil::open_in_explorer(&p.to_string_lossy());
                }
                if ui.button("Pfad kopieren").clicked() {
                    ui.ctx().copy_text(p.to_string_lossy().to_string());
                }
                if ui.button("Schließen").clicked() {
                    *self.crash_notice.lock().unwrap() = None;
                }
            });
            ui.add_space(4.0);
        });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("{} Prozesse", self.frame.procs.len()));
                ui.separator();
                ui.label(format!("Messung {:.0} ms", self.frame.took_ms));
                if let Some((s, t)) = &self.status {
                    if t.elapsed() < Duration::from_secs(8) {
                        ui.separator();
                        ui.label(s);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.tab == Tab::Processes {
                        processes::end_task_button(self, ui);
                    }
                });
            });
        });
    }
}

impl eframe::App for TaskPulseApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        let ms = if self.bench.recording { 1000 } else { self.speed.ms() };
        self.collector.interval_ms.store(ms, Ordering::Relaxed);
        benchmark::record(self);
        if self.always_on_top_applied != Some(self.cfg.always_on_top) {
            let lvl = if self.cfg.always_on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(lvl));
            self.always_on_top_applied = Some(self.cfg.always_on_top);
        }
        if self.font_applied != self.cfg.ui_font_size {
            apply_font_size(ctx, self.cfg.ui_font_size);
            self.font_applied = self.cfg.ui_font_size;
        }
        if self.last_signal_check.elapsed() > Duration::from_millis(500) {
            self.last_signal_check = Instant::now();
            // Linux: zweiter Start → dieses Fenster nach vorn holen
            #[cfg(target_os = "linux")]
            {
                let front = Config::dir().join("nach_vorn.signal");
                if front.exists() {
                    let _ = std::fs::remove_file(front);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
            }
            let sig = Config::dir().join("open_settings.signal");
            if sig.exists() {
                let _ = std::fs::remove_file(sig);
                self.tab = Tab::Settings;
            }
        }
        self.update.auto_start(ctx, &self.cfg);
        self.top_bar(ctx);
        self.update.banner(ctx);
        self.crash_banner(ctx);
        self.status_bar(ctx);
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Processes => processes::show(self, ui),
            Tab::Performance => performance::show(self, ui),
            Tab::Startup => startup::show(self, ui),
            Tab::Benchmark => benchmark::show(self, ui),
            Tab::Settings => settings::show(self, ui),
        });
        processes::dialogs(self, ctx);
        self.persist();
        // Spätestens jede Sekunde neu zeichnen (Statuszeile, Signale)
        ctx.request_repaint_after(Duration::from_millis(1000));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Laufende FPS-Messung beenden (die Windows-Ereignissitzung würde sonst weiterlaufen)
        self.bench.end();
        if self.cfg != self.saved_cfg {
            let _ = self.cfg.save();
        }
    }
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut names = Vec::new();
    #[cfg(windows)]
    let candidates: Vec<(&str, Vec<String>)> = {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        vec![("segoe", vec![format!(r"{windir}\Fonts\segoeui.ttf")]), ("segoe_sym", vec![format!(r"{windir}\Fonts\seguisym.ttf")])]
    };
    // Linux: verbreitete Schriften (Noto/DejaVu) – sonst bleibt egui bei seiner eingebauten Schrift
    #[cfg(not(windows))]
    let candidates: Vec<(&str, Vec<String>)> = vec![
        (
            "sans",
            [
                "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
                "/usr/share/fonts/noto/NotoSans-Regular.ttf",
                "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/TTF/DejaVuSans.ttf",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        ),
        (
            "symbols",
            ["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/dejavu/DejaVuSans.ttf", "/usr/share/fonts/TTF/DejaVuSans.ttf"].iter().map(|s| s.to_string()).collect(),
        ),
    ];
    for (name, files) in candidates {
        if let Some(bytes) = files.iter().find_map(|f| std::fs::read(f).ok()) {
            fonts.font_data.insert(name.into(), egui::FontData::from_owned(bytes));
            names.push(name);
        }
    }
    if let Some(p) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        for (i, n) in names.iter().enumerate() {
            p.insert(i, n.to_string());
        }
    }
    ctx.set_fonts(fonts);
}

pub fn setup_fonts_pub(ctx: &egui::Context) {
    setup_fonts(ctx);
}

/// Setzt alle Textgrößen aus der Grundschriftgröße (auch die kleinen Beschriftungen).
pub fn apply_font_size(ctx: &egui::Context, size: f32) {
    use egui::{FontId, TextStyle};
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 5.0);
        s.spacing.interact_size.y = size * 1.45;
        s.text_styles.insert(TextStyle::Body, FontId::proportional(size));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(size));
        s.text_styles.insert(TextStyle::Small, FontId::proportional((size * 0.82).round()));
        s.text_styles.insert(TextStyle::Heading, FontId::proportional((size * 1.4).round()));
        s.text_styles.insert(TextStyle::Monospace, FontId::monospace((size * 0.95).round()));
    });
}

/// Programmsymbol für Fenster und Taskleisten-Schaltfläche (128×128 RGBA, aus resources/icon).
pub fn app_icon() -> egui::IconData {
    egui::IconData { rgba: include_bytes!("../../resources/icon/window_128.rgba").to_vec(), width: 128, height: 128 }
}

/// Startet das Hauptfenster (blockiert bis zum Schließen).
pub fn run(page: Option<Tab>) -> eframe::Result<()> {
    #[cfg(target_os = "linux")]
    winutil::register_main_window();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(winutil::MAIN_TITLE)
            .with_inner_size([1180.0, 720.0])
            .with_min_inner_size([720.0, 420.0])
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(winutil::MAIN_TITLE, options, Box::new(move |cc| Ok(Box::new(TaskPulseApp::new(cc, page)))))
}

/// Wurde PawnIO in diesem Lauf über den Knopf installiert? (Dann bleibt die Erfolgsmeldung sichtbar.)
pub fn pawnio_state_done() -> bool {
    #[cfg(windows)]
    {
        matches!(crate::pawnio_setup::state(), crate::pawnio_setup::State::Done)
    }
    #[cfg(not(windows))]
    false
}

/// Knopf „PawnIO installieren“ mit Fortschritt (Windows x64; sonst nichts).
/// Lädt den signierten Installer des PawnIO-Entwicklers und startet ihn mit Adminrechten.
pub fn pawnio_install_button(ui: &mut egui::Ui) {
    #[cfg(windows)]
    {
        use crate::pawnio_setup::{start, state, State};
        if crate::winutil::native_arm64() {
            return;
        }
        match state() {
            State::Working(t) => {
                ui.spinner();
                ui.label(egui::RichText::new(t).small());
            }
            State::Done => {
                ui.label(egui::RichText::new("✔ PawnIO installiert").small().color(egui::Color32::from_rgb(40, 170, 110)));
                if !crate::winutil::is_elevated() {
                    ui.label(egui::RichText::new("– für die Werte TaskPulse mit Adminrechten starten (oben „Als Admin“)").small().weak());
                }
            }
            s => {
                if crate::installer::pawnio_installed() {
                    return;
                }
                let ctx = ui.ctx().clone();
                if ui
                    .button("PawnIO installieren")
                    .on_hover_text("Lädt den offiziellen PawnIO-Installer (github.com/namazso/PawnIO.Setup), prüft seine digitale Signatur und startet ihn – Windows fragt nach Adminrechten.")
                    .clicked()
                {
                    start(move || ctx.request_repaint());
                }
                if ui.small_button("pawnio.eu").on_hover_text("Webseite von PawnIO öffnen").clicked() {
                    crate::winutil::shell_open("https://pawnio.eu/");
                }
                if let State::Failed(e) = s {
                    ui.label(egui::RichText::new(format!("Fehler: {e}")).small().color(egui::Color32::from_rgb(220, 120, 40)));
                }
            }
        }
    }
    #[cfg(not(windows))]
    let _ = ui;
}
