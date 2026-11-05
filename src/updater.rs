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
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Networking::WinHttp::*;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Name der Installer-Datei, die jede Veröffentlichung auf GitHub anhängen muss.
pub const ASSET_NAME: &str = "TaskPulse-Setup.exe";

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

/// Ist `remote` neuer als `local`?
pub fn is_newer(remote: &str, local: &str) -> bool {
    let (mut a, mut b) = (parse_version(remote), parse_version(local));
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a > b
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page_url: String,
    pub asset_url: String,
    pub asset_size: u64,
}

// ------------------------------------------------------------------ HTTP (WinHTTP)

struct H(*mut std::ffi::c_void);

impl Drop for H {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn last_err(what: &str) -> String {
    format!("{what}: {}", windows::core::Error::from_win32().message())
}

/// HTTPS-GET mit Weiterleitungen (GitHub leitet Downloads auf einen anderen Server um).
/// `max` begrenzt die Antwortgröße.
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
    let asset = v["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"].as_str().map(|n| n.eq_ignore_ascii_case(ASSET_NAME)).unwrap_or(false)));
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
        return Err(format!("Version {} ist veröffentlicht, aber ohne {ASSET_NAME}", r.version));
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
    if data.len() < 1 << 20 || &data[..2] != b"MZ" {
        return Err("Heruntergeladene Datei ist kein gültiges Programm".into());
    }
    let dir = update_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let p = dir.join(ASSET_NAME);
    std::fs::write(&p, data).map_err(|e| format!("Speichern: {e}"))?;
    Ok(p)
}

/// Startet den heruntergeladenen Installer im Update-Modus. Er beendet TaskPulse, ersetzt die
/// installierte EXE und startet TaskPulse wieder (`reopen`: auch das Hauptfenster).
/// Ohne Adminrechte fragt Windows einmal nach (Program Files ist geschützt).
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

/// Läuft diese EXE aus dem Installationsordner? Nur dann wird aktualisiert.
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
                if let Ok(Some(r)) = check(&cfg) {
                    if winutil::find_own_window(winutil::MAIN_TITLE).is_some() {
                        wait = Duration::from_secs(1800);
                    } else if let Ok(p) = download(&r) {
                        let _ = apply(&p, false);
                    }
                }
            }
            std::thread::sleep(wait);
        }
    });
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
    fn release_json() {
        let j = br#"{"tag_name":"v0.2.0","html_url":"https://github.com/a/b/releases/tag/v0.2.0","body":"Neu",
            "assets":[{"name":"other.zip","size":1,"browser_download_url":"x"},
                      {"name":"TaskPulse-Setup.exe","size":5930496,"browser_download_url":"https://github.com/a/b/releases/download/v0.2.0/TaskPulse-Setup.exe"}]}"#;
        let r = parse_release(j).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.asset_size, 5930496);
        assert!(r.asset_url.ends_with("/TaskPulse-Setup.exe"));
        assert!(parse_release(b"{}").is_err());
    }
}
