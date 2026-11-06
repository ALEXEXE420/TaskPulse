//! Bildrate (FPS) und Frametimes aller Programme über die Windows-Ereignisablaufverfolgung (ETW) –
//! dasselbe Verfahren wie Intel PresentMon, NVIDIA FrameView und CapFrameX.
//!
//! Anti-Cheat-freundlich: TaskPulse klinkt sich NICHT ins Spiel ein (keine DLL, kein Hook, kein
//! Overlay), liest keinen Spielspeicher und öffnet keinen Zugriff auf den Spielprozess. Es liest nur
//! mit, was Windows bei jedem ausgegebenen Bild ohnehin protokolliert:
//! - Microsoft-Windows-DXGI, Present_Start (Id 42): DirectX 10/11/12
//! - Microsoft-Windows-D3D9, Present_Start (Id 1): DirectX 9
//! - Microsoft-Windows-DxgKrnl, PresentHistory(Detailed)_Start (Id 171/215) bzw. Present_Info (Id 184):
//!   Vulkan, OpenGL und alles, was nicht über DXGI/D3D9 geht
//! (Provider und Ereignis-Ids wie in PresentMon, PresentData/ETW/*.h.)
//!
//! Braucht Adminrechte (oder die Gruppe „Leistungsprotokollbenutzer“).

#[cfg(windows)]
use std::collections::HashMap;
use std::collections::VecDeque;
#[cfg(windows)]
use std::sync::{Mutex, OnceLock};
#[cfg(windows)]
use windows::core::{GUID, PCWSTR, PWSTR};
#[cfg(windows)]
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_SUCCESS};
#[cfg(windows)]
use windows::Win32::System::Diagnostics::Etw::*;

#[cfg(windows)]
const SESSION: &str = "TaskPulse-FPS";

#[cfg(windows)]
const DXGI: GUID = GUID::from_u128(0xCA11C036_0102_4A2D_A6AD_F03CFED5D3C9);
#[cfg(windows)]
const D3D9: GUID = GUID::from_u128(0x783ACA0A_790E_4D7F_8451_AA850511C6B9);
#[cfg(windows)]
const DXGKRNL: GUID = GUID::from_u128(0x802EC45A_1E99_4B83_9920_87C98277BA9D);

#[cfg(windows)]
const DXGI_PRESENT_START: u16 = 0x2A;
#[cfg(windows)]
const D3D9_PRESENT_START: u16 = 0x01;
#[cfg(windows)]
const KRNL_PRESENT_HISTORY_START: u16 = 0xAB;
#[cfg(windows)]
const KRNL_PRESENT_HISTORY_DETAILED_START: u16 = 0xD7;
#[cfg(windows)]
const KRNL_PRESENT_INFO: u16 = 0xB8;

#[cfg(windows)]
/// DxgKrnl nur mit den Schlüsselwörtern Base (0x1) und Present (0x8000000) – sonst sehr viele Ereignisse.
const KRNL_KEYWORDS: u64 = 0x0000_0000_0800_0001;

#[cfg(windows)]
/// Zeitstempel der letzten Bilder je Prozess (QPC-Ticks).
#[derive(Default)]
struct PidFrames {
    /// über DXGI/D3D9 gemeldet (genau ein Ereignis pro Bild)
    api: VecDeque<i64>,
    /// über DxgKrnl gemeldet (für Vulkan/OpenGL)
    krnl: VecDeque<i64>,
}

/// Welche Uhr die Zeitstempel nutzen – wird am ersten Ereignis erkannt.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Clock {
    /// QueryPerformanceCounter (gewünscht)
    Qpc,
    /// FILETIME, 100 ns seit 1601 (falls Windows umrechnet)
    FileTime,
}

#[cfg(windows)]
#[derive(Default)]
struct Shared {
    clock: Option<Clock>,
    /// Ereignisse insgesamt (Diagnose: kommen überhaupt Grafik-Ereignisse an?)
    events: u64,
    /// Jüngster bisher angekommener Zeitstempel (beliebiges Programm)
    latest: i64,
    pids: HashMap<u32, PidFrames>,
}

#[cfg(windows)]
fn shared() -> &'static Mutex<Shared> {
    static S: OnceLock<Mutex<Shared>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Shared::default()))
}

#[cfg(windows)]
fn qpc_freq() -> f64 {
    static F: OnceLock<f64> = OnceLock::new();
    *F.get_or_init(|| {
        let mut f = 0i64;
        unsafe {
            let _ = windows::Win32::System::Performance::QueryPerformanceFrequency(&mut f);
        }
        f.max(1) as f64
    })
}

#[cfg(windows)]
fn filetime_now() -> i64 {
    let ft = unsafe { windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime() };
    ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
}

/// Erkennt am Zeitstempel, ob er QPC oder FILETIME ist (der nähere „Jetzt“-Wert gewinnt).
pub fn detect_clock(ts: i64, qpc_now: i64, filetime_now: i64) -> Clock {
    if (ts - qpc_now).abs() <= (ts - filetime_now).abs() {
        Clock::Qpc
    } else {
        Clock::FileTime
    }
}

#[cfg(windows)]
fn qpc_now() -> i64 {
    let mut t = 0i64;
    unsafe {
        let _ = windows::Win32::System::Performance::QueryPerformanceCounter(&mut t);
    }
    t
}

#[cfg(windows)]
/// Höchstens so viele Zeitstempel je Prozess behalten (≈ 4 s bei 1000 FPS)
const KEEP: usize = 4000;

#[cfg(windows)]
unsafe extern "system" fn on_event(rec: *mut EVENT_RECORD) {
    let Some(r) = rec.as_ref() else { return };
    let h = &r.EventHeader;
    let id = h.EventDescriptor.Id;
    let p = h.ProviderId;
    let api = (p == DXGI && id == DXGI_PRESENT_START) || (p == D3D9 && id == D3D9_PRESENT_START);
    let krnl = p == DXGKRNL && matches!(id, KRNL_PRESENT_HISTORY_START | KRNL_PRESENT_HISTORY_DETAILED_START | KRNL_PRESENT_INFO);
    if !api && !krnl {
        return;
    }
    let Ok(mut s) = shared().lock() else { return };
    if s.clock.is_none() {
        s.clock = Some(detect_clock(h.TimeStamp, qpc_now(), filetime_now()));
    }
    s.events += 1;
    s.latest = s.latest.max(h.TimeStamp);
    let e = s.pids.entry(h.ProcessId).or_default();
    let q = if api { &mut e.api } else { &mut e.krnl };
    q.push_back(h.TimeStamp);
    if q.len() > KEEP {
        q.pop_front();
    }
}

#[cfg(windows)]
fn props_buffer() -> Vec<u8> {
    let name: Vec<u16> = SESSION.encode_utf16().chain(std::iter::once(0)).collect();
    let base = std::mem::size_of::<EVENT_TRACE_PROPERTIES>();
    let mut buf = vec![0u8; base + name.len() * 2 + 16];
    unsafe {
        let p = &mut *(buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES);
        p.Wnode.BufferSize = buf.len() as u32;
        p.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        p.Wnode.ClientContext = 1; // Zeitstempel als QueryPerformanceCounter
        p.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
        p.LoggerNameOffset = base as u32;
        p.BufferSize = 64; // KB
        p.MinimumBuffers = 8;
        p.MaximumBuffers = 64;
        p.FlushTimer = 1;
        std::ptr::copy_nonoverlapping(name.as_ptr() as *const u8, buf.as_mut_ptr().add(base), name.len() * 2);
    }
    buf
}

#[cfg(windows)]
fn stop_session() {
    let mut buf = props_buffer();
    let name: Vec<u16> = SESSION.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = ControlTraceW(CONTROLTRACE_HANDLE::default(), PCWSTR(name.as_ptr()), buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES, EVENT_TRACE_CONTROL_STOP);
    }
}

#[cfg(windows)]
/// Laufende Messung; beim Fallenlassen wird die Sitzung beendet.
pub struct FpsTrace {
    handle: CONTROLTRACE_HANDLE,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(windows)]
unsafe impl Send for FpsTrace {}

#[cfg(windows)]
impl FpsTrace {
    /// Startet die Ereignissitzung. Fehler als lesbarer Text (z. B. fehlende Adminrechte).
    pub fn start() -> Result<FpsTrace, String> {
        shared()
            .lock()
            .map(|mut s| {
                s.pids.clear();
                s.clock = None;
                s.events = 0;
                s.latest = 0;
            })
            .ok();
        let name: Vec<u16> = SESSION.encode_utf16().chain(std::iter::once(0)).collect();
        let mut handle = CONTROLTRACE_HANDLE::default();
        let mut buf = props_buffer();
        let mut err = unsafe { StartTraceW(&mut handle, PCWSTR(name.as_ptr()), buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES) };
        if err == ERROR_ALREADY_EXISTS {
            // Reste einer früheren TaskPulse-Sitzung (z. B. nach Absturz) beenden
            stop_session();
            buf = props_buffer();
            err = unsafe { StartTraceW(&mut handle, PCWSTR(name.as_ptr()), buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES) };
        }
        if err == ERROR_ACCESS_DENIED {
            return Err("FPS-Messung braucht Adminrechte".into());
        }
        if err != ERROR_SUCCESS {
            return Err(format!("Ereignissitzung nicht gestartet (Fehler {})", err.0));
        }
        let enable = |g: &GUID, keywords: u64| unsafe {
            EnableTraceEx2(handle, g, EVENT_CONTROL_CODE_ENABLE_PROVIDER.0, TRACE_LEVEL_INFORMATION as u8, keywords, 0, 0, None)
        };
        let ok = [enable(&DXGI, u64::MAX), enable(&D3D9, u64::MAX), enable(&DXGKRNL, KRNL_KEYWORDS)];
        if ok.iter().all(|e| *e != ERROR_SUCCESS) {
            stop_session();
            return Err("Grafik-Ereignisse nicht verfügbar".into());
        }
        // Verbraucher-Thread: ProcessTrace blockiert, bis die Sitzung beendet wird
        let thread = std::thread::Builder::new()
            .name("fps-etw".into())
            .spawn(move || unsafe {
                // Auch unter Volllast (z. B. CPU-Burner auf allen Kernen) zügig mitlesen
                let _ = windows::Win32::System::Threading::SetThreadPriority(
                    windows::Win32::System::Threading::GetCurrentThread(),
                    windows::Win32::System::Threading::THREAD_PRIORITY_HIGHEST,
                );
                let mut name: Vec<u16> = SESSION.encode_utf16().chain(std::iter::once(0)).collect();
                let mut log = EVENT_TRACE_LOGFILEW::default();
                log.LoggerName = PWSTR(name.as_mut_ptr());
                // RAW_TIMESTAMP: Zeitstempel im Takt der Sitzung (ClientContext 1 = QueryPerformanceCounter);
                // ohne diesen Schalter rechnet Windows sie in Uhrzeit (FILETIME) um.
                log.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
                log.Anonymous2.EventRecordCallback = Some(on_event);
                let h = OpenTraceW(&mut log);
                if h.Value == u64::MAX {
                    return;
                }
                let _ = ProcessTrace(&[h], None, None);
                let _ = CloseTrace(h);
            })
            .ok();
        Ok(FpsTrace { handle, thread })
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if self.handle.Value != 0 {
            stop_session();
            self.handle = CONTROLTRACE_HANDLE::default();
        }
        if let Some(t) = self.thread.take() {
            let t0 = std::time::Instant::now();
            while !t.is_finished() && t0.elapsed().as_secs() < 2 {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }
}

#[cfg(windows)]
impl Drop for FpsTrace {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Bilder eines Prozesses in der letzten Sekunde.
#[derive(Debug, Clone, Default)]
pub struct FpsSample {
    pub pid: u32,
    pub fps: f64,
    /// Abstände zwischen den Bildern dieser Sekunde, ms
    pub frametimes_ms: Vec<f64>,
}

/// Frametimes (ms) aus Zeitstempeln innerhalb [von, bis].
pub fn frametimes(ts: &VecDeque<i64>, from: i64, to: i64, freq: f64) -> Vec<f64> {
    let mut v = Vec::new();
    let mut prev: Option<i64> = None;
    for &t in ts.iter() {
        if t > to {
            break;
        }
        if t >= from {
            if let Some(p) = prev {
                v.push((t - p) as f64 * 1000.0 / freq);
            }
        }
        prev = Some(t);
    }
    v
}

/// Bildrate aller Prozesse in der letzten Sekunde, ohne `exclude` (z. B. dwm.exe, TaskPulse).
/// Windows liefert Echtzeit-Ereignisse gepuffert aus (bis ~1 s später). Gezählt wird deshalb die
/// Sekunde, die 1,5 s zurückliegt – dann sind ihre Bilder sicher angekommen.
const DELAY_S: f64 = 1.5;

/// Ende des Zählfensters: höchstens `now − DELAY_S`, aber nicht später als 0,25 s vor dem jüngsten
/// angekommenen Ereignis (Ereignisse verschiedener Prozessorkerne kommen leicht durcheinander an).
pub fn window_end(now: i64, latest: i64, freq: f64) -> i64 {
    let by_clock = now - (DELAY_S * freq) as i64;
    if latest <= 0 {
        return by_clock;
    }
    by_clock.min(latest - (0.25 * freq) as i64)
}

#[cfg(windows)]
/// Wie viele Grafik-Ereignisse bisher ankamen (0 nach einigen Sekunden = Sitzung empfängt nichts).
pub fn event_count() -> u64 {
    shared().lock().map(|s| s.events).unwrap_or(0)
}

#[cfg(windows)]
pub fn snapshot(exclude: &[u32]) -> Vec<FpsSample> {
    let Ok(s) = shared().lock() else { return Vec::new() };
    let (freq, now) = match s.clock {
        Some(Clock::FileTime) => (1e7, filetime_now()),
        _ => (qpc_freq(), qpc_now()),
    };
    // Gezählt wird die letzte Sekunde, die sicher vollständig angekommen ist: höchstens bis
    // „jetzt − 1,5 s“ und nie über das jüngste angekommene Ereignis hinaus (unter Volllast kommen
    // Ereignisse später – sonst würden halbe Sekunden gezählt und die FPS halbiert)
    let to = window_end(now, s.latest, freq);
    let from = to - freq as i64;
    let now = to;
    let mut out: Vec<FpsSample> = s
        .pids
        .iter()
        .filter(|(pid, _)| !exclude.contains(pid) && **pid > 4)
        .filter_map(|(pid, f)| {
            // Programme mit DXGI/D3D9-Ereignissen darüber zählen, sonst über DxgKrnl
            let q = if f.api.iter().any(|t| *t >= from) { &f.api } else { &f.krnl };
            let n = q.iter().filter(|t| **t >= from && **t <= now).count();
            (n > 0).then(|| FpsSample { pid: *pid, fps: n as f64, frametimes_ms: frametimes(q, from, now, freq) })
        })
        .collect();
    out.sort_by(|a, b| b.fps.total_cmp(&a.fps));
    out
}

// ------------------------------------------------------------------ Linux: (noch) keine FPS-Messung
// Unter Linux gibt es kein ETW. Ein Weg ohne Eingriff ins Spiel wird für eine spätere Version geprüft.

#[cfg(target_os = "linux")]
pub struct FpsTrace;

#[cfg(target_os = "linux")]
impl FpsTrace {
    pub fn start() -> Result<FpsTrace, String> {
        Err("FPS-Messung gibt es unter Linux noch nicht".into())
    }
    pub fn stop(self) {}
}

#[cfg(target_os = "linux")]
pub fn event_count() -> u64 {
    0
}

#[cfg(target_os = "linux")]
pub fn snapshot(_exclude: &[u32]) -> Vec<FpsSample> {
    Vec::new()
}

/// Kennzahlen über eine ganze Aufzeichnung: (Ø FPS, 1 % Low, 0,1 % Low) aus allen Frametimes.
/// „1 % Low“ = Bildrate, die von den langsamsten 1 % der Bilder erreicht wird (wie CapFrameX/PresentMon).
pub fn lows(frametimes_ms: &[f64]) -> Option<(f64, f64, f64)> {
    let ft: Vec<f64> = frametimes_ms.iter().copied().filter(|x| x.is_finite() && *x > 0.0).collect();
    if ft.is_empty() {
        return None;
    }
    let total: f64 = ft.iter().sum();
    let avg = ft.len() as f64 * 1000.0 / total;
    let mut sorted = ft.clone();
    sorted.sort_by(|a, b| b.total_cmp(a)); // langsamste zuerst
    let low = |share: f64| {
        let n = ((sorted.len() as f64 * share).ceil() as usize).max(1);
        let worst: f64 = sorted[..n].iter().sum::<f64>() / n as f64;
        1000.0 / worst
    };
    Some((avg, low(0.01), low(0.001)))
}

/// Ruckler: Bilder, die mehr als 2,5-mal so lange dauern wie üblich (Median der Aufzeichnung)
/// und mindestens 20 ms – das spürt man als kurzes Stocken. Gibt (Anzahl, Grenze in ms) zurück.
pub fn stutters(frametimes_ms: &[f64]) -> Option<(usize, f64)> {
    let mut v: Vec<f64> = frametimes_ms.iter().copied().filter(|x| x.is_finite() && *x > 0.0).collect();
    if v.len() < 10 {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let median = v[v.len() / 2];
    let limit = (median * 2.5).max(20.0);
    Some((v.iter().filter(|x| **x > limit).count(), limit))
}

/// Bewertung der Gleichmäßigkeit: 0,1 % Low im Verhältnis zu 1 % Low.
/// Fällt die 0,1 % Low weit unter die 1 % Low, gibt es seltene, aber deutliche Hänger.
pub fn smoothness(l1: f64, l01: f64) -> (&'static str, u8) {
    let r = if l1 > 0.0 { l01 / l1 } else { 1.0 };
    if r >= 0.75 {
        ("gleichmäßig", 0)
    } else if r >= 0.5 {
        ("vereinzelte Mikroruckler", 1)
    } else {
        ("deutliche Hänger (kurzes Einfrieren)", 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frametimes_from_timestamps() {
        // 10-MHz-Takt: 100 000 Ticks = 10 ms
        let ts: VecDeque<i64> = [0, 100_000, 200_000, 400_000, 500_000].into_iter().collect();
        assert_eq!(frametimes(&ts, 0, 500_000, 10_000_000.0), vec![10.0, 10.0, 20.0, 10.0]);
        // nur ab 200 000: erster Abstand zählt zum vorherigen Bild
        assert_eq!(frametimes(&ts, 200_000, 500_000, 10_000_000.0), vec![10.0, 20.0, 10.0]);
    }

    #[test]
    fn one_percent_low() {
        // 99 Bilder mit 10 ms, 1 Bild mit 50 ms → Ø ≈ 96,2 FPS, 1 % Low = 20 FPS
        let mut ft = vec![10.0; 99];
        ft.push(50.0);
        let (avg, l1, l01) = lows(&ft).unwrap();
        assert!((avg - 100.0 * 1000.0 / 1040.0).abs() < 1e-9);
        assert!((l1 - 20.0).abs() < 1e-9);
        assert!((l01 - 20.0).abs() < 1e-9);
        assert_eq!(lows(&[]), None);
    }

    #[test]
    fn stutter_example_from_request() {
        // 60 FPS (16,7 ms) mit seltenen 66-ms-Hängern: 1 % Low hoch, 0,1 % Low ≈ 15 FPS
        let mut ft = vec![16.7; 9990];
        for i in 0..10 {
            ft[i * 999] = 66.7;
        }
        let (_, l1, l01) = lows(&ft).unwrap();
        assert!(l1 > 25.0, "1 % Low {l1}");
        assert!((l01 - 15.0).abs() < 0.1, "0,1 % Low {l01}");
        assert_eq!(smoothness(l1, l01).1, 2);
        assert_eq!(stutters(&ft), Some((10, 41.75)));
        // gleichmäßig: keine Ruckler
        assert_eq!(stutters(&vec![16.7; 500]).unwrap().0, 0);
        assert_eq!(smoothness(60.0, 58.0).1, 0);
    }

    #[test]
    fn window_waits_for_late_events() {
        let f = 10_000_000.0;
        // Ereignisse aktuell: Fenster endet 1,5 s vor jetzt
        assert_eq!(window_end(100 * 10_000_000, 100 * 10_000_000, f), 985_000_000);
        // Ereignisse hängen 3 s hinterher: Fenster endet 0,25 s vor dem jüngsten Ereignis
        assert_eq!(window_end(100 * 10_000_000, 97 * 10_000_000, f), 967_500_000);
        assert_eq!(window_end(100 * 10_000_000, 0, f), 985_000_000);
    }

    #[test]
    fn clock_detection() {
        // QPC (10 MHz, ~3 Tage Laufzeit) vs. FILETIME (2026)
        let qpc = 2_600_000_000_000i64;
        let ft = 134_066_000_000_000_000i64;
        assert_eq!(detect_clock(qpc - 5_000_000, qpc, ft), Clock::Qpc);
        assert_eq!(detect_clock(ft - 5_000_000, qpc, ft), Clock::FileTime);
    }

    #[cfg(windows)]
    #[test]
    fn provider_guids() {
        assert_eq!(format!("{:?}", DXGI), "CA11C036-0102-4A2D-A6AD-F03CFED5D3C9");
        assert_eq!(format!("{:?}", DXGKRNL), "802EC45A-1E99-4B83-9920-87C98277BA9D");
    }
}
