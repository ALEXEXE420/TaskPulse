//! Linux-Gegenstück zur Taskleisten-Anzeige: Unter Linux gibt es keine einheitliche Taskleiste,
//! in die sich ein Programm einbetten kann (GNOME, KDE, XFCE … machen das alle anders). TaskPulse
//! zeigt die Werte deshalb in einem kleinen, randlosen Fenster, das immer im Vordergrund bleibt –
//! dieselben Felder und Texte wie unter Windows (layout.rs ist gemeinsam).
//!
//! Bedienung: ziehen = verschieben, Doppelklick = Hauptfenster, Rechtsklick = Menü.
#![allow(dead_code)]

#[path = "../taskbar/layout.rs"]
pub mod layout;

use crate::config::Config;
use crate::sensors::{Basic, BasicSensors};
use eframe::egui::{self, Color32, FontId, RichText};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn pid_file() -> std::path::PathBuf {
    Config::dir().join("anzeige.pid")
}

/// Läuft die Anzeige schon (in einem anderen Prozess)?
pub fn running() -> bool {
    let Some(pid) = std::fs::read_to_string(pid_file()).ok().and_then(|s| s.trim().parse::<u32>().ok()) else { return false };
    pid != std::process::id()
        && std::fs::read_to_string(format!("/proc/{pid}/cmdline")).map(|c| c.contains("--taskbar") || c.to_lowercase().contains("taskpulse")).unwrap_or(false)
}

/// Unter Linux gibt es keine Taskleisten-Fenster zum Einbetten.
pub fn taskbars() -> Vec<u32> {
    Vec::new()
}

pub fn request_elevated_restart() {}

pub fn describe_taskbars() -> String {
    "Linux: keine Taskleisten-Einbettung – TaskPulse zeigt ein eigenes, schwebendes Anzeigefenster.".into()
}

pub fn render_test(_dark: &std::path::Path, _light: &std::path::Path) -> Result<(), String> {
    Err("Unter Linux nicht verfügbar".into())
}

struct Anzeige {
    cfg: Config,
    cfg_mtime: Option<std::time::SystemTime>,
    data: Arc<Mutex<Basic>>,
    shared_cfg: Arc<Mutex<Config>>,
    last_check: Instant,
    last_pos: Option<egui::Pos2>,
}

impl eframe::App for Anzeige {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        [0.08, 0.08, 0.09, 0.92]
    }

    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        // Einstellungen neu laden, wenn das Hauptfenster sie geändert hat
        if self.last_check.elapsed() > Duration::from_secs(2) {
            self.last_check = Instant::now();
            let m = Config::modified();
            if m != self.cfg_mtime {
                self.cfg_mtime = m;
                self.cfg = Config::load();
                *self.shared_cfg.lock().unwrap() = self.cfg.clone();
                if !self.cfg.taskbar.enabled {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            // Position merken (unter X11; Wayland verrät sie nicht)
            if let Some(r) = ctx.input(|i| i.viewport().outer_rect) {
                if self.last_pos != Some(r.min) {
                    self.last_pos = Some(r.min);
                    let mut c = Config::load();
                    if (c.taskbar.float_x, c.taskbar.float_y) != (r.min.x as i32, r.min.y as i32) {
                        c.taskbar.float_x = r.min.x as i32;
                        c.taskbar.float_y = r.min.y as i32;
                        let _ = c.save();
                        self.cfg_mtime = Config::modified();
                    }
                }
            }
        }
        let b = self.data.lock().unwrap().clone();
        let cfg = &self.cfg;
        let size = cfg.taskbar.font_size_pt.clamp(7.0, 14.0) * 1.45;
        let fields = cfg.taskbar.visible_fields();
        let resp = egui::CentralPanel::default()
            .frame(egui::Frame::none().inner_margin(egui::Margin::symmetric(8.0, 3.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for col in layout::columns(&fields) {
                        ui.vertical(|ui| {
                            for f in col {
                                let c = layout::cell(f, &b, cfg);
                                let t = match c.extra {
                                    Some(e) => format!("{}  {e}", c.text),
                                    None => c.text,
                                };
                                ui.label(RichText::new(t).font(FontId::proportional(size)).color(Color32::from_gray(235)));
                            }
                        });
                        ui.add_space(10.0);
                    }
                });
            })
            .response;
        let resp = resp.interact(egui::Sense::click_and_drag());
        if resp.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if resp.double_clicked() {
            crate::winutil::open_main_window(false);
        }
        resp.context_menu(|ui| {
            if ui.button("Hauptfenster öffnen").clicked() {
                crate::winutil::open_main_window(false);
                ui.close_menu();
            }
            if ui.button("Einstellungen").clicked() {
                crate::winutil::open_main_window(true);
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Anzeige beenden").clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        // Fenstergröße an den Inhalt anpassen
        let want = ctx.used_size() + egui::vec2(2.0, 2.0);
        if let Some(r) = ctx.input(|i| i.viewport().inner_rect) {
            if (r.size() - want).length() > 2.0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(want));
            }
        }
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

/// Startet die schwebende Anzeige (blockiert bis zum Schließen).
pub fn run() {
    if running() {
        return; // nur eine Anzeige
    }
    let _ = std::fs::create_dir_all(Config::dir());
    let _ = std::fs::write(pid_file(), std::process::id().to_string());
    let cfg = Config::load();
    let data = Arc::new(Mutex::new(Basic::default()));
    let shared_cfg = Arc::new(Mutex::new(cfg.clone()));
    {
        let (data, shared_cfg) = (data.clone(), shared_cfg.clone());
        std::thread::spawn(move || {
            let mut sensors = BasicSensors::new(&shared_cfg.lock().unwrap().clone());
            loop {
                let t0 = Instant::now();
                let c = shared_cfg.lock().unwrap().clone();
                *data.lock().unwrap() = sensors.sample(&c);
                let iv = c.taskbar.interval();
                std::thread::sleep(iv.saturating_sub(t0.elapsed()));
            }
        });
    }
    crate::updater::background_thread();
    let mut vp = egui::ViewportBuilder::default()
        .with_title("TaskPulse Anzeige")
        .with_decorations(false)
        .with_always_on_top()
        .with_resizable(false)
        .with_taskbar(false)
        .with_transparent(true)
        .with_inner_size([600.0, 46.0])
        .with_icon(crate::ui::app_icon());
    if cfg.taskbar.float_x != 0 || cfg.taskbar.float_y != 0 {
        vp = vp.with_position([cfg.taskbar.float_x as f32, cfg.taskbar.float_y as f32]);
    }
    let app = Anzeige { cfg_mtime: Config::modified(), cfg, data, shared_cfg, last_check: Instant::now(), last_pos: None };
    let _ = eframe::run_native(
        "TaskPulse Anzeige",
        eframe::NativeOptions { viewport: vp, ..Default::default() },
        Box::new(move |cc| {
            crate::ui::setup_fonts_pub(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    );
    let _ = std::fs::remove_file(pid_file());
}
