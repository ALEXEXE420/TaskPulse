//! Updates über GitHub-Releases.
//!
//! Ablauf: Die neueste Veröffentlichung wird über die GitHub-API abgefragt
//! (`/repos/<repo>/releases/latest`). Ist ihre Version höher als die laufende und hängt dort
//! `TaskPulse-Setup.exe` an, wird diese Datei geladen und mit `--update` gestartet. Der Installer
//! aktualisiert dann die installierte Version (Einstellungen bleiben) und startet TaskPulse neu.
//!
//! Aktualisiert wird nur eine installierte Version (Program Files), nicht eine EXE, die
//! irgendwo anders liegt, z. B. im Entwicklungsordner.
//!
//! Internetzugriff gibt es nur hier: eine Abfrage beim Start und danach alle 12 Stunden,
//! abschaltbar in den Einstellungen (Updates → automatisch).

use crate::config::Config;
use crate::winutil;
use std::path::{Path, PathBuf};
use std::time::Duration;
#[cfg(windows)]
use windows::core::{HSTRING, PCWSTR};
#[cfg(windows)]
use windows::Win32::Networking::WinHttp::*;

/// Laufende Version: Release "0.1.7" oder Entwicklerversion "0.1.6.1-Dev" (siehe build.rs).
pub const CURRENT: &str = env!("TASKPULSE_VERSION");

/// Lokal gebaute Entwicklerversion?
pub fn is_dev(v: &str) -> bool {
    v.to_lowercase().ends_with("-dev")
}
/// Name der Installer-Datei für x64-Windows (jede Veröffentlichung hängt sie an).
pub const ASSET_NAME: &str = "TaskPulse-Setup.exe";
/// Name der Installer-Datei für Windows auf ARM (z. B. NVIDIA RTX Spark N1X, Snapdragon X).
pub const ASSET_NAME_ARM64: &str = "TaskPulse-Setup-ARM64.exe";

/// Welche Datei dieser Rechner braucht: auf ARM die native ARM64-Version (auch wenn gerade noch
/// die x64-Version in der Emulation läuft – so wechselt sie beim nächsten Update), sonst x64.
#[cfg(target_os = "linux")]
pub fn asset_names() -> [&'static str; 2] {
    // Linux: AppImage je Prozessor
    if cfg!(target_arch = "aarch64") {
        ["TaskPulse-aarch64.AppImage", "TaskPulse-aarch64.AppImage"]
    } else {
        ["TaskPulse-x86_64.AppImage", "TaskPulse-x86_64.AppImage"]
    }
}

#[cfg(windows)]
pub fn asset_names() -> [&'static str; 2] {
    if winutil::native_arm64() {
        [ASSET_NAME_ARM64, ASSET_NAME]
    } else {
        [ASSET_NAME, ASSET_NAME]
    }
}

/// Fest eingebautes Repository "Besitzer/Name": beim Bau auf GitHub automatisch (build.rs),
/// sonst aus `repository` in Cargo.toml.
pub fn builtin_repo() -> String {
    option_env!("TASKPULSE_REPO").and_then(repo_from_url).or_else(|| repo_from_url(env!("CARGO_PKG_REPOSITORY"))).unwrap_or_default()
}

/// "https://github.com/a/b(.git)" oder "a/b" → "a/b"
pub fn repo_from_url(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches('/').trim_end_matches(".git");
    let s = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).unwrap_or(s);
    let s = s.strip_prefix("www.").unwrap_or(s);
    let s = s.strip_prefix("github.com/").unwrap_or(s);
    let mut parts = s.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    let ok = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && !p.contains("OWNER");
    (parts.next().is_none() && ok(owner) && ok(name)).then(|| format!("{owner}/{name}"))
}

/// Repository laut Einstellung, sonst das eingebaute.
pub fn repo(cfg: &Config) -> Option<String> {
    repo_from_url(&cfg.update_repo).or_else(|| repo_from_url(&builtin_repo()))
}

/// Versionsnummer "v1.2.3" / "1.2.3-beta" → [1, 2, 3]
fn parse_version(v: &str) -> Vec<u64> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next().unwrap_or("");
    core.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Vergleichsschlüssel: (Hauptversion a.b.c, Release vor Dev, Dev-Nummer).
/// Eine Entwicklerversion „0.1.8.3-Dev“ ist eine Vorabversion von 0.1.8: neuer als 0.1.7,
/// aber älter als das Release 0.1.8 – so holt sie sich das fertige Release per Update.
fn version_key(v: &str) -> (Vec<u64>, u8, u64) {
    let mut p = parse_version(v);
    p.resize(p.len().max(3), 0);
    if is_dev(v) {
        let n = p.get(3).copied().unwrap_or(0);
        p.truncate(3);
        (p, 0, n)
    } else {
        // vierstellige Releases (falls es je welche gibt) zählen wie a.b.c mit Zusatz
        let extra = p.get(3).copied().unwrap_or(0);
        p.truncate(3);
        (p, 1, extra)
    }
}

/// Ist `remote` neuer als `local`?
pub fn is_newer(remote: &str, local: &str) -> bool {
    version_key(remote) > version_key(local)
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page_url: String,
    pub asset_url: String,
    pub asset_size: u64,
}

// ------------------------------------------------------------------ HTTP (Linux: curl)

/// Linux: HTTPS-GET über curl (auf praktisch jeder Distribution vorhanden), mit Weiterleitungen.
#[cfg(target_os = "linux")]
pub fn http_get(url: &str, accept: &str, max: usize) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
        return Err("Nur https-Adressen".into());
    }
    let tmp = std::env::temp_dir().join(format!("taskpulse-http-{}", std::process::id()));
    let out = std::process::Command::new("curl")
        .args(["-sSL", "--max-time", "120", "--max-filesize"])
        .arg(max.to_string())
        .args(["-H", &format!("Accept: {accept}"), "-A", &format!("TaskPulse-Updater/{CURRENT}"), "-w", "%{http_code}", "-o"])
        .arg(&tmp)
        .arg(url)
        .output()
        .map_err(|e| format!("curl nicht gefunden ({e}) – bitte curl installieren"))?;
    let status: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0);
    let body = std::fs::read(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    if !out.status.success() && status == 0 {
        return Err(format!("Download fehlgeschlagen: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    match status {
        200 => Ok(body),
        404 => Err("Keine Veröffentlichung gefunden (404) – Repository-Name prüfen oder erst ein Release anlegen".into()),
        403 | 429 => Err(format!("GitHub hat die Anfrage abgelehnt ({status}) – später erneut versuchen")),
        s => Err(format!("HTTP-Status {s}")),
    }
}

// ------------------------------------------------------------------ HTTP (WinHTTP)

#[cfg(windows)]
struct H(*mut std::ffi::c_void);

#[cfg(windows)]
impl Drop for H {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

#[cfg(windows)]
fn last_err(what: &str) -> String {
    format!("{what}: {}", windows::core::Error::from_win32().message())
}

/// HTTPS-GET mit Weiterleitungen (GitHub leitet Downloads auf einen anderen Server um).
/// `max` begrenzt die Antwortgröße.
#[cfg(windows)]
pub fn http_get(url: &str, accept: &str, max: usize) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("https://").ok_or("Nur https-Adressen")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    unsafe {
        let agent = HSTRING::from(format!("TaskPulse-Updater/{CURRENT}"));
        let s = H(WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
        if s.0.is_null() {
            return Err(last_err("WinHttpOpen"));
        }
        let _ = WinHttpSetTimeouts(s.0, 10_000, 10_000, 15_000, 30_000);
        let c = H(WinHttpConnect(s.0, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT, 0));
        if c.0.is_null() {
            return Err(last_err("Verbindung"));
        }
        let r = H(WinHttpOpenRequest(c.0, &HSTRING::from("GET"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE));
        if r.0.is_null() {
            return Err(last_err("Anfrage"));
        }
        let headers: Vec<u16> = format!("Accept: {accept}\r\n").encode_utf16().collect();
        WinHttpSendRequest(r.0, Some(&headers), None, 0, 0, 0).map_err(|e| format!("Senden: {}", e.message()))?;
        WinHttpReceiveResponse(r.0, std::ptr::null_mut()).map_err(|e| format!("Antwort: {}", e.message()))?;
        let mut status: u32 = 0;
        let mut len = 4u32;
        WinHttpQueryHeaders(
            r.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut _),
            &mut len,
            std::ptr::null_mut(),
        )
        .map_err(|e| format!("Status: {}", e.message()))?;
        let mut body = Vec::new();
        loop {
            let mut avail = 0u32;
            WinHttpQueryDataAvailable(r.0, &mut avail).map_err(|e| format!("Lesen: {}", e.message()))?;
            if avail == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + avail as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(r.0, body[start..].as_mut_ptr() as *mut _, avail, &mut read).map_err(|e| format!("Lesen: {}", e.message()))?;
            body.truncate(start + read as usize);
            if body.len() > max {
                return Err("Antwort zu groß".into());
            }
        }
        match status {
            200 => Ok(body),
            404 => Err("Keine Veröffentlichung gefunden (404) – Repository-Name prüfen oder erst ein Release anlegen".into()),
            403 | 429 => Err(format!("GitHub hat die Anfrage abgelehnt ({status}) – später erneut versuchen")),
            s => Err(format!("HTTP-Status {s}")),
        }
    }
}

// ------------------------------------------------------------------ Prüfen, laden, installieren

pub fn parse_release(json: &[u8]) -> Result<Release, String> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| format!("Antwort unlesbar: {e}"))?;
    let tag = v["tag_name"].as_str().ok_or("Release ohne Versionsnummer")?;
    let assets = v["assets"].as_array();
    let find = |name: &str| assets.and_then(|a| a.iter().find(|x| x["name"].as_str().map(|n| n.eq_ignore_ascii_case(name)).unwrap_or(false)));
    let [first, fallback] = asset_names();
    let asset = find(first).or_else(|| find(fallback));
    Ok(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        notes: v["body"].as_str().unwrap_or("").to_string(),
        page_url: v["html_url"].as_str().unwrap_or("").to_string(),
        asset_url: asset.and_then(|a| a["browser_download_url"].as_str()).unwrap_or("").to_string(),
        asset_size: asset.and_then(|a| a["size"].as_u64()).unwrap_or(0),
    })
}

/// Fragt die neueste Veröffentlichung ab.
pub fn latest(repo: &str) -> Result<Release, String> {
    let body = http_get(&format!("https://api.github.com/repos/{repo}/releases/latest"), "application/vnd.github+json", 2 << 20)?;
    parse_release(&body)
}

/// Neuere Version vorhanden? `Ok(None)` = aktuell.
pub fn check(cfg: &Config) -> Result<Option<Release>, String> {
    let repo = repo(cfg).ok_or("Kein GitHub-Repository eingestellt")?;
    let r = latest(&repo)?;
    if !is_newer(&r.version, CURRENT) {
        return Ok(None);
    }
    if r.asset_url.is_empty() {
        return Err(format!("Version {} ist veröffentlicht, aber ohne {}", r.version, asset_names()[0]));
    }
    Ok(Some(r))
}

fn update_dir() -> PathBuf {
    Config::dir().join("update")
}

/// Lädt den Installer der Veröffentlichung herunter.
pub fn download(r: &Release) -> Result<PathBuf, String> {
    let data = http_get(&r.asset_url, "application/octet-stream", 200 << 20)?;
    if r.asset_size > 0 && data.len() as u64 != r.asset_size {
        return Err(format!("Download unvollständig ({} von {} Byte)", data.len(), r.asset_size));
    }
    // Windows-Programm beginnt mit „MZ“, eine Linux-AppImage mit der ELF-Kennung
    let magic: &[u8] = if cfg!(windows) { b"MZ" } else { b"\x7fELF" };
    if data.len() < 1 << 20 || !data.starts_with(magic) {
        return Err("Heruntergeladene Datei ist kein gültiges Programm".into());
    }
    let dir = update_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let p = dir.join(asset_names()[0]);
    std::fs::write(&p, data).map_err(|e| format!("Speichern: {e}"))?;
    Ok(p)
}

/// Linux: die laufende AppImage-Datei durch die neue ersetzen (geht auch im laufenden Betrieb),
/// TaskPulse neu starten und diesen Prozess beenden.
#[cfg(target_os = "linux")]
pub fn apply(new_file: &Path, reopen: bool) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let target = PathBuf::from(std::env::var("APPIMAGE").map_err(|_| "Nicht als AppImage gestartet – bitte die neue Datei von GitHub laden")?);
    let tmp = target.with_extension("neu");
    std::fs::copy(new_file, &tmp).map_err(|e| format!("Kopieren: {e}"))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &target).map_err(|e| format!("Ersetzen von {}: {e}", target.display()))?;
    write_result(&format!("✔ TaskPulse wurde auf Version {} aktualisiert.", latest_version_hint(new_file)));
    // Neu starten: Anzeige bzw. Hauptfenster aus der neuen Datei
    let _ = std::fs::remove_file(Config::dir().join("anzeige.pid"));
    let _ = std::fs::remove_file(Config::dir().join("hauptfenster.pid"));
    let args: &[&str] = if reopen { &[] } else { &["--taskbar"] };
    std::process::Command::new(&target).args(args).spawn().map_err(|e| e.to_string())?;
    cleanup();
    winutil::fast_exit(0);
}

#[cfg(target_os = "linux")]
fn latest_version_hint(_f: &Path) -> String {
    "neueste Version".into()
}

/// Startet den heruntergeladenen Installer im Update-Modus. Er beendet TaskPulse, ersetzt die
/// installierte EXE und startet TaskPulse wieder (`reopen`: auch das Hauptfenster).
/// Ohne Adminrechte fragt Windows einmal nach (Program Files ist geschützt).
#[cfg(windows)]
pub fn apply(setup: &Path, reopen: bool) -> Result<(), String> {
    let args = if reopen { "--update --reopen" } else { "--update" };
    if winutil::is_elevated() {
        std::process::Command::new(setup).args(args.split(' ')).current_dir(update_dir()).spawn().map_err(|e| e.to_string())?;
        return Ok(());
    }
    unsafe {
        use windows::core::w;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let r = ShellExecuteW(None, w!("runas"), &HSTRING::from(setup.as_os_str()), &HSTRING::from(args), PCWSTR::null(), SW_SHOWNORMAL);
        if r.0 as isize <= 32 {
            return Err("Update abgebrochen (Adminrechte verweigert)".into());
        }
    }
    Ok(())
}

/// Linux: nur eine gestartete AppImage-Datei wird aktualisiert (nicht ein selbst gebautes Programm)
#[cfg(target_os = "linux")]
pub fn running_installed() -> bool {
    std::env::var_os("APPIMAGE").map(|p| std::path::Path::new(&p).is_file()).unwrap_or(false)
}

/// Läuft diese EXE aus dem Installationsordner? Nur dann wird aktualisiert.
#[cfg(windows)]
pub fn running_installed() -> bool {
    let Some(dir) = crate::installer::installed_dir() else { return false };
    match (std::env::current_exe().and_then(|p| p.canonicalize()), dir.join("TaskPulse.exe").canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Übrig gebliebenen Installer eines erledigten Updates löschen.
pub fn cleanup() {
    let _ = std::fs::remove_dir_all(update_dir());
}

/// Hintergrundprüfung im Prozess der Taskleisten-Anzeige: 2 Minuten nach dem Start, dann alle
/// 12 Stunden. Ein Update wird nur eingespielt, wenn das Hauptfenster nicht offen ist (das
/// Hauptfenster zeigt dann selbst einen Hinweis); sonst erneuter Versuch nach 30 Minuten.
pub fn background_thread() {
    let _ = std::thread::Builder::new().name("updates".into()).spawn(|| {
        std::thread::sleep(Duration::from_secs(120));
        cleanup();
        loop {
            let cfg = Config::load();
            let mut wait = Duration::from_secs(12 * 3600);
            if cfg.auto_update && running_installed() {
                match check(&cfg) {
                    Ok(Some(r)) => {
                        if winutil::find_own_window(winutil::MAIN_TITLE).is_some() {
                            wait = Duration::from_secs(1800);
                        } else {
                            match download(&r).and_then(|p| apply(&p, false)) {
                                Ok(()) => crate::diag::log(format!("Update auf {} gestartet", r.version)),
                                Err(e) => crate::diag::log(format!("Automatisches Update auf {} fehlgeschlagen: {e}", r.version)),
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => crate::diag::log(format!("Update-Prüfung fehlgeschlagen: {e}")),
                }
            }
            std::thread::sleep(wait);
        }
    });
}

/// Änderungsprotokoll, in die EXE eingebaut (CHANGELOG.md).
pub const CHANGELOG: &str = include_str!("../CHANGELOG.md");

/// Abschnitte des Änderungsprotokolls: (Überschrift, Punkte). Leere Abschnitte fallen weg.
pub fn changelog_sections() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in CHANGELOG.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            out.push((h.trim().to_string(), String::new()));
        } else if let Some((_, body)) = out.last_mut() {
            if !line.trim().is_empty() {
                let l = line.trim_end();
                // Markdown-Aufzählung als Punkt anzeigen
                match l.strip_prefix("- ") {
                    Some(rest) => {
                        body.push_str("• ");
                        body.push_str(rest);
                    }
                    None => body.push_str(l),
                }
                body.push('\n');
            }
        }
    }
    out.retain(|(_, b)| !b.trim().is_empty());
    out
}

/// Punkte einer Version, z. B. "0.1.6". Für Entwicklerversionen der Abschnitt „Unveröffentlicht“.
pub fn changelog_for(version: &str) -> Option<String> {
    let version = if is_dev(version) { "Unveröffentlicht" } else { version };
    changelog_sections().into_iter().find(|(h, _)| h == version || h.starts_with(&format!("{version} "))).map(|(_, b)| b)
}

/// Ergebnis des letzten Updates (vom Installer geschrieben), einmalig abholen.
pub fn take_last_result() -> Option<String> {
    let p = Config::dir().join("update_result.txt");
    let s = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(p);
    Some(s)
}

pub fn write_result(s: &str) {
    let _ = std::fs::create_dir_all(Config::dir());
    let _ = std::fs::write(Config::dir().join("update_result.txt"), s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("0.1.6", "0.1.5"));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("0.1.5", "0.1.5"));
        assert!(!is_newer("v0.1.5", "0.1.5"));
        assert!(!is_newer("0.1.4", "0.1.5"));
        assert!(!is_newer("0.1.5-beta", "0.1.5"));
        // Entwicklerversion 0.1.8.1-Dev = Vorabversion von 0.1.8:
        // das ältere Release 0.1.7 ersetzt sie nicht, das fertige 0.1.8 schon
        assert!(!is_newer("0.1.7", "0.1.8.1-Dev"));
        assert!(is_newer("0.1.8", "0.1.8.1-Dev"));
        assert!(is_newer("0.1.8", "0.1.8.12-Dev"));
        assert!(is_newer("0.1.9", "0.1.8.12-Dev"));
        assert!(is_newer("0.1.8.2-Dev", "0.1.8.1-Dev"));
        assert!(!is_newer("0.1.8.1-Dev", "0.1.8"));
        assert!(is_dev("0.1.6.1-Dev"));
        assert!(!is_dev("0.1.7"));
    }

    #[test]
    fn repo_names() {
        assert_eq!(repo_from_url("https://github.com/max/TaskPulse").as_deref(), Some("max/TaskPulse"));
        assert_eq!(repo_from_url("https://github.com/max/TaskPulse.git").as_deref(), Some("max/TaskPulse"));
        assert_eq!(repo_from_url("max/Task-Pulse_2").as_deref(), Some("max/Task-Pulse_2"));
        assert_eq!(repo_from_url(""), None);
        assert_eq!(repo_from_url("max"), None);
        assert_eq!(repo_from_url("https://github.com/OWNER/TaskPulse"), None);
        assert_eq!(repo_from_url("a/b/c"), None);
    }

    #[test]
    fn changelog_has_released_versions() {
        let s = changelog_sections();
        assert!(s.iter().any(|(h, _)| h.starts_with("0.1.6")));
        assert!(changelog_for("0.1.6").unwrap().contains("CPU RAM"));
        // Entwicklerversion zeigt, was seit dem letzten Release dazukam
        assert_eq!(changelog_for("0.1.6.3-Dev"), changelog_for("Unveröffentlicht"));
        assert!(changelog_for("0.1").is_none());
        assert!(s.iter().any(|(h, _)| h.starts_with("0.1.7")));
    }

    #[test]
    fn release_json() {
        // Ein Release mit allen Dateien – jedes System muss seine eigene finden
        // (Windows x64/ARM64: Setup-Datei, Linux: AppImage)
        let j = br#"{"tag_name":"v0.2.0","html_url":"https://github.com/a/b/releases/tag/v0.2.0","body":"Neu",
            "assets":[{"name":"other.zip","size":1,"browser_download_url":"x"},
                      {"name":"TaskPulse-Setup.exe","size":5930496,"browser_download_url":"https://github.com/a/b/releases/download/v0.2.0/TaskPulse-Setup.exe"},
                      {"name":"TaskPulse-Setup-ARM64.exe","size":5000000,"browser_download_url":"https://github.com/a/b/releases/download/v0.2.0/TaskPulse-Setup-ARM64.exe"},
                      {"name":"TaskPulse-x86_64.AppImage","size":7000000,"browser_download_url":"https://github.com/a/b/releases/download/v0.2.0/TaskPulse-x86_64.AppImage"},
                      {"name":"TaskPulse-aarch64.AppImage","size":6500000,"browser_download_url":"https://github.com/a/b/releases/download/v0.2.0/TaskPulse-aarch64.AppImage"}]}"#;
        let r = parse_release(j).unwrap();
        assert_eq!(r.version, "0.2.0");
        let want = asset_names()[0];
        assert!(r.asset_url.ends_with(&format!("/{want}")), "{} statt {want}", r.asset_url);
        assert!(r.asset_size > 1);
        // Fehlt die Datei für dieses System, bleibt die Adresse leer (kein Update mit einer falschen Datei)
        let only_other = br#"{"tag_name":"v0.2.0","html_url":"h","body":"",
            "assets":[{"name":"other.zip","size":1,"browser_download_url":"x"}]}"#;
        assert!(parse_release(only_other).unwrap().asset_url.is_empty());
        assert!(parse_release(b"{}").is_err());
    }
}
