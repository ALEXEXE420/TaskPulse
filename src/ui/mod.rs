//! Hauptfenster (egui): Prozesse, Leistung, Einstellungen.

mod performance;
mod processes;
mod settings;
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
                let mut seq = 0u64;
                loop {
                    let iv = interval_ms.load(Ordering::Relaxed);
                    if iv == 0 {
                        std::thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                    let t0 = Instant::now();
                    let c = cfg.lock().unwrap().clone();
                    let b = basic.sample(&c);
                    let d = disk.as_mut().map(|d| d.sample()).unwrap_or_default();
                    let g = gpu.as_mut().map(|g| g.sample()).unwrap_or_default();
                    let p = procs.sample();
                    let adapters = gpu.as_ref().map(|g| g.adapters.clone()).unwrap_or_default();
                    let vram_total = b.gpu.mem_total.or_else(|| adapters.iter().map(|a| a.dedicated).reduce(f64::max));
                    let vram_used = b.gpu.mem_used.or(g.adapter_dedicated_used);
                    seq += 1;
                    let took_ms = t0.elapsed().as_secs_f64() * 1000.0;
                    *frame.lock().unwrap() =
                        Some(Frame { seq, basic: b, disk: d, gpu: g, procs: p, adapters, vram_total, vram_used, took_ms });
                    ctx.request_repaint();
                    let spent = t0.elapsed();
                    let iv = Duration::from_millis(iv);
                    if spent < iv {
                        std::thread::sleep(iv - spent);
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
            perf: performance::PerfState { item: if std::env::args().any(|a| a == "gpu") { performance::Item::Gpu } else { performance::Item::Cpu }, per_core: false },
            settings: settings::SettingsState::default(),
            update: update::UpdateUi::default(),
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
                ui.selectable_value(&mut self.tab, Tab::Settings, "Einstellungen");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.checkbox(&mut self.cfg.always_on_top, "Immer im Vordergrund").changed() {}
                    egui::ComboBox::from_id_salt("speed").selected_text(self.speed.label()).show_ui(ui, |ui| {
                        for s in [Speed::High, Speed::Normal, Speed::Low, Speed::Paused] {
                            ui.selectable_value(&mut self.speed, s, s.label());
                        }
                    });
                    ui.label("Aktualisierung:");
                    if !self.admin {
                        if ui
                            .button("🛡 Mit Adminrechten neu starten")
                            .on_hover_text("Ohne Adminrechte fehlen CPU-Temperatur, einige Prozessdaten und Aktionen.")
                            .clicked()
                        {
                            let arg = if self.tab == Tab::Settings { "--settings" } else { "" };
                            if winutil::relaunch_elevated(arg) {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                        ui.colored_label(egui::Color32::from_rgb(200, 140, 0), "Eingeschränkt");
                    }
                });
            });
            ui.add_space(2.0);
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
        self.collector.interval_ms.store(self.speed.ms(), Ordering::Relaxed);
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
            let sig = Config::dir().join("open_settings.signal");
            if sig.exists() {
                let _ = std::fs::remove_file(sig);
                self.tab = Tab::Settings;
            }
        }
        self.update.auto_start(ctx, &self.cfg);
        self.top_bar(ctx);
        self.update.banner(ctx);
        self.status_bar(ctx);
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Processes => processes::show(self, ui),
            Tab::Performance => performance::show(self, ui),
            Tab::Settings => settings::show(self, ui),
        });
        processes::dialogs(self, ctx);
        self.persist();
        // Spätestens jede Sekunde neu zeichnen (Statuszeile, Signale)
        ctx.request_repaint_after(Duration::from_millis(1000));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.cfg != self.saved_cfg {
            let _ = self.cfg.save();
        }
    }
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let mut names = Vec::new();
    for (name, file) in [("segoe", "segoeui.ttf"), ("segoe_sym", "seguisym.ttf")] {
        if let Ok(bytes) = std::fs::read(format!(r"{windir}\Fonts\{file}")) {
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
