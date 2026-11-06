//! Grafikkarte für Taskleiste und Kopfzeilen – herstellerunabhängig.
//!
//! - NVIDIA: NVML (Temperatur, Auslastung, Takt, Speicher, Leistung in Watt).
//! - AMD, Intel und andere: Temperatur und Takt über D3DKMTQueryAdapterInfo
//!   (ADAPTERPERFDATA/NODEPERFDATA, dieselbe Quelle wie der Task-Manager, ab Windows 10 2004
//!   mit WDDM-2.7-Treibern), Auslastung und belegter VRAM über die Leistungsindikatoren
//!   "GPU Engine" und "GPU Adapter Memory", Gesamt-VRAM über DXGI. Watt gibt es dort nicht.

use super::gpu_proc::{dxgi_adapters, parse_engine_instance, parse_memory_luid, DxgiAdapter, EngineKey};
use super::nvgpu::{GpuSample, NvGpu};
use super::pdh::{Counter, Query};
use std::collections::HashMap;
use std::ffi::c_void;
use windows::core::{s, w};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

const NVIDIA: u32 = 0x10DE;
const KMTQAITYPE_ADAPTERPERFDATA: u32 = 62;
const KMTQAITYPE_NODEPERFDATA: u32 = 63;

#[repr(C)]
struct OpenAdapterFromLuid {
    luid_low: u32,
    luid_high: i32,
    h_adapter: u32,
}

#[repr(C)]
struct QueryAdapterInfo {
    h_adapter: u32,
    ty: u32,
    data: *mut c_void,
    size: u32,
}

#[repr(C)]
struct CloseAdapter {
    h_adapter: u32,
}

#[repr(C)]
#[derive(Default)]
struct AdapterPerfData {
    physical_adapter_index: u32,
    memory_frequency: u64,
    max_memory_frequency: u64,
    max_memory_frequency_oc: u64,
    memory_bandwidth: u64,
    pcie_bandwidth: u64,
    fan_rpm: u32,
    power: u32,
    /// Zehntel Grad Celsius
    temperature: u32,
    power_state_override: u8,
}

#[repr(C)]
#[derive(Default)]
struct NodePerfData {
    node_ordinal: u32,
    physical_adapter_index: u32,
    /// Hz
    frequency: u64,
    max_frequency: u64,
    max_frequency_oc: u64,
    voltage: u32,
    voltage_max: u32,
    voltage_max_oc: u32,
    max_transition_latency: u64,
}

type FnOpen = unsafe extern "system" fn(*mut OpenAdapterFromLuid) -> i32;
type FnQuery = unsafe extern "system" fn(*const QueryAdapterInfo) -> i32;
type FnClose = unsafe extern "system" fn(*const CloseAdapter) -> i32;

/// Zugriff auf einen Adapter über D3DKMT (gdi32.dll).
pub struct Kmt {
    query: FnQuery,
    close: FnClose,
    h: u32,
}

unsafe impl Send for Kmt {}

impl Kmt {
    pub fn open(a: &DxgiAdapter) -> Option<Kmt> {
        unsafe {
            let lib = LoadLibraryW(w!("gdi32.dll")).ok()?;
            let open: FnOpen = std::mem::transmute(GetProcAddress(lib, s!("D3DKMTOpenAdapterFromLuid"))?);
            let query: FnQuery = std::mem::transmute(GetProcAddress(lib, s!("D3DKMTQueryAdapterInfo"))?);
            let close: FnClose = std::mem::transmute(GetProcAddress(lib, s!("D3DKMTCloseAdapter"))?);
            let mut o = OpenAdapterFromLuid { luid_low: a.luid_low, luid_high: a.luid_high, h_adapter: 0 };
            (open(&mut o) >= 0).then_some(Kmt { query, close, h: o.h_adapter })
        }
    }

    fn query<T>(&self, ty: u32, data: &mut T) -> bool {
        let q = QueryAdapterInfo { h_adapter: self.h, ty, data: data as *mut T as *mut c_void, size: std::mem::size_of::<T>() as u32 };
        unsafe { (self.query)(&q) >= 0 }
    }

    /// (Temperatur °C, Speichertakt MHz)
    pub fn adapter_perf(&self) -> (Option<f64>, Option<f64>) {
        let mut d = AdapterPerfData::default();
        if !self.query(KMTQAITYPE_ADAPTERPERFDATA, &mut d) {
            return (None, None);
        }
        let t = (d.temperature > 0).then(|| d.temperature as f64 / 10.0).filter(|t| (1.0..150.0).contains(t));
        let m = (d.memory_frequency > 0).then(|| d.memory_frequency as f64 / 1e6);
        (t, m)
    }

    /// Takt des Grafikkerns (Knoten 0 = 3D-Engine) in MHz. Manche Treiber (z. B. NVIDIA)
    /// liefern hier nichts; dann `None`. Erst die Strukturgröße laut Windows-SDK (56 Byte),
    /// dann die kurze Variante (40 Byte), die manche Windows-Versionen erwarten.
    pub fn core_clock(&self) -> Option<f64> {
        for size in [std::mem::size_of::<NodePerfData>() as u32, 40] {
            let mut d = NodePerfData { node_ordinal: 0, ..Default::default() };
            let q = QueryAdapterInfo { h_adapter: self.h, ty: KMTQAITYPE_NODEPERFDATA, data: &mut d as *mut _ as *mut c_void, size };
            if unsafe { (self.query)(&q) } < 0 {
                continue;
            }
            let f = d.frequency as f64 / 1e6;
            let max = d.max_frequency as f64 / 1e6;
            if (50.0..6000.0).contains(&f) && (max == 0.0 || f <= max * 1.1) {
                return Some(f);
            }
        }
        None
    }
}

impl Drop for Kmt {
    fn drop(&mut self) {
        let c = CloseAdapter { h_adapter: self.h };
        unsafe {
            (self.close)(&c);
        }
    }
}

/// Leichte Leistungsindikator-Abfrage für Auslastung und belegten VRAM eines Adapters.
struct LightPdh {
    q: Query,
    engine: Option<Counter>,
    mem: Option<Counter>,
}

impl LightPdh {
    fn new() -> Option<LightPdh> {
        let q = Query::new()?;
        let engine = q.add(r"\GPU Engine(*)\Utilization Percentage");
        let mem = q.add(r"\GPU Adapter Memory(*)\Dedicated Usage");
        q.collect();
        Some(LightPdh { q, engine, mem })
    }

    /// (Auslastung %, belegter VRAM Bytes) für die LUID.
    fn sample(&self, luid: &str) -> (Option<f64>, Option<f64>) {
        if !self.q.collect() {
            return (None, None);
        }
        let load = self.engine.map(|c| {
            let mut sums: HashMap<EngineKey, f64> = HashMap::new();
            for (name, v) in self.q.array(c) {
                if let Some(i) = parse_engine_instance(&name) {
                    if i.key.luid == luid {
                        *sums.entry(i.key).or_insert(0.0) += v.max(0.0);
                    }
                }
            }
            sums.values().copied().fold(0.0, f64::max).min(100.0)
        });
        let mem = self.mem.map(|c| self.q.array(c).iter().filter(|(n, _)| parse_memory_luid(n).as_deref() == Some(luid)).map(|(_, v)| *v).sum());
        (load, mem)
    }
}

/// Automatische Wahl: die Karte mit dem meisten eigenen Grafikspeicher (also meist die
/// dedizierte statt der integrierten).
pub fn auto_adapter(list: &[DxgiAdapter]) -> Option<&DxgiAdapter> {
    list.iter().max_by(|a, b| a.dedicated.total_cmp(&b.dedicated))
}

enum Backend {
    Nvml(NvGpu),
    Windows { kmt: Option<Kmt>, pdh: Option<LightPdh> },
    None,
}

pub struct Gpu {
    wanted: String,
    adapter: Option<DxgiAdapter>,
    backend: Backend,
}

impl Gpu {
    pub fn new(wanted: &str) -> Gpu {
        Self::build(wanted, false)
    }

    /// Erzwingt den herstellerunabhängigen Windows-Weg (Tests: Vergleich mit NVML).
    pub fn new_windows_only(wanted: &str) -> Gpu {
        Self::build(wanted, true)
    }

    fn build(wanted: &str, windows_only: bool) -> Gpu {
        let list = dxgi_adapters();
        let adapter = if wanted.is_empty() { auto_adapter(&list) } else { list.iter().find(|a| a.luid == wanted).or(auto_adapter(&list)) }.cloned();
        let backend = match &adapter {
            Some(a) if a.vendor_id == NVIDIA && !windows_only => match NvGpu::new() {
                Some(n) => Backend::Nvml(n),
                None => Backend::Windows { kmt: Kmt::open(a), pdh: LightPdh::new() },
            },
            Some(a) => Backend::Windows { kmt: Kmt::open(a), pdh: LightPdh::new() },
            // Keine DXGI-Liste (sehr selten): NVML trotzdem versuchen
            None => NvGpu::new().map(Backend::Nvml).unwrap_or(Backend::None),
        };
        Gpu { wanted: wanted.to_string(), adapter, backend }
    }

    pub fn wanted(&self) -> &str {
        &self.wanted
    }

    pub fn sample(&mut self) -> GpuSample {
        match &self.backend {
            Backend::Nvml(n) => n.sample(),
            Backend::Windows { kmt, pdh } => {
                let a = self.adapter.as_ref();
                let (temp_c, mem_clock_mhz) = kmt.as_ref().map(|k| k.adapter_perf()).unwrap_or((None, None));
                let clock_mhz = kmt.as_ref().and_then(|k| k.core_clock());
                let (load_pct, mem_used) = match (pdh, a) {
                    (Some(p), Some(a)) => p.sample(&a.luid),
                    _ => (None, None),
                };
                GpuSample {
                    name: a.map(|a| a.name.clone()),
                    source: Some(format!("Windows (D3DKMT + Leistungsindikatoren), {}", a.map(|a| a.vendor_name()).unwrap_or("?"))),
                    temp_c,
                    load_pct,
                    clock_mhz,
                    mem_clock_mhz,
                    mem_used,
                    mem_total: a.map(|a| a.dedicated).filter(|d| *d > 0.0),
                    power_w: None,
                }
            }
            Backend::None => GpuSample::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_sizes_match_windows_sdk() {
        // Größen aus d3dkmthk.h (x64)
        assert_eq!(std::mem::size_of::<OpenAdapterFromLuid>(), 12);
        assert_eq!(std::mem::size_of::<QueryAdapterInfo>(), 24);
        assert_eq!(std::mem::size_of::<AdapterPerfData>(), 64);
        assert_eq!(std::mem::size_of::<NodePerfData>(), 56);
    }

    fn a(name: &str, ded_gb: f64, vendor: u32) -> DxgiAdapter {
        DxgiAdapter { luid: name.into(), vendor_id: vendor, name: name.into(), dedicated: ded_gb * 1e9, ..Default::default() }
    }

    #[test]
    fn auto_prefers_dedicated_gpu() {
        let list = vec![a("Intel UHD 770", 0.13, 0x8086), a("RTX 4070", 12.0, 0x10DE)];
        assert_eq!(auto_adapter(&list).unwrap().name, "RTX 4070");
        let list = vec![a("Radeon RX 7800 XT", 16.0, 0x1002), a("AMD Radeon(TM) Graphics", 0.5, 0x1002)];
        assert_eq!(auto_adapter(&list).unwrap().name, "Radeon RX 7800 XT");
        assert!(auto_adapter(&[]).is_none());
    }

    #[test]
    fn board_vendor_and_driver() {
        let mut x = a("RTX 4070", 12.0, 0x10DE);
        x.device_id = 0x2786;
        x.subsys_id = 0x8888_1043;
        x.revision = 0xA1;
        x.driver = Some("32.0.15.6094".into());
        assert_eq!(x.board_vendor(), Some("ASUS"));
        assert_eq!(x.pci_id(), "10DE:2786 · Subsystem 1043:8888 · Rev A1");
        assert_eq!(x.driver_display().as_deref(), Some("560.94 (32.0.15.6094)"));
        assert_eq!(crate::sensors::gpu_proc::nvidia_driver("31.0.15.5222").as_deref(), Some("552.22"));
        assert_eq!(crate::sensors::gpu_proc::nvidia_driver("x"), None);
    }

    #[test]
    fn vendor_names() {
        assert_eq!(a("x", 1.0, 0x10DE).vendor_name(), "NVIDIA");
        assert_eq!(a("x", 1.0, 0x1002).vendor_name(), "AMD");
        assert_eq!(a("x", 1.0, 0x8086).vendor_name(), "Intel");
    }
}
