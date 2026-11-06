//! Reiter „Autostart“: Programme, die beim Anmelden starten – übersichtlich getrennt nach
//! eingeschaltet und ausgeschaltet, mit Hinzufügen, Ein-/Ausschalten und Entfernen.

use super::TaskPulseApp;
use crate::sensors::startup::{self, Source, StartupEntry};
use eframe::egui::{self, Color32, RichText, Sense};
use egui_extras::{Column, TableBuilder};

#[derive(Default)]
pub struct StartupState {
    entries: Vec<StartupEntry>,
    loaded: bool,
    search: String,
    /// Meldung nach einer Aktion: (Erfolg, Text)
    message: Option<(bool, String)>,
    add_open: bool,
    add_name: String,
    add_path: String,
    add_args: String,
    confirm_remove: Option<StartupEntry>,
}

enum Act {
    Toggle(StartupEntry, bool),
    Remove(StartupEntry),
    OpenPath(String),
    Search(String),
}

const C_ON: Color32 = Color32::from_rgb(80, 190, 110);
const C_OFF: Color32 = Color32::from_rgb(150, 150, 150);

/// Technischer Name (z. B. „MicrosoftEdgeAutoLaunch_B2E2…“, „com.overwolf.curseforge“) → lieber die Beschreibung
fn looks_technical(name: &str) -> bool {
    let hex_tail = name.rsplit('_').next().map(|t| t.len() >= 16 && t.chars().all(|c| c.is_ascii_hexdigit())).unwrap_or(false);
    hex_tail || name.starts_with("com.") || (name.contains('.') && !name.contains(' ') && name.len() > 12)
}

fn title_of(e: &StartupEntry) -> String {
    match &e.description {
        Some(d) if looks_technical(&e.name) => d.clone(),
        _ => e.name.clone(),
    }
}

/// Zweite Zeile: Beschreibung (falls anders als der Titel) bzw. Fehlerhinweis
fn subtitle_of(e: &StartupEntry) -> Option<String> {
    if e.missing {
        return Some("⚠ Programmdatei nicht gefunden – Eintrag ist verwaist".into());
    }
    let t = title_of(e);
    e.description.clone().filter(|d| !d.eq_ignore_ascii_case(&t) && !d.to_lowercase().ends_with(".exe")).or_else(|| looks_technical(&e.name).then(|| e.name.clone()).filter(|n| *n != t))
}

impl StartupState {
    pub fn mark_stale(&mut self) {
        self.loaded = false;
        self.message = None;
    }
}

fn reload(st: &mut StartupState) {
    st.entries = startup::list();
    st.loaded = true;
}

pub fn show(app: &mut TaskPulseApp, ui: &mut egui::Ui) {
    let admin = app.admin;
    let st = &mut app.startup;
    if !st.loaded {
        reload(st);
    }
    let on = st.entries.iter().filter(|e| e.enabled).count();
    let off = st.entries.len() - on;

    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut st.search).hint_text("🔍 Name, Herausgeber oder Befehl suchen").desired_width(300.0));
        if !st.search.is_empty() && ui.small_button("✕").clicked() {
            st.search.clear();
        }
        ui.separator();
        ui.label(RichText::new(format!("{on} eingeschaltet")).color(C_ON).strong());
        ui.label(RichText::new(format!("{off} ausgeschaltet")).color(C_OFF).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("⟳ Aktualisieren").clicked() {
                reload(st);
                st.message = None;
            }
            if ui.button("+ App hinzufügen …").clicked() {
                st.add_open = true;
                st.add_name.clear();
                st.add_path.clear();
                st.add_args.clear();
            }
        });
    });
    if let Some((ok, m)) = &st.message {
        ui.label(RichText::new(m).color(if *ok { C_ON } else { Color32::from_rgb(230, 90, 80) }));
    }
    if !admin && cfg!(windows) && st.entries.iter().any(|e| e.source.needs_admin()) {
        ui.label(RichText::new("Einträge „für alle Benutzer“ lassen sich nur mit Adminrechten ändern (oben „Als Admin“).").small().weak());
    }
    ui.add_space(4.0);

    let q = st.search.to_lowercase();
    let matches = |e: &StartupEntry| {
        q.is_empty()
            || title_of(e).to_lowercase().contains(&q)
            || e.name.to_lowercase().contains(&q)
            || e.publisher.as_deref().unwrap_or("").to_lowercase().contains(&q)
            || e.command.to_lowercase().contains(&q)
    };
    let shown_on: Vec<StartupEntry> = st.entries.iter().filter(|e| e.enabled && matches(e)).cloned().collect();
    let shown_off: Vec<StartupEntry> = st.entries.iter().filter(|e| !e.enabled && matches(e)).cloned().collect();
    let mut acts: Vec<Act> = Vec::new();

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.style_mut().interaction.selectable_labels = false;
        for (title, list, color, id) in [("Eingeschaltet – startet beim Anmelden", &shown_on, C_ON, "on"), ("Ausgeschaltet", &shown_off, C_OFF, "off")] {
            egui::CollapsingHeader::new(RichText::new(format!("{title} ({})", list.len())).strong().size(17.0).color(color)).id_salt(("startup", id)).default_open(true).show(ui, |ui| {
                if list.is_empty() {
                    ui.label(RichText::new(if q.is_empty() { "Keine Einträge." } else { "Keine Treffer." }).weak());
                    return;
                }
                table(ui, id, list, admin, &mut acts);
            });
            ui.add_space(8.0);
        }
        ui.label(
            RichText::new("Ausschalten wirkt wie im Windows-Task-Manager: Der Eintrag bleibt erhalten und lässt sich jederzeit wieder einschalten. Änderungen gelten ab der nächsten Anmeldung.")
                .small()
                .weak(),
        );
    });

    for a in acts {
        match a {
            Act::Toggle(e, v) => {
                st.message = Some(match startup::set_enabled(&e, v) {
                    Ok(()) => (true, format!("„{}“ {}.", title_of(&e), if v { "eingeschaltet – startet ab der nächsten Anmeldung" } else { "ausgeschaltet" })),
                    Err(err) => (false, format!("„{}“ konnte nicht geändert werden: {err}", title_of(&e))),
                });
                reload(st);
            }
            Act::Remove(e) => st.confirm_remove = Some(e),
            Act::OpenPath(p) => crate::winutil::open_in_explorer(&p),
            Act::Search(t) => crate::winutil::search_online(&t),
        }
    }
    let ctx = ui.ctx().clone();
    add_dialog(app, &ctx);
    remove_dialog(&mut app.startup, &ctx);
}

fn table(ui: &mut egui::Ui, id: &str, list: &[StartupEntry], admin: bool, acts: &mut Vec<Act>) {
    let w = ui.available_width();
    let rest = (w - 70.0 - 24.0).max(400.0);
    TableBuilder::new(ui)
        .id_salt(("startup_table", id))
        // Kein eigener Scrollbereich in der Seite (sonst abgeschnitten, unten nicht erreichbar) – es scrollt die ganze Seite
        .vscroll(false)
        .striped(true)
        .sense(Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(70.0))
        .column(Column::exact(rest * 0.34).clip(true))
        .column(Column::exact(rest * 0.2).clip(true))
        .column(Column::exact(rest * 0.18).clip(true))
        .column(Column::remainder().clip(true))
        .header(24.0, |mut h| {
            for t in ["", "Name", "Herausgeber", "Quelle", "Befehl"] {
                h.col(|ui| {
                    ui.label(RichText::new(t).weak());
                });
            }
        })
        .body(|body| {
            body.rows(40.0, list.len(), |mut row| {
                let e = &list[row.index()];
                let locked = cfg!(windows) && e.source.needs_admin() && !admin;
                row.col(|ui| {
                    let mut v = e.enabled;
                    let r = ui.add_enabled(!locked, toggle(&mut v));
                    let r = if locked { r.on_disabled_hover_text("Gilt für alle Benutzer – nur mit Adminrechten änderbar") } else { r.on_hover_text(if e.enabled { "Ausschalten" } else { "Einschalten" }) };
                    if r.changed() {
                        acts.push(Act::Toggle(e.clone(), v));
                    }
                });
                row.col(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(title_of(e)).strong());
                        if let Some(s) = subtitle_of(e) {
                            ui.label(RichText::new(s).small().color(if e.missing { Color32::from_rgb(230, 160, 60) } else { ui.visuals().weak_text_color() }));
                        } else if let Some(d) = &e.disabled_since {
                            ui.label(RichText::new(format!("aus seit {d}")).small().weak());
                        }
                    });
                });
                row.col(|ui| {
                    ui.label(e.publisher.clone().unwrap_or("–".into()));
                });
                row.col(|ui| {
                    ui.label(RichText::new(e.source.label()).weak());
                });
                row.col(|ui| {
                    ui.label(RichText::new(&e.command).small().weak()).on_hover_text(&e.command);
                });
                let resp = row.response();
                resp.context_menu(|ui| {
                    let label = if e.enabled { "Ausschalten" } else { "Einschalten" };
                    if ui.add_enabled(!locked, egui::Button::new(label)).clicked() {
                        acts.push(Act::Toggle(e.clone(), !e.enabled));
                        ui.close_menu();
                    }
                    if let Some(p) = e.path.clone().filter(|_| !e.missing) {
                        if ui.button("Dateipfad öffnen").clicked() {
                            acts.push(Act::OpenPath(p));
                            ui.close_menu();
                        }
                    }
                    if ui.button("Online suchen").clicked() {
                        acts.push(Act::Search(title_of(e)));
                        ui.close_menu();
                    }
                    ui.separator();
                    let removable = !matches!(e.source, Source::StoreApp | Source::DesktopSystem) && !locked;
                    if ui.add_enabled(removable, egui::Button::new("Aus dem Autostart entfernen …")).on_disabled_hover_text("Store-Apps und systemweite Einträge lassen sich nur ausschalten (bzw. nur mit Adminrechten ändern)").clicked() {
                        acts.push(Act::Remove(e.clone()));
                        ui.close_menu();
                    }
                });
            });
        });
}

/// Schalter (an/aus) wie in den Windows-Einstellungen
fn toggle(on: &mut bool) -> impl egui::Widget + '_ {
    move |ui: &mut egui::Ui| {
        let size = egui::vec2(44.0, 22.0);
        let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
        if resp.clicked() {
            *on = !*on;
            resp.mark_changed();
        }
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let enabled = ui.is_enabled();
        let bg = if *on { C_ON } else { ui.visuals().widgets.inactive.bg_fill };
        let bg = if enabled { bg } else { bg.linear_multiply(0.4) };
        ui.painter().rect_filled(rect, 11.0, bg);
        let x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
        ui.painter().circle_filled(egui::pos2(x, rect.center().y), 8.0, if enabled { Color32::WHITE } else { Color32::from_gray(170) });
        resp
    }
}

fn add_dialog(app: &mut TaskPulseApp, ctx: &egui::Context) {
    if !app.startup.add_open {
        return;
    }
    // Laufende Apps mit Programmdatei (zum schnellen Übernehmen)
    let mut running: Vec<(String, String)> = app
        .frame
        .procs
        .iter()
        .filter(|p| p.category == crate::sensors::procs::Category::App)
        .filter_map(|p| p.path.clone().map(|path| (p.display.clone(), path)))
        .collect();
    running.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    running.dedup_by(|a, b| a.1.eq_ignore_ascii_case(&b.1));
    let st = &mut app.startup;
    let mut open = true;
    let mut close = false;
    egui::Window::new("App zum Autostart hinzufügen").open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.label("Programm");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut st.add_path).hint_text(if cfg!(windows) { r"C:\Programme\…\App.exe" } else { "/usr/bin/app" }).desired_width(360.0));
            if ui.button("Durchsuchen …").clicked() {
                if let Some(p) = crate::winutil::open_file_dialog("Programm für den Autostart wählen", &[("Programme", "*.exe;*.bat;*.cmd;*.lnk"), ("Alle Dateien", "*.*")]) {
                    st.add_path = p.to_string_lossy().to_string();
                    if st.add_name.is_empty() {
                        st.add_name = default_name(&st.add_path);
                    }
                }
            }
        });
        if !running.is_empty() {
            egui::ComboBox::from_id_salt("startup_running").selected_text("… oder eine laufende App übernehmen").width(360.0).show_ui(ui, |ui| {
                for (name, path) in &running {
                    if ui.selectable_label(false, name).on_hover_text(path).clicked() {
                        st.add_path = path.clone();
                        st.add_name = name.clone();
                    }
                }
            });
        }
        ui.add_space(6.0);
        ui.label("Name im Autostart");
        ui.add(egui::TextEdit::singleline(&mut st.add_name).desired_width(360.0));
        ui.label("Startparameter (optional, z. B. --minimized)");
        ui.add(egui::TextEdit::singleline(&mut st.add_args).desired_width(360.0));
        ui.add_space(4.0);
        ui.label(RichText::new("Wird nur für dich eingetragen (Registrierung „Run“ des aktuellen Benutzers) – keine Adminrechte nötig.").small().weak());
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let ready = !st.add_path.trim().is_empty() && !st.add_name.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new("Hinzufügen")).clicked() {
                let path = st.add_path.trim().trim_matches('"').to_string();
                match startup::add(&st.add_name, &path, &st.add_args) {
                    Ok(()) => {
                        st.message = Some((true, format!("„{}“ startet ab der nächsten Anmeldung automatisch.", st.add_name.trim())));
                        close = true;
                    }
                    Err(e) => st.message = Some((false, e)),
                }
            }
            if ui.button("Abbrechen").clicked() {
                close = true;
            }
        });
        if let Some((false, m)) = &st.message {
            ui.label(RichText::new(m).color(Color32::from_rgb(230, 90, 80)));
        }
    });
    if close || !open {
        st.add_open = false;
        reload(st);
    }
}

/// Vorschlag für den Namen: Dateibeschreibung der Programmdatei, sonst Dateiname
fn default_name(path: &str) -> String {
    #[cfg(windows)]
    if let (Some(d), _) = startup::version_info(path) {
        return d;
    }
    std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

fn remove_dialog(st: &mut StartupState, ctx: &egui::Context) {
    let Some(e) = st.confirm_remove.clone() else { return };
    let mut keep_open = true;
    egui::Window::new("Aus dem Autostart entfernen?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.label(format!("„{}“ wird ganz aus dem Autostart gelöscht ({}).", title_of(&e), e.source.label()));
        ui.label(RichText::new("Das Programm selbst bleibt installiert. Nur ausschalten lässt sich jederzeit rückgängig machen – Entfernen nicht.").small().weak());
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Nur ausschalten").clicked() {
                st.message = Some(match startup::set_enabled(&e, false) {
                    Ok(()) => (true, format!("„{}“ ausgeschaltet.", title_of(&e))),
                    Err(err) => (false, err),
                });
                keep_open = false;
            }
            if ui.button(RichText::new("Entfernen").color(Color32::from_rgb(230, 90, 80))).clicked() {
                st.message = Some(match startup::remove(&e) {
                    Ok(()) => (true, format!("„{}“ aus dem Autostart entfernt.", title_of(&e))),
                    Err(err) => (false, err),
                });
                keep_open = false;
            }
            if ui.button("Abbrechen").clicked() {
                keep_open = false;
            }
        });
    });
    if !keep_open {
        st.confirm_remove = None;
        reload(st);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn technical_names() {
        assert!(looks_technical("MicrosoftEdgeAutoLaunch_B2E206152823CA825CAB5FC835C2FD28"));
        assert!(looks_technical("com.overwolf.curseforge"));
        assert!(!looks_technical("Steam"));
        assert!(!looks_technical("Stream Deck"));
        assert!(!looks_technical("Volume Controller SD plugin"));
    }
}
