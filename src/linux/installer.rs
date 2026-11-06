//! Linux-Gegenstück zu installer.rs. Unter Linux gibt es TaskPulse als AppImage – eine einzelne
//! Datei, die ohne Installation läuft. Ein Installer ist deshalb nicht nötig; die Funktionen hier
//! liefern die Angaben, die der Rest des Programms erwartet.
#![allow(dead_code)]

use std::path::PathBuf;

pub fn is_setup_exe() -> bool {
    false
}

/// „Installiert“ = als AppImage gestartet: der Ordner, in dem die AppImage-Datei liegt.
pub fn installed_dir() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE").map(PathBuf::from).and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

pub fn default_dir() -> PathBuf {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Anwendungen")).unwrap_or_else(|| PathBuf::from("."))
}

/// PawnIO ist ein Windows-Treiber; unter Linux liefert der Kernel die Sensoren selbst (hwmon).
pub fn pawnio_installed() -> bool {
    false
}

pub fn run(_uninstall: bool) {
    eprintln!("Unter Linux braucht TaskPulse keinen Installer – die AppImage-Datei einfach starten.");
}

/// Update unter Linux erledigt der Updater selbst (AppImage-Datei ersetzen) – hier nichts zu tun.
pub fn run_update(_reopen: bool) {}
