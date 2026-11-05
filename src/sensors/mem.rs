//! Arbeitsspeicher (GlobalMemoryStatusEx) und Datenträger gesamt (Leistungsindikatoren).

use super::pdh::{Counter, Query};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MemSample {
    pub used_pct: Option<f64>,
    pub used_bytes: f64,
    pub total_bytes: f64,
    pub commit_used: f64,
    pub commit_limit: f64,
}

pub fn sample_mem() -> MemSample {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    if unsafe { GlobalMemoryStatusEx(&mut m) }.is_err() || m.ullTotalPhys == 0 {
        return MemSample::default();
    }
    let total = m.ullTotalPhys as f64;
    let used = (m.ullTotalPhys - m.ullAvailPhys) as f64;
    MemSample {
        used_pct: Some(used / total * 100.0),
        used_bytes: used,
        total_bytes: total,
        commit_used: (m.ullTotalPageFile - m.ullAvailPageFile) as f64,
        commit_limit: m.ullTotalPageFile as f64,
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DiskSample {
    pub active_pct: Option<f64>,
    pub read_bps: Option<f64>,
    pub write_bps: Option<f64>,
}

pub struct Disk {
    q: Query,
    idle: Option<Counter>,
    read: Option<Counter>,
    write: Option<Counter>,
}

impl Disk {
    pub fn new() -> Option<Disk> {
        let q = Query::new()?;
        let idle = q.add(r"\PhysicalDisk(_Total)\% Idle Time");
        let read = q.add(r"\PhysicalDisk(_Total)\Disk Read Bytes/sec");
        let write = q.add(r"\PhysicalDisk(_Total)\Disk Write Bytes/sec");
        q.collect();
        Some(Disk { q, idle, read, write })
    }

    pub fn sample(&mut self) -> DiskSample {
        if !self.q.collect() {
            return DiskSample::default();
        }
        DiskSample {
            active_pct: self.idle.and_then(|c| self.q.value(c)).map(|idle| (100.0 - idle).clamp(0.0, 100.0)),
            read_bps: self.read.and_then(|c| self.q.value(c)),
            write_bps: self.write.and_then(|c| self.q.value(c)),
        }
    }
}
