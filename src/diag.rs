//! Fehlerberichte und Diagnose – alles bleibt auf dem Rechner (kein automatisches Senden).
//!
//! Ordner: %APPDATA%\TaskPulse\Fehlerberichte
//! - `TaskPulse.log`: laufendes Protokoll kleinerer Probleme (Sensor nicht lesbar, Update fehlgeschlagen …)
//! - `Fehler-<Zeit>.txt`: sofort bei einem Programmfehler (Panik) oder harten Absturz (Ausnahme)
//! - `Absturz-<Zeit>.txt`: nachträglich, wenn Windows einen Absturz protokolliert hat, bei dem
//!   TaskPulse selbst nichts mehr schreiben konnte (z. B. Fail-Fast in ntdll)
//! - `Diagnose-<Zeit>.txt`: auf Knopfdruck alles in einer Datei (zum Weitergeben)

use crate::config::Config;
use std::io::Write;
use std::path::PathBuf;

pub fn dir() -> PathBuf {
    Config::dir().join("Fehlerberichte")
}

fn local_time() -> windows::Win32::Foundation::SYSTEMTIME {
    unsafe { windows::Win32::System::SystemInformation::GetLocalTime() }
}

/// "2026-11-06_14-03-22"
pub fn stamp() -> String {
    let t = local_time();
    format!("{:04}-{:02}-{:02}_{:02}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// "06.11.2026 14:03:22"
pub fn now_text() -> String {
    let t = local_time();
    format!("{:02}.{:02}.{:04} {:02}:{:02}:{:02}", t.wDay, t.wMonth, t.wYear, t.wHour, t.wMinute, t.wSecond)
}

/// Welcher Teil von TaskPulse läuft (Hauptfenster, Taskleiste, Installer …)
fn role() -> &'static str {
    let a: Vec<String> = std::env::args().collect();
    if a.iter().any(|x| x == "--taskbar") {
        "Taskleisten-Anzeige"
    } else if a.iter().any(|x| x == "--update") {
        "Update"
    } else if a.iter().any(|x| x == "--install" || x == "--uninstall") || crate::installer::is_setup_exe() {
        "Installer"
    } else {
        "Hauptfenster"
    }
}

const LOG_MAX: u64 = 1 << 20;

/// Eine Zeile ins Protokoll schreiben (für kleinere Probleme). Schlägt nie fehl.
pub fn log(msg: impl AsRef<str>) {
    let d = dir();
    let _ = std::fs::create_dir_all(&d);
    let p = d.join("TaskPulse.log");
    // Bei 1 MB: alte Datei behalten, neue beginnen
    if std::fs::metadata(&p).map(|m| m.len() > LOG_MAX).unwrap_or(false) {
        let _ = std::fs::rename(&p, d.join("TaskPulse.alt.log"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{} [{} {} PID {}] {}", now_text(), role(), crate::updater::CURRENT, std::process::id(), msg.as_ref());
    }
}

/// Windows-Version, z. B. "Windows 10.0.26100"
pub fn windows_version() -> String {
    #[repr(C)]
    struct OsVersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd: [u16; 128],
    }
    type RtlGetVersion = unsafe extern "system" fn(*mut OsVersionInfo) -> i32;
    unsafe {
        use windows::core::{s, w};
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        let mut v = OsVersionInfo { size: std::mem::size_of::<OsVersionInfo>() as u32, major: 0, minor: 0, build: 0, platform: 0, csd: [0; 128] };
        if let Ok(nt) = GetModuleHandleW(w!("ntdll.dll")) {
            if let Some(f) = GetProcAddress(nt, s!("RtlGetVersion")) {
                let f: RtlGetVersion = std::mem::transmute(f);
                f(&mut v);
            }
        }
        // Windows 11 meldet sich intern weiter als 10.0 – erkennbar an Build 22000 und höher
        let name = if v.major == 10 && v.build >= 22000 { "11" } else if v.major == 10 { "10" } else { "" };
        format!("Windows {name} (Version {}.{}, Build {}){}", v.major, v.minor, v.build, if crate::winutil::native_arm64() { " ARM64" } else { "" })
    }
}

fn header(kind: &str) -> String {
    let ci = crate::sensors::cputemp::cpu_info();
    format!(
        "TaskPulse – {kind}\n\
         ==================\n\
         Zeit:        {}\n\
         Version:     {}\n\
         Teil:        {} (PID {})\n\
         Aufruf:      {}\n\
         System:      {}\n\
         Adminrechte: {}\n\
         CPU:         {} ({} Familie {:X}h Modell {:X}h)\n\n",
        now_text(),
        crate::updater::CURRENT,
        role(),
        std::process::id(),
        std::env::args().collect::<Vec<_>>().join(" "),
        windows_version(),
        if crate::winutil::is_elevated() { "ja" } else { "nein" },
        ci.brand,
        ci.vendor,
        ci.family,
        ci.model
    )
}

fn log_tail(lines: usize) -> String {
    let mut all = String::new();
    for name in ["TaskPulse.alt.log", "TaskPulse.log"] {
        if let Ok(s) = std::fs::read_to_string(dir().join(name)) {
            all.push_str(&s);
        }
    }
    let v: Vec<&str> = all.lines().collect();
    v[v.len().saturating_sub(lines)..].join("\n")
}

/// Nach einem sofort geschriebenen Bericht: Windows protokolliert denselben Absturz gleich danach –
/// den beim nächsten Start nicht noch einmal melden (Marke = jetzt + 60 s, UTC wie im Protokoll).
fn mark_reported() {
    let t = unsafe { windows::Win32::System::SystemInformation::GetSystemTime() };
    let secs = t.wHour as u32 * 3600 + t.wMinute as u32 * 60 + t.wSecond as u32 + 60;
    let (h, m, sec) = ((secs / 3600).min(23), (secs / 60) % 60, secs % 60);
    let mark = format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.9999999Z", t.wYear, t.wMonth, t.wDay, h, m, sec);
    let _ = std::fs::create_dir_all(dir());
    let p = dir().join("zuletzt_geprueft.txt");
    let old = std::fs::read_to_string(&p).unwrap_or_default();
    if mark.as_str() > old.trim() {
        let _ = std::fs::write(p, mark);
    }
}

fn write_report(prefix: &str, body: &str) -> Option<PathBuf> {
    let d = dir();
    std::fs::create_dir_all(&d).ok()?;
    let p = d.join(format!("{prefix}-{}.txt", stamp()));
    std::fs::write(&p, body).ok()?;
    Some(p)
}

// ------------------------------------------------------------------ Programmfehler und Abstürze

/// Fängt Programmfehler (Panik) und harte Abstürze (unbehandelte Ausnahme) ab und schreibt
/// sofort einen Fehlerbericht. Einmal am Programmstart aufrufen.
pub fn install_handlers() {
    std::panic::set_hook(Box::new(|info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(keine Meldung)".into());
        let loc = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("unbenannt").to_string();
        let body = format!(
            "{}Art:    Programmfehler (Panik)\nThread: {thread}\nStelle: {loc}\nMeldung: {msg}\n\nAufrufkette:\n{}\n\nLetzte Protokollzeilen:\n{}\n",
            header("Fehlerbericht"),
            std::backtrace::Backtrace::force_capture(),
            log_tail(40)
        );
        log(format!("PANIK in {thread} bei {loc}: {msg}"));
        let _ = write_report("Fehler", &body);
        mark_reported();
    }));
    unsafe {
        windows::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter(Some(on_exception));
    }
}

/// Modul (DLL/EXE), in dem eine Adresse liegt, und der Abstand dazu.
fn module_of(addr: usize) -> String {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT};
    unsafe {
        let mut m = HMODULE::default();
        if GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            windows::core::PCWSTR(addr as *const u16),
            &mut m,
        )
        .is_err()
        {
            return format!("unbekannt (0x{addr:X})");
        }
        let mut buf = [0u16; 260];
        let n = GetModuleFileNameW(m, &mut buf) as usize;
        let name = String::from_utf16_lossy(&buf[..n]);
        format!("{name} + 0x{:X}", addr - m.0 as usize)
    }
}

pub fn exception_name(code: u32) -> &'static str {
    match code {
        0xC0000005 => "Speicherzugriffsfehler (ACCESS_VIOLATION)",
        0xC000001D => "Unzulässiger Befehl (ILLEGAL_INSTRUCTION)",
        0xC0000094 => "Division durch null",
        0xC00000FD => "Stapelüberlauf (STACK_OVERFLOW)",
        0xC000000D => "Ungültiger Parameter (INVALID_PARAMETER)",
        0xC0000409 => "Sicherheitsabbruch / Fail-Fast (STACK_BUFFER_OVERRUN)",
        0xC0000374 => "Heap beschädigt (HEAP_CORRUPTION)",
        0x80000003 => "Haltepunkt (BREAKPOINT)",
        _ => "unbekannte Ausnahme",
    }
}

unsafe extern "system" fn on_exception(info: *const windows::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS) -> i32 {
    if let Some(rec) = info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) {
        let code = rec.ExceptionCode.0 as u32;
        let addr = rec.ExceptionAddress as usize;
        let body = format!(
            "{}Art:    Absturz (unbehandelte Ausnahme)\nCode:   0x{code:08X} – {}\nStelle: {}\nThread: {}\n\nLetzte Protokollzeilen:\n{}\n",
            header("Fehlerbericht"),
            exception_name(code),
            module_of(addr),
            std::thread::current().name().unwrap_or("unbenannt"),
            log_tail(40)
        );
        log(format!("ABSTURZ 0x{code:08X} bei {}", module_of(addr)));
        let _ = write_report("Fehler", &body);
        mark_reported();
    }
    0 // EXCEPTION_CONTINUE_SEARCH: Windows beendet das Programm wie gewohnt
}

// ------------------------------------------------------------------ Abstürze aus dem Windows-Protokoll

/// Ein von Windows protokollierter Absturz von TaskPulse (Ereignis 1000 „Application Error“).
#[derive(Debug, Clone, PartialEq)]
pub struct CrashEvent {
    /// UTC, z. B. "2026-11-06T00:15:31.123Z"
    pub time: String,
    pub module: String,
    pub code: String,
    pub offset: String,
    pub path: String,
}

/// Zerlegt die XML-Ausgabe von wevtutil (Ereignis 1000). Datenfelder laut Windows:
/// 0 App, 1 App-Version, 2 App-Zeit, 3 Modul, 4 Modulversion, 5 Modulzeit, 6 Code, 7 Offset, 8 PID, 9 Start, 10 Pfad, 11 Modulpfad
pub fn parse_crash_events(xml: &str) -> Vec<CrashEvent> {
    let mut out = Vec::new();
    for ev in xml.split("<Event ").skip(1) {
        let time = ev.split("SystemTime='").nth(1).or_else(|| ev.split("SystemTime=\"").nth(1)).and_then(|s| s.split(['\'', '"']).next()).unwrap_or("").to_string();
        let data: Vec<String> = ev
            .split("<Data")
            .skip(1)
            .map(|d| d.split_once('>').map(|(_, r)| r.split("</Data>").next().unwrap_or("").to_string()).unwrap_or_default())
            .collect();
        let get = |i: usize| data.get(i).cloned().unwrap_or_default();
        if !get(0).eq_ignore_ascii_case("TaskPulse.exe") {
            continue;
        }
        out.push(CrashEvent { time, module: get(3), code: get(6), offset: get(7), path: get(10) });
    }
    out
}

/// Die letzten Abstürze von TaskPulse laut Windows (höchstens `max`, neueste zuerst).
pub fn crash_events(max: usize) -> Vec<CrashEvent> {
    use std::os::windows::process::CommandExt;
    let q = "*[System[Provider[@Name='Application Error'] and (EventID=1000)]]";
    let out = std::process::Command::new("wevtutil")
        .args(["qe", "Application", &format!("/q:{q}"), "/c:60", "/rd:true", "/f:xml"])
        .creation_flags(crate::winutil::CREATE_NO_WINDOW)
        .output();
    let Ok(o) = out else { return Vec::new() };
    let mut v = parse_crash_events(&String::from_utf8_lossy(&o.stdout));
    v.truncate(max);
    v
}

/// Beim Start des Hauptfensters: Ist TaskPulse seit dem letzten Mal abgestürzt, einen
/// Absturzbericht schreiben. Gibt die neue Datei zurück.
pub fn check_new_crashes() -> Option<PathBuf> {
    let marker = dir().join("zuletzt_geprueft.txt");
    let last = std::fs::read_to_string(&marker).unwrap_or_default();
    let events = crash_events(20);
    let newest = events.first().map(|e| e.time.clone());
    if let Some(n) = newest.as_ref().filter(|n| n.as_str() > last.trim()) {
        let _ = std::fs::create_dir_all(dir());
        let _ = std::fs::write(&marker, n);
    }
    // Beim allerersten Mal nur merken, keine alten Abstürze melden
    if last.trim().is_empty() {
        return None;
    }
    let fresh: Vec<&CrashEvent> = events.iter().filter(|e| e.time.as_str() > last.trim()).collect();
    if fresh.is_empty() {
        return None;
    }
    let mut body = header("Absturzbericht (aus dem Windows-Fehlerprotokoll)");
    body += "TaskPulse wurde von Windows beendet, ohne selbst einen Bericht schreiben zu können.\n\n";
    for e in &fresh {
        let code = u32::from_str_radix(e.code.trim_start_matches("0x"), 16).unwrap_or(0);
        body += &format!("- {} UTC: {} in {} + {} ({})\n", e.time, e.code, e.module, e.offset, exception_name(code));
        body += &format!("  Programm: {}\n", e.path);
    }
    body += &format!("\nLetzte Protokollzeilen:\n{}\n", log_tail(60));
    log(format!("{} Absturz/Abstürze aus dem Windows-Protokoll gemeldet", fresh.len()));
    write_report("Absturz", &body)
}

// ------------------------------------------------------------------ Diagnosebericht

/// Alles in einer Datei: System, Sensoren, Einstellungen, Protokoll, Abstürze.
pub fn diagnose(extra: &str) -> Option<PathBuf> {
    let mut b = header("Diagnosebericht");
    b += "Diesen Bericht kannst du weitergeben – er enthält keine Passwörter. Er wird nicht automatisch gesendet.\n\n";
    b += "## Sensoren\n";
    b += &format!("PawnIO installiert: {}\n", if crate::installer::pawnio_installed() { "ja" } else { "nein" });
    b += &format!(
        "CPU-Temperatur/Watt: {}\n",
        match crate::sensors::cputemp::CpuTemp::open() {
            Ok(_) => "PawnIO aktiv".to_string(),
            Err(e) => e.describe(),
        }
    );
    b += &format!("NVML (NVIDIA): {}\n", if crate::sensors::nvgpu::NvGpu::new().is_some() { "verfügbar" } else { "nicht verfügbar" });
    let ci = crate::sensors::cputemp::cpu_info();
    for g in crate::sensors::topology::groups(&crate::sensors::topology::query(), ci.vendor.contains("AMD") && !crate::winutil::native_arm64(), ci.family) {
        b += &format!("CPU-Gruppe: {} – {} – LP {:?}\n", g.title, g.detail, g.lps);
    }
    for a in crate::sensors::gpu_proc::dxgi_adapters() {
        b += &format!(
            "GPU {}: {} · {} · {:.1} GB · Treiber {} · PCI {}\n",
            a.index,
            a.name,
            a.board_vendor().unwrap_or("?"),
            a.dedicated / 1073741824.0,
            a.driver_display().unwrap_or_default(),
            a.pci_id()
        );
    }
    b += &format!("Taskleisten-Anzeige läuft: {}\n", if crate::ui::taskbar_running() { "ja" } else { "nein" });
    b += &format!("Installiert in: {}\n", crate::installer::installed_dir().map(|d| d.display().to_string()).unwrap_or("nicht installiert".into()));
    if !extra.is_empty() {
        b += &format!("\n## Hauptfenster\n{extra}\n");
    }
    b += "\n## Einstellungen (settings.json)\n";
    b += &std::fs::read_to_string(Config::path()).unwrap_or_else(|_| "(keine)".into());
    b += "\n\n## Abstürze laut Windows (neueste zuerst)\n";
    let ev = crash_events(15);
    if ev.is_empty() {
        b += "keine\n";
    }
    for e in ev {
        let code = u32::from_str_radix(e.code.trim_start_matches("0x"), 16).unwrap_or(0);
        b += &format!("- {} UTC: {} in {} + {} ({}) – {}\n", e.time, e.code, e.module, e.offset, exception_name(code), e.path);
    }
    b += "\n## Fehlerberichte im Ordner\n";
    if let Ok(rd) = std::fs::read_dir(dir()) {
        let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.starts_with("Fehler-") || n.starts_with("Absturz-")).collect();
        names.sort();
        for n in names.iter().rev().take(10) {
            b += &format!("- {n}\n");
        }
        // Inhalt des neuesten Fehlerberichts direkt anhängen
        if let Some(n) = names.last() {
            b += &format!("\n### Neuester Bericht: {n}\n{}\n", std::fs::read_to_string(dir().join(n)).unwrap_or_default());
        }
    }
    b += &format!("\n## Protokoll (letzte 300 Zeilen)\n{}\n", log_tail(300));
    write_report("Diagnose", &b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_event_xml() {
        let xml = "<Events><Event xmlns='x'><System><Provider Name='Application Error'/><EventID>1000</EventID>\
            <TimeCreated SystemTime='2026-11-06T00:15:31.1234567Z'/></System><EventData>\
            <Data>TaskPulse.exe</Data><Data>0.0.0.0</Data><Data>6aed12b9</Data><Data>ntdll.dll</Data>\
            <Data>10.0.26100.9818</Data><Data>67156ed3</Data><Data>c000000d</Data><Data>0000000000075427</Data>\
            <Data>0x1234</Data><Data>0x1</Data><Data>C:\\Program Files\\TaskPulse\\TaskPulse.exe</Data><Data>C:\\WINDOWS\\SYSTEM32\\ntdll.dll</Data>\
            </EventData></Event><Event xmlns='x'><System><TimeCreated SystemTime='2026-11-05T10:00:00Z'/></System>\
            <EventData><Data>chrome.exe</Data></EventData></Event></Events>";
        let v = parse_crash_events(xml);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].time, "2026-11-06T00:15:31.1234567Z");
        assert_eq!(v[0].module, "ntdll.dll");
        assert_eq!(v[0].code, "c000000d");
        assert_eq!(v[0].offset, "0000000000075427");
        assert!(v[0].path.ends_with("TaskPulse.exe"));
        assert_eq!(exception_name(0xc000000d), "Ungültiger Parameter (INVALID_PARAMETER)");
    }

    #[test]
    fn this_pc_reads_windows_crash_log() {
        // darf leer sein, aber nicht abstürzen; Einträge gehören zu TaskPulse
        for e in crash_events(5) {
            assert!(e.path.to_lowercase().contains("taskpulse"));
        }
        let v = windows_version();
        assert!(v.starts_with("Windows 1"), "{v}");
        assert!(v.contains("Build "), "{v}");
    }
}
