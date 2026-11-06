//! Linux-Gegenstück zu autostart.rs: Autostart über eine .desktop-Datei in ~/.config/autostart
//! (XDG-Standard, gilt für GNOME, KDE, XFCE, Cinnamon …). „Immer mit Adminrechten“ gibt es unter
//! Linux nicht – TaskPulse braucht dort für fast alle Werte keine root-Rechte.
#![allow(dead_code)]

use std::path::PathBuf;

fn autostart_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("autostart").join("taskpulse.desktop"))
}

/// Inhalt der .desktop-Datei für den Autostart
pub fn desktop_entry(exe: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=TaskPulse\nComment=Anzeige von CPU-, GPU- und Netzwerkwerten\nExec=\"{exe}\" --taskbar\nIcon=taskpulse\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    )
}

pub fn is_enabled() -> bool {
    autostart_file().map(|p| p.exists()).unwrap_or(false)
}

pub fn enable() -> Result<(), String> {
    enable_for(&crate::winutil::exe_path())
}

pub fn enable_for(exe: &str) -> Result<(), String> {
    let p = autostart_file().ok_or("Ordner ~/.config nicht gefunden")?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, desktop_entry(exe)).map_err(|e| e.to_string())
}

pub fn disable() -> Result<(), String> {
    match autostart_file() {
        Some(p) if p.exists() => std::fs::remove_file(p).map_err(|e| e.to_string()),
        _ => Ok(()),
    }
}

pub fn admin_is_enabled() -> bool {
    false
}

pub fn admin_enable() -> Result<(), String> {
    Err("Unter Linux nicht nötig – TaskPulse liest die Werte ohne root-Rechte".into())
}

pub fn admin_enable_for(_exe: &str) -> Result<(), String> {
    admin_enable()
}

pub fn admin_disable() -> Result<(), String> {
    Ok(())
}

pub fn run_admin_task() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_file() {
        let d = desktop_entry("/home/a/TaskPulse.AppImage");
        assert!(d.contains("Exec=\"/home/a/TaskPulse.AppImage\" --taskbar"));
        assert!(d.starts_with("[Desktop Entry]"));
    }
}
