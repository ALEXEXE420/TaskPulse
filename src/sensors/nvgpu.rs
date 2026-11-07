//! NVIDIA-Grafikkarte über NVML (nvml-wrapper): Temperatur, Auslastung, Takt, Speicher.

use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use nvml_wrapper::Nvml;

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GpuSample {
    pub name: Option<String>,
    /// Woher die Werte stammen (NVML oder Windows).
    pub source: Option<String>,
    pub temp_c: Option<f64>,
    pub load_pct: Option<f64>,
    pub clock_mhz: Option<f64>,
    pub mem_clock_mhz: Option<f64>,
    pub mem_used: Option<f64>,
    pub mem_total: Option<f64>,
    pub power_w: Option<f64>,
    /// Heißeste Stelle des Grafikchips (NVIDIA RTX 30/40 über NVAPI, RTX 50 über PawnIO mit Adminrechten)
    pub hotspot_c: Option<f64>,
    /// Heißester Speicherchip (NVAPI bei GDDR6X, sonst PawnIO mit Adminrechten)
    pub mem_temp_c: Option<f64>,
}

impl GpuSample {
    pub fn mem_pct(&self) -> Option<f64> {
        match (self.mem_used, self.mem_total) {
            (Some(u), Some(t)) if t > 0.0 => Some(u / t * 100.0),
            _ => None,
        }
    }
}

pub struct NvGpu {
    nvml: Nvml,
    /// Zweiter Zugriff auf nvml.dll für nvmlDeviceGetMemoryInfo_v2 (rechnet den vom Treiber
    /// reservierten Speicher heraus – so zählt auch nvidia-smi).
    lib2: Option<nvml_wrapper_sys::bindings::NvmlLib>,
    index: u32,
    name: Option<String>,
}

impl NvGpu {
    pub fn new() -> Option<NvGpu> {
        let nvml = Nvml::init().ok()?;
        let count = nvml.device_count().ok()?;
        if count == 0 {
            return None;
        }
        let name = nvml.device_by_index(0).ok().and_then(|d| d.name().ok());
        let lib2 = unsafe { nvml_wrapper_sys::bindings::NvmlLib::new(if cfg!(windows) { "nvml.dll" } else { "libnvidia-ml.so.1" }).ok() };
        Some(NvGpu { nvml, lib2, index: 0, name })
    }

    /// RTX 50 (Blackwell) oder neuer? Dort meldet NVAPI keinen Hot-Spot mehr (Feld 1 = normale GPU-Temperatur).
    pub fn blackwell_or_newer(&self) -> bool {
        self.nvml.device_by_index(self.index).ok().and_then(|d| d.cuda_compute_capability().ok()).map(|c| c.major >= 10).unwrap_or(false)
    }

    /// PCI-Kennung der Karte, z. B. "00000000:0A:00.0"
    pub fn bus_id(&self) -> Option<String> {
        self.nvml.device_by_index(self.index).ok()?.pci_info().ok().map(|p| p.bus_id)
    }

    fn memory_v2(&self, d: &nvml_wrapper::Device) -> Option<(f64, f64)> {
        use nvml_wrapper_sys::bindings::nvmlMemory_v2_t;
        let f = self.lib2.as_ref()?.nvmlDeviceGetMemoryInfo_v2.as_ref().ok()?;
        unsafe {
            let mut m: nvmlMemory_v2_t = std::mem::zeroed();
            m.version = std::mem::size_of::<nvmlMemory_v2_t>() as u32 | (2 << 24);
            (f(d.handle(), &mut m) == 0 && m.total > 0).then_some((m.used as f64, m.total as f64))
        }
    }

    pub fn sample(&self) -> GpuSample {
        let Ok(d) = self.nvml.device_by_index(self.index) else {
            return GpuSample::default();
        };
        let mem = self.memory_v2(&d).or_else(|| d.memory_info().ok().map(|m| (m.used as f64, m.total as f64)));
        GpuSample {
            name: self.name.clone(),
            source: Some("NVIDIA NVML".into()),
            temp_c: d.temperature(TemperatureSensor::Gpu).ok().map(|v| v as f64),
            load_pct: d.utilization_rates().ok().map(|u| u.gpu as f64),
            clock_mhz: d.clock_info(Clock::Graphics).ok().map(|v| v as f64),
            mem_clock_mhz: d.clock_info(Clock::Memory).ok().map(|v| v as f64),
            mem_used: mem.map(|m| m.0),
            mem_total: mem.map(|m| m.1),
            power_w: d.power_usage().ok().map(|mw| mw as f64 / 1000.0),
            ..Default::default()
        }
    }
}

/// Feste Angaben der NVIDIA-Karte für die Spezifikation (wie GPU-Z).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct NvSpec {
    /// NVML-Architekturnummer (10 = Blackwell …)
    pub arch: Option<u32>,
    pub compute: Option<(i32, i32)>,
    pub cores: Option<u32>,
    pub bus_width: Option<u32>,
    pub max_gpu_mhz: Option<u32>,
    pub boost_mhz: Option<u32>,
    pub max_mem_mhz: Option<u32>,
    pub vbios: Option<String>,
    pub part_number: Option<String>,
    pub bar1_total: Option<u64>,
    pub mem_total: Option<u64>,
    pub power_default_w: Option<f64>,
    pub power_min_w: Option<f64>,
    pub power_max_w: Option<f64>,
    pub temp_slowdown: Option<u32>,
    pub temp_shutdown: Option<u32>,
    pub fans: Option<u32>,
    pub driver: Option<String>,
    /// z. B. 13000 = CUDA 13.0
    pub cuda: Option<i32>,
    /// Aktuelle und höchste mit diesem Rechner mögliche PCIe-Verbindung (Generation, Breite)
    pub pcie_cur: Option<(u32, u32)>,
    pub pcie_host_max: Option<(u32, u32)>,
}

/// NVML-Architekturnummer → Name
pub fn arch_name(arch: Option<u32>, compute: Option<(i32, i32)>) -> Option<&'static str> {
    let by_arch = match arch? {
        2 => Some("Kepler"),
        3 => Some("Maxwell"),
        4 => Some("Pascal"),
        5 => Some("Volta"),
        6 => Some("Turing"),
        7 => Some("Ampere"),
        8 => Some("Ada Lovelace"),
        9 => Some("Hopper"),
        10 => Some("Blackwell"),
        _ => None,
    };
    by_arch.or_else(|| match compute? {
        (10, _) | (12, _) => Some("Blackwell"),
        (9, _) => Some("Hopper"),
        (8, 9) => Some("Ada Lovelace"),
        (8, _) => Some("Ampere"),
        (7, 5) => Some("Turing"),
        _ => None,
    })
}

impl NvGpu {
    pub fn spec(&self) -> NvSpec {
        use nvml_wrapper::enum_wrappers::device::TemperatureThreshold;
        let Ok(d) = self.nvml.device_by_index(self.index) else { return NvSpec::default() };
        let arch = self.lib2.as_ref().and_then(|l| l.nvmlDeviceGetArchitecture.as_ref().ok()).and_then(|f| unsafe {
            let mut a: nvml_wrapper_sys::bindings::nvmlDeviceArchitecture_t = 0;
            (f(d.handle(), &mut a) == 0).then_some(a)
        });
        let pc = d.power_management_limit_constraints().ok();
        let ok = |s: Result<String, nvml_wrapper::error::NvmlError>| s.ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        NvSpec {
            arch,
            compute: d.cuda_compute_capability().ok().map(|c| (c.major, c.minor)),
            cores: d.num_cores().ok().filter(|c| *c > 0),
            bus_width: d.memory_bus_width().ok().filter(|b| *b > 0),
            max_gpu_mhz: d.max_clock_info(Clock::Graphics).ok().filter(|c| *c > 0),
            boost_mhz: d.max_customer_boost_clock(Clock::Graphics).ok().filter(|c| *c > 0),
            max_mem_mhz: d.max_clock_info(Clock::Memory).ok().filter(|c| *c > 0),
            vbios: ok(d.vbios_version()),
            part_number: ok(d.board_part_number()),
            bar1_total: d.bar1_memory_info().ok().map(|b| b.total).filter(|t| *t > 0),
            mem_total: d.memory_info().ok().map(|m| m.total),
            power_default_w: d.power_management_limit_default().ok().map(|mw| mw as f64 / 1000.0).filter(|w| *w > 0.0),
            power_min_w: pc.as_ref().map(|c| c.min_limit as f64 / 1000.0).filter(|w| *w > 0.0),
            power_max_w: pc.as_ref().map(|c| c.max_limit as f64 / 1000.0).filter(|w| *w > 0.0),
            temp_slowdown: d.temperature_threshold(TemperatureThreshold::Slowdown).ok().filter(|t| *t > 0),
            temp_shutdown: d.temperature_threshold(TemperatureThreshold::Shutdown).ok().filter(|t| *t > 0),
            fans: d.num_fans().ok(),
            driver: ok(self.nvml.sys_driver_version()),
            cuda: self.nvml.sys_cuda_driver_version().ok().filter(|v| *v > 0),
            pcie_cur: d.current_pcie_link_gen().ok().zip(d.current_pcie_link_width().ok()),
            pcie_host_max: d.max_pcie_link_gen().ok().zip(d.max_pcie_link_width().ok()),
        }
    }
}
