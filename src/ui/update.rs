//! Updates im Hauptfenster: Hinweisleiste und Abschnitt in den Einstellungen.

use crate::config::Config;
use crate::updater::{self, Release};
use eframe::egui::{self, RichText};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub enum Phase {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Working(String),
    Error(String),
}

pub struct UpdateUi {
    pub phase: Arc<Mutex<Phase>>,
    /// Meldung des zuletzt eingespielten Updates
    pub last_result: Option<String>,
    pub banner_hidden: bool,
    started: bool,
}

impl Default for UpdateUi {
    fn default() -> Self {
        UpdateUi { phase: Arc::new(Mutex::new(Phase::Idle)), last_result: updater::take_last_result(), banner_hidden: false, started: false }
    }
}

const GREEN: egui::Color32 = egui::Color32::from_rgb(40, 170, 110);
const ORANGE: egui::Color32 = egui::Color32::from_rgb(200, 140, 0);
const RED: egui::Color32 = egui::Color32::from_rgb(220, 70, 60);

impl UpdateUi {
    pub fn phase(&self) -> Phase {
        self.phase.lock().unwrap().clone()
    }

    /// Einmal beim Start automatisch prüfen (nur installierte Version, nur wenn eingeschaltet).
    pub fn auto_start(&mut self, ctx: &egui::Context, cfg: &Config) {
        if self.started {
            return;
        }
        self.started = true;
        if cfg.auto_update && updater::running_installed() {
            self.check(ctx, cfg);
        }
    }

    pub fn check(&self, ctx: &egui::Context, cfg: &Config) {
        *self.phase.lock().unwrap() = Phase::Checking;
        let (p, ctx, cfg) = (self.phase.clone(), ctx.clone(), cfg.clone());
        std::thread::spawn(move || {
            let r = match updater::check(&cfg) {
                Ok(Some(r)) => Phase::Available(r),
                Ok(None) => Phase::UpToDate,
                Err(e) => Phase::Error(e),
            };
            *p.lock().unwrap() = r;
            ctx.request_repaint();
        });
    }

    pub fn install(&self, ctx: &egui::Context, r: Release) {
        *self.phase.lock().unwrap() = Phase::Working(format!("Lade Version {} herunter …", r.version));
        let (p, ctx) = (self.phase.clone(), ctx.clone());
        std::thread::spawn(move || {
            let res = updater::download(&r).and_then(|f| updater::apply(&f, true));
            *p.lock().unwrap() = match res {
                Ok(()) => Phase::Working("Update wird installiert – TaskPulse startet gleich neu …".into()),
                Err(e) => Phase::Error(e),
            };
            ctx.request_repaint();
        });
    }

    /// Hinweisleiste oben im Hauptfenster.
    pub fn banner(&mut self, ctx: &egui::Context) {
        let phase = self.phase();
        let show_avail = matches!(phase, Phase::Available(_)) && !self.banner_hidden;
        if !show_avail && self.last_result.is_none() {
            return;
        }
        egui::TopBottomPanel::top("update_banner").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if let Some(msg) = self.last_result.clone() {
                    ui.colored_label(if msg.starts_with('✔') { GREEN } else { RED }, msg);
                    if ui.small_button("OK").clicked() {
                        self.last_result = None;
                    }
                } else if let Phase::Available(r) = phase {
                    ui.colored_label(GREEN, RichText::new(format!("⬆ Update verfügbar: TaskPulse {} (installiert: {})", r.version, updater::CURRENT)).strong());
                    if ui.button("Jetzt installieren").clicked() {
                        self.install(ctx, r.clone());
                    }
                    let notes = if r.notes.trim().is_empty() { "Keine Beschreibung".to_string() } else { r.notes.chars().take(1500).collect() };
                    if !r.page_url.is_empty() && ui.button("Was ist neu?").on_hover_text(notes).clicked() {
                        crate::winutil::shell_open(&r.page_url);
                    }
                    if ui.small_button("Später").clicked() {
                        self.banner_hidden = true;
                    }
                }
            });
        });
    }

    /// Abschnitt "Updates" in den Einstellungen.
    pub fn settings(&mut self, ui: &mut egui::Ui, cfg: &mut Config) {
        let installed = updater::running_installed();
        ui.checkbox(&mut cfg.auto_update, "Automatisch nach Updates suchen und installieren");
        ui.label(
            RichText::new(
                "Prüft beim Start und alle 12 Stunden die Veröffentlichungen auf GitHub. Ist das Hauptfenster zu, spielt die Taskleisten-Anzeige \
                 ein Update selbst ein; ist es offen, erscheint oben ein Hinweis. Ohne Adminrechte fragt Windows dabei einmal nach.",
            )
            .small()
            .weak(),
        );
        ui.horizontal(|ui| {
            ui.label("GitHub-Repository");
            let hint = updater::repo_from_url(&updater::builtin_repo()).unwrap_or_else(|| "Besitzer/TaskPulse".into());
            ui.add(egui::TextEdit::singleline(&mut cfg.update_repo).hint_text(hint).desired_width(260.0));
        });
        let repo = updater::repo(cfg);
        if repo.is_none() {
            ui.colored_label(ORANGE, "Kein Repository eingetragen – Updates sind aus, bis hier „Besitzer/Name“ steht.");
        }
        ui.add_space(4.0);
        let phase = self.phase();
        ui.horizontal(|ui| {
            ui.label(format!("Installiert: {}", updater::CURRENT));
            let busy = matches!(phase, Phase::Checking | Phase::Working(_));
            if ui.add_enabled(!busy && repo.is_some(), egui::Button::new("Jetzt nach Updates suchen")).clicked() {
                self.check(ui.ctx(), cfg);
            }
            match &phase {
                Phase::Idle => {}
                Phase::Checking => {
                    ui.spinner();
                    ui.label("Prüfe …");
                }
                Phase::UpToDate => {
                    ui.colored_label(GREEN, "✔ Aktuell");
                }
                Phase::Available(r) => {
                    ui.colored_label(GREEN, format!("Version {} verfügbar", r.version));
                    if ui.add_enabled(installed, egui::Button::new(RichText::new("Installieren").strong())).clicked() {
                        self.install(ui.ctx(), r.clone());
                    }
                    if !r.page_url.is_empty() && ui.link("Was ist neu?").clicked() {
                        crate::winutil::shell_open(&r.page_url);
                    }
                }
                Phase::Working(s) => {
                    ui.spinner();
                    ui.label(s);
                }
                Phase::Error(e) => {
                    ui.colored_label(RED, e);
                }
            }
        });
        if !installed {
            ui.label(
                RichText::new("Diese TaskPulse-Datei ist nicht die installierte Version (z. B. Entwicklungsordner) – sie wird nicht automatisch ersetzt. Prüfen geht trotzdem.")
                    .small()
                    .color(ORANGE),
            );
        }
    }
}
