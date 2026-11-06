//! Linux-Gegenstück zu winutil.rs: dieselben Funktionen, umgesetzt mit Linux-Mitteln
//! (Signale, xdg-open, zenity/kdialog, /proc). Was es unter Linux nicht gibt, ist ein leerer Ersatz.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Unter Windows Prozess-Erstellungsflag; unter Linux ohne Bedeutung.
pub const CREATE_NO_WINDOW: u32 = 0;
pub const MAIN_TITLE: &str = "TaskPulse";

/// „Fenster“ = Prozess-ID des Hauptfensters (Linux kennt keine fensterübergreifende Suche wie EnumWindows).
pub type HWND = u32;

pub fn native_arm64() -> bool {
    cfg!(target_arch = "aarch64")
}

/// root?
pub fn is_elevated() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub fn exe_path() -> String {
    // In einem AppImage ist current_exe der eingebundene Pfad – für Neustarts die AppImage-Datei nehmen
    std::env::var("APPIMAGE").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default())
}

/// Unter Linux braucht TaskPulse für fast alles keine root-Rechte; ein Neustart als root wird nicht angeboten.
pub fn relaunch_elevated(_args: &str) -> bool {
    false
}

pub fn spawn_self(args: &[&str]) -> bool {
    std::process::Command::new(exe_path()).args(args).spawn().is_ok()
}

/// Mit root-Rechten ausführen (pkexec fragt nach dem Passwort) und auf das Ende warten.
pub fn run_elevated_wait(file: &str, params: &str) -> Option<u32> {
    let st = std::process::Command::new("pkexec").arg(file).args(params.split_whitespace()).status().ok()?;
    st.code().map(|c| c as u32)
}

pub fn run_hidden(file: &str, args: &[&str]) -> Option<i32> {
    std::process::Command::new(file).args(args).status().ok().and_then(|s| s.code())
}

/// Ordner im Dateimanager öffnen (bei einer Datei: den Ordner, in dem sie liegt).
pub fn open_in_explorer(path: &str) {
    let p = Path::new(path);
    let dir = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

pub fn shell_open(target: &str) {
    let _ = std::process::Command::new("xdg-open").arg(target).spawn();
}

pub fn search_online(term: &str) {
    let q: String = term
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    shell_open(&format!("https://www.bing.com/search?q={q}"));
}

fn has_program(name: &str) -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file())).unwrap_or(false)
}

/// „Speichern unter“: zenity (GNOME) oder kdialog (KDE); gibt es keins von beiden, wird ohne Rückfrage
/// im Startordner unter dem vorgeschlagenen Namen gespeichert.
pub fn save_file_dialog(title: &str, default_name: &str, start_dir: Option<&Path>, _filters: &[(&str, &str)], _default_ext: &str) -> Option<PathBuf> {
    let dir = start_dir.map(Path::to_path_buf).or_else(|| std::env::var_os("HOME").map(PathBuf::from)).unwrap_or_else(|| PathBuf::from("."));
    let suggested = dir.join(default_name);
    let out = if has_program("zenity") {
        std::process::Command::new("zenity")
            .args(["--file-selection", "--save", "--confirm-overwrite", "--title", title, "--filename"])
            .arg(&suggested)
            .output()
            .ok()?
    } else if has_program("kdialog") {
        std::process::Command::new("kdialog").args(["--title", title, "--getsavefilename"]).arg(&suggested).output().ok()?
    } else {
        return Some(suggested);
    };
    if !out.status.success() {
        return None; // abgebrochen
    }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!p.is_empty()).then(|| PathBuf::from(p))
}

/// „Öffnen“: Datei auswählen über zenity bzw. kdialog (ohne beides: None).
pub fn open_file_dialog(title: &str, _filters: &[(&str, &str)]) -> Option<PathBuf> {
    let out = if has_program("zenity") {
        std::process::Command::new("zenity").args(["--file-selection", "--title", title]).output().ok()?
    } else if has_program("kdialog") {
        std::process::Command::new("kdialog").args(["--title", title, "--getopenfilename"]).arg(std::env::var_os("HOME").unwrap_or_default()).output().ok()?
    } else {
        return None;
    };
    if !out.status.success() {
        return None;
    }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!p.is_empty()).then(|| PathBuf::from(p))
}

pub fn fast_exit(code: u32) -> ! {
    unsafe { libc::_exit(code as i32) }
}

/// Prozess beenden: erst höflich (SIGTERM), nach 2 s hart (SIGKILL).
pub fn terminate(pid: u32) -> Result<(), String> {
    let r = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    if r != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        if Path::new(&format!("/proc/{pid}")).exists() {
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
    });
    Ok(())
}

/// „Effizienzmodus“ unter Linux: niedrigere Priorität (nice 10). Zurücksetzen auf 0 braucht root.
pub fn set_efficiency_mode(pid: u32, on: bool) -> Result<(), String> {
    let r = unsafe { libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, if on { 10 } else { 0 }) };
    if r != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

pub fn restart_explorer() {}

pub fn taskbar_is_light() -> bool {
    false
}

pub fn reg_dword(_subkey: &str, _value: &str) -> Option<u32> {
    None
}

fn main_pid_file() -> PathBuf {
    crate::config::Config::dir().join("hauptfenster.pid")
}

/// Das Hauptfenster meldet sich beim Start an (für „schon offen? → nach vorn holen“).
pub fn register_main_window() {
    let _ = std::fs::create_dir_all(crate::config::Config::dir());
    let _ = std::fs::write(main_pid_file(), std::process::id().to_string());
}

/// Läuft schon ein Hauptfenster von TaskPulse? Liefert dessen Prozess-ID.
pub fn find_own_window(_title: &str) -> Option<HWND> {
    let pid: u32 = std::fs::read_to_string(main_pid_file()).ok()?.trim().parse().ok()?;
    if pid == std::process::id() {
        return None;
    }
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    comm.trim().to_lowercase().starts_with("taskpulse").then_some(pid)
}

/// Laufendes Hauptfenster nach vorn holen (per Signaldatei, die das Fenster abfragt).
pub fn bring_to_front(_hwnd: HWND) {
    let _ = std::fs::write(crate::config::Config::dir().join("nach_vorn.signal"), b"1");
}

pub fn open_main_window(settings: bool) {
    if let Some(h) = find_own_window(MAIN_TITLE) {
        bring_to_front(h);
        if settings {
            let _ = std::fs::write(crate::config::Config::dir().join("open_settings.signal"), b"1");
        }
        return;
    }
    if settings {
        spawn_self(&["--settings"]);
    } else {
        spawn_self(&[]);
    }
}

/// (privater Speicher, Arbeitssatz) dieses Prozesses in Bytes
pub fn own_memory() -> (f64, f64) {
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as f64;
    let m: Vec<f64> = std::fs::read_to_string("/proc/self/statm").unwrap_or_default().split_whitespace().filter_map(|x| x.parse().ok()).collect();
    let rss = m.get(1).copied().unwrap_or(0.0) * page;
    let shared = m.get(2).copied().unwrap_or(0.0) * page;
    ((rss - shared).max(0.0), rss)
}

pub fn trim_working_set() {}
