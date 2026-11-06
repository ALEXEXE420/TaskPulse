//! TaskPulse – erweiterter Windows-Task-Manager mit GPU-/VRAM-Werten pro Prozess und
//! dauerhafter Taskleisten-Anzeige.
//!
//! Aufruf:
//!   TaskPulse.exe               Hauptfenster (startet die Taskleisten-Anzeige mit, falls aktiviert)
//!   TaskPulse-Setup.exe / --install   Installationsfenster
//!   TaskPulse.exe --uninstall   Deinstallation
//!   TaskPulse-Setup.exe --update [--reopen]   stilles Update der installierten Version
//!   TaskPulse.exe --taskbar     nur Taskleisten-Anzeige (Autostart)
//!   TaskPulse.exe --settings    Hauptfenster auf der Einstellungsseite
//!   TaskPulse.exe --page leistung|gpu|prozesse|autostart|benchmark|einstellungen
//!   TaskPulse.exe --dump DATEI  einen Messdurchlauf als JSON schreiben (Tests)
//!   TaskPulse.exe --taskbar-info DATEI   gefundene Taskleisten beschreiben (Diagnose)
//!   TaskPulse.exe --render-test BASIS   Anzeige als BASIS_dunkel.bmp / BASIS_hell.bmp speichern (Tests)

#![cfg_attr(not(test), windows_subsystem = "windows")]
// Unter Linux bleiben die reinen Windows-Hilfsfunktionen (Registerformate u. Ä.) ungenutzt
#![cfg_attr(not(windows), allow(dead_code))]

// Windows-gebundene Module haben unter Linux eigene Dateien mit denselben Schnittstellen (src/linux/)
#[cfg(windows)]
mod autostart;
#[cfg(target_os = "linux")]
#[path = "linux/autostart.rs"]
mod autostart;
mod config;
mod diag;
mod format;
#[cfg(windows)]
mod installer;
#[cfg(target_os = "linux")]
#[path = "linux/installer.rs"]
mod installer;
mod sensors;
#[cfg(windows)]
mod taskbar;
#[cfg(target_os = "linux")]
#[path = "linux/taskbar.rs"]
mod taskbar;
mod ui;
mod updater;
#[cfg(windows)]
mod winutil;
#[cfg(target_os = "linux")]
#[path = "linux/winutil.rs"]
mod winutil;

use config::Config;

fn dump(path: Option<&str>) {
    use sensors::gpu_proc::GpuProc;
    use sensors::BasicSensors;
    let cfg = Config::load();
    let mut basic = BasicSensors::new(&cfg);
    let mut gpu = GpuProc::new();
    let mut procs = sensors::procs::Procs::new();
    let mut disk = sensors::mem::Disk::new();
    // Zwei Durchläufe: Raten brauchen zwei Messpunkte
    let _ = basic.sample(&cfg);
    let _ = gpu.as_mut().map(|g| g.sample());
    let _ = procs.sample();
    std::thread::sleep(std::time::Duration::from_millis(1000));
    let t0 = std::time::Instant::now();
    let b = basic.sample(&cfg);
    let t_basic = t0.elapsed().as_secs_f64() * 1000.0;
    let t1 = std::time::Instant::now();
    let g = gpu.as_mut().map(|g| g.sample()).unwrap_or_default();
    let t_gpu = t1.elapsed().as_secs_f64() * 1000.0;
    let t2 = std::time::Instant::now();
    let p = procs.sample();
    let t_procs = t2.elapsed().as_secs_f64() * 1000.0;
    let d = disk.as_mut().map(|d| d.sample()).unwrap_or_default();
    // Herstellerunabhängiger Windows-Weg zum Vergleich (bei NVIDIA gegen NVML)
    let mut generic = sensors::gpu::Gpu::new_windows_only(&cfg.gpu_adapter);
    let _ = generic.sample();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let gw = generic.sample();
    let mut top: Vec<serde_json::Value> = p
        .iter()
        .map(|x| {
            let gp = g.per_pid.get(&x.pid);
            serde_json::json!({
                "pid": x.pid, "image": x.image, "name": x.display, "category": x.category,
                "cpu_pct": x.cpu_pct, "mem_private": x.mem_private,
                "gpu_pct": gp.map(|v| v.max_pct).unwrap_or(0.0),
                "gpu_engine": gp.map(|v| v.engine_label.clone()).unwrap_or_default(),
                "vram_dedicated": gp.map(|v| v.dedicated).unwrap_or(0.0),
            })
        })
        .collect();
    top.sort_by(|a, b| b["vram_dedicated"].as_f64().unwrap_or(0.0).total_cmp(&a["vram_dedicated"].as_f64().unwrap_or(0.0)));
    top.truncate(15);
    let cells: Vec<String> =
        cfg.taskbar.visible_fields().iter().map(|f| taskbar::layout::cell(*f, &b, &cfg)).map(|c| format!("{}{}", c.text, c.extra.map(|e| format!(" {e}")).unwrap_or_default())).collect();
    let ci = sensors::cputemp::cpu_info();
    let cpu_groups: Vec<serde_json::Value> = sensors::topology::groups(&sensors::topology::query(), ci.vendor.contains("AMD") && !winutil::native_arm64(), ci.family)
        .iter()
        .map(|g| serde_json::json!({"title": g.title, "detail": g.detail, "lps": g.lps}))
        .collect();
    let gpu_spec = {
        let list = sensors::gpu_proc::dxgi_adapters();
        sensors::gpu::auto_adapter(&list).map(|a| {
            let nv = sensors::nvgpu::NvGpu::new().filter(|_| a.vendor_id == 0x10DE).map(|n| n.spec());
            sensors::spec::gpu_spec(a, nv.as_ref(), &sensors::spec::pnp_info(a), None, None)
        })
    };
    let cpu_spec = sensors::spec::cpu_spec(b.cpu.nominal_mhz);
    let v = serde_json::json!({
        "elevated": winutil::is_elevated(),
        "cpu_brand": sensors::cputemp::cpu_info().brand,
        "arm64": winutil::native_arm64(),
        "cpu_groups": cpu_groups,
        "basic": b,
        "taskbar_text": cells,
        "disk": d,
        "dimms": sensors::dimm::modules(),
        "startup": sensors::startup::list(),
        "gpu_spec": gpu_spec,
        "cpu_spec": cpu_spec,
        "gpu_windows_generic": gw,
        "gpus": sensors::gpu_proc::dxgi_adapters().iter().map(|a| serde_json::json!({"name": a.name, "vendor": a.vendor_name(), "dedicated_gb": a.dedicated / 1073741824.0, "luid": a.luid})).collect::<Vec<_>>(),
        "gpu_total_pct_counters": g.total_pct,
        "gpu_by_engine": g.by_engtype,
        "adapter_dedicated_used": g.adapter_dedicated_used,
        "process_count": p.len(),
        "top_vram_processes": top,
        "timing_ms": { "basic": t_basic, "gpu_counters": t_gpu, "processes": t_procs },
    });
    let s = serde_json::to_string_pretty(&v).unwrap_or_default();
    match path {
        Some(p) => {
            let _ = std::fs::write(p, s);
        }
        None => println!("{s}"),
    }
}

/// Datei, über die eine normal gestartete Instanz ihre Argumente an die erhöhte übergibt.
fn launch_args_file() -> std::path::PathBuf {
    Config::dir().join("launch_args.txt")
}

fn main() {
    // Fehlerberichte bei Programmfehlern und Abstürzen (Ordner %APPDATA%\TaskPulse\Fehlerberichte)
    diag::install_handlers();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--test-fehler") {
        // Test: absichtlicher Programmfehler – muss einen Fehlerbericht erzeugen
        diag::log("Test-Fehler ausgelöst (--test-fehler)");
        let v: Vec<u32> = Vec::new();
        let _ = v[std::hint::black_box(3)];
    }
    if args.iter().any(|a| a == "--test-absturz") {
        // Test: absichtlicher harter Absturz (Speicherzugriffsfehler) – muss einen Absturzbericht erzeugen
        // (Windows: sofort; Linux: kleine Datei sofort, vollständiger Bericht beim nächsten Start)
        diag::log("Test-Absturz ausgelöst (--test-absturz)");
        unsafe {
            let p: *mut u32 = std::hint::black_box(std::ptr::null_mut());
            std::ptr::write_volatile(p, 1);
        }
    }
    if args.iter().any(|a| a == "--elevated-launch") {
        // Über die Admin-Aufgabe gestartet: ursprüngliche Argumente übernehmen
        let f = launch_args_file();
        args = std::fs::read_to_string(&f).map(|s| s.lines().filter(|l| !l.is_empty()).map(String::from).collect()).unwrap_or_default();
        let _ = std::fs::remove_file(f);
    }
    let args = args;
    // Update aus TaskPulse heraus: heruntergeladener Installer, ohne Fenster
    if args.iter().any(|a| a == "--update") {
        installer::run_update(args.iter().any(|a| a == "--reopen"));
        return;
    }
    // Installer: "TaskPulse-Setup.exe" oder --install; Deinstallation über --uninstall
    if args.iter().any(|a| a == "--uninstall") {
        installer::run(true);
        return;
    }
    if args.iter().any(|a| a == "--install") || (installer::is_setup_exe() && args.iter().all(|a| a == "--preview")) {
        installer::run(false);
        return;
    }
    let has = |a: &str| args.iter().any(|x| x == a);
    let value_after = |a: &str| args.iter().position(|x| x == a).and_then(|i| args.get(i + 1)).map(|s| s.as_str());

    if has("--dump") {
        dump(value_after("--dump"));
        return;
    }
    if has("--update-check") {
        // Test: neueste Veröffentlichung eines Repositorys abfragen ("Besitzer/Name" DATEI)
        let repo = value_after("--update-check").unwrap_or("");
        let out = args.iter().position(|x| x == "--update-check").and_then(|i| args.get(i + 2)).cloned().unwrap_or("update_check.txt".into());
        let s = match updater::latest(repo) {
            Ok(r) => format!("version={}
newer_than_{}={}
asset={} ({} Byte)
page={}
", r.version, updater::CURRENT, updater::is_newer(&r.version, updater::CURRENT), r.asset_url, r.asset_size, r.page_url),
            Err(e) => format!("Fehler: {e}
"),
        };
        let _ = std::fs::write(&out, s);
        // --soak N: danach N Sekunden weiterlaufen (Absturz-Test: Timer von WinHTTP feuern nachträglich)
        if let Some(n) = value_after("--soak").and_then(|v| v.parse::<u64>().ok()) {
            std::thread::sleep(std::time::Duration::from_secs(n));
            let _ = std::fs::write(format!("{out}.soak"), "überlebt");
        }
        return;
    }
    if has("--http-test") {
        // Test: Download über Weiterleitung (GitHub-Anhänge liegen auf einem anderen Server)
        let url = value_after("--http-test").unwrap_or("");
        let s = match updater::http_get(url, "application/octet-stream", 200 << 20) {
            Ok(d) => format!("ok {} Byte, Anfang {:02x?}
", d.len(), &d[..d.len().min(4)]),
            Err(e) => format!("Fehler: {e}
"),
        };
        let _ = std::fs::write(Config::dir().join("http_test.txt"), s);
        return;
    }
    if has("--test-speichern") {
        // Test: „Speichern unter“-Dialog des Benchmarks öffnen (Bildschirmfoto)
        let docs = std::env::var_os("USERPROFILE").map(|p| std::path::PathBuf::from(p).join("Documents"));
        let r = winutil::save_file_dialog(
            "Benchmark-Log speichern",
            "TaskPulse-Benchmark-Beispielspiel-2026-11-06_14-03-22.txt",
            docs.as_deref(),
            &[("Textdatei (*.txt)", "*.txt"), ("Markdown (*.md)", "*.md"), ("Alle Dateien (*.*)", "*.*")],
            "txt",
        );
        diag::log(format!("Test Speichern-Dialog: {r:?}"));
        return;
    }
    if has("--autostart-selbsttest") {
        // Hinzufügen → ausschalten → einschalten → entfernen, mit einem Testeintrag (nur aktueller Benutzer)
        let name = "TaskPulse Selbsttest";
        let exe = if cfg!(windows) { std::env::var("WINDIR").unwrap_or(r"C:\Windows".into()) + r"\notepad.exe" } else { "/bin/true".into() };
        let find = || sensors::startup::list().into_iter().find(|e| e.name == name);
        let mut log = Vec::new();
        log.push(format!("hinzufügen: {:?}", sensors::startup::add(name, &exe, "--test")));
        let e = find();
        log.push(format!("gefunden: {}, an: {:?}", e.is_some(), e.as_ref().map(|e| e.enabled)));
        if let Some(e) = e {
            log.push(format!("ausschalten: {:?} → an: {:?}", sensors::startup::set_enabled(&e, false), find().map(|e| e.enabled)));
            log.push(format!("einschalten: {:?} → an: {:?}", sensors::startup::set_enabled(&e, true), find().map(|e| e.enabled)));
            log.push(format!("entfernen: {:?} → noch da: {}", sensors::startup::remove(&e), find().is_some()));
        }
        log.push(format!("doppelt hinzufügen abgelehnt: {}", {
            let _ = sensors::startup::add(name, &exe, "");
            let second = sensors::startup::add(name, &exe, "").is_err();
            if let Some(e) = find() {
                let _ = sensors::startup::remove(&e);
            }
            second
        }));
        log.push(format!("aufgeräumt: {}", find().is_none()));
        let _ = std::fs::write(value_after("--autostart-selbsttest").unwrap_or("autostart_test.txt"), log.join("\n"));
        return;
    }
    if has("--diagnose") {
        // Diagnosebericht ohne Hauptfenster erstellen (z. B. wenn TaskPulse nicht mehr startet)
        if let Some(p) = diag::diagnose("") {
            if !has("--still") {
                winutil::open_in_explorer(&p.to_string_lossy());
            }
        }
        return;
    }
    if has("--taskbar-info") {
        let s = taskbar::describe_taskbars();
        match value_after("--taskbar-info") {
            Some(p) => {
                let _ = std::fs::write(p, s);
            }
            None => println!("{s}"),
        }
        return;
    }
    #[cfg(windows)]
    if has("--mem-probe") {
        // Speicherbedarf der Bausteine der Taskleisten-Anzeige messen
        let mut out = String::new();
        let mut step = |name: &str| {
            let (p, w) = winutil::own_memory();
            out += &format!("{name:<28} privat {:>6.1} MB  Arbeitssatz {:>6.1} MB
", p / 1048576.0, w / 1048576.0);
        };
        step("Start");
        let cfg = Config::load();
        let c = sensors::cpu::Cpu::new();
        step("+ CPU-Zähler (PDH)");
        let n = sensors::nvgpu::NvGpu::new();
        step("+ NVML");
        let t = sensors::cputemp::CpuTemp::open().ok();
        step("+ PawnIO");
        let mut b = sensors::BasicSensors::new(&cfg);
        let _ = b.sample(&cfg);
        step("+ Messkern komplett");
        let r = taskbar::render::Renderer::new().ok();
        step("+ Direct2D/DirectWrite");
        winutil::trim_working_set();
        step("nach EmptyWorkingSet");
        std::thread::sleep(std::time::Duration::from_secs(3));
        for _ in 0..3 {
            let _ = b.sample(&cfg);
            std::thread::sleep(std::time::Duration::from_millis(1000));
        }
        step("nach 3 Messungen");
        drop((c, n, t, r));
        let _ = std::fs::write(value_after("--mem-probe").unwrap_or("mem_probe.txt"), out);
        return;
    }
    if has("--render-test") {
        let base = value_after("--render-test").unwrap_or("taskleiste");
        let r = taskbar::render_test(std::path::Path::new(&format!("{base}_dunkel.bmp")), std::path::Path::new(&format!("{base}_hell.bmp")));
        if let Err(e) = r {
            let _ = std::fs::write(format!("{base}_fehler.txt"), e);
        }
        return;
    }
    if has("--admin-task") {
        // Test/Skripte: Aufgabe "TaskPulse Admin" anlegen oder entfernen; Ergebnis in Datei
        let r = match value_after("--admin-task") {
            Some("create") => autostart::admin_enable(),
            Some("delete") => autostart::admin_disable(),
            _ => Err("create oder delete angeben".into()),
        };
        let _ = std::fs::write(Config::dir().join("admin_task_result.txt"), format!("{r:?}"));
        return;
    }
    if has("--taskbar") {
        taskbar::run();
        winutil::fast_exit(0);
    }

    // Neustart aus dem Fenster heraus: warten, bis das alte Fenster zu ist
    if has("--restart") {
        for _ in 0..40 {
            if winutil::find_own_window(winutil::MAIN_TITLE).is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    // Hauptfenster: läuft es schon, nach vorn holen
    let settings = has("--settings");
    if let Some(h) = winutil::find_own_window(winutil::MAIN_TITLE) {
        winutil::bring_to_front(h);
        if settings {
            let _ = std::fs::write(Config::dir().join("open_settings.signal"), b"1");
        }
        return;
    }
    // "Immer mit Adminrechten": ohne UAC-Abfrage über die geplante Aufgabe neu starten
    let cfg = Config::load();
    if cfg.always_admin && !winutil::is_elevated() && !has("--no-elevate") {
        let _ = std::fs::create_dir_all(Config::dir());
        let _ = std::fs::write(launch_args_file(), args.join("\n"));
        if autostart::run_admin_task() {
            return;
        }
        let _ = std::fs::remove_file(launch_args_file());
    }
    if cfg.taskbar.enabled {
        if !ui::taskbar_running() {
            winutil::spawn_self(&["--taskbar"]);
        } else if winutil::is_elevated() {
            // Läuft die Taskleisten-Anzeige noch ohne Adminrechte, durch eine erhöhte ersetzen
            std::thread::spawn(taskbar::request_elevated_restart);
        }
    }
    let page = if settings {
        Some(ui::Tab::Settings)
    } else {
        match value_after("--page") {
            Some("leistung") | Some("gpu") => Some(ui::Tab::Performance),
            Some("prozesse") => Some(ui::Tab::Processes),
            Some("benchmark") => Some(ui::Tab::Benchmark),
            Some("autostart") => Some(ui::Tab::Startup),
            Some("einstellungen") => Some(ui::Tab::Settings),
            _ => None,
        }
    };
    let _ = ui::run(page);
    winutil::fast_exit(0);
}
