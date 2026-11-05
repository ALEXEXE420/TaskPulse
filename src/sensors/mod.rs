//! Messkern: liest alle Sensoren und stellt einen gemeinsamen Datenstand bereit.

pub mod cpu;
pub mod cputemp;
pub mod gpu;
pub mod gpu_proc;
pub mod mem;
pub mod net;
pub mod nvgpu;
pub mod pdh;
pub mod procs;

use crate::config::{Config, CpuFreqMode};
use crate::format;

/// Werte für die Taskleisten-Anzeige (und Kopfzeilen des Hauptfensters).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Basic {
    pub cpu: cpu::CpuSample,
    pub cpu_freq_mhz: Option<f64>,
    pub cpu_temp_c: Option<f64>,
    /// Leistungsaufnahme des Prozessors (Package) in Watt – über PawnIO, braucht Adminrechte.
    pub cpu_power_w: Option<f64>,
    pub cpu_temp_status: String,
    pub mem: mem::MemSample,
    pub net: net::NetSample,
    pub line_up_pct: Option<f64>,
    pub line_down_pct: Option<f64>,
    pub gpu: nvgpu::GpuSample,
}

pub struct BasicSensors {
    cpu: Option<cpu::Cpu>,
    temp: Option<cputemp::CpuTemp>,
    temp_status: cputemp::TempStatus,
    temp_retry: u32,
    net: net::Net,
    gpu: gpu::Gpu,
}

impl BasicSensors {
    pub fn new(cfg: &Config) -> BasicSensors {
        let (temp, temp_status) = open_temp(cfg);
        BasicSensors { cpu: cpu::Cpu::new(), temp, temp_status, temp_retry: 0, net: net::Net::new(), gpu: gpu::Gpu::new(&cfg.gpu_adapter) }
    }

    pub fn sample(&mut self, cfg: &Config) -> Basic {
        if self.gpu.wanted() != cfg.gpu_adapter {
            self.gpu = gpu::Gpu::new(&cfg.gpu_adapter);
        }
        // PawnIO bei Einstellungsänderung nachladen bzw. freigeben
        if !cfg.use_pawnio && self.temp.is_some() {
            self.temp = None;
            self.temp_status = cputemp::TempStatus::Disabled;
        } else if cfg.use_pawnio && self.temp.is_none() {
            self.temp_retry += 1;
            if matches!(self.temp_status, cputemp::TempStatus::Disabled) || self.temp_retry % 60 == 0 {
                let (t, s) = open_temp(cfg);
                self.temp = t;
                self.temp_status = s;
            }
        }
        let cpu = self.cpu.as_mut().map(|c| c.sample()).unwrap_or_default();
        let cpu_freq_mhz = format::freq_from_cores(&cpu.cores_mhz, cfg.cpu_freq_mode == CpuFreqMode::Average);
        let cpu_temp_c = self.temp.as_ref().and_then(|t| t.read());
        let cpu_power_w = self.temp.as_mut().and_then(|t| t.power());
        let net = self.net.sample(&cfg.net_adapter);
        let line_up_pct = net.up_bps.and_then(|b| format::line_usage_pct(b, cfg.line_up_mbit));
        let line_down_pct = net.down_bps.and_then(|b| format::line_usage_pct(b, cfg.line_down_mbit));
        Basic {
            cpu,
            cpu_freq_mhz,
            cpu_temp_c,
            cpu_power_w,
            cpu_temp_status: self.temp_status.describe(),
            mem: mem::sample_mem(),
            net,
            line_up_pct,
            line_down_pct,
            gpu: self.gpu.sample(),
        }
    }
}

fn open_temp(cfg: &Config) -> (Option<cputemp::CpuTemp>, cputemp::TempStatus) {
    if !cfg.use_pawnio {
        return (None, cputemp::TempStatus::Disabled);
    }
    match cputemp::CpuTemp::open() {
        Ok(t) => (Some(t), cputemp::TempStatus::Ok),
        Err(e) => (None, e),
    }
}
