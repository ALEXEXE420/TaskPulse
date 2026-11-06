//! Kleine Windows-Hilfsfunktionen.

use std::os::windows::process::CommandExt;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, SetPriorityClass, SetProcessInformation, TerminateProcess, WaitForSingleObject,
    IDLE_PRIORITY_CLASS, INFINITE, NORMAL_PRIORITY_CLASS, PROCESS_POWER_THROTTLING_STATE, PROCESS_SET_INFORMATION,
    PROCESS_TERMINATE, ProcessPowerThrottling,
};
use windows::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteExW, ShellExecuteW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsIconic, SetForegroundWindow, ShowWindow, SW_HIDE, SW_RESTORE,
    SW_SHOWNORMAL,
};

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Läuft Windows auf einem ARM-Prozessor (z. B. NVIDIA RTX Spark N1X, Snapdragon X)?
/// Ein x64-Programm läuft dort in der Emulation (Prism) und sieht per CPUID nur eine
/// nachgebildete x86-CPU – deshalb hier die echte Architektur des Systems.
pub fn native_arm64() -> bool {
    use windows::Win32::System::SystemInformation::IMAGE_FILE_MACHINE;
    use windows::Win32::System::Threading::{GetCurrentProcess, IsWow64Process2};
    static ARM: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ARM.get_or_init(|| {
        if cfg!(target_arch = "aarch64") {
            return true;
        }
        let mut process = IMAGE_FILE_MACHINE::default();
        let mut native = IMAGE_FILE_MACHINE::default();
        unsafe { IsWow64Process2(GetCurrentProcess(), &mut process, Some(&mut native)) }.is_ok() && native.0 == 0xAA64
    })
}

pub fn is_elevated() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}

pub fn exe_path() -> String {
    std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
}

/// Startet TaskPulse mit Adminrechten neu (UAC-Abfrage). true, wenn gestartet.
/// Ist die Aufgabe "TaskPulse Admin" eingerichtet, geht das ohne UAC-Abfrage.
pub fn relaunch_elevated(args: &str) -> bool {
    let args = format!("{args} --restart").trim().to_string();
    if crate::autostart::admin_is_enabled() {
        let _ = std::fs::write(crate::config::Config::dir().join("launch_args.txt"), args.split_whitespace().collect::<Vec<_>>().join("\n"));
        if crate::autostart::run_admin_task() {
            return true;
        }
    }
    unsafe {
        let r = ShellExecuteW(None, w!("runas"), &HSTRING::from(exe_path()), &HSTRING::from(args), PCWSTR::null(), SW_SHOWNORMAL);
        r.0 as isize > 32
    }
}

/// Startet TaskPulse ohne Rechteänderung mit Argumenten.
pub fn spawn_self(args: &[&str]) -> bool {
    std::process::Command::new(exe_path()).args(args).spawn().is_ok()
}

/// Führt ein Programm mit Adminrechten aus und wartet auf das Ende. Rückgabe: Exitcode.
pub fn run_elevated_wait(file: &str, params: &str) -> Option<u32> {
    unsafe {
        let file = HSTRING::from(file);
        let params = HSTRING::from(params);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        ShellExecuteExW(&mut info).ok()?;
        if info.hProcess.is_invalid() {
            return None;
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0u32;
        let ok = GetExitCodeProcess(info.hProcess, &mut code).is_ok();
        let _ = CloseHandle(info.hProcess);
        ok.then_some(code)
    }
}

/// Führt ein Programm unsichtbar aus und liefert den Exitcode.
pub fn run_hidden(file: &str, args: &[&str]) -> Option<i32> {
    std::process::Command::new(file).args(args).creation_flags(CREATE_NO_WINDOW).status().ok().and_then(|s| s.code())
}

pub fn open_in_explorer(path: &str) {
    let _ = std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{path}\"")).spawn();
}

/// Windows-Dialog „Speichern unter“: Ordner und Dateiname frei wählbar.
/// `filters`: (Beschreibung, Muster), z. B. ("Textdatei", "*.txt"). `None` = abgebrochen.
/// Blockiert, bis der Dialog geschlossen ist – deshalb in einem eigenen Thread aufrufen.
pub fn save_file_dialog(title: &str, default_name: &str, start_dir: Option<&std::path::Path>, filters: &[(&str, &str)], default_ext: &str) -> Option<std::path::PathBuf> {
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{FileSaveDialog, IFileSaveDialog, IShellItem, SHCreateItemFromParsingName, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, SIGDN_FILESYSPATH};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let d: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let names: Vec<(HSTRING, HSTRING)> = filters.iter().map(|(n, p)| (HSTRING::from(*n), HSTRING::from(*p))).collect();
        let specs: Vec<COMDLG_FILTERSPEC> = names.iter().map(|(n, p)| COMDLG_FILTERSPEC { pszName: PCWSTR(n.as_ptr()), pszSpec: PCWSTR(p.as_ptr()) }).collect();
        if !specs.is_empty() {
            let _ = d.SetFileTypes(&specs);
        }
        let _ = d.SetTitle(&HSTRING::from(title));
        let _ = d.SetFileName(&HSTRING::from(default_name));
        let _ = d.SetDefaultExtension(&HSTRING::from(default_ext));
        if let Ok(o) = d.GetOptions() {
            let _ = d.SetOptions(o | FOS_OVERWRITEPROMPT | FOS_FORCEFILESYSTEM);
        }
        if let Some(dir) = start_dir.filter(|d| d.is_dir()) {
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(dir.as_os_str()), None) {
                let _ = d.SetFolder(&item);
            }
        }
        let owner = find_own_window(MAIN_TITLE).unwrap_or_default();
        d.Show(owner).ok()?; // abgebrochen → Fehler → None
        let item = d.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(std::path::PathBuf::from)
    }
}

pub fn shell_open(target: &str) {
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(target), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Online-Suche im Standardbrowser (nur auf ausdrücklichen Klick).
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

/// Beendet diesen Prozess sofort, ohne die Abmelderoutinen der geladenen Bibliotheken.
///
/// Grund: Mit Adminrechten ist der PawnIO-Treiber geladen (dazu NVML und die Leistungsindikatoren).
/// Beim normalen Beenden räumt Windows diese Bibliotheken auf, während noch Messthreads laufen –
/// das endete bisher in einem Absturz in ntdll (0xC000000D) genau beim Schließen. Alles, was
/// gespeichert werden muss (Einstellungen), ist zu diesem Zeitpunkt schon geschrieben.
pub fn fast_exit(code: u32) -> ! {
    unsafe {
        use windows::Win32::System::Threading::GetCurrentProcess;
        let _ = TerminateProcess(GetCurrentProcess(), code);
    }
    std::process::exit(code as i32)
}

pub fn terminate(pid: u32) -> Result<(), String> {
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, false, pid).map_err(|e| e.message())?;
        let r = TerminateProcess(h, 1);
        let _ = CloseHandle(h);
        r.map_err(|e| e.message())
    }
}

/// Effizienzmodus wie im Task-Manager: EcoQoS (Power Throttling) + niedrige Priorität.
pub fn set_efficiency_mode(pid: u32, on: bool) -> Result<(), String> {
    unsafe {
        let h = OpenProcess(PROCESS_SET_INFORMATION, false, pid).map_err(|e| e.message())?;
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: 1,
            ControlMask: 1, // PROCESS_POWER_THROTTLING_EXECUTION_SPEED
            StateMask: if on { 1 } else { 0 },
        };
        let r1 = SetProcessInformation(
            h,
            ProcessPowerThrottling,
            &state as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
        let r2 = SetPriorityClass(h, if on { IDLE_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS });
        let _ = CloseHandle(h);
        r1.and(r2).map_err(|e| e.message())
    }
}

/// Startet den Windows-Explorer neu.
pub fn restart_explorer() {
    let _ = run_hidden("taskkill.exe", &["/F", "/IM", "explorer.exe"]);
    std::thread::sleep(std::time::Duration::from_millis(800));
    let _ = std::process::Command::new("explorer.exe").spawn();
}

/// Helles Taskleisten-Design?
pub fn taskbar_is_light() -> bool {
    reg_dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "SystemUsesLightTheme").unwrap_or(0) != 0
}

pub fn reg_dword(subkey: &str, value: &str) -> Option<u32> {
    let mut data: u32 = 0;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(subkey),
            &HSTRING::from(value),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut std::ffi::c_void),
            Some(&mut size),
        )
    };
    r.is_ok().then_some(data)
}

struct FindCtx {
    title: String,
    exe: String,
    found: Option<HWND>,
}

unsafe extern "system" fn find_cb(hwnd: HWND, lp: LPARAM) -> BOOL {
    let ctx = &mut *(lp.0 as *mut FindCtx);
    let mut buf = [0u16; 128];
    let n = GetWindowTextW(hwnd, &mut buf);
    if n <= 0 || String::from_utf16_lossy(&buf[..n as usize]) != ctx.title {
        return true.into();
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if let Some(p) = crate::sensors::procs::process_path(pid) {
        if p.eq_ignore_ascii_case(&ctx.exe) {
            ctx.found = Some(hwnd);
            return false.into();
        }
    }
    true.into()
}

/// Sucht ein Fenster dieses Programms mit genau diesem Titel.
pub fn find_own_window(title: &str) -> Option<HWND> {
    let mut ctx = FindCtx { title: title.into(), exe: exe_path(), found: None };
    unsafe {
        let _ = EnumWindows(Some(find_cb), LPARAM(&mut ctx as *mut _ as isize));
    }
    ctx.found
}

pub fn bring_to_front(hwnd: HWND) {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

pub const MAIN_TITLE: &str = "TaskPulse";

/// Öffnet das Hauptfenster bzw. holt ein laufendes nach vorn.
pub fn open_main_window(settings: bool) {
    if let Some(h) = find_own_window(MAIN_TITLE) {
        bring_to_front(h);
        if settings {
            // Laufendes Fenster per Datei-Signal auf die Einstellungen umschalten
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

/// Privater Arbeitssatz und Arbeitssatz dieses Prozesses in Bytes (wie im Task-Manager).
pub fn own_memory() -> (f64, f64) {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
    unsafe {
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut _, c.cb);
    }
    (c.PrivateUsage as f64, c.WorkingSetSize as f64)
}

/// Gibt selten genutzte Speicherseiten an Windows zurück (nach der Initialisierung).
pub fn trim_working_set() {
    use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
    use windows::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Task beenden": ein echter Prozess wird beendet.
    #[test]
    fn terminate_ends_process() {
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/c", "ping -n 30 127.0.0.1 >nul"])
            .creation_flags_hidden()
            .spawn()
            .expect("Testprozess startet");
        terminate(child.id()).expect("beenden klappt");
        let t = std::time::Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            assert!(t.elapsed().as_secs() < 5, "Prozess läuft noch");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // Ein nicht vorhandener Prozess liefert einen Fehler statt Absturz
        assert!(terminate(0xFFFF_FFF0).is_err());
    }

    trait Hidden {
        fn creation_flags_hidden(&mut self) -> &mut Self;
    }
    impl Hidden for std::process::Command {
        fn creation_flags_hidden(&mut self) -> &mut Self {
            use std::os::windows::process::CommandExt;
            self.creation_flags(CREATE_NO_WINDOW)
        }
    }
}
