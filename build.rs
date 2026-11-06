use embed_manifest::{embed_manifest, new_manifest};
use std::path::PathBuf;
use std::process::Command;

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // Manifest: Windows-10/11-kompatibel (nötig für Layered-Child-Fenster in der Taskleiste),
        // Per-Monitor-DPI v2, ohne Adminzwang (asInvoker).
        embed_manifest(new_manifest("TaskPulse")).expect("Manifest konnte nicht eingebettet werden");
        embed_icon();
    }
    // Update-Quelle: Beim Bauen auf GitHub (Actions) ist GITHUB_REPOSITORY = "Besitzer/Name" gesetzt –
    // so kennt jede dort gebaute Version ihr Repository, ohne dass es eingetragen werden muss.
    if let Ok(r) = std::env::var("GITHUB_REPOSITORY") {
        println!("cargo:rustc-env=TASKPULSE_REPO={r}");
    }
    println!("cargo:rerun-if-env-changed=GITHUB_REPOSITORY");

    // Angezeigte Version: Auf GitHub gebaute Releases tragen die Version aus Cargo.toml (z. B. 0.1.7),
    // lokale Builds eine Entwicklerversion (z. B. 0.1.6.1-Dev). Die laufende Nummer steht in
    // dev_version.txt ("0.1.6=1") und wird von Installer\setup_erstellen.ps1 hochgezählt.
    let pkg = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let version = if std::env::var_os("GITHUB_ACTIONS").is_some() {
        pkg.clone()
    } else {
        let n = std::fs::read_to_string("dev_version.txt")
            .ok()
            .and_then(|s| {
                let (v, n) = s.trim().split_once('=')?;
                (v.trim() == pkg).then(|| n.trim().parse::<u32>().ok()).flatten()
            })
            .unwrap_or(1);
        format!("{pkg}.{n}-Dev")
    };
    println!("cargo:rustc-env=TASKPULSE_VERSION={version}");
    println!("cargo:rerun-if-changed=dev_version.txt");
    println!("cargo:rerun-if-env-changed=GITHUB_ACTIONS");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=resources/icon/TaskPulse.ico");
    println!("cargo:rerun-if-changed=resources/icon/TaskPulse.res");
}

/// Programmsymbol (Explorer, Verknüpfungen, "Apps") als Windows-Ressource einbetten.
/// GNU-Toolchain: über windres (MSYS2-binutils, siehe build.ps1); MSVC: direkt.
fn embed_icon() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("resources").join("icon");
    // MSVC (z. B. GitHub Actions): link.exe nimmt die .res-Datei direkt
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins={}", dir.join("TaskPulse.res").display());
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("taskpulse_icon.o");
    let ok = Command::new("windres")
        .current_dir(&dir)
        .args(["-J", "res", "-i", "TaskPulse.res", "-O", "coff", "-o"])
        .arg(&out)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        println!("cargo:rustc-link-arg-bins={}", out.display());
    } else {
        println!("cargo:warning=windres fehlgeschlagen – EXE ohne Programmsymbol gebaut");
    }
}
