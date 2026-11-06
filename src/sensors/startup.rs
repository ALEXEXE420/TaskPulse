//! Autostart von Apps – dieselben Stellen wie der Windows-Task-Manager:
//! - Registrierung „Run“ (aktueller Benutzer, alle Benutzer, 32-Bit-Programme)
//! - Autostart-Ordner (eigener und für alle Benutzer)
//! - Apps aus dem Microsoft Store (StartupTask)
//!
//! Ein-/Ausschalten wie im Task-Manager über „StartupApproved“ (der Eintrag selbst bleibt erhalten),
//! bei Store-Apps über deren Status. Linux: ~/.config/autostart und /etc/xdg/autostart (.desktop).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[cfg_attr(windows, allow(dead_code))]
pub enum Source {
    RunUser,
    RunMachine,
    RunMachine32,
    FolderUser,
    FolderCommon,
    StoreApp,
    /// Linux: ~/.config/autostart
    DesktopUser,
    /// Linux: /etc/xdg/autostart
    DesktopSystem,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Source::RunUser => "Registrierung (nur du)",
            Source::RunMachine => "Registrierung (alle Benutzer)",
            Source::RunMachine32 => "Registrierung (alle, 32 Bit)",
            Source::FolderUser => "Autostart-Ordner (nur du)",
            Source::FolderCommon => "Autostart-Ordner (alle Benutzer)",
            Source::StoreApp => "Microsoft Store",
            Source::DesktopUser => "~/.config/autostart",
            Source::DesktopSystem => "/etc/xdg/autostart",
        }
    }

    /// Änderungen brauchen Adminrechte (gilt für alle Benutzer)
    pub fn needs_admin(&self) -> bool {
        matches!(self, Source::RunMachine | Source::RunMachine32 | Source::FolderCommon)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct StartupEntry {
    /// Name des Eintrags (Registrierungswert, Dateiname ohne Endung, Store-App)
    pub name: String,
    /// Schöner Name aus der Programmdatei (Dateibeschreibung), falls vorhanden
    pub description: Option<String>,
    pub publisher: Option<String>,
    /// Befehlszeile bzw. Ziel der Verknüpfung
    pub command: String,
    /// Programmdatei (aus der Befehlszeile ermittelt)
    pub path: Option<String>,
    pub source: Source,
    pub enabled: bool,
    /// Wann ausgeschaltet (falls bekannt)
    pub disabled_since: Option<String>,
    /// Programmdatei fehlt (Eintrag verwaist)
    pub missing: bool,
    /// Schlüssel für Änderungen: Wertname, Dateiname bzw. "Paket\\Task"
    pub key: String,
}

impl StartupEntry {
    pub fn display_name(&self) -> String {
        match &self.description {
            Some(d) if !d.eq_ignore_ascii_case(&self.name) && d.len() > 2 => d.clone(),
            _ => self.name.clone(),
        }
    }
}

/// „StartupApproved“-Wert → eingeschaltet? (Byte 0: gerade = an, ungerade = aus; fehlt = an)
pub fn approved_enabled(v: Option<&[u8]>) -> bool {
    match v {
        Some(b) if !b.is_empty() => b[0] & 1 == 0,
        _ => true,
    }
}

/// Wert für „StartupApproved“: an = 02 und Nullen, aus = 03 und Zeitpunkt (FILETIME)
pub fn approved_value(enabled: bool, filetime: u64) -> [u8; 12] {
    let mut v = [0u8; 12];
    if enabled {
        v[0] = 2;
    } else {
        v[0] = 3;
        v[4..12].copy_from_slice(&filetime.to_le_bytes());
    }
    v
}

/// %Umgebungsvariablen% ersetzen
pub fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) => {
                let var = &after[..j];
                match std::env::var(var) {
                    Ok(v) if !var.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(var);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Programmdatei aus einer Befehlszeile: "C:\x y\a.exe" -arg → C:\x y\a.exe; ohne Anführungszeichen bis „.exe“
pub fn command_path(cmd: &str) -> Option<String> {
    let c = expand_env(cmd.trim());
    if let Some(rest) = c.strip_prefix('"') {
        return rest.find('"').map(|e| rest[..e].to_string()).filter(|p| !p.is_empty());
    }
    let l = c.to_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".lnk", ".com"] {
        if let Some(i) = l.find(ext) {
            return Some(c[..i + ext.len()].to_string());
        }
    }
    c.split_whitespace().next().map(String::from)
}

/// Store-Paket "5319275A.WhatsAppDesktop_cv1g1gvanyjgm" → ("WhatsAppDesktop", Herausgeber falls lesbar)
pub fn store_names(family: &str) -> (String, Option<String>) {
    let base = family.split('_').next().unwrap_or(family);
    match base.split_once('.') {
        Some((publisher, name)) => {
            // Herausgeber nur, wenn es kein Kürzel aus Ziffern/Hex ist
            let readable = publisher.chars().any(|c| c.is_ascii_lowercase());
            (name.to_string(), readable.then(|| publisher.to_string()))
        }
        None => (base.to_string(), None),
    }
}

/// Sortierung: eingeschaltet zuerst, darin nach Name (ohne Groß-/Kleinschreibung)
pub fn sort_entries(v: &mut [StartupEntry]) {
    v.sort_by(|a, b| b.enabled.cmp(&a.enabled).then_with(|| a.display_name().to_lowercase().cmp(&b.display_name().to_lowercase())));
}

#[cfg(windows)]
pub use win::*;

#[cfg(windows)]
mod win {
    use super::*;
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::*;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const RUN32: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
    const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
    const STORE: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData";

    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    fn open(root: HKEY, sub: &str, write: bool) -> Option<Key> {
        let mut h = HKEY::default();
        let sam = if write { KEY_READ | KEY_WRITE } else { KEY_READ } | KEY_WOW64_64KEY;
        unsafe { (RegOpenKeyExW(root, &HSTRING::from(sub), 0, sam, &mut h) == ERROR_SUCCESS).then_some(Key(h)) }
    }

    fn create(root: HKEY, sub: &str) -> Result<Key, String> {
        let mut h = HKEY::default();
        let r = unsafe { RegCreateKeyExW(root, &HSTRING::from(sub), 0, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_READ | KEY_WRITE | KEY_WOW64_64KEY, None, &mut h, None) };
        if r == ERROR_SUCCESS {
            Ok(Key(h))
        } else {
            Err(denied(r.0))
        }
    }

    fn denied(code: u32) -> String {
        if code == 5 {
            "Zugriff verweigert – dafür braucht TaskPulse Adminrechte".into()
        } else {
            format!("Windows-Fehler {code}")
        }
    }

    /// Alle Werte eines Schlüssels: (Name, Typ, Daten)
    fn values(k: &Key) -> Vec<(String, u32, Vec<u8>)> {
        let mut out = Vec::new();
        for i in 0..512u32 {
            let mut name = [0u16; 512];
            let mut nlen = name.len() as u32;
            let mut typ = 0u32;
            let mut data = vec![0u8; 8192];
            let mut dlen = data.len() as u32;
            let r = unsafe { RegEnumValueW(k.0, i, PWSTR(name.as_mut_ptr()), &mut nlen, None, Some(&mut typ), Some(data.as_mut_ptr()), Some(&mut dlen)) };
            if r != ERROR_SUCCESS {
                break;
            }
            data.truncate(dlen as usize);
            out.push((String::from_utf16_lossy(&name[..nlen as usize]), typ, data));
        }
        out
    }

    fn subkeys(k: &Key) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..4096u32 {
            let mut name = [0u16; 512];
            let mut nlen = name.len() as u32;
            let r = unsafe { RegEnumKeyExW(k.0, i, PWSTR(name.as_mut_ptr()), &mut nlen, None, PWSTR::null(), None, None) };
            if r != ERROR_SUCCESS {
                break;
            }
            out.push(String::from_utf16_lossy(&name[..nlen as usize]));
        }
        out
    }

    fn reg_string(data: &[u8]) -> String {
        let w: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&w[..w.iter().position(|c| *c == 0).unwrap_or(w.len())]).trim().to_string()
    }

    fn reg_dword(k: &Key, name: &str) -> Option<u32> {
        let mut v = 0u32;
        let mut len = 4u32;
        let r = unsafe { RegQueryValueExW(k.0, &HSTRING::from(name), None, None, Some(&mut v as *mut u32 as *mut u8), Some(&mut len)) };
        (r == ERROR_SUCCESS).then_some(v)
    }

    /// StartupApproved-Werte eines Bereichs (Run, Run32, StartupFolder)
    fn approved(root: HKEY, part: &str) -> std::collections::HashMap<String, Vec<u8>> {
        open(root, &format!(r"{APPROVED}\{part}"), false).map(|k| values(&k).into_iter().map(|(n, _, d)| (n.to_lowercase(), d)).collect()).unwrap_or_default()
    }

    fn disabled_date(v: Option<&Vec<u8>>) -> Option<String> {
        let b = v?;
        (b.len() >= 12 && b[0] & 1 == 1).then(|| u64::from_le_bytes(b[4..12].try_into().unwrap())).filter(|t| *t > 0).and_then(super::super::spec::filetime_date)
    }

    /// Dateibeschreibung und Firma aus der Versionsinfo einer Programmdatei
    pub fn version_info(path: &str) -> (Option<String>, Option<String>) {
        use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
        unsafe {
            let p = HSTRING::from(path);
            let n = GetFileVersionInfoSizeW(&p, None);
            if n == 0 {
                return (None, None);
            }
            let mut buf = vec![0u8; n as usize];
            if GetFileVersionInfoW(&p, 0, n, buf.as_mut_ptr() as *mut _).is_err() {
                return (None, None);
            }
            // Sprache/Zeichensatz der ersten Übersetzung, sonst Englisch/Unicode
            let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut len = 0u32;
            let lang = if VerQueryValueW(buf.as_ptr() as *const _, &HSTRING::from(r"\VarFileInfo\Translation"), &mut ptr, &mut len).as_bool() && len >= 4 {
                let t = std::slice::from_raw_parts(ptr as *const u16, 2);
                format!("{:04x}{:04x}", t[0], t[1])
            } else {
                "040904b0".into()
            };
            let get = |field: &str| -> Option<String> {
                let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
                let mut len = 0u32;
                if !VerQueryValueW(buf.as_ptr() as *const _, &HSTRING::from(format!(r"\StringFileInfo\{lang}\{field}")), &mut ptr, &mut len).as_bool() || len == 0 {
                    return None;
                }
                let s = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                let t = String::from_utf16_lossy(&s[..s.iter().position(|c| *c == 0).unwrap_or(s.len())]).trim().to_string();
                (!t.is_empty()).then_some(t)
            };
            (get("FileDescription"), get("CompanyName"))
        }
    }

    /// Ziel und Argumente einer Verknüpfung (.lnk)
    fn shortcut_target(lnk: &std::path::Path) -> Option<(String, String)> {
        use windows::core::Interface;
        use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ};
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            let pf: IPersistFile = link.cast().ok()?;
            pf.Load(&HSTRING::from(lnk.as_os_str()), STGM_READ).ok()?;
            let mut path = [0u16; 1024];
            link.GetPath(&mut path, std::ptr::null_mut(), 0).ok()?;
            let mut args = [0u16; 2048];
            let _ = link.GetArguments(&mut args);
            let z = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]);
            Some((z(&path), z(&args)))
        }
    }

    fn folder(common: bool) -> Option<std::path::PathBuf> {
        let base = std::env::var_os(if common { "ProgramData" } else { "APPDATA" })?;
        Some(std::path::PathBuf::from(base).join(if common { r"Microsoft\Windows\Start Menu\Programs\StartUp" } else { r"Microsoft\Windows\Start Menu\Programs\Startup" }))
    }

    fn finish(mut e: StartupEntry) -> StartupEntry {
        if let Some(p) = &e.path {
            let exists = std::path::Path::new(p).exists();
            e.missing = !exists && p.contains('\\');
            if exists {
                let (d, c) = version_info(p);
                e.description = e.description.or(d);
                e.publisher = e.publisher.or(c);
            }
        }
        e
    }

    /// Alle Autostart-Einträge (eingeschaltete zuerst, nach Name).
    pub fn list() -> Vec<StartupEntry> {
        let mut out = Vec::new();
        for (root, sub, part, source) in [
            (HKEY_CURRENT_USER, RUN, "Run", Source::RunUser),
            (HKEY_LOCAL_MACHINE, RUN, "Run", Source::RunMachine),
            (HKEY_LOCAL_MACHINE, RUN32, "Run32", Source::RunMachine32),
        ] {
            let Some(k) = open(root, sub, false) else { continue };
            let ap = approved(root, part);
            for (name, typ, data) in values(&k) {
                // REG_SZ (1) bzw. REG_EXPAND_SZ (2)
                if (typ != 1 && typ != 2) || name.is_empty() {
                    continue;
                }
                let command = reg_string(&data);
                let a = ap.get(&name.to_lowercase());
                out.push(finish(StartupEntry {
                    path: command_path(&command),
                    command,
                    enabled: approved_enabled(a.map(|v| v.as_slice())),
                    disabled_since: disabled_date(a),
                    key: name.clone(),
                    name,
                    description: None,
                    publisher: None,
                    source,
                    missing: false,
                }));
            }
        }
        for (common, root, source) in [(false, HKEY_CURRENT_USER, Source::FolderUser), (true, HKEY_LOCAL_MACHINE, Source::FolderCommon)] {
            let Some(dir) = folder(common) else { continue };
            let ap = approved(root, "StartupFolder");
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for f in rd.flatten() {
                let p = f.path();
                let file = f.file_name().to_string_lossy().to_string();
                if file.eq_ignore_ascii_case("desktop.ini") || p.is_dir() {
                    continue;
                }
                let (target, args) = if file.to_lowercase().ends_with(".lnk") { shortcut_target(&p).unwrap_or_default() } else { (p.to_string_lossy().to_string(), String::new()) };
                let a = ap.get(&file.to_lowercase());
                out.push(finish(StartupEntry {
                    name: p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(file.clone()),
                    description: None,
                    publisher: None,
                    command: if args.is_empty() { target.clone() } else { format!("\"{target}\" {args}") },
                    path: (!target.is_empty()).then_some(target),
                    source,
                    enabled: approved_enabled(a.map(|v| v.as_slice())),
                    disabled_since: disabled_date(a),
                    missing: false,
                    key: file,
                }));
            }
        }
        // Store-Apps: SystemAppData\<Paket>\<Task> mit DWORD „State“ (0/1/3 aus, 2/4 an)
        if let Some(k) = open(HKEY_CURRENT_USER, STORE, false) {
            for pkg in subkeys(&k) {
                let Some(pk) = open(HKEY_CURRENT_USER, &format!(r"{STORE}\{pkg}"), false) else { continue };
                let tasks: Vec<(String, u32)> = subkeys(&pk)
                    .into_iter()
                    .filter_map(|t| open(HKEY_CURRENT_USER, &format!(r"{STORE}\{pkg}\{t}"), false).and_then(|tk| reg_dword(&tk, "State")).map(|s| (t, s)))
                    .collect();
                let many = tasks.len() > 1;
                let (name, publisher) = store_names(&pkg);
                for (task, state) in tasks {
                    out.push(StartupEntry {
                        name: if many { format!("{name} ({task})") } else { name.clone() },
                        description: None,
                        publisher: publisher.clone(),
                        command: format!("Store-App {pkg}"),
                        path: None,
                        source: Source::StoreApp,
                        enabled: state == 2 || state == 4,
                        disabled_since: None,
                        missing: false,
                        key: format!(r"{pkg}\{task}"),
                    });
                }
            }
        }
        sort_entries(&mut out);
        out
    }

    fn now_filetime() -> u64 {
        let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        (d.as_nanos() / 100) as u64 + 116_444_736_000_000_000
    }

    fn approved_place(e: &StartupEntry) -> Option<(HKEY, &'static str)> {
        match e.source {
            Source::RunUser => Some((HKEY_CURRENT_USER, "Run")),
            Source::RunMachine => Some((HKEY_LOCAL_MACHINE, "Run")),
            Source::RunMachine32 => Some((HKEY_LOCAL_MACHINE, "Run32")),
            Source::FolderUser => Some((HKEY_CURRENT_USER, "StartupFolder")),
            Source::FolderCommon => Some((HKEY_LOCAL_MACHINE, "StartupFolder")),
            _ => None,
        }
    }

    /// Ein- oder ausschalten – wie der Task-Manager (Eintrag bleibt erhalten).
    pub fn set_enabled(e: &StartupEntry, on: bool) -> Result<(), String> {
        if e.source == Source::StoreApp {
            // 2 = an, 1 = vom Benutzer ausgeschaltet
            let k = open(HKEY_CURRENT_USER, &format!(r"{STORE}\{}", e.key), true).ok_or("Store-Eintrag nicht gefunden")?;
            let v: u32 = if on { 2 } else { 1 };
            let r = unsafe { RegSetValueExW(k.0, &HSTRING::from("State"), 0, REG_DWORD, Some(&v.to_le_bytes())) };
            return if r == ERROR_SUCCESS { Ok(()) } else { Err(denied(r.0)) };
        }
        let (root, part) = approved_place(e).ok_or("Unbekannte Quelle")?;
        let k = create(root, &format!(r"{APPROVED}\{part}"))?;
        let v = approved_value(on, now_filetime());
        let r = unsafe { RegSetValueExW(k.0, &HSTRING::from(e.key.as_str()), 0, REG_BINARY, Some(&v)) };
        if r == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(denied(r.0))
        }
    }

    /// Neues Programm in den Autostart (nur für dich, Registrierung „Run“).
    pub fn add(name: &str, exe: &str, args: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Bitte einen Namen eingeben".into());
        }
        if !std::path::Path::new(exe).is_file() {
            return Err("Die Programmdatei wurde nicht gefunden".into());
        }
        if let Some(k) = open(HKEY_CURRENT_USER, RUN, false) {
            if values(&k).iter().any(|(n, _, _)| n.eq_ignore_ascii_case(name)) {
                return Err(format!("Es gibt schon einen Autostart-Eintrag „{name}“"));
            }
        }
        let cmd = if args.trim().is_empty() { format!("\"{exe}\"") } else { format!("\"{exe}\" {}", args.trim()) };
        let k = create(HKEY_CURRENT_USER, RUN)?;
        let mut data: Vec<u8> = cmd.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect();
        data.shrink_to_fit();
        let r = unsafe { RegSetValueExW(k.0, &HSTRING::from(name), 0, REG_SZ, Some(&data)) };
        if r != ERROR_SUCCESS {
            return Err(denied(r.0));
        }
        // Als eingeschaltet markieren (falls früher ein gleichnamiger Eintrag ausgeschaltet war)
        let e = StartupEntry {
            name: name.into(),
            description: None,
            publisher: None,
            command: cmd,
            path: None,
            source: Source::RunUser,
            enabled: true,
            disabled_since: None,
            missing: false,
            key: name.into(),
        };
        set_enabled(&e, true)
    }

    /// Eintrag ganz entfernen (Registrierungswert bzw. Verknüpfung). Store-Apps lassen sich nur ausschalten.
    pub fn remove(e: &StartupEntry) -> Result<(), String> {
        match e.source {
            Source::RunUser | Source::RunMachine | Source::RunMachine32 => {
                let (root, sub) = match e.source {
                    Source::RunUser => (HKEY_CURRENT_USER, RUN),
                    Source::RunMachine => (HKEY_LOCAL_MACHINE, RUN),
                    _ => (HKEY_LOCAL_MACHINE, RUN32),
                };
                let k = open(root, sub, true).ok_or_else(|| denied(5))?;
                let r = unsafe { RegDeleteValueW(k.0, &HSTRING::from(e.key.as_str())) };
                if r != ERROR_SUCCESS {
                    return Err(denied(r.0));
                }
            }
            Source::FolderUser | Source::FolderCommon => {
                let dir = folder(e.source == Source::FolderCommon).ok_or("Autostart-Ordner nicht gefunden")?;
                std::fs::remove_file(dir.join(&e.key)).map_err(|err| if err.kind() == std::io::ErrorKind::PermissionDenied { denied(5) } else { err.to_string() })?;
            }
            _ => return Err("Store-Apps lassen sich nur ausschalten".into()),
        }
        // Markierung aufräumen (Fehler egal)
        if let Some((root, part)) = approved_place(e) {
            if let Some(k) = open(root, &format!(r"{APPROVED}\{part}"), true) {
                unsafe {
                    let _ = RegDeleteValueW(k.0, &HSTRING::from(e.key.as_str()));
                }
            }
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ Linux

#[cfg(target_os = "linux")]
pub use lx::*;

#[cfg(target_os = "linux")]
mod lx {
    use super::*;
    use std::path::PathBuf;

    fn user_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))).map(|c| c.join("autostart"))
    }

    fn field(text: &str, key: &str) -> Option<String> {
        text.lines().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('='))).map(|v| v.trim().to_string())
    }

    fn entry(path: &std::path::Path, source: Source) -> Option<StartupEntry> {
        let t = std::fs::read_to_string(path).ok()?;
        let off = |k: &str, v: &str| field(&t, k).map(|x| x.eq_ignore_ascii_case(v)).unwrap_or(false);
        let enabled = !off("Hidden", "true") && !off("X-GNOME-Autostart-enabled", "false");
        let command = field(&t, "Exec").unwrap_or_default();
        let file = path.file_name()?.to_string_lossy().to_string();
        Some(StartupEntry {
            name: field(&t, "Name").unwrap_or_else(|| file.trim_end_matches(".desktop").to_string()),
            description: field(&t, "Comment"),
            publisher: None,
            path: command.split_whitespace().next().map(String::from),
            command,
            source,
            enabled,
            disabled_since: None,
            missing: false,
            key: file,
        })
    }

    pub fn list() -> Vec<StartupEntry> {
        let mut out: Vec<StartupEntry> = Vec::new();
        let user: Vec<PathBuf> = user_dir().map(|d| super::super::sysfs::dirs(d)).unwrap_or_default();
        for p in user.iter().filter(|p| p.extension().map(|e| e == "desktop").unwrap_or(false)) {
            out.extend(entry(p, Source::DesktopUser));
        }
        // Systemweite Einträge, sofern nicht durch eine gleichnamige eigene Datei ersetzt
        for p in super::super::sysfs::dirs("/etc/xdg/autostart").iter().filter(|p| p.extension().map(|e| e == "desktop").unwrap_or(false)) {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if !out.iter().any(|e| e.key == name) {
                out.extend(entry(p, Source::DesktopSystem));
            }
        }
        sort_entries(&mut out);
        out
    }

    /// Ein/aus: eigene Kopie in ~/.config/autostart mit Hidden=true bzw. ohne
    pub fn set_enabled(e: &StartupEntry, on: bool) -> Result<(), String> {
        let dir = user_dir().ok_or("Kein Benutzerordner")?;
        std::fs::create_dir_all(&dir).map_err(|x| x.to_string())?;
        let src = if e.source == Source::DesktopSystem { PathBuf::from("/etc/xdg/autostart").join(&e.key) } else { dir.join(&e.key) };
        let text = std::fs::read_to_string(&src).map_err(|x| x.to_string())?;
        let mut lines: Vec<String> = text.lines().filter(|l| !l.starts_with("Hidden=") && !l.starts_with("X-GNOME-Autostart-enabled=")).map(String::from).collect();
        if !on {
            lines.push("Hidden=true".into());
            lines.push("X-GNOME-Autostart-enabled=false".into());
        }
        std::fs::write(dir.join(&e.key), lines.join("\n") + "\n").map_err(|x| x.to_string())
    }

    pub fn add(name: &str, exe: &str, args: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Bitte einen Namen eingeben".into());
        }
        let dir = user_dir().ok_or("Kein Benutzerordner")?;
        std::fs::create_dir_all(&dir).map_err(|x| x.to_string())?;
        let file: String = name.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect::<String>() + ".desktop";
        let p = dir.join(&file);
        if p.exists() {
            return Err(format!("Es gibt schon einen Autostart-Eintrag „{name}“"));
        }
        let exec = if args.trim().is_empty() { format!("\"{exe}\"") } else { format!("\"{exe}\" {}", args.trim()) };
        std::fs::write(p, format!("[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\n")).map_err(|x| x.to_string())
    }

    pub fn remove(e: &StartupEntry) -> Result<(), String> {
        if e.source == Source::DesktopSystem {
            return Err("Systemweite Einträge lassen sich nur ausschalten".into());
        }
        let dir = user_dir().ok_or("Kein Benutzerordner")?;
        std::fs::remove_file(dir.join(&e.key)).map_err(|x| x.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_values() {
        assert!(approved_enabled(None));
        assert!(approved_enabled(Some(&[2, 0, 0, 0])));
        assert!(approved_enabled(Some(&[6, 0])));
        assert!(!approved_enabled(Some(&[3, 0, 0, 0])));
        assert!(!approved_enabled(Some(&[7])));
        let off = approved_value(false, 0x0102030405060708);
        assert_eq!(off[0], 3);
        assert_eq!(&off[4..12], &0x0102030405060708u64.to_le_bytes());
        assert_eq!(approved_value(true, 99), [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn command_paths() {
        assert_eq!(command_path(r#""C:\Program Files\Steam\steam.exe" -silent"#).as_deref(), Some(r"C:\Program Files\Steam\steam.exe"));
        assert_eq!(command_path(r"C:\Windows\system32\SecurityHealthSystray.exe").as_deref(), Some(r"C:\Windows\system32\SecurityHealthSystray.exe"));
        assert_eq!(command_path(r"C:\Tools\a b\x.EXE /min").as_deref(), Some(r"C:\Tools\a b\x.EXE"));
        std::env::set_var("TP_TEST_DIR", r"C:\Test");
        assert_eq!(expand_env(r"%TP_TEST_DIR%\a.exe"), r"C:\Test\a.exe");
        assert_eq!(expand_env("100% sicher"), "100% sicher");
    }

    #[test]
    fn store_and_sorting() {
        assert_eq!(store_names("5319275A.WhatsAppDesktop_cv1g1gvanyjgm"), ("WhatsAppDesktop".into(), None));
        assert_eq!(store_names("AppleInc.iCloud_nzyj5cx40ttqa"), ("iCloud".into(), Some("AppleInc".into())));
        assert_eq!(store_names("Claude_pzs8sxrjxfjjc"), ("Claude".into(), None));
        let e = |n: &str, on: bool| StartupEntry {
            name: n.into(),
            description: None,
            publisher: None,
            command: String::new(),
            path: None,
            source: Source::RunUser,
            enabled: on,
            disabled_since: None,
            missing: false,
            key: n.into(),
        };
        let mut v = vec![e("steam", false), e("Discord", true), e("ARC", true), e("EADM", false)];
        sort_entries(&mut v);
        let names: Vec<&str> = v.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["ARC", "Discord", "EADM", "steam"]);
    }
}
