//! Installer und Deinstaller – in TaskPulse.exe eingebaut.
//!
//! Heißt die Datei "...Setup...exe" (z. B. TaskPulse-Setup.exe) oder wird `--install` übergeben,
//! erscheint das Installationsfenster. Die Deinstallation läuft über `--uninstall`
//! (eingetragen unter "Apps & Features").
//!
//! Installiert wird nach %ProgramFiles%\TaskPulse: Weil TaskPulse auf Wunsch über geplante
//! Aufgaben mit Adminrechten startet, darf die EXE nicht in einem für jeden beschreibbaren
//! Ordner liegen.

use crate::autostart;
use crate::config::Config;
use crate::winutil;
use eframe::egui::{self, RichText};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::{FOLDERID_CommonPrograms, FOLDERID_Desktop, IShellLinkW, SHGetKnownFolderPath, ShellLink, KF_FLAG_DEFAULT};

pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\TaskPulse";
const VERSION: &str = crate::updater::CURRENT;

/// Soll diese EXE als Installer laufen?
pub fn is_setup_exe() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()))
        .map(|s| s.contains("setup") || s.contains("install"))
        .unwrap_or(false)
}

pub fn default_dir() -> PathBuf {
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
    PathBuf::from(pf).join("TaskPulse")
}

/// Bereits installiert? Liefert den Installationsordner.
pub fn installed_dir() -> Option<PathBuf> {
    reg_get_string(HKEY_LOCAL_MACHINE, UNINSTALL_KEY, "InstallLocation").map(PathBuf::from).filter(|p| p.join("TaskPulse.exe").exists())
}

pub fn pawnio_installed() -> bool {
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
    Path::new(&pf).join("PawnIO").join("PawnIOLib.dll").exists()
}

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

pub fn start_menu_link() -> Option<PathBuf> {
    known_folder(&FOLDERID_CommonPrograms).map(|p| p.join("TaskPulse.lnk"))
}

pub fn desktop_link() -> Option<PathBuf> {
    known_folder(&FOLDERID_Desktop).map(|p| p.join("TaskPulse.lnk"))
}

fn create_shortcut(link: &Path, target: &Path, desc: &str) -> Result<(), String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
        sl.SetPath(&HSTRING::from(target.as_os_str())).map_err(|e| e.message())?;
        if let Some(dir) = target.parent() {
            sl.SetWorkingDirectory(&HSTRING::from(dir.as_os_str())).map_err(|e| e.message())?;
        }
        sl.SetDescription(&HSTRING::from(desc)).map_err(|e| e.message())?;
        sl.SetIconLocation(&HSTRING::from(target.as_os_str()), 0).map_err(|e| e.message())?;
        let pf: IPersistFile = sl.cast().map_err(|e| e.message())?;
        pf.Save(&HSTRING::from(link.as_os_str()), true).map_err(|e| e.message())?;
    }
    Ok(())
}

fn reg_get_string(root: HKEY, key: &str, name: &str) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(root, &HSTRING::from(key), &HSTRING::from(name), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut size))
    };
    if r.is_err() {
        return None;
    }
    let n = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..n]))
}

fn write_uninstall_entry(dir: &Path, exe: &Path) -> Result<(), String> {
    unsafe {
        let mut hk = HKEY::default();
        RegCreateKeyExW(HKEY_LOCAL_MACHINE, &HSTRING::from(UNINSTALL_KEY), 0, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut hk, None)
            .ok()
            .map_err(|e| e.message())?;
        let set_sz = |name: &str, val: &str| {
            let mut w: Vec<u16> = val.encode_utf16().collect();
            w.push(0);
            let bytes = std::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 2);
            RegSetValueExW(hk, &HSTRING::from(name), 0, REG_SZ, Some(bytes))
        };
        let set_dw = |name: &str, val: u32| RegSetValueExW(hk, &HSTRING::from(name), 0, REG_DWORD, Some(&val.to_le_bytes()));
        let exe_s = exe.to_string_lossy();
        let size_kb = std::fs::metadata(exe).map(|m| (m.len() / 1024) as u32).unwrap_or(0);
        let results = [
            set_sz("DisplayName", "TaskPulse"),
            set_sz("DisplayVersion", VERSION),
            set_sz("Publisher", "TaskPulse"),
            set_sz("DisplayIcon", &exe_s),
            set_sz("InstallLocation", &dir.to_string_lossy()),
            set_sz("UninstallString", &format!("\"{exe_s}\" --uninstall")),
            set_dw("NoModify", 1),
            set_dw("NoRepair", 1),
            set_dw("EstimatedSize", size_kb),
        ];
        let _ = RegCloseKey(hk);
        if results.iter().any(|r| r.is_err()) {
            return Err("Eintrag unter „Apps & Features“ unvollständig".into());
        }
    }
    Ok(())
}

/// Beendet laufende TaskPulse-Prozesse aus dem Zielordner (nicht diesen Prozess).
fn stop_running(exe: &Path) -> usize {
    let me = std::process::id();
    let target = exe.to_string_lossy().to_lowercase();
    let mut n = 0;
    let mut procs = crate::sensors::procs::Procs::new();
    for p in procs.sample() {
        if p.pid != me && p.path.as_deref().map(|x| x.to_lowercase() == target).unwrap_or(false) && winutil::terminate(p.pid).is_ok() {
            n += 1;
        }
    }
    if n > 0 {
        std::thread::sleep(std::time::Duration::from_millis(600));
    }
    n
}

#[derive(Clone)]
pub struct Options {
    pub dir: String,
    pub start_menu: bool,
    pub desktop: bool,
    pub autostart: bool,
    pub always_admin: bool,
    pub start_after: bool,
}

/// Führt die Installation aus; `log` erhält die Schritte.
pub fn install(o: &Options, log: &dyn Fn(String)) -> Result<PathBuf, String> {
    let dir = PathBuf::from(o.dir.trim());
    if dir.as_os_str().is_empty() {
        return Err("Kein Zielordner angegeben".into());
    }
    let exe = dir.join("TaskPulse.exe");
    let n = stop_running(&exe);
    if n > 0 {
        log(format!("{n} laufende TaskPulse-Instanz(en) beendet"));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("Ordner {}: {e}", dir.display()))?;
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    if !same_file(&me, &exe) {
        // Zweimal versuchen: eine gerade beendete Instanz gibt die Datei mit kurzer Verzögerung frei
        let mut res = std::fs::copy(&me, &exe);
        if res.is_err() {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            res = std::fs::copy(&me, &exe);
        }
        res.map_err(|e| format!("Kopieren nach {}: {e}", exe.display()))?;
    }
    log(format!("Programm kopiert nach {}", exe.display()));

    for (want, link, name) in [(o.start_menu, start_menu_link(), "Startmenü"), (o.desktop, desktop_link(), "Desktop")] {
        let Some(link) = link else { continue };
        if want {
            match create_shortcut(&link, &exe, "TaskPulse – Task-Manager mit GPU-Werten und Taskleisten-Anzeige") {
                Ok(()) => log(format!("Verknüpfung im {name} angelegt")),
                Err(e) => log(format!("Verknüpfung im {name} fehlgeschlagen: {e}")),
            }
        } else if link.exists() {
            let _ = std::fs::remove_file(&link);
        }
    }

    // Geplante Aufgaben auf die installierte EXE ausrichten
    let exe_s = exe.to_string_lossy().to_string();
    if o.autostart {
        autostart::enable_for(&exe_s)?;
        log("Autostart eingerichtet (nur Taskleisten-Anzeige)".into());
    } else {
        let _ = autostart::disable();
    }
    if o.always_admin {
        autostart::admin_enable_for(&exe_s)?;
        log("„Immer mit Adminrechten“ eingerichtet".into());
    } else {
        let _ = autostart::admin_disable();
    }
    let mut cfg = Config::load();
    cfg.autostart = o.autostart;
    cfg.always_admin = o.always_admin;
    let _ = cfg.save();

    write_uninstall_entry(&dir, &exe)?;
    log("Unter „Apps & Features“ eingetragen".into());
    Ok(exe)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Entfernt TaskPulse. Der Ordner selbst wird nach dem Beenden gelöscht.
pub fn uninstall(keep_settings: bool, log: &dyn Fn(String)) -> Result<Option<PathBuf>, String> {
    let dir = installed_dir().or_else(|| std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())));
    if let Some(d) = &dir {
        let n = stop_running(&d.join("TaskPulse.exe"));
        if n > 0 {
            log(format!("{n} laufende TaskPulse-Instanz(en) beendet"));
        }
    }
    let _ = autostart::disable();
    let _ = autostart::admin_disable();
    log("Geplante Aufgaben entfernt".into());
    for l in [start_menu_link(), desktop_link()].into_iter().flatten() {
        if l.exists() {
            let _ = std::fs::remove_file(l);
        }
    }
    log("Verknüpfungen entfernt".into());
    unsafe {
        let _ = RegDeleteTreeW(HKEY_LOCAL_MACHINE, &HSTRING::from(UNINSTALL_KEY));
    }
    log("Eintrag unter „Apps & Features“ entfernt".into());
    if !keep_settings {
        let _ = std::fs::remove_dir_all(Config::dir());
        log("Einstellungen gelöscht".into());
    }
    Ok(dir)
}

/// Löscht den Programmordner, sobald dieser Prozess beendet ist.
pub fn schedule_dir_removal(dir: &Path) {
    use std::os::windows::process::CommandExt;
    let d = dir.to_string_lossy().to_string();
    if d.len() < 4 {
        return; // Sicherheitsnetz: niemals ein Laufwerk o. Ä. löschen
    }
    let _ = std::process::Command::new("cmd.exe")
        .raw_arg(format!("/c ping 127.0.0.1 -n 3 >nul & rmdir /s /q \"{d}\""))
        .current_dir(std::env::temp_dir())
        .creation_flags(winutil::CREATE_NO_WINDOW)
        .spawn();
}

// ---------------------------------------------------------------- Fenster

#[derive(PartialEq)]
enum Phase {
    Ask,
    Running,
    Done(Result<(), String>),
}

struct SetupApp {
    uninstall: bool,
    preview: bool,
    opts: Options,
    keep_settings: bool,
    phase: Phase,
    log: Arc<Mutex<Vec<String>>>,
    result: Arc<Mutex<Option<Result<Option<PathBuf>, String>>>>,
    installed_exe: Option<PathBuf>,
    remove_dir: Option<PathBuf>,
}

impl SetupApp {
    fn start(&mut self, ctx: &egui::Context) {
        self.phase = Phase::Running;
        let log = self.log.clone();
        let result = self.result.clone();
        let opts = self.opts.clone();
        let uninstall = self.uninstall;
        let keep = self.keep_settings;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let l = |s: String| {
                log.lock().unwrap().push(s);
                ctx.request_repaint();
            };
            let r = if uninstall { uninstall_wrap(keep, &l) } else { install(&opts, &l).map(Some) };
            *result.lock().unwrap() = Some(r);
            ctx.request_repaint();
        });
    }
}

fn uninstall_wrap(keep: bool, l: &dyn Fn(String)) -> Result<Option<PathBuf>, String> {
    uninstall(keep, l)
}

impl eframe::App for SetupApp {
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        if let Some(r) = self.result.lock().unwrap().take() {
            match r {
                Ok(p) => {
                    if self.uninstall {
                        self.remove_dir = p;
                    } else {
                        self.installed_exe = p;
                    }
                    self.phase = Phase::Done(Ok(()));
                }
                Err(e) => self.phase = Phase::Done(Err(e)),
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(6.0);
            ui.heading(if self.uninstall { "TaskPulse deinstallieren" } else { "TaskPulse installieren" });
            ui.label(RichText::new(format!("Version {VERSION}")).weak());
            ui.add_space(10.0);
            match &self.phase {
                Phase::Ask if self.uninstall => {
                    ui.label("TaskPulse, die Verknüpfungen, den Autostart und die Aufgabe „TaskPulse Admin“ entfernen.");
                    ui.add_space(6.0);
                    ui.checkbox(&mut self.keep_settings, "Einstellungen behalten");
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!self.preview, egui::Button::new(RichText::new("  Deinstallieren  ").strong())).clicked() {
                            self.start(ctx);
                        }
                        if ui.button("Abbrechen").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                }
                Phase::Ask => {
                    ui.label("Zielordner");
                    ui.add(egui::TextEdit::singleline(&mut self.opts.dir).desired_width(f32::INFINITY));
                    if installed_dir().is_some() {
                        ui.label(RichText::new("TaskPulse ist bereits installiert – die Installation wird aktualisiert, Einstellungen bleiben erhalten.").small().weak());
                    }
                    ui.add_space(8.0);
                    ui.checkbox(&mut self.opts.start_menu, "Verknüpfung im Startmenü");
                    ui.checkbox(&mut self.opts.desktop, "Verknüpfung auf dem Desktop");
                    ui.checkbox(&mut self.opts.autostart, "Mit Windows starten (nur Taskleisten-Anzeige)");
                    ui.checkbox(&mut self.opts.always_admin, "TaskPulse immer mit Administratorrechten starten (ohne UAC-Abfrage)");
                    ui.checkbox(&mut self.opts.start_after, "TaskPulse nach der Installation starten");
                    ui.add_space(8.0);
                    if pawnio_installed() {
                        ui.label(RichText::new("✔ PawnIO gefunden – CPU-Temperatur und -Watt verfügbar (mit Adminrechten).").color(egui::Color32::from_rgb(40, 170, 110)));
                    } else {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("PawnIO ist nicht installiert. Ohne den Treiber fehlen CPU-Temperatur und CPU-Watt; alles andere funktioniert.").color(egui::Color32::from_rgb(200, 140, 0)));
                            if ui.link("PawnIO herunterladen (pawnio.eu)").clicked() {
                                winutil::shell_open("https://pawnio.eu/");
                            }
                        });
                    }
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!self.preview, egui::Button::new(RichText::new("  Installieren  ").strong())).clicked() {
                            self.start(ctx);
                        }
                        if ui.button("Abbrechen").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                }
                Phase::Running => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Bitte warten …");
                    });
                }
                Phase::Done(r) => {
                    match r {
                        Ok(()) => ui.label(RichText::new(if self.uninstall { "✔ TaskPulse wurde entfernt." } else { "✔ TaskPulse wurde installiert." }).strong()),
                        Err(e) => ui.label(RichText::new(format!("Fehler: {e}")).color(egui::Color32::from_rgb(220, 70, 60))),
                    };
                    ui.add_space(10.0);
                    if ui.button(RichText::new("  Fertig  ").strong()).clicked() {
                        if let (Some(exe), true) = (&self.installed_exe, self.opts.start_after) {
                            let _ = std::process::Command::new(exe).current_dir(exe.parent().unwrap_or(Path::new("."))).spawn();
                        }
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
            ui.add_space(10.0);
            let log = self.log.lock().unwrap();
            if !log.is_empty() {
                ui.separator();
                for l in log.iter() {
                    ui.label(RichText::new(format!("• {l}")).small());
                }
            }
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(d) = &self.remove_dir {
            // Nur einen Ordner löschen, der wirklich TaskPulse enthält
            if d.join("TaskPulse.exe").exists() && d.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case("TaskPulse")).unwrap_or(false) {
                schedule_dir_removal(d);
            }
        }
    }
}

/// Stilles Update (`--update`, vom Updater gestartet): installierte Version ersetzen,
/// Verknüpfungen und Aufgaben wie bisher lassen, TaskPulse wieder starten.
pub fn run_update(reopen: bool) {
    if !winutil::is_elevated() {
        let arg = if reopen { "--update --reopen" } else { "--update" };
        unsafe {
            use windows::core::w;
            use windows::Win32::UI::Shell::ShellExecuteW;
            use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
            ShellExecuteW(None, w!("runas"), &HSTRING::from(winutil::exe_path()), &HSTRING::from(arg), PCWSTR::null(), SW_SHOWNORMAL);
        }
        return;
    }
    let lines = std::cell::RefCell::new(Vec::<String>::new());
    let r = match installed_dir() {
        None => Err("TaskPulse ist nicht installiert".to_string()),
        Some(dir) => {
            let o = Options {
                dir: dir.to_string_lossy().to_string(),
                start_menu: start_menu_link().map(|l| l.exists()).unwrap_or(false),
                desktop: desktop_link().map(|l| l.exists()).unwrap_or(false),
                autostart: autostart::is_enabled(),
                always_admin: autostart::admin_is_enabled(),
                start_after: true,
            };
            install(&o, &|s| lines.borrow_mut().push(s))
        }
    };
    match &r {
        Ok(_) => crate::updater::write_result(&format!("✔ TaskPulse wurde auf Version {VERSION} aktualisiert.")),
        Err(e) => {
            crate::diag::log(format!("Update-Installation fehlgeschlagen: {e}"));
            crate::updater::write_result(&format!("Update auf Version {VERSION} fehlgeschlagen: {e}"))
        }
    }
    if let Ok(exe) = r {
        let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
        if Config::load().taskbar.enabled {
            let _ = std::process::Command::new(&exe).arg("--taskbar").current_dir(&dir).spawn();
        }
        if reopen {
            let _ = std::process::Command::new(&exe).arg("--no-elevate").current_dir(&dir).spawn();
        }
    }
}

/// Startet das Installations- bzw. Deinstallationsfenster (mit Adminrechten).
pub fn run(uninstall: bool) {
    // --preview: nur ansehen (Tests/Doku), ohne Adminrechte – Installieren ist dann gesperrt
    let preview = std::env::args().any(|a| a == "--preview");
    if !winutil::is_elevated() && !preview {
        let arg = if uninstall { "--uninstall" } else { "--install" };
        // Die UAC-Abfrage startet diese EXE erneut mit Adminrechten
        unsafe {
            use windows::core::w;
            use windows::Win32::UI::Shell::ShellExecuteW;
            use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
            ShellExecuteW(None, w!("runas"), &HSTRING::from(winutil::exe_path()), &HSTRING::from(arg), PCWSTR::null(), SW_SHOWNORMAL);
        }
        return;
    }
    let cfg = Config::load();
    let dir = installed_dir().unwrap_or_else(default_dir);
    let app = SetupApp {
        uninstall,
        preview,
        opts: Options {
            dir: dir.to_string_lossy().to_string(),
            start_menu: true,
            desktop: true,
            autostart: cfg.autostart || autostart::is_enabled(),
            always_admin: cfg.always_admin || autostart::admin_is_enabled(),
            start_after: true,
        },
        keep_settings: true,
        phase: Phase::Ask,
        log: Arc::new(Mutex::new(Vec::new())),
        result: Arc::new(Mutex::new(None)),
        installed_exe: None,
        remove_dir: None,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(if uninstall { "TaskPulse deinstallieren" } else { "TaskPulse Setup" })
            .with_inner_size([620.0, 480.0])
            .with_resizable(false)
            .with_icon(crate::ui::app_icon()),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "TaskPulse Setup",
        options,
        Box::new(move |cc| {
            crate::ui::setup_fonts_pub(&cc.egui_ctx);
            crate::ui::apply_font_size(&cc.egui_ctx, 16.0);
            Ok(Box::new(app))
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_dir_is_program_files() {
        let d = default_dir();
        assert!(d.ends_with("TaskPulse"));
        assert!(d.to_string_lossy().to_lowercase().contains("program"));
    }
}
