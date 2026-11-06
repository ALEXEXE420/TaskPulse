//! GPU-Werte pro Prozess über die herstellerunabhängigen Windows-Leistungsindikatoren
//! "GPU Engine" und "GPU Process Memory" (wie der Task-Manager), plus Adapterliste über DXGI.

#[cfg(windows)]
use super::pdh::{Counter, Query};
use std::collections::HashMap;
#[cfg(windows)]
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIDevice, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EngineKey {
    pub luid: String,
    pub phys: u32,
    pub eng: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EngineInstance {
    pub pid: u32,
    pub key: EngineKey,
    pub engtype: String,
}

/// "pid_1234_luid_0x00000000_0x0000C44E_phys_0_eng_3_engtype_VideoDecode"
pub fn parse_engine_instance(name: &str) -> Option<EngineInstance> {
    let pid: u32 = between(name, "pid_", "_")?.parse().ok()?;
    let luid = luid_of(name)?;
    let phys: u32 = between(name, "_phys_", "_")?.parse().ok()?;
    let eng: u32 = between(name, "_eng_", "_")?.parse().ok()?;
    let engtype = name.split("_engtype_").nth(1).unwrap_or("").to_string();
    Some(EngineInstance { pid, key: EngineKey { luid, phys, eng }, engtype })
}

/// "pid_1234_luid_0x00000000_0x0000C44E_phys_0" -> (1234, luid)
pub fn parse_memory_instance(name: &str) -> Option<(u32, String)> {
    let pid: u32 = between(name, "pid_", "_")?.parse().ok()?;
    Some((pid, luid_of(name)?))
}

/// "luid_0x00000000_0x0000D1B5_phys_0" (GPU Adapter Memory) -> luid
pub fn parse_memory_luid(name: &str) -> Option<String> {
    luid_of(name)
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = s.find(start)? + start.len();
    let rest = &s[i..];
    Some(match rest.find(end) {
        Some(j) => &rest[..j],
        None => rest,
    })
}

fn luid_of(name: &str) -> Option<String> {
    let i = name.find("luid_")? + 5;
    let rest = &name[i..];
    // zwei Hex-Teile: 0x00000000_0x0000C44E
    let mut parts = rest.splitn(3, '_');
    let hi = parts.next()?;
    let lo = parts.next()?;
    Some(format!("{}_{}", hi.to_ascii_lowercase(), lo.to_ascii_lowercase()))
}

/// Lesbarer Engine-Name: "VideoDecode" -> "Video Decode"
pub fn pretty_engtype(t: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in t.chars() {
        if c.is_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = c.is_lowercase();
        out.push(c);
    }
    if out.is_empty() {
        "Engine".into()
    } else {
        out.replace('_', " ")
    }
}

#[derive(Debug, Clone, Default)]
pub struct DxgiAdapter {
    pub index: usize,
    pub luid: String,
    pub luid_low: u32,
    pub luid_high: i32,
    pub vendor_id: u32,
    pub name: String,
    pub dedicated: f64,
    /// PCI-Geräte-ID des Grafikchips
    pub device_id: u32,
    /// PCI-Subsystem: untere 16 Bit = Hersteller der Karte (ASUS, MSI …), obere = Kartenmodell
    pub subsys_id: u32,
    pub revision: u32,
    /// Treiberversion wie in den Windows-Geräteeigenschaften, z. B. "32.0.15.6094"
    pub driver: Option<String>,
}

impl DxgiAdapter {
    pub fn vendor_name(&self) -> &'static str {
        match self.vendor_id {
            0x10DE => "NVIDIA",
            0x1002 | 0x1022 => "AMD",
            0x8086 => "Intel",
            0x5143 => "Qualcomm",
            _ => "Unbekannt",
        }
    }

    /// Hersteller der Karte aus der PCI-Subsystem-Kennung (wie in HWiNFO/GPU-Z).
    pub fn board_vendor(&self) -> Option<&'static str> {
        board_vendor(self.subsys_id & 0xFFFF)
    }

    /// "10DE:2C02 · Subsystem 1043:89F4 · Rev A1"
    pub fn pci_id(&self) -> String {
        format!("{:04X}:{:04X} · Subsystem {:04X}:{:04X} · Rev {:02X}", self.vendor_id, self.device_id, self.subsys_id & 0xFFFF, self.subsys_id >> 16, self.revision)
    }

    /// NVIDIA zählt Treiber anders als Windows: 32.0.15.6094 → 560.94
    pub fn driver_display(&self) -> Option<String> {
        let d = self.driver.as_ref()?;
        if self.vendor_id == 0x10DE {
            if let Some(n) = nvidia_driver(d) {
                return Some(format!("{n} ({d})"));
            }
        }
        Some(d.clone())
    }
}

/// Bekannte Kartenhersteller (PCI-SIG-Herstellerkennungen).
pub fn board_vendor(id: u32) -> Option<&'static str> {
    Some(match id {
        0x1043 => "ASUS",
        0x1462 => "MSI",
        0x1458 => "Gigabyte",
        0x3842 => "EVGA",
        0x19DA => "Zotac",
        0x1569 => "Palit",
        0x10B0 => "Gainward",
        0x196E => "PNY",
        0x1DA2 => "Sapphire",
        0x148C => "PowerColor",
        0x1682 => "XFX",
        0x1849 => "ASRock",
        0x1B4C => "KFA2 / Galax",
        0x7377 => "Colorful",
        0x1ACC => "Point of View",
        0x174B => "PC Partner (Sapphire)",
        0x1028 => "Dell",
        0x103C => "HP",
        0x17AA => "Lenovo",
        0x1025 => "Acer",
        0x1D05 => "Tongfang",
        0x1558 => "Clevo",
        0x152D => "Quanta",
        0x10DE => "NVIDIA (Founders Edition)",
        0x1002 => "AMD (Referenzdesign)",
        0x8086 => "Intel",
        0x1EAE => "XFX",
        0x16F3 => "Inno3D",
        0x1B0A => "Biostar",
        _ => return None,
    })
}

/// Windows-Treiberversion "a.b.c.dddd" → NVIDIA-Zählung: letzte Stelle von c + d → "560.94".
pub fn nvidia_driver(win: &str) -> Option<String> {
    let p: Vec<&str> = win.split('.').collect();
    if p.len() != 4 {
        return None;
    }
    let digits = format!("{}{:0>4}", p[2].chars().last()?, p[3]);
    (digits.len() == 5).then(|| format!("{}.{}", &digits[..3], &digits[3..]))
}

#[cfg(windows)]
pub fn dxgi_adapters() -> Vec<DxgiAdapter> {
    let mut out: Vec<DxgiAdapter> = Vec::new();
    unsafe {
        let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
        let mut i = 0;
        while let Ok(a) = f.EnumAdapters1(i) {
            i += 1;
            let Ok(d) = a.GetDesc1() else { continue };
            if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let name = String::from_utf16_lossy(&d.Description[..d.Description.iter().position(|c| *c == 0).unwrap_or(128)]);
            let luid = format!("0x{:08x}_0x{:08x}", d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart);
            if out.iter().any(|x| x.luid == luid) {
                continue;
            }
            // Treiberversion: CheckInterfaceSupport(IDXGIDevice) liefert die Version des Benutzermodus-Treibers
            let driver = a.CheckInterfaceSupport(&<IDXGIDevice as windows::core::Interface>::IID).ok().map(|v| {
                let v = v as u64;
                format!("{}.{}.{}.{}", v >> 48, (v >> 32) & 0xFFFF, (v >> 16) & 0xFFFF, v & 0xFFFF)
            });
            out.push(DxgiAdapter {
                index: out.len(),
                luid,
                luid_low: d.AdapterLuid.LowPart,
                luid_high: d.AdapterLuid.HighPart,
                vendor_id: d.VendorId,
                name,
                dedicated: d.DedicatedVideoMemory as f64,
                device_id: d.DeviceId,
                subsys_id: d.SubSysId,
                revision: d.Revision,
                driver,
            });
        }
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct ProcGpu {
    /// Auslastung je Engine (für die Gruppenberechnung).
    pub engines: Vec<(EngineKey, f64)>,
    pub max_pct: f64,
    pub engine_label: String,
    pub dedicated: f64,
    pub shared: f64,
    /// Windows meldete einen unmöglichen VRAM-Wert (größer als der Grafikspeicher der Karte).
    pub vram_implausible: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GpuProcSample {
    pub per_pid: HashMap<u32, ProcGpu>,
    /// Gesamtauslastung wie im Task-Manager: höchste Engine (Summe über Prozesse).
    pub total_pct: Option<f64>,
    /// Auslastung je Engine-Typ (gesamt) für die Leistungsseite.
    pub by_engtype: Vec<(String, f64)>,
    pub adapter_dedicated_used: Option<f64>,
    pub adapter_shared_used: Option<f64>,
}

#[cfg(windows)]
struct Counters {
    q: Query,
    engine: Option<Counter>,
    ded: Option<Counter>,
    shared: Option<Counter>,
    ad_ded: Option<Counter>,
    ad_shared: Option<Counter>,
}

#[cfg(windows)]
impl Counters {
    fn build() -> Option<Counters> {
        let q = Query::new()?;
        let engine = q.add(r"\GPU Engine(*)\Utilization Percentage");
        let ded = q.add(r"\GPU Process Memory(*)\Dedicated Usage");
        let shared = q.add(r"\GPU Process Memory(*)\Shared Usage");
        let ad_ded = q.add(r"\GPU Adapter Memory(*)\Dedicated Usage");
        let ad_shared = q.add(r"\GPU Adapter Memory(*)\Shared Usage");
        q.collect();
        Some(Counters { q, engine, ded, shared, ad_ded, ad_shared })
    }
}

#[cfg(windows)]
pub struct GpuProc {
    c: Counters,
    pending: Option<Counters>,
    pub adapters: Vec<DxgiAdapter>,
    since_rebuild: u32,
}

#[cfg(windows)]
impl GpuProc {
    pub fn new() -> Option<GpuProc> {
        Some(GpuProc { c: Counters::build()?, pending: None, adapters: dxgi_adapters(), since_rebuild: 0 })
    }

    fn adapter_memory(&self, luid: &str) -> Option<f64> {
        self.adapters.iter().find(|a| a.luid == luid).map(|a| a.dedicated).filter(|d| *d > 0.0)
    }

    fn gpu_index(&self, luid: &str) -> usize {
        self.adapters.iter().find(|a| a.luid == luid).map(|a| a.index).unwrap_or(0)
    }

    pub fn sample(&mut self) -> GpuProcSample {
        // Platzhalter-Instanzen regelmäßig neu auflösen, damit neue Prozesse sicher erscheinen.
        // Die neue Abfrage sammelt einen Takt vor, damit ihre erste Rate ein volles Intervall umfasst.
        if let Some(p) = self.pending.take() {
            self.c = p;
        }
        self.since_rebuild += 1;
        if self.since_rebuild >= 15 {
            self.since_rebuild = 0;
            self.pending = Counters::build();
        }
        let c = &self.c;
        if !c.q.collect() {
            return GpuProcSample::default();
        }
        let mut out = GpuProcSample::default();
        let mut engine_totals: HashMap<EngineKey, (f64, String)> = HashMap::new();
        if let Some(e) = c.engine {
            for (name, v) in c.q.array(e) {
                let Some(inst) = parse_engine_instance(&name) else { continue };
                let v = v.max(0.0);
                let t = engine_totals.entry(inst.key.clone()).or_insert((0.0, inst.engtype.clone()));
                t.0 += v;
                let p = out.per_pid.entry(inst.pid).or_default();
                if let Some(x) = p.engines.iter_mut().find(|(k, _)| *k == inst.key) {
                    x.1 += v;
                } else {
                    p.engines.push((inst.key, v));
                }
            }
            for p in out.per_pid.values_mut() {
                let best = p.engines.iter().filter(|(_, v)| *v > 0.0).max_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((k, v)) = best {
                    p.max_pct = v.min(100.0);
                    let t = engine_totals.get(k).map(|x| x.1.as_str()).unwrap_or("");
                    p.engine_label = format!("GPU {} – {}", self.gpu_index(&k.luid), pretty_engtype(t));
                }
            }
            out.total_pct = engine_totals.values().map(|(v, _)| *v).reduce(f64::max).map(|v| v.min(100.0));
            let mut by_type: HashMap<String, f64> = HashMap::new();
            for (v, t) in engine_totals.values() {
                let e = by_type.entry(pretty_engtype(t)).or_insert(0.0);
                *e = e.max(*v);
            }
            let mut bt: Vec<(String, f64)> = by_type.into_iter().map(|(k, v)| (k, v.min(100.0))).collect();
            bt.sort_by(|a, b| a.0.cmp(&b.0));
            out.by_engtype = bt;
        }
        if let Some(e) = c.ded {
            for (name, v) in c.q.array(e) {
                if let Some((pid, luid)) = parse_memory_instance(&name) {
                    let p = out.per_pid.entry(pid).or_default();
                    if plausible(v, self.adapter_memory(&luid)) {
                        p.dedicated += v.max(0.0);
                    } else {
                        p.vram_implausible = true;
                    }
                }
            }
        }
        if let Some(e) = c.shared {
            for (name, v) in c.q.array(e) {
                if let Some((pid, _)) = parse_memory_instance(&name) {
                    out.per_pid.entry(pid).or_default().shared += v.max(0.0);
                }
            }
        }
        out.adapter_dedicated_used = c.ad_ded.map(|e| c.q.array(e).iter().map(|(_, v)| *v).sum());
        out.adapter_shared_used = c.ad_shared.map(|e| c.q.array(e).iter().map(|(_, v)| *v).sum());
        out
    }
}

/// Ein Prozess kann nicht mehr dedizierten Speicher belegen, als die Karte hat (+5 % Toleranz).
// ------------------------------------------------------------------ Linux: /sys/class/drm, NVML, fdinfo

#[cfg_attr(windows, allow(dead_code))]
/// fdinfo-Einträge eines DRM-Clients (Kernel-Doku „drm-usage-stats“):
/// → (Client-ID, Engine → Nanosekunden, VRAM in Bytes)
pub fn parse_fdinfo(text: &str) -> Option<(u64, Vec<(String, u64)>, f64)> {
    let mut id = None;
    let mut engines = Vec::new();
    let mut vram = 0.0f64;
    let mut is_drm = false;
    for l in text.lines() {
        let Some((k, v)) = l.split_once(':') else { continue };
        let v = v.trim();
        if k == "drm-driver" {
            is_drm = true;
        } else if k == "drm-client-id" {
            id = v.parse().ok();
        } else if let Some(e) = k.strip_prefix("drm-engine-") {
            if !e.starts_with("capacity") {
                if let Some(ns) = v.split_whitespace().next().and_then(|n| n.parse().ok()) {
                    engines.push((e.to_string(), ns));
                }
            }
        } else if k == "drm-memory-vram" || k == "drm-resident-vram0" || k == "drm-total-vram" {
            let mut it = v.split_whitespace();
            let n: f64 = it.next().and_then(|n| n.parse().ok()).unwrap_or(0.0);
            let unit = it.next().unwrap_or("");
            vram = vram.max(match unit {
                "KiB" => n * 1024.0,
                "MiB" => n * 1048576.0,
                _ => n,
            });
        }
    }
    (is_drm).then_some((id?, engines, vram))
}

#[cfg_attr(windows, allow(dead_code))]
/// Linux-Engine-Namen in die Namen des Windows-Task-Managers übersetzen
pub fn linux_engine_name(e: &str) -> String {
    match e {
        "gfx" | "render" | "rcs" => "3D".into(),
        "compute" | "ccs" => "Compute".into(),
        "dec" | "video" | "vcs" => "Video Decode".into(),
        "enc" | "enc_1" | "video-enhance" | "vecs" => "Video Encode".into(),
        "copy" | "dma" | "sdma" | "bcs" => "Copy".into(),
        other => other.to_string(),
    }
}

#[cfg(target_os = "linux")]
pub fn dxgi_adapters() -> Vec<DxgiAdapter> {
    use super::sysfs::{read, read_u64};
    let hex = |p: std::path::PathBuf| read(p).and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok()).unwrap_or(0);
    let nvml = nvml_wrapper::Nvml::init().ok();
    let mut out = Vec::new();
    let mut nv_index = 0u32;
    for card in super::sysfs::dirs("/sys/class/drm") {
        let name = card.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if !name.starts_with("card") || name.contains('-') {
            continue; // card0-DP-1 usw. sind Anschlüsse
        }
        let dev = card.join("device");
        let vendor_id = hex(dev.join("vendor"));
        if vendor_id == 0 {
            continue;
        }
        let driver = std::fs::read_link(dev.join("driver")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_default();
        let mut a = DxgiAdapter {
            index: out.len(),
            luid: name.clone(),
            vendor_id,
            device_id: hex(dev.join("device")),
            subsys_id: hex(dev.join("subsystem_vendor")) | (hex(dev.join("subsystem_device")) << 16),
            revision: hex(dev.join("revision")),
            driver: read(format!("/sys/module/{driver}/version")).map(|v| format!("{driver} {v}")).or_else(|| (!driver.is_empty()).then(|| driver.clone())),
            dedicated: read_u64(dev.join("mem_info_vram_total")).map(|v| v as f64).unwrap_or(0.0),
            ..Default::default()
        };
        if vendor_id == 0x10DE {
            if let Some(d) = nvml.as_ref().and_then(|n| n.device_by_index(nv_index).ok()) {
                a.name = d.name().unwrap_or_default();
                a.dedicated = d.memory_info().map(|m| m.total as f64).unwrap_or(a.dedicated);
            }
            nv_index += 1;
        }
        if a.name.is_empty() {
            // Name aus dem Treiber, sonst Hersteller + Geräte-ID
            a.name = read(dev.join("product_name")).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("{} Grafik ({:04X}:{:04X})", a.vendor_name(), a.vendor_id, a.device_id));
        }
        out.push(a);
    }
    out
}

#[cfg(target_os = "linux")]
pub struct GpuProc {
    pub adapters: Vec<DxgiAdapter>,
    nvml: Option<nvml_wrapper::Nvml>,
    last_ts: u64,
    /// fdinfo: (PID, Client-ID) → (Engine → ns, Zeitpunkt)
    prev: HashMap<(u32, u64), (HashMap<String, u64>, std::time::Instant)>,
    tick: u32,
    cached: GpuProcSample,
}

#[cfg(target_os = "linux")]
impl GpuProc {
    pub fn new() -> Option<GpuProc> {
        let adapters = dxgi_adapters();
        let nvml = adapters.iter().any(|a| a.vendor_id == 0x10DE).then(|| nvml_wrapper::Nvml::init().ok()).flatten();
        Some(GpuProc { adapters, nvml, last_ts: 0, prev: HashMap::new(), tick: 0, cached: GpuProcSample::default() })
    }

    pub fn sample(&mut self) -> GpuProcSample {
        let main = crate::sensors::gpu::auto_adapter(&self.adapters).cloned();
        let Some(a) = main else { return GpuProcSample::default() };
        if a.vendor_id == 0x10DE {
            if let Some(n) = &self.nvml {
                return self.sample_nvml(n, &a);
            }
        }
        // fdinfo ist teuer (alle Prozesse) – nur jede zweite Messung neu, dazwischen das letzte Ergebnis
        self.tick = self.tick.wrapping_add(1);
        if self.tick % 2 == 0 {
            return self.cached.clone();
        }
        let s = self.sample_fdinfo(&a);
        self.cached = s.clone();
        s
    }

    fn sample_nvml(&self, nvml: &nvml_wrapper::Nvml, a: &DxgiAdapter) -> GpuProcSample {
        use nvml_wrapper::enums::device::UsedGpuMemory;
        let mut out = GpuProcSample::default();
        let Ok(d) = nvml.device_by_index(0) else { return out };
        let gpu = d.utilization_rates().ok().map(|u| u.gpu as f64);
        out.total_pct = gpu;
        let enc = d.encoder_utilization().ok().map(|u| u.utilization as f64);
        let dec = d.decoder_utilization().ok().map(|u| u.utilization as f64);
        out.by_engtype = vec![("3D".into(), gpu.unwrap_or(0.0)), ("Video Encode".into(), enc.unwrap_or(0.0)), ("Video Decode".into(), dec.unwrap_or(0.0))];
        out.adapter_dedicated_used = d.memory_info().ok().map(|m| m.used as f64);
        let mut procs: Vec<_> = d.running_graphics_processes().unwrap_or_default();
        procs.extend(d.running_compute_processes().unwrap_or_default());
        for p in procs {
            let e = out.per_pid.entry(p.pid).or_default();
            if let UsedGpuMemory::Used(b) = p.used_gpu_memory {
                e.dedicated = e.dedicated.max(b as f64);
            }
        }
        // Auslastung je Prozess (Stichproben seit 2 s)
        let since = now_us().saturating_sub(2_000_000);
        for u in d.process_utilization_stats(since).unwrap_or_default() {
            let e = out.per_pid.entry(u.pid).or_default();
            let (pct, label) = [(u.sm_util, "3D"), (u.enc_util, "Video Encode"), (u.dec_util, "Video Decode")].into_iter().max_by_key(|x| x.0).unwrap();
            if pct as f64 >= e.max_pct {
                e.max_pct = pct as f64;
                e.engine_label = if pct > 0 { format!("GPU {} – {label}", a.index) } else { String::new() };
            }
        }
        out
    }

    fn sample_fdinfo(&mut self, a: &DxgiAdapter) -> GpuProcSample {
        let mut out = GpuProcSample::default();
        let now = std::time::Instant::now();
        let mut seen = std::collections::HashSet::new();
        let mut by_eng: HashMap<String, f64> = HashMap::new();
        if let Ok(rd) = std::fs::read_dir("/proc") {
            for e in rd.flatten() {
                let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
                let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
                for fd in fds.flatten() {
                    let target = std::fs::read_link(fd.path()).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                    if !target.starts_with("/dev/dri/") {
                        continue;
                    }
                    let Some(name) = fd.file_name().to_str().map(String::from) else { continue };
                    let Ok(info) = std::fs::read_to_string(format!("/proc/{pid}/fdinfo/{name}")) else { continue };
                    let Some((client, engines, vram)) = parse_fdinfo(&info) else { continue };
                    if !seen.insert((pid, client)) {
                        continue; // derselbe Client über mehrere Dateideskriptoren
                    }
                    let cur: HashMap<String, u64> = engines.into_iter().collect();
                    let p = out.per_pid.entry(pid).or_default();
                    p.dedicated += vram;
                    if let Some((old, t0)) = self.prev.get(&(pid, client)) {
                        let dt_ns = now.duration_since(*t0).as_nanos() as f64;
                        for (eng, ns) in &cur {
                            let d = ns.saturating_sub(*old.get(eng).unwrap_or(ns)) as f64;
                            let pct = (d / dt_ns * 100.0).clamp(0.0, 100.0);
                            let label = linux_engine_name(eng);
                            *by_eng.entry(label.clone()).or_insert(0.0) += pct;
                            if pct > p.max_pct {
                                p.max_pct = pct;
                                p.engine_label = if pct >= 0.5 { format!("GPU {} – {label}", a.index) } else { String::new() };
                            }
                        }
                    }
                    self.prev.insert((pid, client), (cur, now));
                }
            }
        }
        self.prev.retain(|k, _| seen.contains(k));
        let dev = std::path::PathBuf::from(format!("/sys/class/drm/{}/device", a.luid));
        out.total_pct = super::sysfs::read_f64(dev.join("gpu_busy_percent")).or_else(|| by_eng.values().copied().reduce(f64::max).map(|v| v.min(100.0)));
        out.adapter_dedicated_used = super::sysfs::read_f64(dev.join("mem_info_vram_used"));
        out.adapter_shared_used = super::sysfs::read_f64(dev.join("mem_info_gtt_used"));
        let mut v: Vec<(String, f64)> = by_eng.into_iter().map(|(k, x)| (k, x.min(100.0))).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        out.by_engtype = v;
        out
    }
}

#[cfg(target_os = "linux")]
fn now_us() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0)
}

pub fn plausible(v: f64, adapter_total: Option<f64>) -> bool {
    match adapter_total {
        Some(t) => v.is_finite() && v <= t * 1.05,
        None => v.is_finite() && v < 1024.0 * 1024.0 * 1024.0 * 1024.0,
    }
}

/// GPU-Wert einer Gruppe: Engines aufsummieren, dann die stärkste nehmen (nicht alle addieren).
pub fn group_gpu_pct<'a>(members: impl Iterator<Item = &'a ProcGpu>) -> f64 {
    let mut sums: HashMap<&EngineKey, f64> = HashMap::new();
    for m in members {
        for (k, v) in &m.engines {
            *sums.entry(k).or_insert(0.0) += *v;
        }
    }
    sums.values().copied().fold(0.0, f64::max).min(100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_fdinfo() {
        let t = "pos:\t0\nflags:\t02100002\ndrm-driver:\tamdgpu\ndrm-client-id:\t42\ndrm-memory-vram:\t2048 KiB\ndrm-engine-gfx:\t1500000 ns\ndrm-engine-dec:\t0 ns\ndrm-engine-capacity-gfx:\t1\n";
        let (id, eng, vram) = parse_fdinfo(t).unwrap();
        assert_eq!(id, 42);
        assert_eq!(eng, vec![("gfx".to_string(), 1500000), ("dec".to_string(), 0)]);
        assert_eq!(vram, 2048.0 * 1024.0);
        assert!(parse_fdinfo("pos: 0\nflags: 1\n").is_none());
        assert_eq!(linux_engine_name("gfx"), "3D");
        assert_eq!(linux_engine_name("enc"), "Video Encode");
    }

    #[test]
    fn engine_instance() {
        let i = parse_engine_instance("pid_1234_luid_0x00000000_0x0000C44E_phys_0_eng_3_engtype_VideoDecode").unwrap();
        assert_eq!(i.pid, 1234);
        assert_eq!(i.key.luid, "0x00000000_0x0000c44e");
        assert_eq!(i.key.phys, 0);
        assert_eq!(i.key.eng, 3);
        assert_eq!(i.engtype, "VideoDecode");
        assert_eq!(pretty_engtype(&i.engtype), "Video Decode");
        assert_eq!(pretty_engtype("3D"), "3D");
        assert_eq!(pretty_engtype("JPEG_Decode_1"), "JPEG Decode 1");
        assert!(parse_engine_instance("_Total").is_none());
    }

    #[test]
    fn memory_instance() {
        assert_eq!(
            parse_memory_instance("pid_42_luid_0x00000000_0x0000D1B5_phys_0"),
            Some((42, "0x00000000_0x0000d1b5".to_string()))
        );
    }

    #[test]
    fn implausible_vram_rejected() {
        let gb = 1024.0 * 1024.0 * 1024.0;
        assert!(plausible(6.4 * gb, Some(16.0 * gb)));
        assert!(!plausible(587.4 * gb, Some(16.0 * gb)));
        assert!(!plausible(f64::NAN, Some(16.0 * gb)));
        assert!(plausible(2.0 * gb, None));
    }

    #[test]
    fn group_takes_strongest_engine_not_sum() {
        let k3d = EngineKey { luid: "a".into(), phys: 0, eng: 0 };
        let kvid = EngineKey { luid: "a".into(), phys: 0, eng: 3 };
        let a = ProcGpu { engines: vec![(k3d.clone(), 30.0), (kvid.clone(), 20.0)], ..Default::default() };
        let b = ProcGpu { engines: vec![(k3d.clone(), 10.0)], ..Default::default() };
        // 3D: 30 + 10 = 40, Video: 20 -> 40 (nicht 60)
        assert_eq!(group_gpu_pct([a, b].iter()), 40.0);
    }
}
