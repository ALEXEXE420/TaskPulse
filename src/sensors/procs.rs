//! Prozessliste über NtQuerySystemInformation(SystemProcessInformation) – eine Abfrage liefert
//! CPU-Zeiten, privaten Arbeitssatz, E/A-Zähler, Threads und Handles aller Prozesse.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::time::Instant;
use windows::core::{s, w, HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowLongW, GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible, GWL_EXSTYLE,
    GW_OWNER, WS_EX_TOOLWINDOW,
};

#[repr(C)]
struct UnicodeString {
    length: u16,
    max_length: u16,
    buffer: *const u16,
}

/// Vollständiges Layout von SYSTEM_PROCESS_INFORMATION (x64).
#[repr(C)]
struct SystemProcessInformation {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UnicodeString,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_page_count: usize,
    read_operation_count: i64,
    write_operation_count: i64,
    other_operation_count: i64,
    read_transfer_count: i64,
    write_transfer_count: i64,
    other_transfer_count: i64,
}

type NtQsi = unsafe extern "system" fn(u32, *mut c_void, u32, *mut u32) -> i32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
pub enum Category {
    App,
    Background,
    Windows,
}

impl Category {
    pub fn title(self) -> &'static str {
        match self {
            Category::App => "Apps",
            Category::Background => "Hintergrundprozesse",
            Category::Windows => "Windows-Prozesse",
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub image: String,
    pub display: String,
    pub path: Option<String>,
    pub session: u32,
    pub threads: u32,
    pub handles: u32,
    pub base_priority: i32,
    pub cpu_pct: f64,
    /// Privater Arbeitssatz (wie Spalte "Arbeitsspeicher" im Task-Manager).
    pub mem_private: f64,
    pub working_set: f64,
    /// Lese- und Schreibrate aller E/A (Näherung für Datenträger).
    pub io_bps: f64,
    pub cpu_time_100ns: u64,
    pub create_time: i64,
    pub has_window: bool,
    pub category: Category,
}

struct Prev {
    cpu: u64,
    io: u64,
    at: Instant,
}

pub struct Procs {
    ntqsi: Option<NtQsi>,
    buf: Vec<u64>,
    prev: HashMap<(u32, i64), Prev>,
    paths: HashMap<(u32, i64), Option<String>>,
    descriptions: HashMap<String, String>,
    windows_dir: String,
    ncpu: f64,
}

fn read_unicode(u: &UnicodeString) -> String {
    if u.buffer.is_null() || u.length == 0 {
        return String::new();
    }
    let s = unsafe { std::slice::from_raw_parts(u.buffer, (u.length / 2) as usize) };
    String::from_utf16_lossy(s)
}

pub fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// "Dateibeschreibung" aus der Versionsressource (z. B. "Google Chrome").
pub fn file_description(path: &str) -> Option<String> {
    unsafe {
        let hp = HSTRING::from(path);
        let size = GetFileVersionInfoSizeW(&hp, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(&hp, 0, size, data.as_mut_ptr() as *mut c_void).ok()?;
        let mut p: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let mut langs: Vec<(u16, u16)> = Vec::new();
        if VerQueryValueW(data.as_ptr() as *const c_void, w!("\\VarFileInfo\\Translation"), &mut p, &mut len).as_bool() && !p.is_null() {
            let n = len as usize / 4;
            let arr = std::slice::from_raw_parts(p as *const u16, n * 2);
            for i in 0..n {
                langs.push((arr[i * 2], arr[i * 2 + 1]));
            }
        }
        langs.push((0x0409, 0x04B0));
        langs.push((0x0407, 0x04B0));
        langs.push((0x0409, 0x04E4));
        for (lang, cp) in langs {
            let key = HSTRING::from(format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\FileDescription"));
            let mut p: *mut c_void = std::ptr::null_mut();
            let mut len = 0u32;
            if VerQueryValueW(data.as_ptr() as *const c_void, &key, &mut p, &mut len).as_bool() && !p.is_null() && len > 1 {
                let s = std::slice::from_raw_parts(p as *const u16, len as usize);
                let s = String::from_utf16_lossy(&s[..s.iter().position(|c| *c == 0).unwrap_or(s.len())]);
                let s = s.trim().to_string();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
        None
    }
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lp: LPARAM) -> BOOL {
    let set = &mut *(lp.0 as *mut HashSet<u32>);
    if !IsWindowVisible(hwnd).as_bool() {
        return true.into();
    }
    if GetWindow(hwnd, GW_OWNER).map(|h| !h.is_invalid()).unwrap_or(false) {
        return true.into();
    }
    if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
        return true.into();
    }
    if GetWindowTextLengthW(hwnd) == 0 {
        return true.into();
    }
    let mut cloaked: u32 = 0;
    if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4).is_ok() && cloaked != 0 {
        return true.into();
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    set.insert(pid);
    true.into()
}

/// PIDs mit sichtbarem Hauptfenster (wie die "Apps" im Task-Manager).
pub fn pids_with_windows() -> HashSet<u32> {
    let mut set: HashSet<u32> = HashSet::new();
    unsafe {
        let _ = EnumWindows(Some(enum_cb), LPARAM(&mut set as *mut _ as isize));
    }
    set
}

const WINDOWS_NAMES: &[&str] = &[
    "system",
    "registry",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "services.exe",
    "lsass.exe",
    "lsaiso.exe",
    "winlogon.exe",
    "svchost.exe",
    "fontdrvhost.exe",
    "dwm.exe",
    "memory compression",
    "secure system",
    "spoolsv.exe",
    "system idle process",
];

pub fn classify(pid: u32, image: &str, path: Option<&str>, session: u32, has_window: bool, windows_dir: &str) -> Category {
    let lower = image.to_ascii_lowercase();
    if pid == 0 || pid == 4 || WINDOWS_NAMES.contains(&lower.as_str()) {
        return Category::Windows;
    }
    if has_window {
        return Category::App;
    }
    match path {
        Some(p) if !windows_dir.is_empty() && p.to_ascii_lowercase().starts_with(windows_dir) => Category::Windows,
        None if session == 0 => Category::Windows,
        _ => Category::Background,
    }
}

impl Procs {
    pub fn new() -> Procs {
        let ntqsi = unsafe {
            GetModuleHandleW(w!("ntdll.dll"))
                .ok()
                .and_then(|m| GetProcAddress(m, s!("NtQuerySystemInformation")))
                .map(|f| std::mem::transmute::<_, NtQsi>(f))
        };
        let windows_dir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()).to_ascii_lowercase() + "\\";
        Procs {
            ntqsi,
            buf: vec![0u64; 256 * 1024],
            prev: HashMap::new(),
            paths: HashMap::new(),
            descriptions: HashMap::new(),
            windows_dir,
            ncpu: std::thread::available_parallelism().map(|n| n.get() as f64).unwrap_or(1.0),
        }
    }

    fn query(&mut self) -> bool {
        let Some(f) = self.ntqsi else { return false };
        for _ in 0..8 {
            let mut needed = 0u32;
            let bytes = (self.buf.len() * 8) as u32;
            let st = unsafe { f(5, self.buf.as_mut_ptr() as *mut c_void, bytes, &mut needed) };
            if st >= 0 {
                return true;
            }
            // STATUS_INFO_LENGTH_MISMATCH
            if st as u32 == 0xC000_0004 {
                let want = (needed as usize).max(bytes as usize * 2) / 8 + 4096;
                self.buf = vec![0u64; want];
                continue;
            }
            return false;
        }
        false
    }

    pub fn sample(&mut self) -> Vec<Proc> {
        if !self.query() {
            return Vec::new();
        }
        let now = Instant::now();
        let windows = pids_with_windows();
        let mut out = Vec::with_capacity(512);
        let mut seen: HashSet<(u32, i64)> = HashSet::new();
        let base = self.buf.as_ptr() as *const u8;
        let mut off = 0usize;
        loop {
            let e = unsafe { &*(base.add(off) as *const SystemProcessInformation) };
            let pid = e.unique_process_id as u32;
            let key = (pid, e.create_time);
            seen.insert(key);
            let image = if pid == 0 { "System Idle Process".to_string() } else { read_unicode(&e.image_name) };
            let cpu = (e.user_time + e.kernel_time).max(0) as u64;
            let io = (e.read_transfer_count + e.write_transfer_count).max(0) as u64;
            let (cpu_pct, io_bps) = match self.prev.get(&key) {
                Some(p) => {
                    let dt = now.duration_since(p.at).as_secs_f64();
                    if dt > 0.01 {
                        let cpu_s = cpu.saturating_sub(p.cpu) as f64 / 1e7;
                        ((cpu_s / dt / self.ncpu * 100.0).clamp(0.0, 100.0), io.saturating_sub(p.io) as f64 / dt)
                    } else {
                        (0.0, 0.0)
                    }
                }
                None => (0.0, 0.0),
            };
            self.prev.insert(key, Prev { cpu, io, at: now });
            if pid != 0 {
                let path = self.paths.entry(key).or_insert_with(|| if pid == 4 { None } else { process_path(pid) }).clone();
                let display = match &path {
                    Some(p) => self.descriptions.entry(p.clone()).or_insert_with(|| file_description(p).unwrap_or_default()).clone(),
                    None => String::new(),
                };
                let display = if display.is_empty() { image.clone() } else { display };
                let has_window = windows.contains(&pid);
                let category = classify(pid, &image, path.as_deref(), e.session_id, has_window, &self.windows_dir);
                out.push(Proc {
                    pid,
                    ppid: e.inherited_from_unique_process_id as u32,
                    image,
                    display,
                    path,
                    session: e.session_id,
                    threads: e.number_of_threads,
                    handles: e.handle_count,
                    base_priority: e.base_priority,
                    cpu_pct,
                    mem_private: e.working_set_private_size.max(0) as f64,
                    working_set: e.working_set_size as f64,
                    io_bps,
                    cpu_time_100ns: cpu,
                    create_time: e.create_time,
                    has_window,
                    category,
                });
            }
            if e.next_entry_offset == 0 {
                break;
            }
            off += e.next_entry_offset as usize;
            if off >= self.buf.len() * 8 {
                break;
            }
        }
        self.prev.retain(|k, _| seen.contains(k));
        self.paths.retain(|k, _| seen.contains(k));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_layout() {
        // Offsets laut Windows-SDK/PHNT (x64)
        assert_eq!(std::mem::offset_of!(SystemProcessInformation, image_name), 56);
        assert_eq!(std::mem::offset_of!(SystemProcessInformation, unique_process_id), 80);
        assert_eq!(std::mem::offset_of!(SystemProcessInformation, handle_count), 96);
        assert_eq!(std::mem::offset_of!(SystemProcessInformation, private_page_count), 200);
        assert_eq!(std::mem::offset_of!(SystemProcessInformation, read_transfer_count), 232);
        assert_eq!(std::mem::size_of::<SystemProcessInformation>(), 256);
    }

    #[test]
    fn classification() {
        let w = r"c:\windows\";
        assert_eq!(classify(4, "System", None, 0, false, w), Category::Windows);
        assert_eq!(classify(100, "svchost.exe", Some(r"C:\Windows\System32\svchost.exe"), 0, false, w), Category::Windows);
        assert_eq!(classify(200, "javaw.exe", Some(r"C:\Java\bin\javaw.exe"), 1, true, w), Category::App);
        assert_eq!(classify(300, "helper.exe", Some(r"C:\Programs\helper.exe"), 1, false, w), Category::Background);
        assert_eq!(classify(400, "RuntimeBroker.exe", Some(r"C:\Windows\System32\RuntimeBroker.exe"), 1, false, w), Category::Windows);
    }

    #[test]
    fn live_query_finds_self() {
        let mut p = Procs::new();
        let list = p.sample();
        assert!(list.len() > 10);
        assert!(list.iter().any(|x| x.pid == std::process::id()));
    }
}
