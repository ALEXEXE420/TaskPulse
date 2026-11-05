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
        let lib2 = unsafe { nvml_wrapper_sys::bindings::NvmlLib::new("nvml.dll").ok() };
        Some(NvGpu { nvml, lib2, index: 0, name })
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
        }
    }
}
