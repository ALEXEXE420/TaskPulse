//! Taskleisten-Anzeige: eigenes, sparsames Win32-Fenster, das (wie TrafficMonitor) als
//! Kindfenster in die Taskleiste eingepasst wird. Windows 11 kennt keine DeskBands mehr.
//! Rückfall: frei schwebendes Fenster.

pub mod layout;
pub mod render;

use crate::config::{Config, Side};
use crate::sensors::{Basic, BasicSensors};
use crate::winutil;
use render::{color, Renderer, TextItem};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_APP_DATA: u32 = WM_APP + 1;
const TIMER_HEALTH: usize = 1;
const CLASS_WIDGET: PCWSTR = w!("TaskPulseTaskbarWidget");
const CLASS_HOST: PCWSTR = w!("TaskPulseTaskbarHost");
pub const WIDGET_TITLE: &str = "TaskPulse Taskleiste";
const ELEVATE_SIGNAL: &str = "elevate_taskbar.signal";

/// Vom erhöhten Hauptfenster aufgerufen: Eine nicht erhöhte Anzeige beendet sich auf das Signal hin,
/// danach wird sie mit Adminrechten (für die CPU-Temperatur) neu gestartet.
pub fn request_elevated_restart() {
    let sig = Config::dir().join(ELEVATE_SIGNAL);
    if std::fs::write(&sig, b"1").is_err() {
        return;
    }
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if !crate::ui::taskbar_running() {
            winutil::spawn_self(&["--taskbar"]);
            return;
        }
        if !sig.exists() {
            // Signal gelesen – war die Anzeige schon erhöht, läuft sie weiter
            std::thread::sleep(std::time::Duration::from_millis(500));
            if !crate::ui::taskbar_running() {
                winutil::spawn_self(&["--taskbar"]);
            }
            return;
        }
    }
    let _ = std::fs::remove_file(sig);
}

struct Drag {
    start: POINT,
    offset: i32,
    float: (i32, i32),
    moved: bool,
}

struct App {
    host: HWND,
    widget: HWND,
    parent: Option<HWND>,
    cfg: Config,
    cfg_mtime: Option<SystemTime>,
    shared_cfg: Arc<Mutex<Config>>,
    data: Arc<Mutex<Basic>>,
    last: Basic,
    renderer: Renderer,
    cols: Vec<layout::ColumnLayout>,
    layout_key: Option<(u32, String)>,
    width: f32,
    light: bool,
    last_theme: Instant,
    drag: Option<Drag>,
    taskbar_created: u32,
    ticks: u32,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| match a.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
        Err(_) => None,
    })
}

/// Alle Taskleisten: [Haupt, weitere von links nach rechts].
pub fn taskbars() -> Vec<HWND> {
    let mut v = Vec::new();
    unsafe {
        if let Ok(p) = FindWindowW(w!("Shell_TrayWnd"), None) {
            if !p.is_invalid() {
                v.push(p);
            }
        }
        let mut sec = Vec::new();
        let mut after = HWND::default();
        loop {
            match FindWindowExW(None, after, w!("Shell_SecondaryTrayWnd"), None) {
                Ok(h) if !h.is_invalid() => {
                    let mut r = RECT::default();
                    let _ = GetWindowRect(h, &mut r);
                    sec.push((r.left, h));
                    after = h;
                }
                _ => break,
            }
        }
        sec.sort_by_key(|(l, _)| *l);
        v.extend(sec.into_iter().map(|(_, h)| h));
    }
    v
}

fn rect_of(h: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(h, &mut r);
    }
    r
}

/// Linke Kante des Infobereichs (Uhr/Symbole), an der die Anzeige rechts andockt.
fn tray_notify_left(taskbar: HWND, scale: f32) -> i32 {
    unsafe {
        if let Ok(n) = FindWindowExW(taskbar, None, w!("TrayNotifyWnd"), None) {
            if !n.is_invalid() {
                let r = rect_of(n);
                if r.right > r.left {
                    return r.left;
                }
            }
        }
    }
    rect_of(taskbar).right - (110.0 * scale) as i32
}

impl App {
    fn scale(&self) -> f32 {
        let h = self.parent.unwrap_or(self.widget);
        let dpi = unsafe { GetDpiForWindow(h) };
        if dpi == 0 {
            1.0
        } else {
            dpi as f32 / 96.0
        }
    }

    fn font_px(&self) -> f32 {
        self.cfg.taskbar.font_size_pt * 96.0 / 72.0 * self.scale()
    }

    fn line_h(&self) -> f32 {
        (self.font_px() * 1.33).ceil()
    }

    fn relayout(&mut self) {
        let px = self.font_px();
        let _ = self.renderer.set_font(&self.cfg.taskbar.font_family, px);
        let key = (
            (px * 100.0) as u32,
            format!("{:?}{:?}{}{}{:?}{:?}{:?}", self.cfg.taskbar.fields, self.cfg.taskbar.hidden, self.cfg.line_up_mbit, self.cfg.line_down_mbit, self.cfg.taskbar.net_unit, self.cfg.taskbar.gpu_mem_mode, self.cfg.taskbar.cpu_mem_mode)
                + &self.cfg.taskbar.show_line_usage.to_string()
                + &self.cfg.taskbar.separator.to_string()
                + &self.cfg.taskbar.font_family,
        );
        if self.layout_key.as_ref() == Some(&key) {
            return;
        }
        let s = self.scale();
        let r = &self.renderer;
        let mut m = |t: &str| r.measure(t);
        let (cols, width) = layout::layout(&self.cfg, &mut m, (12.0 * s).round(), (4.0 * s).round());
        self.cols = cols;
        self.width = width;
        self.layout_key = Some(key);
    }

    /// Zielrechteck (Bildschirmkoordinaten).
    fn target_rect(&self) -> RECT {
        let w = self.width.ceil() as i32;
        let s = self.scale();
        match self.parent {
            Some(tb) => {
                let r = rect_of(tb);
                let h = (r.bottom - r.top).max(1);
                let tw = r.right - r.left;
                let off = (self.cfg.taskbar.offset_px as f32 * s) as i32;
                let x = match self.cfg.taskbar.side {
                    Side::Left => r.left + off,
                    Side::Right => tray_notify_left(tb, s) - w - off,
                };
                let x = x.clamp(r.left, (r.left + tw - w).max(r.left));
                RECT { left: x, top: r.top, right: x + w, bottom: r.top + h }
            }
            None => {
                let h = (self.line_h() * 2.0 + 8.0 * s) as i32;
                let x = self.cfg.taskbar.float_x;
                let y = self.cfg.taskbar.float_y;
                RECT { left: x, top: y, right: x + w + (8.0 * s) as i32, bottom: y + h }
            }
        }
    }

    fn place(&self) {
        let r = self.target_rect();
        unsafe {
            let (x, y) = match self.parent {
                Some(tb) => {
                    let pr = rect_of(tb);
                    (r.left - pr.left, r.top - pr.top)
                }
                None => (r.left, r.top),
            };
            let after = if self.parent.is_some() { HWND_TOP } else { HWND_TOPMOST };
            let _ = SetWindowPos(self.widget, after, x, y, r.right - r.left, r.bottom - r.top, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    fn render(&mut self) {
        if self.widget.is_invalid() {
            return;
        }
        self.relayout();
        if self.last_theme.elapsed().as_secs() >= 3 {
            self.light = winutil::taskbar_is_light();
            self.last_theme = Instant::now();
        }
        self.place();
        let r = self.target_rect();
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let line_h = self.line_h();
        let floating = self.parent.is_none();
        let top = ((h as f32 - line_h * 2.0) / 2.0).max(0.0).floor();
        let xoff = if floating { 4.0 * self.scale() } else { 0.0 };
        let mut items = Vec::new();
        for c in &self.cols {
            for (row, f) in c.fields.iter().enumerate() {
                let cell = layout::cell(*f, &self.last, &self.cfg);
                let y = top + row as f32 * line_h;
                items.push(TextItem { x: c.x + xoff, y, text: cell.text });
                if let (Some(ex), Some(text)) = (c.extra_x, cell.extra) {
                    items.push(TextItem { x: ex + xoff, y, text });
                }
            }
        }
        let (fg, bg) = if floating {
            (color(1.0, 1.0, 1.0, 1.0), color(0.08, 0.08, 0.08, 0.78))
        } else if self.light {
            (color(0.0, 0.0, 0.0, 1.0), color(1.0, 1.0, 1.0, 1.0 / 255.0))
        } else {
            (color(1.0, 1.0, 1.0, 1.0), color(0.0, 0.0, 0.0, 1.0 / 255.0))
        };
        let seps = separator_rects(&self.cols, xoff, top, line_h, self.scale());
        let _ = self.renderer.draw(self.widget, w, h, line_h, &items, &seps, fg, bg);
    }

    fn reload_config_if_changed(&mut self) -> bool {
        let m = Config::modified();
        if m == self.cfg_mtime {
            return false;
        }
        self.cfg_mtime = m;
        let new = Config::load();
        let reattach = new.taskbar.floating != self.cfg.taskbar.floating || new.taskbar.monitor != self.cfg.taskbar.monitor;
        self.cfg = new.clone();
        *self.shared_cfg.lock().unwrap() = new;
        self.layout_key = None;
        reattach
    }

    fn save_config(&mut self) {
        let _ = self.cfg.save();
        self.cfg_mtime = Config::modified();
        *self.shared_cfg.lock().unwrap() = self.cfg.clone();
    }
}

/// Rechtecke der Trennstriche: so hoch wie die beiden Textzeilen, Breite je nach Skalierung.
fn separator_rects(cols: &[layout::ColumnLayout], xoff: f32, top: f32, line_h: f32, scale: f32) -> Vec<windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F> {
    let w = scale.round().max(1.0);
    cols.iter()
        .filter_map(|c| c.sep_x)
        .map(|x| {
            let x = (x + xoff - w / 2.0).round();
            windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F { left: x, top: top + 2.0 * scale, right: x + w, bottom: top + line_h * 2.0 - 2.0 * scale }
        })
        .collect()
}

fn hinstance() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default() }
}

/// Erstellt (oder ersetzt) das Anzeigefenster.
fn attach() {
    // Altes Fenster außerhalb der Ausleihe zerstören (DestroyWindow ruft die Fensterprozedur auf)
    let old = with_app(|a| std::mem::take(&mut a.widget)).unwrap_or_default();
    if !old.is_invalid() {
        unsafe {
            let _ = DestroyWindow(old);
        }
    }
    let Some((floating, monitor)) = with_app(|a| (a.cfg.taskbar.floating, a.cfg.taskbar.monitor)) else { return };
    let bars = taskbars();
    let parent = if floating { None } else { bars.get(monitor).or(bars.first()).copied() };
    let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    let title = HSTRING::from(WIDGET_TITLE);
    let hwnd = unsafe {
        match parent {
            Some(p) => CreateWindowExW(ex, CLASS_WIDGET, &title, WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS, 0, 0, 10, 10, p, None, hinstance(), None)
                .ok(),
            None => None,
        }
    };
    let (hwnd, parent) = match hwnd {
        Some(h) => (h, parent),
        None => {
            // Rückfall: frei schwebend über allen Fenstern
            let h = unsafe {
                CreateWindowExW(ex | WS_EX_TOPMOST, CLASS_WIDGET, &title, WS_POPUP | WS_VISIBLE, 100, 100, 10, 10, None, None, hinstance(), None)
            };
            match h {
                Ok(h) => (h, None),
                Err(_) => return,
            }
        }
    };
    with_app(|a| {
        a.widget = hwnd;
        a.parent = parent;
        a.layout_key = None;
        a.render();
    });
}

fn health_check() {
    let need = with_app(|a| {
        let reattach = a.reload_config_if_changed();
        // Nach dem Start und danach alle 10 Minuten selten genutzte Seiten an Windows zurückgeben
        a.ticks = a.ticks.wrapping_add(1);
        if a.ticks == 5 || a.ticks % 300 == 0 {
            winutil::trim_working_set();
        }
        if !a.cfg.taskbar.enabled {
            // In den Einstellungen abgeschaltet
            unsafe { PostQuitMessage(0) };
            return false;
        }
        let sig = Config::dir().join(ELEVATE_SIGNAL);
        if sig.exists() {
            let _ = std::fs::remove_file(&sig);
            if !winutil::is_elevated() {
                // Ein erhöhtes Hauptfenster startet gleich eine erhöhte Anzeige
                unsafe { PostQuitMessage(0) };
                return false;
            }
        }
        let widget_ok = unsafe { IsWindow(a.widget).as_bool() };
        let parent_ok = match a.parent {
            Some(p) => unsafe { IsWindow(p).as_bool() && GetParent(a.widget).map(|x| x == p).unwrap_or(false) },
            None => true,
        };
        // Taskleiste wieder verfügbar, obwohl wir schweben mussten?
        let want_bar = !a.cfg.taskbar.floating && a.parent.is_none() && !taskbars().is_empty();
        reattach || !widget_ok || !parent_ok || want_bar
    })
    .unwrap_or(false);
    if need {
        attach();
    } else {
        with_app(|a| a.render());
    }
}

const ID_OPEN: usize = 1;
const ID_SETTINGS: usize = 2;
const ID_LEFT: usize = 3;
const ID_RIGHT: usize = 4;
const ID_FLOAT: usize = 5;
const ID_RESET: usize = 6;
const ID_QUIT: usize = 9;

fn context_menu() {
    let Some((host, side, floating)) = with_app(|a| (a.host, a.cfg.taskbar.side, a.cfg.taskbar.floating)) else { return };
    let cmd = unsafe {
        let Ok(m) = CreatePopupMenu() else { return };
        let chk = |b: bool| if b { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(m, MF_STRING, ID_OPEN, w!("TaskPulse öffnen"));
        let _ = AppendMenuW(m, MF_STRING, ID_SETTINGS, w!("Einstellungen …"));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(m, MF_STRING | chk(!floating && side == Side::Left), ID_LEFT, w!("Links in der Taskleiste"));
        let _ = AppendMenuW(m, MF_STRING | chk(!floating && side == Side::Right), ID_RIGHT, w!("Rechts in der Taskleiste"));
        let _ = AppendMenuW(m, MF_STRING | chk(floating), ID_FLOAT, w!("Frei schwebend"));
        let _ = AppendMenuW(m, MF_STRING, ID_RESET, w!("Abstand zurücksetzen"));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(m, MF_STRING, ID_QUIT, w!("Taskleisten-Anzeige beenden"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(host);
        let r = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, host, None);
        let _ = PostMessageW(host, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(m);
        r.0 as usize
    };
    match cmd {
        ID_OPEN => winutil::open_main_window(false),
        ID_SETTINGS => winutil::open_main_window(true),
        ID_LEFT | ID_RIGHT => {
            let re = with_app(|a| {
                let re = a.cfg.taskbar.floating;
                a.cfg.taskbar.floating = false;
                a.cfg.taskbar.side = if cmd == ID_LEFT { Side::Left } else { Side::Right };
                a.save_config();
                a.render();
                re
            });
            if re == Some(true) {
                attach();
            }
        }
        ID_FLOAT => {
            with_app(|a| {
                a.cfg.taskbar.floating = !a.cfg.taskbar.floating;
                if a.cfg.taskbar.floating {
                    let r = rect_of(a.widget);
                    a.cfg.taskbar.float_x = r.left;
                    a.cfg.taskbar.float_y = (r.top - 60).max(0);
                }
                a.save_config();
            });
            attach();
        }
        ID_RESET => {
            with_app(|a| {
                a.cfg.taskbar.offset_px = 10;
                a.save_config();
                a.render();
            });
        }
        ID_QUIT => unsafe { PostQuitMessage(0) },
        _ => {}
    }
}

fn lparam_point(hwnd: HWND, lp: LPARAM) -> POINT {
    let _ = hwnd;
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    let _ = lp;
    pt
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_APP_DATA => {
            with_app(|a| {
                a.last = a.data.lock().unwrap().clone();
                a.render();
            });
            LRESULT(0)
        }
        WM_TIMER if wp.0 == TIMER_HEALTH => {
            health_check();
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let pt = lparam_point(hwnd, lp);
            with_app(|a| {
                a.drag = Some(Drag { start: pt, offset: a.cfg.taskbar.offset_px, float: (a.cfg.taskbar.float_x, a.cfg.taskbar.float_y), moved: false });
            });
            SetCapture(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let pt = lparam_point(hwnd, lp);
            with_app(|a| {
                let s = a.scale();
                let Some(d) = a.drag.as_mut() else { return };
                let dx = pt.x - d.start.x;
                let dy = pt.y - d.start.y;
                if !d.moved && dx.abs() < 4 && dy.abs() < 4 {
                    return;
                }
                d.moved = true;
                if a.parent.is_some() {
                    let dpx = (dx as f32 / s) as i32;
                    let off = match a.cfg.taskbar.side {
                        Side::Right => d.offset - dpx,
                        Side::Left => d.offset + dpx,
                    };
                    a.cfg.taskbar.offset_px = off.max(0);
                } else {
                    a.cfg.taskbar.float_x = d.float.0 + dx;
                    a.cfg.taskbar.float_y = d.float.1 + dy;
                }
                a.place();
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            with_app(|a| {
                if let Some(d) = a.drag.take() {
                    if d.moved {
                        // Abstand auf den tatsächlich möglichen Wert begrenzen und speichern
                        if let Some(tb) = a.parent {
                            let r = a.target_rect();
                            let tr = rect_of(tb);
                            let s = a.scale();
                            a.cfg.taskbar.offset_px = match a.cfg.taskbar.side {
                                Side::Left => ((r.left - tr.left) as f32 / s) as i32,
                                Side::Right => ((tray_notify_left(tb, s) - r.right) as f32 / s) as i32,
                            }
                            .max(0);
                        }
                        a.save_config();
                        a.render();
                    }
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            with_app(|a| a.drag = None);
            let _ = ReleaseCapture();
            winutil::open_main_window(false);
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            context_menu();
            LRESULT(0)
        }
        WM_DPICHANGED | WM_SETTINGCHANGE | WM_DISPLAYCHANGE => {
            with_app(|a| {
                a.layout_key = None;
                a.last_theme = Instant::now() - std::time::Duration::from_secs(10);
            });
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_DESTROY => {
            // Zerstört Windows das Fenster mit dem Explorer, baut der Timer es neu auf.
            LRESULT(0)
        }
        _ => {
            let tc = with_app(|a| a.taskbar_created).unwrap_or(0);
            if tc != 0 && msg == tc {
                // Explorer wurde neu gestartet
                attach();
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
    }
}

/// Startet die Taskleisten-Anzeige (blockiert bis "Beenden").
pub fn run() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // Nur eine Instanz
        // Gehört die Sperre einer Instanz mit Adminrechten, meldet Windows einer normalen
        // „Zugriff verweigert“ statt „existiert schon“ – auch dann läuft bereits eine Anzeige.
        let _mutex = CreateMutexW(None, true, w!("Local\\TaskPulse.Taskbar"));
        let err = GetLastError();
        if _mutex.is_err() || err == ERROR_ALREADY_EXISTS || err == ERROR_ACCESS_DENIED {
            return;
        }
        crate::updater::background_thread();

        let hinst = hinstance();
        for (class, style) in [(CLASS_WIDGET, CS_DBLCLKS), (CLASS_HOST, WNDCLASS_STYLES(0))] {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style,
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassExW(&wc);
        }
        let Ok(host) = CreateWindowExW(WS_EX_TOOLWINDOW, CLASS_HOST, w!("TaskPulse Host"), WS_POPUP, 0, 0, 0, 0, None, None, hinst, None) else {
            return;
        };
        let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
        // Auch mit Adminrechten die Nachricht des (nicht erhöhten) Explorers annehmen
        let _ = ChangeWindowMessageFilterEx(host, taskbar_created, MSGFLT_ALLOW, None);

        let cfg = Config::load();
        let shared_cfg = Arc::new(Mutex::new(cfg.clone()));
        let data = Arc::new(Mutex::new(Basic::default()));
        let stop = Arc::new(AtomicBool::new(false));

        // Messthread
        let measure = {
            let shared_cfg = shared_cfg.clone();
            let data = data.clone();
            let stop = stop.clone();
            let host_raw = host.0 as isize;
            std::thread::Builder::new()
                .name("messkern".into())
                .spawn(move || {
                    let mut sensors = BasicSensors::new(&shared_cfg.lock().unwrap().clone());
                    while !stop.load(Ordering::Relaxed) {
                        let t0 = Instant::now();
                        let c = shared_cfg.lock().unwrap().clone();
                        let b = sensors.sample(&c);
                        *data.lock().unwrap() = b;
                        let _ = PostMessageW(HWND(host_raw as *mut _), WM_APP_DATA, WPARAM(0), LPARAM(0));
                        // in kleinen Schritten warten, damit das Beenden nicht hängt
                        let iv = c.taskbar.interval();
                        while t0.elapsed() < iv && !stop.load(Ordering::Relaxed) {
                            std::thread::sleep(std::time::Duration::from_millis(50).min(iv));
                        }
                    }
                    // Sensoren (PawnIO, NVML, Leistungsindikatoren) hier geordnet schließen
                    drop(sensors);
                })
                .ok()
        };

        let renderer = match Renderer::new() {
            Ok(r) => r,
            Err(_) => return,
        };
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                host,
                widget: HWND::default(),
                parent: None,
                cfg_mtime: Config::modified(),
                cfg,
                shared_cfg,
                data,
                last: Basic::default(),
                renderer,
                cols: Vec::new(),
                layout_key: None,
                width: 100.0,
                light: winutil::taskbar_is_light(),
                last_theme: Instant::now(),
                drag: None,
                taskbar_created,
                ticks: 0,
            })
        });
        attach();
        SetTimer(host, TIMER_HEALTH, 2000, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        stop.store(true, Ordering::Relaxed);
        if let Some(t) = measure {
            // höchstens ~3 s auf eine laufende Messung warten
            let t0 = Instant::now();
            while !t.is_finished() && t0.elapsed().as_secs() < 3 {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        let w = with_app(|a| a.widget).unwrap_or_default();
        if !w.is_invalid() {
            let _ = DestroyWindow(w);
        }
    }
}

unsafe extern "system" fn plain_wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h, m, w, l)
}

/// Test/Doku: zeichnet die Anzeige mit echten Messwerten in ein unsichtbares Fenster und
/// speichert das Bild (dunkles und helles Design) als BMP.
pub fn render_test(out_dark: &std::path::Path, out_light: &std::path::Path) -> Result<(), String> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hinst = hinstance();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(plain_wndproc),
            hInstance: hinst,
            lpszClassName: w!("TaskPulseRenderTest"),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(WS_EX_LAYERED | WS_EX_TOOLWINDOW, w!("TaskPulseRenderTest"), w!("test"), WS_POPUP, -5000, -5000, 10, 10, None, None, hinst, None)
            .map_err(|e| e.message())?;
        let mut cfg = Config::load();
        if cfg.line_down_mbit <= 0.0 {
            // Für das Bild eine Beispielleitung hinterlegen, damit die %-Spalte sichtbar ist
            cfg.line_down_mbit = 250.0;
            cfg.line_up_mbit = 40.0;
        }
        let mut sensors = BasicSensors::new(&cfg);
        let _ = sensors.sample(&cfg);
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let b = sensors.sample(&cfg);
        let mut r = Renderer::new().map_err(|e| e.message())?;
        let px = cfg.taskbar.font_size_pt * 96.0 / 72.0;
        r.set_font(&cfg.taskbar.font_family, px).map_err(|e| e.message())?;
        let rr = &r;
        let mut m = |t: &str| rr.measure(t);
        let (cols, width) = layout::layout(&cfg, &mut m, 12.0, 4.0);
        let line_h = (px * 1.33).ceil();
        let h = 48;
        let top = ((h as f32 - line_h * 2.0) / 2.0).floor();
        let mut items = Vec::new();
        for c in &cols {
            for (row, f) in c.fields.iter().enumerate() {
                let cell = layout::cell(*f, &b, &cfg);
                let y = top + row as f32 * line_h;
                items.push(TextItem { x: c.x, y, text: cell.text });
                if let (Some(ex), Some(t)) = (c.extra_x, cell.extra) {
                    items.push(TextItem { x: ex, y, text: t });
                }
            }
        }
        let w = width.ceil() as i32;
        let seps = separator_rects(&cols, 0.0, top, line_h, 1.0);
        r.draw(hwnd, w, h, line_h, &items, &seps, color(1.0, 1.0, 1.0, 1.0), color(0.0, 0.0, 0.0, 1.0 / 255.0)).map_err(|e| e.message())?;
        r.save_bmp(out_dark, [32, 32, 32]).map_err(|e| e.to_string())?;
        r.draw(hwnd, w, h, line_h, &items, &seps, color(0.0, 0.0, 0.0, 1.0), color(1.0, 1.0, 1.0, 1.0 / 255.0)).map_err(|e| e.message())?;
        r.save_bmp(out_light, [238, 238, 238]).map_err(|e| e.to_string())?;
        let _ = DestroyWindow(hwnd);
    }
    Ok(())
}

/// Diagnose: beschreibt die gefundenen Taskleisten und ihre Kindfenster.
pub fn describe_taskbars() -> String {
    let mut out = String::new();
    for (i, tb) in taskbars().iter().enumerate() {
        let r = rect_of(*tb);
        out += &format!("Taskleiste {i}: {:?} x={} y={} w={} h={} dpi={}\n", tb.0, r.left, r.top, r.right - r.left, r.bottom - r.top, unsafe {
            GetDpiForWindow(*tb)
        });
        unsafe {
            let mut after = HWND::default();
            while let Ok(c) = FindWindowExW(*tb, after, None, None) {
                if c.is_invalid() {
                    break;
                }
                let mut cls = [0u16; 128];
                let n = GetClassNameW(c, &mut cls);
                let cr = rect_of(c);
                out += &format!(
                    "   {} x={} w={} sichtbar={}\n",
                    String::from_utf16_lossy(&cls[..n as usize]),
                    cr.left,
                    cr.right - cr.left,
                    IsWindowVisible(c).as_bool()
                );
                after = c;
            }
        }
    }
    out
}
