//! Seite "Einstellungen" und Dialog "Neue Aufgabe ausführen".

use super::TaskPulseApp;
use crate::autostart;
use crate::config::{CpuFreqMode, Field, GpuMemMode, MemUnit, NetUnit, Side, Threshold};
use crate::sensors::net;
use crate::winutil;
use eframe::egui::{self, RichText};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Default)]
pub struct SettingsState {
    pub adapters: Vec<net::Adapter>,
    pub adapters_at: Option<Instant>,
    pub autostart_actual: Option<bool>,
    pub autostart_job: Option<Arc<Mutex<Option<Result<bool, String>>>>>,
    pub admin_actual: Option<bool>,
    pub admin_job: Option<Arc<Mutex<Option<Result<bool, String>>>>>,
    pub taskbars: usize,
    pub gpus: Vec<crate::sensors::gpu_proc::DxgiAdapter>,
    pub run_dialog: Option<String>,
    pub run_admin: bool,
}

/// `--section NAME`: beim Öffnen einmal zu diesem Abschnitt scrollen (Tests, Bildschirmfotos).
fn scroll_target() -> Option<&'static str> {
    static T: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let a: Vec<String> = std::env::args().collect();
        a.iter().position(|x| x == "--section").and_then(|i| a.get(i + 1)).map(|s| s.to_lowercase())
    })
    .as_deref()
}

static SCROLLED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(6.0);
    // erst nach ein paar Bildern scrollen, wenn das Layout steht
    if scroll_target().map(|t| title.to_lowercase().starts_with(t)).unwrap_or(false) {
        let n = SCROLLED.load(std::sync::atomic::Ordering::Relaxed);
        if n < 10 {
            SCROLLED.store(n + 1, std::sync::atomic::Ordering::Relaxed);
            ui.scroll_to_cursor(Some(egui::Align::TOP));
            ui.ctx().request_repaint();
        }
    }
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(title).strong().heading());
        ui.add_space(4.0);
        add(ui);
    });
}

fn threshold(ui: &mut egui::Ui, label: &str, t: &mut Threshold, unit: &str, max: f64) {
    ui.label(label);
    ui.add(egui::DragValue::new(&mut t.start).range(0.0..=max).speed(0.5).suffix(format!(" {unit}")));
    ui.label("bis");
    ui.add(egui::DragValue::new(&mut t.full).range(0.0..=max).speed(0.5).suffix(format!(" {unit}")));
    ui.end_row();
}

pub fn taskbar_running() -> bool {
    use windows::core::w;
    use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    unsafe {
        match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, w!("Local\\TaskPulse.Taskbar")) {
            Ok(h) => {
                let _ = windows::Win32::Foundation::CloseHandle(h);
                true
            }
            // Sperre gehört einer Instanz mit Adminrechten → läuft
            Err(e) => e.code() == windows::Win32::Foundation::ERROR_ACCESS_DENIED.to_hresult(),
        }
    }
}

pub fn show(app: &mut TaskPulseApp, ui: &mut egui::Ui) {
    let st = &mut app.settings;
    if st.adapters_at.map(|t| t.elapsed().as_secs() > 5).unwrap_or(true) {
        st.adapters = net::list_adapters();
        st.adapters_at = Some(Instant::now());
        st.taskbars = crate::taskbar::taskbars().len();
        if st.gpus.is_empty() {
            st.gpus = crate::sensors::gpu_proc::dxgi_adapters();
        }
    }
    if st.autostart_actual.is_none() && st.autostart_job.is_none() {
        st.autostart_actual = Some(autostart::is_enabled());
    }
    // Ergebnis eines laufenden Autostart-Auftrags abholen
    if let Some(job) = &st.autostart_job {
        let r = job.lock().unwrap().take();
        if let Some(r) = r {
            st.autostart_job = None;
            match r {
                Ok(on) => {
                    st.autostart_actual = Some(on);
                    app.cfg.autostart = on;
                }
                Err(e) => {
                    st.autostart_actual = Some(autostart::is_enabled());
                    app.cfg.autostart = st.autostart_actual.unwrap_or(false);
                    app.status = Some((format!("Autostart: {e}"), Instant::now()));
                }
            }
        }
    }
    if st.admin_actual.is_none() && st.admin_job.is_none() {
        st.admin_actual = Some(autostart::admin_is_enabled());
        // Einstellung und tatsächliche Aufgabe abgleichen
        app.cfg.always_admin = st.admin_actual.unwrap_or(false);
    }
    if let Some(job) = &st.admin_job {
        let r = job.lock().unwrap().take();
        if let Some(r) = r {
            st.admin_job = None;
            match r {
                Ok(on) => {
                    st.admin_actual = Some(on);
                    app.cfg.always_admin = on;
                }
                Err(e) => {
                    let on = autostart::admin_is_enabled();
                    st.admin_actual = Some(on);
                    app.cfg.always_admin = on;
                    app.status = Some((format!("Adminrechte: {e}"), Instant::now()));
                }
            }
        }
    }
    let basic = app.frame.basic.clone();
    let admin_now = app.admin;
    let cfg = &mut app.cfg;
    let app_update = &mut app.update;
    let diag_message = &mut app.diag_message;
    let basic_took_ms = app.frame.took_ms;
    let basic_procs = app.frame.procs.len();

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.set_max_width(820.0);

        section(ui, "Taskleisten-Anzeige", |ui| {
            let running = taskbar_running();
            ui.horizontal(|ui| {
                if ui.checkbox(&mut cfg.taskbar.enabled, "Anzeige in der Taskleiste").changed() && cfg.taskbar.enabled && !running {
                    // Datei zuerst schreiben, damit die neue Anzeige den aktuellen Stand liest
                    let _ = cfg.save();
                    winutil::spawn_self(&["--taskbar"]);
                }
                ui.label(RichText::new(if running { "läuft" } else { "läuft nicht" }).weak());
                if !running && cfg.taskbar.enabled && ui.button("Jetzt starten").clicked() {
                    let _ = cfg.save();
                    winutil::spawn_self(&["--taskbar"]);
                }
            });
            egui::Grid::new("tb_grid").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label("Position");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut cfg.taskbar.side, Side::Left, "Links");
                    ui.radio_value(&mut cfg.taskbar.side, Side::Right, "Rechts");
                    ui.checkbox(&mut cfg.taskbar.floating, "frei schwebend (Rückfall)");
                });
                ui.end_row();
                ui.label("Abstand zur Seite");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut cfg.taskbar.offset_px).range(0..=4000).suffix(" px"));
                    ui.label(RichText::new("oder die Anzeige mit der Maus ziehen").weak());
                });
                ui.end_row();
                ui.label("Monitor");
                egui::ComboBox::from_id_salt("monitor")
                    .selected_text(if cfg.taskbar.monitor == 0 { "Haupt-Taskleiste".to_string() } else { format!("Taskleiste {}", cfg.taskbar.monitor + 1) })
                    .show_ui(ui, |ui| {
                        for i in 0..st.taskbars.max(1) {
                            let label = if i == 0 { "Haupt-Taskleiste".to_string() } else { format!("Taskleiste {}", i + 1) };
                            ui.selectable_value(&mut cfg.taskbar.monitor, i, label);
                        }
                    });
                ui.end_row();
                ui.label("Aktualisierung");
                let mut secs = cfg.taskbar.interval_ms as f32 / 1000.0;
                if ui.add(egui::Slider::new(&mut secs, 0.25..=5.0).step_by(0.25).suffix(" s")).changed() {
                    cfg.taskbar.interval_ms = (secs * 1000.0).round() as u32;
                }
                ui.end_row();
                ui.label("Schriftgröße");
                ui.add(egui::Slider::new(&mut cfg.taskbar.font_size_pt, 7.0..=14.0).step_by(0.5).suffix(" pt"));
                ui.end_row();
                ui.label("Netzwerk-Einheit");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut cfg.taskbar.net_unit, NetUnit::MBps, "MB/s");
                    ui.radio_value(&mut cfg.taskbar.net_unit, NetUnit::Mbitps, "Mbit/s");
                });
                ui.end_row();
                ui.label("Trennstrich");
                ui.checkbox(&mut cfg.taskbar.separator, "zwischen Internet- und PC-Werten");
                ui.end_row();
                ui.label("CPU RAM");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut cfg.taskbar.cpu_mem_mode, GpuMemMode::Percent, "belegt in %");
                    ui.radio_value(&mut cfg.taskbar.cpu_mem_mode, GpuMemMode::Gigabytes, "belegt in GB");
                });
                ui.end_row();
                ui.label("GPU VRAM");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut cfg.taskbar.gpu_mem_mode, GpuMemMode::Percent, "belegt in %");
                    ui.radio_value(&mut cfg.taskbar.gpu_mem_mode, GpuMemMode::Gigabytes, "belegt in GB");
                });
                ui.end_row();
            });
            ui.add_space(6.0);
            ui.label(RichText::new("Felder (je zwei bilden eine Spalte, oben/unten)").strong());
            let mut swap: Option<(usize, usize)> = None;
            let n = cfg.taskbar.fields.len();
            egui::Grid::new("fields").num_columns(3).spacing([10.0, 2.0]).show(ui, |ui| {
                for i in 0..n {
                    let f = cfg.taskbar.fields[i];
                    let mut vis = !cfg.taskbar.hidden.contains(&f);
                    if ui.checkbox(&mut vis, f.label()).changed() {
                        if vis {
                            cfg.taskbar.hidden.retain(|x| *x != f);
                        } else {
                            cfg.taskbar.hidden.push(f);
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.add_enabled(i > 0, egui::Button::new("▲").small()).clicked() {
                            swap = Some((i, i - 1));
                        }
                        if ui.add_enabled(i + 1 < n, egui::Button::new("▼").small()).clicked() {
                            swap = Some((i, i + 1));
                        }
                    });
                    ui.label(RichText::new(crate::taskbar::layout::cell(f, &basic, cfg).text).weak());
                    ui.end_row();
                }
            });
            if let Some((a, b)) = swap {
                cfg.taskbar.fields.swap(a, b);
            }
            if ui.button("Standardreihenfolge").clicked() {
                cfg.taskbar.fields = Field::ALL.to_vec();
                cfg.taskbar.hidden.clear();
            }
        });

        section(ui, "Internetleitung", |ui| {
            ui.checkbox(&mut cfg.taskbar.show_line_usage, "Leitungsauslastung in % hinter Up- und Download anzeigen");
            egui::Grid::new("line").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label("Gebuchter Download");
                ui.add(egui::DragValue::new(&mut cfg.line_down_mbit).range(0.0..=100000.0).speed(1.0).suffix(" MBit/s"));
                ui.end_row();
                ui.label("Gebuchter Upload");
                ui.add(egui::DragValue::new(&mut cfg.line_up_mbit).range(0.0..=100000.0).speed(1.0).suffix(" MBit/s"));
                ui.end_row();
                ui.label("Internet-Adapter");
                let auto = net::auto_adapter(&st.adapters).map(|a| a.name.clone()).unwrap_or("keiner gefunden".into());
                let cur = if cfg.net_adapter.is_empty() {
                    format!("Automatisch ({auto})")
                } else {
                    st.adapters.iter().find(|a| a.guid == cfg.net_adapter).map(|a| a.name.clone()).unwrap_or("nicht verbunden".into())
                };
                egui::ComboBox::from_id_salt("adapter").width(320.0).selected_text(cur).show_ui(ui, |ui| {
                    ui.selectable_value(&mut cfg.net_adapter, String::new(), format!("Automatisch ({auto})"));
                    ui.separator();
                    for a in st.adapters.iter().filter(|a| a.hardware) {
                        let label = format!("{} – {}{}", a.name, a.description, if a.up { "" } else { " (getrennt)" });
                        ui.selectable_value(&mut cfg.net_adapter, a.guid.clone(), label);
                    }
                    ui.separator();
                    ui.label(RichText::new("Virtuelle Adapter (VPN usw.)").small().weak());
                    for a in st.adapters.iter().filter(|a| !a.hardware && a.up) {
                        ui.selectable_value(&mut cfg.net_adapter, a.guid.clone(), format!("{} – {}", a.name, a.description));
                    }
                });
                ui.end_row();
            });
            ui.label(
                RichText::new("Berechnung: MB/s × 8 ÷ gebuchte MBit/s × 100. Ohne hinterlegten Wert bleibt die Angabe ausgeblendet; Werte über 100 % werden angezeigt. Windows zählt auch Verkehr im Heimnetz (z. B. NAS-Kopien) mit.")
                    .small()
                    .weak(),
            );
        });

        section(ui, "Grafikkarte", |ui| {
            let auto = crate::sensors::gpu::auto_adapter(&st.gpus).map(|a| a.name.clone()).unwrap_or("keine gefunden".into());
            let cur = if cfg.gpu_adapter.is_empty() {
                format!("Automatisch ({auto})")
            } else {
                st.gpus.iter().find(|a| a.luid == cfg.gpu_adapter).map(|a| a.name.clone()).unwrap_or("nicht gefunden".into())
            };
            ui.horizontal(|ui| {
                ui.label("Angezeigte GPU");
                egui::ComboBox::from_id_salt("gpu").width(360.0).selected_text(cur).show_ui(ui, |ui| {
                    ui.selectable_value(&mut cfg.gpu_adapter, String::new(), format!("Automatisch ({auto})"));
                    for a in &st.gpus {
                        let label = format!("{} – {} ({} GB)", a.name, a.vendor_name(), crate::format::de(a.dedicated / crate::format::GIB, 1));
                        ui.selectable_value(&mut cfg.gpu_adapter, a.luid.clone(), label);
                    }
                });
            });
            ui.label(RichText::new(format!("Quelle: {}", basic.gpu.source.clone().unwrap_or("–".into()))).weak());
            ui.label(
                RichText::new(
                    "NVIDIA: alle Werte über NVML. AMD, Intel und andere: Temperatur und Takt über die Windows-Grafikschnittstelle (wie der Task-Manager), \
                     Auslastung und VRAM über die Windows-Leistungsindikatoren; die Leistung in Watt gibt es dort nicht.",
                )
                .small()
                .weak(),
            );
        });

        section(ui, "CPU-Sensoren", |ui| {
            ui.horizontal(|ui| {
                ui.label("CPU Freq");
                ui.radio_value(&mut cfg.cpu_freq_mode, CpuFreqMode::MaxCore, "höchster Kerntakt");
                ui.radio_value(&mut cfg.cpu_freq_mode, CpuFreqMode::Average, "Durchschnitt aller Kerne");
            });
            ui.checkbox(&mut cfg.use_pawnio, "CPU-Temperatur über PawnIO-Treiber lesen (abschaltbar, falls Antivirus oder Anti-Cheat stört)");
            ui.label(RichText::new(format!("Status: {}", basic.cpu_temp_status)).weak());
        });

        section(ui, "Prozesse: Darstellung", |ui| {
            egui::Grid::new("units").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label("Speichereinheit");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut cfg.mem_unit, MemUnit::Auto, "automatisch");
                    ui.radio_value(&mut cfg.mem_unit, MemUnit::MB, "MB");
                    ui.radio_value(&mut cfg.mem_unit, MemUnit::GB, "GB");
                });
                ui.end_row();
                ui.label("Nachkommastellen");
                ui.add(egui::Slider::new(&mut cfg.decimals, 0..=3));
                ui.end_row();
            });
            ui.checkbox(&mut cfg.heatmap.enabled, "Heatmap (Einfärbung nach Last)");
            egui::Grid::new("heat").num_columns(5).spacing([8.0, 4.0]).show(ui, |ui| {
                threshold(ui, "CPU", &mut cfg.heatmap.cpu, "%", 100.0);
                threshold(ui, "Arbeitsspeicher", &mut cfg.heatmap.mem_mb, "MB", 256000.0);
                threshold(ui, "Datenträger", &mut cfg.heatmap.disk_mbps, "MB/s", 10000.0);
                threshold(ui, "GPU", &mut cfg.heatmap.gpu, "%", 100.0);
                threshold(ui, "VRAM-Anteil", &mut cfg.heatmap.vram_pct, "%", 100.0);
            });
        });

        section(ui, "Adminrechte", |ui| {
            let busy = st.admin_job.is_some();
            let mut want = st.admin_actual.unwrap_or(false);
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Checkbox::new(&mut want, "TaskPulse immer mit Administratorrechten starten")).changed() {
                    let job = Arc::new(Mutex::new(None));
                    let j = job.clone();
                    std::thread::spawn(move || {
                        let r = if want { autostart::admin_enable().map(|_| true) } else { autostart::admin_disable().map(|_| false) };
                        *j.lock().unwrap() = Some(r);
                    });
                    st.admin_job = Some(job);
                }
                if busy {
                    ui.spinner();
                }
            });
            ui.label(
                RichText::new(
                    "Damit gibt es CPU-Temperatur, alle Prozessdaten und alle Aktionen. Beim Einschalten fragt Windows einmal nach Adminrechten; \
                     danach startet TaskPulse über eine geplante Aufgabe („TaskPulse Admin“) ohne UAC-Abfrage. Abschalten entfernt die Aufgabe.",
                )
                .small()
                .weak(),
            );
            ui.horizontal(|ui| {
                if admin_now {
                    ui.colored_label(egui::Color32::from_rgb(40, 170, 110), "✔ Läuft gerade mit Adminrechten");
                } else {
                    ui.colored_label(egui::Color32::from_rgb(200, 140, 0), "Läuft gerade ohne Adminrechte");
                    if st.admin_actual == Some(true) && ui.button("Jetzt mit Adminrechten neu starten").clicked() {
                        let _ = std::fs::write(crate::config::Config::dir().join("launch_args.txt"), "--settings\n--restart");
                        if autostart::run_admin_task() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                }
            });
        });

        section(ui, "Updates", |ui| app_update.settings(ui, cfg));

        section(ui, "Fehlerberichte", |ui| {
            ui.label(
                RichText::new(
                    "Geht etwas schief, schreibt TaskPulse sofort eine Datei mit dem Fehler (auch bei Abstürzen) und führt ein Protokoll kleinerer Probleme. \
                     „Diagnosebericht erstellen“ fasst alles in einer Datei zusammen – die kannst du weitergeben. Es wird nichts automatisch gesendet.",
                )
                .small()
                .weak(),
            );
            let n = std::fs::read_dir(crate::diag::dir())
                .map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("Fehler-") || e.file_name().to_string_lossy().starts_with("Absturz-")).count())
                .unwrap_or(0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Diagnosebericht erstellen").strong()).clicked() {
                    let extra = format!(
                        "Messung: {:.0} ms · {} Prozesse · GPU-Quelle: {} · CPU-Temperatur: {}",
                        basic_took_ms,
                        basic_procs,
                        basic.gpu.source.clone().unwrap_or_default(),
                        basic.cpu_temp_status
                    );
                    *diag_message = Some(match crate::diag::diagnose(&extra) {
                        Some(p) => {
                            winutil::open_in_explorer(&p.to_string_lossy());
                            format!("Erstellt: {}", p.display())
                        }
                        None => "Diagnosebericht konnte nicht geschrieben werden".into(),
                    });
                }
                if ui.button("Ordner öffnen").clicked() {
                    let _ = std::fs::create_dir_all(crate::diag::dir());
                    winutil::shell_open(&crate::diag::dir().to_string_lossy());
                }
                ui.label(RichText::new(if n == 0 { "keine Fehlerberichte".to_string() } else { format!("{n} Fehlerbericht(e) vorhanden") }).weak());
            });
            if let Some(m) = diag_message.as_ref() {
                ui.label(RichText::new(m).small());
            }
        });

        section(ui, "Allgemein", |ui| {
            let busy = st.autostart_job.is_some();
            let mut want = st.autostart_actual.unwrap_or(false);
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Checkbox::new(&mut want, "Mit Windows starten (nur Taskleisten-Anzeige)")).changed() {
                    let job = Arc::new(Mutex::new(None));
                    let j = job.clone();
                    std::thread::spawn(move || {
                        let r = if want { autostart::enable().map(|_| true) } else { autostart::disable().map(|_| false) };
                        *j.lock().unwrap() = Some(r);
                    });
                    st.autostart_job = Some(job);
                }
                if busy {
                    ui.spinner();
                }
            });
            ui.label(RichText::new("Läuft über eine geplante Aufgabe mit höchsten Rechten, damit beim Anmelden keine UAC-Abfrage erscheint. Beim Abschalten wird sie entfernt.").small().weak());
            ui.horizontal(|ui| {
                ui.label("Startseite");
                ui.radio_value(&mut cfg.start_page, crate::config::StartPage::Processes, "Prozesse");
                ui.radio_value(&mut cfg.start_page, crate::config::StartPage::Performance, "Leistung");
            });
            ui.horizontal(|ui| {
                ui.label("Schriftgröße Hauptfenster");
                ui.add(egui::Slider::new(&mut cfg.ui_font_size, 12.0..=24.0).step_by(1.0).suffix(" pt"));
                if ui.small_button("Standard").clicked() {
                    cfg.ui_font_size = 16.0;
                }
            });
            ui.checkbox(&mut cfg.always_on_top, "Hauptfenster immer im Vordergrund");
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Einstellungsdatei: {}", crate::config::Config::path().display())).small().weak());
                if ui.small_button("Ordner öffnen").clicked() {
                    winutil::shell_open(&crate::config::Config::dir().to_string_lossy());
                }
            });
            ui.label(RichText::new(format!("TaskPulse {} · keine Telemetrie, Internet nur für die Update-Prüfung bei GitHub (abschaltbar)", crate::updater::CURRENT)).small().weak());
        });
    });
}

/// Dialog "Neue Aufgabe ausführen" (auch mit Adminrechten).
pub fn run_dialog(app: &mut TaskPulseApp, ctx: &egui::Context) {
    let Some(cmd) = app.settings.run_dialog.as_mut() else { return };
    let mut close = false;
    let mut run = false;
    egui::Window::new("Neue Aufgabe ausführen").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.label("Programm, Ordner, Dokument oder Internetressource:");
        let r = ui.add(egui::TextEdit::singleline(cmd).desired_width(380.0));
        r.request_focus();
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            run = true;
        }
        ui.checkbox(&mut app.settings.run_admin, "Mit Administratorrechten erstellen");
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() {
                run = true;
            }
            if ui.button("Abbrechen").clicked() {
                close = true;
            }
        });
    });
    if run {
        let c = cmd.trim().to_string();
        if !c.is_empty() {
            let (file, args) = split_command(&c);
            use windows::core::{w, HSTRING, PCWSTR};
            use windows::Win32::UI::Shell::ShellExecuteW;
            use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
            let verb = if app.settings.run_admin { w!("runas") } else { w!("open") };
            let r = unsafe { ShellExecuteW(None, verb, &HSTRING::from(file), &HSTRING::from(args), PCWSTR::null(), SW_SHOWNORMAL) };
            if (r.0 as isize) <= 32 {
                app.status = Some((format!("„{c}“ konnte nicht gestartet werden"), Instant::now()));
            }
        }
        close = true;
    }
    if close {
        app.settings.run_dialog = None;
    }
}

/// "notepad c:\x.txt" -> ("notepad", "c:\x.txt"); "\"C:\a b\x.exe\" -v" -> ("C:\a b\x.exe", "-v")
pub fn split_command(c: &str) -> (String, String) {
    let c = c.trim();
    if let Some(rest) = c.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (rest[..end].to_string(), rest[end + 1..].trim().to_string());
        }
    }
    match c.split_once(' ') {
        Some((a, b)) if !std::path::Path::new(c).exists() => (a.to_string(), b.trim().to_string()),
        _ => (c.to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_split() {
        assert_eq!(split_command("notepad c:\\x.txt"), ("notepad".into(), "c:\\x.txt".into()));
        assert_eq!(split_command("\"C:\\a b\\x.exe\" -v"), ("C:\\a b\\x.exe".into(), "-v".into()));
        assert_eq!(split_command("cmd"), ("cmd".into(), String::new()));
    }
}
