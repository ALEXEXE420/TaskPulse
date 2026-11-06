//! Arbeitsspeicher (GlobalMemoryStatusEx) und Datenträger (Leistungsindikatoren "PhysicalDisk":
//! alle zusammen und jeder einzeln; Modell und Typ über IOCTL_STORAGE_QUERY_PROPERTY).

use super::pdh::{Counter, Query};
use std::collections::HashMap;
use windows::core::HSTRING;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Storage::FileSystem::{CreateFileW, GetDiskFreeSpaceExW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::IO::DeviceIoControl;

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

/// Feste Angaben zu einem physischen Datenträger.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DiskInfo {
    /// Modell, z. B. "Samsung SSD 990 PRO 2TB"
    pub model: Option<String>,
    /// "NVMe-SSD", "SSD", "HDD", "USB" …
    pub kind: Option<String>,
}

/// Ein physischer Datenträger.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DiskOne {
    /// Nummer wie im Task-Manager ("Datenträger 0")
    pub index: u32,
    /// Laufwerksbuchstaben, z. B. ["C:", "D:"]
    pub letters: Vec<String>,
    pub active_pct: Option<f64>,
    pub read_bps: Option<f64>,
    pub write_bps: Option<f64>,
    pub info: DiskInfo,
    /// Größe der Laufwerke darauf (Summe), Bytes
    pub capacity: Option<f64>,
}

impl DiskOne {
    /// "Datenträger 0 (C: D:)"
    pub fn title(&self) -> String {
        if self.letters.is_empty() {
            format!("Datenträger {}", self.index)
        } else {
            format!("Datenträger {} ({})", self.index, self.letters.join(" "))
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DiskSample {
    pub active_pct: Option<f64>,
    pub read_bps: Option<f64>,
    pub write_bps: Option<f64>,
    /// Jeder Datenträger einzeln, nach Nummer sortiert
    pub disks: Vec<DiskOne>,
}

/// Instanzname der Leistungsindikatoren ("0 C: D:") → (Nummer, Laufwerke)
pub fn parse_instance(name: &str) -> Option<(u32, Vec<String>)> {
    let mut parts = name.split_whitespace();
    let index = parts.next()?.parse().ok()?;
    Some((index, parts.filter(|p| p.ends_with(':')).map(|p| p.to_uppercase()).collect()))
}

const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;

/// Modell und Typ eines Datenträgers. Öffnen ohne Zugriffsrechte reicht dafür (keine Adminrechte nötig).
fn disk_info(index: u32) -> DiskInfo {
    unsafe {
        let Ok(h) = CreateFileW(
            &HSTRING::from(format!(r"\\.\PhysicalDrive{index}")),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        ) else {
            return DiskInfo::default();
        };
        let query = |prop: u32, out: &mut [u8]| -> Option<usize> {
            // STORAGE_PROPERTY_QUERY { PropertyId, QueryType = PropertyStandardQuery, AdditionalParameters }
            let q: [u32; 3] = [prop, 0, 0];
            let mut ret = 0u32;
            DeviceIoControl(h, IOCTL_STORAGE_QUERY_PROPERTY, Some(q.as_ptr() as *const _), 12, Some(out.as_mut_ptr() as *mut _), out.len() as u32, Some(&mut ret), None)
                .ok()
                .map(|_| ret as usize)
        };
        let mut buf = [0u8; 1024];
        let mut info = DiskInfo::default();
        let mut bus = 0u32;
        if let Some(n) = query(0, &mut buf) {
            // STORAGE_DEVICE_DESCRIPTOR: VendorIdOffset @12, ProductIdOffset @16, BusType @28
            let rd = |o: usize| u32::from_le_bytes(buf[o..o + 4].try_into().unwrap()) as usize;
            let s = |off: usize| -> Option<String> {
                if off == 0 || off >= n {
                    return None;
                }
                let end = buf[off..n].iter().position(|b| *b == 0).map(|e| off + e).unwrap_or(n);
                let t = String::from_utf8_lossy(&buf[off..end]).trim().to_string();
                (!t.is_empty()).then_some(t)
            };
            if n >= 32 {
                let vendor = s(rd(12));
                let product = s(rd(16));
                bus = rd(28) as u32;
                info.model = match (vendor, product) {
                    (Some(v), Some(p)) if !p.to_lowercase().starts_with(&v.to_lowercase()) => Some(format!("{v} {p}")),
                    (_, Some(p)) => Some(p),
                    (Some(v), None) => Some(v),
                    _ => None,
                };
            }
        }
        // DEVICE_SEEK_PENALTY_DESCRIPTOR: IncursSeekPenalty @8
        let mut sp = [0u8; 16];
        let seek = query(7, &mut sp).filter(|n| *n >= 9).map(|_| sp[8] != 0);
        info.kind = match (bus, seek) {
            (7, _) => Some("USB".into()),
            (17, _) => Some("NVMe-SSD".into()),
            (_, Some(false)) => Some("SSD".into()),
            (_, Some(true)) => Some("HDD".into()),
            _ => None,
        };
        let _ = CloseHandle(h);
        info
    }
}

fn volume_size(letter: &str) -> Option<f64> {
    let mut total = 0u64;
    unsafe { GetDiskFreeSpaceExW(&HSTRING::from(format!("{letter}\\")), None, Some(&mut total), None) }.ok()?;
    (total > 0).then_some(total as f64)
}

pub struct Disk {
    q: Query,
    idle: Option<Counter>,
    read: Option<Counter>,
    write: Option<Counter>,
    idle_each: Option<Counter>,
    read_each: Option<Counter>,
    write_each: Option<Counter>,
    /// Modell/Typ/Größe je Instanzname, einmal ermittelt
    known: HashMap<String, (DiskInfo, Option<f64>)>,
}

impl Disk {
    pub fn new() -> Option<Disk> {
        let q = Query::new()?;
        let idle = q.add(r"\PhysicalDisk(_Total)\% Idle Time");
        let read = q.add(r"\PhysicalDisk(_Total)\Disk Read Bytes/sec");
        let write = q.add(r"\PhysicalDisk(_Total)\Disk Write Bytes/sec");
        let idle_each = q.add(r"\PhysicalDisk(*)\% Idle Time");
        let read_each = q.add(r"\PhysicalDisk(*)\Disk Read Bytes/sec");
        let write_each = q.add(r"\PhysicalDisk(*)\Disk Write Bytes/sec");
        q.collect();
        Some(Disk { q, idle, read, write, idle_each, read_each, write_each, known: HashMap::new() })
    }

    pub fn sample(&mut self) -> DiskSample {
        if !self.q.collect() {
            return DiskSample::default();
        }
        let arr = |c: Option<Counter>| -> HashMap<String, f64> { c.map(|c| self.q.array(c).into_iter().collect()).unwrap_or_default() };
        let (idle, read, write) = (arr(self.idle_each), arr(self.read_each), arr(self.write_each));
        let mut disks = Vec::new();
        for (name, idle_v) in &idle {
            let Some((index, letters)) = parse_instance(name) else { continue }; // "_Total" fällt hier weg
            let (info, capacity) = self
                .known
                .entry(name.clone())
                .or_insert_with(|| {
                    let cap: f64 = letters.iter().filter_map(|l| volume_size(l)).sum();
                    (disk_info(index), (cap > 0.0).then_some(cap))
                })
                .clone();
            disks.push(DiskOne {
                index,
                letters,
                active_pct: Some((100.0 - idle_v).clamp(0.0, 100.0)),
                read_bps: read.get(name).copied(),
                write_bps: write.get(name).copied(),
                info,
                capacity,
            });
        }
        disks.sort_by_key(|d| d.index);
        DiskSample {
            active_pct: self.idle.and_then(|c| self.q.value(c)).map(|idle| (100.0 - idle).clamp(0.0, 100.0)),
            read_bps: self.read.and_then(|c| self.q.value(c)),
            write_bps: self.write.and_then(|c| self.q.value(c)),
            disks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_names() {
        assert_eq!(parse_instance("0 C:"), Some((0, vec!["C:".to_string()])));
        assert_eq!(parse_instance("1 D: E:"), Some((1, vec!["D:".to_string(), "E:".to_string()])));
        assert_eq!(parse_instance("2"), Some((2, vec![])));
        assert_eq!(parse_instance("_Total"), None);
    }

    #[test]
    fn titles() {
        let d = DiskOne { index: 1, letters: vec!["D:".into(), "E:".into()], ..Default::default() };
        assert_eq!(d.title(), "Datenträger 1 (D: E:)");
        assert_eq!(DiskOne { index: 3, ..Default::default() }.title(), "Datenträger 3");
    }
}
