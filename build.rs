use embed_manifest::{embed_manifest, new_manifest};
use std::path::PathBuf;
use std::process::Command;

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // Manifest: Windows-10/11-kompatibel (nötig für Layered-Child-Fenster in der Taskleiste),
        // Per-Monitor-DPI v2, ohne Adminzwang (asInvoker).
        embed_manifest(new_manifest("TaskPulse")).expect("Manifest konnte nicht eingebettet werden");
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
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_resources(&version);
    }
    println!("cargo:rerun-if-changed=dev_version.txt");
    println!("cargo:rerun-if-env-changed=GITHUB_ACTIONS");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=resources/icon/TaskPulse.ico");
    println!("cargo:rerun-if-changed=resources/icon/TaskPulse.res");
}

/// Programmsymbol und Versionsangaben (Hersteller, Beschreibung, Version – sichtbar unter
/// Eigenschaften → Details) als Windows-Ressource einbetten. Ohne Versionsangaben stufen
/// Virenscanner eine EXE eher als verdächtig ein.
/// GNU-Toolchain: über windres (MSYS2-binutils, siehe build.ps1); MSVC: direkt.
fn embed_resources(version: &str) {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("resources").join("icon");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    // .res = Folge von Einträgen: Symbol (fertig in TaskPulse.res) + Versionsangaben
    let mut res = std::fs::read(dir.join("TaskPulse.res")).expect("TaskPulse.res fehlt");
    res.extend(res_entry(16, 1, &version_info(version)));
    let res_path = out_dir.join("taskpulse.res");
    std::fs::write(&res_path, &res).expect("taskpulse.res nicht geschrieben");
    // MSVC (z. B. GitHub Actions): link.exe nimmt die .res-Datei direkt
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins={}", res_path.display());
        return;
    }
    let out = out_dir.join("taskpulse_icon.o");
    let ok = Command::new("windres")
        .args(["-J", "res", "-O", "coff", "-i"])
        .arg(&res_path)
        .arg("-o")
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

/// Ein Eintrag einer .res-Datei (Typ und Kennung als Zahl)
fn res_entry(rtype: u16, id: u16, data: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    for x in [data.len() as u32, 32] {
        v.extend(x.to_le_bytes());
    }
    for x in [0xFFFFu16, rtype, 0xFFFF, id] {
        v.extend(x.to_le_bytes());
    }
    v.extend(0u32.to_le_bytes()); // DataVersion
    v.extend(0x1030u16.to_le_bytes()); // MemoryFlags
    v.extend(0x0407u16.to_le_bytes()); // Sprache: Deutsch
    v.extend(0u32.to_le_bytes()); // Version
    v.extend(0u32.to_le_bytes()); // Characteristics
    v.extend(data);
    while v.len() % 4 != 0 {
        v.push(0);
    }
    v
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain(std::iter::once(0)).flat_map(|c| c.to_le_bytes()).collect()
}

fn pad4(v: &mut Vec<u8>) {
    while v.len() % 4 != 0 {
        v.push(0);
    }
}

/// Knoten der VS_VERSIONINFO-Struktur: Länge, Wertlänge, Typ (0 = binär, 1 = Text), Schlüssel, Wert, Kinder
fn vs_node(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut v = vec![0, 0];
    v.extend(value_len.to_le_bytes());
    v.extend((text as u16).to_le_bytes());
    v.extend(utf16z(key));
    pad4(&mut v);
    v.extend(value);
    for c in children {
        pad4(&mut v);
        v.extend(c);
    }
    let len = v.len() as u16;
    v[0..2].copy_from_slice(&len.to_le_bytes());
    v
}

/// Versionsangaben für Explorer (Eigenschaften → Details) und Virenscanner
fn version_info(version: &str) -> Vec<u8> {
    // "0.1.9.2-Dev" / "0.1.9" → 0, 1, 9, 2
    let mut n = [0u16; 4];
    for (i, p) in version.split(['.', '-']).take(4).enumerate() {
        n[i] = p.parse().unwrap_or(0);
    }
    let ms = ((n[0] as u32) << 16) | n[1] as u32;
    let ls = ((n[2] as u32) << 16) | n[3] as u32;
    let mut fixed = Vec::new();
    // VS_FIXEDFILEINFO: Kennung, Strukturversion, Datei-/Produktversion, Flags, Betriebssystem (NT/Win32), Typ (Programm)
    for x in [0xFEEF_04BDu32, 0x0001_0000, ms, ls, ms, ls, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0] {
        fixed.extend(x.to_le_bytes());
    }
    let repo = env!("CARGO_PKG_REPOSITORY");
    let strings: Vec<Vec<u8>> = [
        ("CompanyName", "TaskPulse-Projekt"),
        ("FileDescription", "TaskPulse – Task-Manager mit GPU-Werten (Programm und Installer)"),
        ("FileVersion", version),
        ("InternalName", "TaskPulse"),
        ("LegalCopyright", "TaskPulse-Projekt – Quellcode auf GitHub"),
        ("OriginalFilename", "TaskPulse.exe"),
        ("ProductName", "TaskPulse"),
        ("ProductVersion", version),
        ("Comments", repo),
    ]
    .iter()
    .map(|(k, val)| {
        let d = utf16z(val);
        vs_node(k, &d, (d.len() / 2) as u16, true, &[])
    })
    .collect();
    // Deutsch (0407), Unicode (04B0)
    let table = vs_node("040704B0", &[], 0, true, &strings);
    let sfi = vs_node("StringFileInfo", &[], 0, true, &[table]);
    let mut tr = Vec::new();
    tr.extend(0x0407u16.to_le_bytes());
    tr.extend(1200u16.to_le_bytes());
    let var = vs_node("Translation", &tr, 4, false, &[]);
    let vfi = vs_node("VarFileInfo", &[], 0, true, &[var]);
    vs_node("VS_VERSION_INFO", &fixed, fixed.len() as u16, false, &[sfi, vfi])
}
