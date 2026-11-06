//! Messkern: liest alle Sensoren und stellt einen gemeinsamen Datenstand bereit.

pub mod cpu;
pub mod cputemp;
pub mod dimm;
pub mod nvtemp;
pub mod spec;
pub mod startup;
pub mod gpu;
pub mod gpu_proc;
pub mod mem;
pub mod net;
pub mod nvgpu;
#[cfg(windows)]
pub mod pdh;
pub mod sysfs;
pub mod procs;
pub mod topology;
pub mod fps;

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
    /// Sensoren auf den Speicherriegeln (SMBus); einmal gesucht
    ram: Option<dimm::RamSensors>,
    ram_tried: bool,
    /// Timings ändern sich im Betrieb nicht → einmal lesen
    ram_timings: Option<Vec<dimm::Timings>>,
    /// NVIDIA: Hot-Spot und Speichertemperatur über PawnIO
    nvreg: Option<nvtemp::NvRegTemps>,
    nvreg_tried: bool,
    net: net::Net,
    gpu: gpu::Gpu,
}

impl BasicSensors {
    pub fn new(cfg: &Config) -> BasicSensors {
        let (temp, temp_status) = open_temp(cfg);
        BasicSensors { cpu: cpu::Cpu::new(), temp, temp_status, temp_retry: 0, ram: None, ram_tried: false, ram_timings: None, nvreg: None, nvreg_tried: false, net: net::Net::new(), gpu: gpu::Gpu::new(&cfg.gpu_adapter) }
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
        let mut gpu = self.gpu.sample();
        // NVIDIA: Hot-Spot und Speichertemperatur (nur mit PawnIO/Adminrechten; sonst wird nichts versucht)
        let pawnio_ok = cfg.use_pawnio && self.temp.is_some();
        if !pawnio_ok {
            self.nvreg = None;
            self.nvreg_tried = false;
        } else if !self.nvreg_tried && gpu.source.as_deref().map(|s| s.contains("NVML")).unwrap_or(false) {
            self.nvreg_tried = true;
            self.nvreg = nvgpu::NvGpu::new().and_then(|n| n.bus_id()).and_then(|id| nvtemp::NvRegTemps::open(&id));
        }
        if let Some(r) = &self.nvreg {
            let t = r.read();
            gpu.hotspot_c = t.hotspot_c;
            gpu.mem_temp_c = t.mem_max_c;
        }
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
            gpu,
        }
    }
}

impl BasicSensors {
    /// Watt und Spannung je physischem Kern (nur mit PawnIO/Adminrechten; sonst leer).
    pub fn per_core(&mut self, cores: &[Vec<usize>]) -> Vec<cputemp::CoreReading> {
        self.temp.as_mut().map(|t| t.per_core(cores)).unwrap_or_default()
    }

    /// Laufende RAM-Timings je Kanal (AMD mit PawnIO; einmal gelesen).
    pub fn ram_timings(&mut self) -> Vec<dimm::Timings> {
        if self.ram_timings.is_none() {
            if let Some(t) = self.temp.as_ref() {
                let v = t.ram_timings();
                crate::diag::log(format!("RAM-Timings: {} Kanäle gelesen", v.len()));
                self.ram_timings = Some(v);
            }
        }
        self.ram_timings.clone().unwrap_or_default()
    }

    /// Temperatur der Speicherriegel (SMBus, nur mit PawnIO; abschaltbar).
    pub fn dimm_temps(&mut self, cfg: &Config) -> Vec<dimm::DimmTemp> {
        let allowed = cfg.ram_sensors && (cfg!(not(windows)) || (cfg.use_pawnio && self.temp.is_some()));
        if !allowed {
            self.ram = None;
            self.ram_tried = false;
            return Vec::new();
        }
        if !self.ram_tried {
            self.ram_tried = true;
            let intel = cputemp::cpu_info().vendor.contains("Intel");
            self.ram = dimm::RamSensors::open(intel);
            crate::diag::log(format!("RAM-Sensoren: {}", if self.ram.is_some() { "gefunden" } else { "keine gefunden" }));
        }
        self.ram.as_ref().map(|r| r.temps()).unwrap_or_default()
    }

    /// AMD: Temperatur je CCD (nur mit PawnIO).
    pub fn ccd_temps(&self) -> Vec<f64> {
        self.temp.as_ref().map(|t| t.ccd_temps()).unwrap_or_default()
    }
}

fn open_temp(cfg: &Config) -> (Option<cputemp::CpuTemp>, cputemp::TempStatus) {
    if !cfg.use_pawnio {
        return (None, cputemp::TempStatus::Disabled);
    }
    match cputemp::CpuTemp::open() {
        Ok(t) => (Some(t), cputemp::TempStatus::Ok),
        Err(e) => {
            // Fehlende Adminrechte sind normal – nur echte Fehler protokollieren
            if matches!(e, cputemp::TempStatus::Failed(_)) {
                crate::diag::log(format!("CPU-Sensoren (PawnIO): {}", e.describe()));
            }
            (None, e)
        }
    }
}
