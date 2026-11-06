//! Arbeitsspeicher (GlobalMemoryStatusEx) und Datenträger (Leistungsindikatoren "PhysicalDisk":
//! alle zusammen und jeder einzeln; Modell und Typ über IOCTL_STORAGE_QUERY_PROPERTY).

#[cfg(windows)]
use super::pdh::{Counter, Query};
use std::collections::HashMap;
#[cfg(windows)]
use windows::core::HSTRING;
#[cfg(windows)]
use windows::Win32::Foundation::CloseHandle;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{CreateFileW, GetDiskFreeSpaceExW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
#[cfg(windows)]
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
#[cfg(windows)]
use windows::Win32::System::IO::DeviceIoControl;

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MemSample {
    pub used_pct: Option<f64>,
    pub used_bytes: f64,
    pub total_bytes: f64,
    pub commit_used: f64,
    pub commit_limit: f64,
}

#[cfg(windows)]
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
    /// Größe des ganzen Laufwerks in Bytes
    pub size: Option<f64>,
}

/// Ein Laufwerk (Partition) auf einem Datenträger, wie im Explorer.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Volume {
    /// "C:" (Linux: Einhängepunkt, z. B. "/home")
    pub letter: String,
    /// Bezeichnung, z. B. "Windows" (leer = keine vergeben)
    pub label: String,
    /// Dateisystem, z. B. "NTFS"
    pub fs: Option<String>,
    pub total: Option<f64>,
    pub free: Option<f64>,
    /// Wechseldatenträger (USB-Stick) – Explorer nennt ihn dann „USB-Laufwerk“
    pub removable: bool,
    /// Datenträger, über die sich das Laufwerk erstreckt (mehr als einer = Verbund,
    /// z. B. übergreifendes Volume auf dynamischen Datenträgern)
    pub disks: Vec<u32>,
    /// Anteil des Laufwerks auf diesem Datenträger (Bytes)
    pub here: Option<f64>,
}

impl Volume {
    /// Name wie im Explorer: "Windows (C:)", ohne Bezeichnung "Lokaler Datenträger (C:)"
    pub fn explorer_name(&self) -> String {
        if !cfg!(windows) {
            return if self.label.is_empty() { self.letter.clone() } else { format!("{} ({})", self.label, self.letter) };
        }
        let label = if !self.label.is_empty() {
            self.label.as_str()
        } else if self.removable {
            "USB-Laufwerk"
        } else {
            "Lokaler Datenträger"
        };
        format!("{label} ({})", self.letter)
    }
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
    /// Temperatur laut Laufwerk (NVMe/SATA, °C)
    pub temp_c: Option<f64>,
    /// Laufwerke darauf mit Namen wie im Explorer
    pub volumes: Vec<Volume>,
}

impl DiskOne {
    /// "Datenträger 0 (C: D:)"
    pub fn number(&self) -> String {
        if self.letters.is_empty() {
            format!("Datenträger {}", self.index)
        } else {
            format!("Datenträger {} ({})", self.index, self.letters.join(" "))
        }
    }

    /// Name wie im Explorer: "Windows (C:)", mehrere Laufwerke mit „ · “ getrennt;
    /// ohne Laufwerk "Datenträger 3"
    /// USB-Adapter bzw. Kartenleser ohne eingesteckten Datenträger: meldet weder Größe noch Laufwerk
    pub fn is_empty_adapter(&self) -> bool {
        self.info.kind.as_deref() == Some("USB") && self.capacity.is_none() && self.letters.is_empty() && self.volumes.is_empty()
    }

    pub fn title(&self) -> String {
        if self.is_empty_adapter() {
            return format!("{} (leer)", self.number());
        }
        if self.volumes.is_empty() {
            return if self.letters.is_empty() { self.number() } else { self.letters.iter().map(|l| format!("Lokaler Datenträger ({l})")).collect::<Vec<_>>().join(" · ") };
        }
        self.volumes
            .iter()
            .map(|v| match v.disks.iter().position(|d| *d == self.index) {
                Some(p) if v.disks.len() > 1 => format!("{} – Verbund {}/{}", v.explorer_name(), p + 1, v.disks.len()),
                _ => v.explorer_name(),
            })
            .collect::<Vec<_>>()
            .join(" · ")
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

/// Reihenfolge wie im Explorer: nach dem ersten Laufwerksbuchstaben (A:, B:, C: …),
/// Datenträger ohne Buchstaben am Ende; gleicher Buchstabe (Verbund) bzw. ohne → nach Nummer.
pub fn sort_disks(disks: &mut [DiskOne]) {
    disks.sort_by(|a, b| {
        let key = |d: &DiskOne| d.letters.iter().map(|l| l.to_uppercase()).min();
        match (key(a), key(b)) {
            (Some(x), Some(y)) => x.cmp(&y).then(a.index.cmp(&b.index)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.index.cmp(&b.index),
        }
    });
}

/// Instanzname der Leistungsindikatoren ("0 C: D:") → (Nummer, Laufwerke)
pub fn parse_instance(name: &str) -> Option<(u32, Vec<String>)> {
    let mut parts = name.split_whitespace();
    let index = parts.next()?.parse().ok()?;
    Some((index, parts.filter(|p| p.ends_with(':')).map(|p| p.to_uppercase()).collect()))
}

#[cfg(windows)]
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;

/// Modell und Typ eines Datenträgers. Öffnen ohne Zugriffsrechte reicht dafür (keine Adminrechte nötig).
#[cfg(windows)]
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
        // Größe des ganzen Laufwerks: IOCTL_DISK_GET_DRIVE_GEOMETRY_EX (DiskSize @24), ohne Zugriffsrechte
        let mut geo = [0u8; 256];
        let mut ret = 0u32;
        if DeviceIoControl(h, 0x0007_00A0, None, 0, Some(geo.as_mut_ptr() as *mut _), geo.len() as u32, Some(&mut ret), None).is_ok() && ret >= 32 {
            let size = i64::from_le_bytes(geo[24..32].try_into().unwrap());
            info.size = (size > 0).then_some(size as f64);
        }
        let _ = CloseHandle(h);
        info
    }
}

/// STORAGE_TEMPERATURE_DATA_DESCRIPTOR → Temperatur (erster Eintrag = Gesamtwert des Laufwerks).
/// Aufbau: Version, Size (je 4), Critical, Warning (je 2), InfoCount (2), 2+8 reserviert,
/// dann je 16 Byte STORAGE_TEMPERATURE_INFO { Index u16, Temperature i16, … }.
pub fn parse_temperature_descriptor(buf: &[u8]) -> Option<f64> {
    if buf.len() < 40 {
        return None;
    }
    let count = u16::from_le_bytes([buf[12], buf[13]]) as usize;
    (0..count.min((buf.len() - 24) / 16))
        .map(|i| i16::from_le_bytes([buf[24 + i * 16 + 2], buf[24 + i * 16 + 3]]) as f64)
        .find(|t| (-40.0..150.0).contains(t) && *t != 0.0)
}

/// Antwort der NVMe-Abfrage: STORAGE_PROTOCOL_DATA_DESCRIPTOR (Version, Size, dann die 40-Byte-Struktur,
/// deren ProtocolDataOffset @16 auf die Daten zeigt) → Health-Log Byte 1–2 = Temperatur in Kelvin.
pub fn parse_nvme_health_temp(buf: &[u8]) -> Option<f64> {
    if buf.len() < 48 {
        return None;
    }
    let off = u32::from_le_bytes(buf[24..28].try_into().ok()?) as usize;
    let len = u32::from_le_bytes(buf[28..32].try_into().ok()?) as usize;
    let start = 8 + off;
    if len < 3 || start + 3 > buf.len() {
        return None;
    }
    let kelvin = u16::from_le_bytes([buf[start + 1], buf[start + 2]]) as f64;
    let t = kelvin - 273.0;
    (kelvin > 0.0 && (-40.0..150.0).contains(&t)).then_some(t)
}

/// SMART-Attributtabelle (512 Byte, READ ATTRIBUTES) → Temperatur aus Attribut 194 bzw. 190.
/// Ab Byte 2 folgen 30 Einträge à 12 Byte: Kennung, Flags (2), aktuell, schlechtester, Rohwert (6), reserviert.
pub fn parse_smart_temperature(data: &[u8]) -> Option<f64> {
    let attr = |id: u8| -> Option<f64> {
        (0..30).map(|i| 2 + i * 12).filter(|o| o + 12 <= data.len()).find(|o| data[*o] == id).map(|o| data[o + 5] as f64)
    };
    attr(194).or_else(|| attr(190)).filter(|t| *t > 0.0 && *t < 100.0)
}

/// Temperatur eines Datenträgers. Ohne Adminrechte gibt Windows sie nicht heraus; mit Adminrechten
/// zuerst StorageDeviceTemperatureProperty (NVMe und viele SATA-Laufwerke), sonst SMART-Attribut 194.
/// Schläft eine Festplatte gerade, wird sie nicht geweckt (dann None).
#[cfg(windows)]
fn disk_temp(index: u32) -> Option<f64> {
    use windows::Win32::Foundation::{BOOL, GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::System::Power::GetDevicePowerState;
    let open = |access: u32| unsafe {
        CreateFileW(
            &HSTRING::from(format!(r"\\.\PhysicalDrive{index}")),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .ok()
    };
    let property = |h| -> Option<f64> {
        // PropertyId 59 = StorageDeviceTemperatureProperty, PropertyStandardQuery
        let q: [u32; 3] = [59, 0, 0];
        let mut out = [0u8; 256];
        let mut ret = 0u32;
        unsafe { DeviceIoControl(h, IOCTL_STORAGE_QUERY_PROPERTY, Some(q.as_ptr() as *const _), 12, Some(out.as_mut_ptr() as *mut _), out.len() as u32, Some(&mut ret), None) }.ok()?;
        parse_temperature_descriptor(&out[..(ret as usize).min(out.len())])
    };
    // NVMe: Health-Log (Log-Seite 2) über StorageDeviceProtocolSpecificProperty – wie Windows/CrystalDiskInfo
    let nvme = |h| -> Option<f64> {
        // STORAGE_PROPERTY_QUERY { PropertyId = 50, QueryType = 0 } + STORAGE_PROTOCOL_SPECIFIC_DATA (40 Byte) + 512 Byte Daten
        let mut buf = vec![0u8; 8 + 40 + 512];
        let put = |b: &mut [u8], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut buf, 0, 50); // StorageDeviceProtocolSpecificProperty
        put(&mut buf, 4, 0); // PropertyStandardQuery
        put(&mut buf, 8, 3); // ProtocolType = NVMe
        put(&mut buf, 12, 2); // DataType = Log-Seite
        put(&mut buf, 16, 2); // Log-Seite 2: SMART/Health
        put(&mut buf, 20, 0);
        put(&mut buf, 24, 40); // ProtocolDataOffset (ab Beginn der Struktur)
        put(&mut buf, 28, 512); // ProtocolDataLength
        let mut ret = 0u32;
        let len = buf.len() as u32;
        unsafe { DeviceIoControl(h, IOCTL_STORAGE_QUERY_PROPERTY, Some(buf.as_ptr() as *const _), len, Some(buf.as_mut_ptr() as *mut _), len, Some(&mut ret), None) }.ok()?;
        parse_nvme_health_temp(&buf[..(ret as usize).min(buf.len())])
    };
    // 1) ohne Rechte (geht bei manchen Treibern)
    if let Some(h) = open(0) {
        let t = property(h).or_else(|| nvme(h));
        unsafe {
            let _ = CloseHandle(h);
        }
        if t.is_some() {
            return t;
        }
    }
    // 2) mit Lese-/Schreibzugriff (nur mit Adminrechten möglich)
    let h = open((GENERIC_READ | GENERIC_WRITE).0)?;
    let mut on = BOOL(1);
    let awake = unsafe { GetDevicePowerState(h, &mut on) }.as_bool().then_some(on.as_bool()).unwrap_or(true);
    let t = if awake {
        property(h).or_else(|| nvme(h)).or_else(|| {
            // SMART READ ATTRIBUTES: SENDCMDINPARAMS (gepackt, 32 Byte + 1)
            const SMART_RCV_DRIVE_DATA: u32 = 0x0007_C088;
            let mut inp = [0u8; 33];
            inp[0..4].copy_from_slice(&512u32.to_le_bytes()); // cBufferSize
            // IDEREGS: Features 0xD0 (READ DATA), SectorCount 1, SectorNumber 1, CylLow 0x4F, CylHigh 0xC2, DriveHead 0xA0, Command 0xB0 (SMART)
            inp[4..12].copy_from_slice(&[0xD0, 1, 1, 0x4F, 0xC2, 0xA0 | (((index & 1) as u8) << 4), 0xB0, 0]);
            inp[12] = index as u8; // bDriveNumber
            let mut out = [0u8; 16 + 512];
            let mut ret = 0u32;
            unsafe { DeviceIoControl(h, SMART_RCV_DRIVE_DATA, Some(inp.as_ptr() as *const _), inp.len() as u32, Some(out.as_mut_ptr() as *mut _), out.len() as u32, Some(&mut ret), None) }.ok()?;
            // SENDCMDOUTPARAMS: cBufferSize (4), DRIVERSTATUS (12), dann die Daten
            (ret as usize >= 16 + 362).then(|| parse_smart_temperature(&out[16..])).flatten()
        })
    } else {
        None
    };
    unsafe {
        let _ = CloseHandle(h);
    }
    t
}

#[cfg(windows)]
fn volume_size(letter: &str) -> Option<f64> {
    let mut total = 0u64;
    unsafe { GetDiskFreeSpaceExW(&HSTRING::from(format!("{letter}\\")), None, Some(&mut total), None) }.ok()?;
    (total > 0).then_some(total as f64)
}

/// VOLUME_DISK_EXTENTS → (Datenträger-Nummer, Länge in Bytes) je Abschnitt.
/// Aufbau: NumberOfDiskExtents (4) + 4 Füllbytes, dann je 24 Byte { DiskNumber u32, 4 frei, StartingOffset i64, ExtentLength i64 }.
pub fn parse_extents(buf: &[u8]) -> Vec<(u32, f64)> {
    if buf.len() < 8 {
        return Vec::new();
    }
    let n = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
    (0..n)
        .map(|i| 8 + i * 24)
        .take_while(|o| o + 24 <= buf.len())
        .map(|o| (u32::from_le_bytes(buf[o..o + 4].try_into().unwrap()), i64::from_le_bytes(buf[o + 16..o + 24].try_into().unwrap()).max(0) as f64))
        .collect()
}

/// Auf welchen Datenträgern liegt ein Volume? (IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, ohne Adminrechte)
#[cfg(windows)]
fn volume_extents(volume: &str) -> Vec<(u32, f64)> {
    unsafe {
        let Ok(h) = CreateFileW(&HSTRING::from(volume), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) else {
            return Vec::new();
        };
        let mut out = vec![0u8; 8 + 24 * 64];
        let mut ret = 0u32;
        let ok = DeviceIoControl(h, 0x0056_0000, None, 0, Some(out.as_mut_ptr() as *mut _), out.len() as u32, Some(&mut ret), None).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return Vec::new();
        }
        parse_extents(&out[..ret as usize])
    }
}

/// Alle Laufwerke mit Buchstaben → je Datenträger-Nummer die Laufwerke darauf.
/// So erscheinen auch Laufwerke auf dynamischen Datenträgern und Verbünde über mehrere Platten,
/// die Windows in den Leistungsindikatoren ohne Buchstaben meldet.
#[cfg(windows)]
fn volume_map() -> HashMap<u32, Vec<Volume>> {
    use windows::Win32::Storage::FileSystem::{FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetVolumePathNamesForVolumeNameW};
    let mut map: HashMap<u32, Vec<Volume>> = HashMap::new();
    let z = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]);
    unsafe {
        let mut name = [0u16; 260];
        let Ok(find) = FindFirstVolumeW(&mut name) else { return map };
        loop {
            let vol = z(&name);
            let mut paths = [0u16; 2048];
            let mut len = 0u32;
            let letter = if GetVolumePathNamesForVolumeNameW(&HSTRING::from(vol.as_str()), Some(&mut paths), &mut len).is_ok() {
                // Mehrere Pfade, je mit 0 getrennt – der Laufwerksbuchstabe ist „X:\“
                paths.split(|c| *c == 0).map(String::from_utf16_lossy).find(|p| p.len() == 3 && p.ends_with(":\\")).map(|p| p[..2].to_uppercase())
            } else {
                None
            };
            if let Some(letter) = letter {
                let ext = volume_extents(vol.trim_end_matches('\\'));
                if !ext.is_empty() {
                    let mut v = volume_info(&letter);
                    let mut disks: Vec<u32> = ext.iter().map(|e| e.0).collect();
                    disks.sort();
                    disks.dedup();
                    v.disks = disks.clone();
                    for d in disks {
                        let mut x = v.clone();
                        x.here = Some(ext.iter().filter(|e| e.0 == d).map(|e| e.1).sum());
                        map.entry(d).or_default().push(x);
                    }
                }
            }
            if FindNextVolumeW(find, &mut name).is_err() {
                break;
            }
        }
        let _ = FindVolumeClose(find);
    }
    for v in map.values_mut() {
        v.sort_by(|a, b| a.letter.cmp(&b.letter));
    }
    map
}

/// Bezeichnung, Dateisystem und Belegung eines Laufwerks (wie im Explorer), ohne Adminrechte.
#[cfg(windows)]
fn volume_info(letter: &str) -> Volume {
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumeInformationW};
    let root = HSTRING::from(format!("{letter}\\"));
    let mut name = [0u16; 261];
    let mut fs = [0u16; 64];
    let mut v = Volume { letter: letter.to_string(), ..Default::default() };
    unsafe {
        if GetVolumeInformationW(&root, Some(&mut name), None, None, None, Some(&mut fs)).is_ok() {
            let z = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]).trim().to_string();
            v.label = z(&name);
            v.fs = Some(z(&fs)).filter(|f| !f.is_empty());
        }
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        if GetDiskFreeSpaceExW(&root, Some(&mut avail), Some(&mut total), Some(&mut free)).is_ok() && total > 0 {
            v.total = Some(total as f64);
            v.free = Some(free as f64);
        }
        // DRIVE_REMOVABLE = 2
        v.removable = GetDriveTypeW(&root) == 2;
    }
    v
}

#[cfg(windows)]
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
    /// Temperatur je Datenträger, alle 10 s neu gelesen
    temps: HashMap<u32, (Option<f64>, std::time::Instant)>,
    /// Laufwerke (Name, Belegung, Verbund) je Datenträger-Nummer, alle 10 s neu gelesen
    volumes: (HashMap<u32, Vec<Volume>>, Option<std::time::Instant>),
}

#[cfg(windows)]
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
        Some(Disk { q, idle, read, write, idle_each, read_each, write_each, known: HashMap::new(), temps: HashMap::new(), volumes: (HashMap::new(), None) })
    }

    pub fn sample(&mut self) -> DiskSample {
        if !self.q.collect() {
            return DiskSample::default();
        }
        let arr = |c: Option<Counter>| -> HashMap<String, f64> { c.map(|c| self.q.array(c).into_iter().collect()).unwrap_or_default() };
        let (idle, read, write) = (arr(self.idle_each), arr(self.read_each), arr(self.write_each));
        let mut disks = Vec::new();
        if self.volumes.1.map(|t| t.elapsed().as_secs() >= 10).unwrap_or(true) {
            self.volumes = (volume_map(), Some(std::time::Instant::now()));
        }
        for (name, idle_v) in &idle {
            let Some((index, pdh_letters)) = parse_instance(name) else { continue }; // "_Total" fällt hier weg
            // Laufwerke über ihre Abschnitte zuordnen (findet auch dynamische Datenträger und Verbünde),
            // sonst die Buchstaben aus den Leistungsindikatoren
            let volumes: Vec<Volume> = match self.volumes.0.get(&index) {
                Some(v) if !v.is_empty() => v.clone(),
                _ => pdh_letters.iter().map(|l| volume_info(l)).collect(),
            };
            let letters: Vec<String> = if volumes.is_empty() { pdh_letters.clone() } else { volumes.iter().map(|v| v.letter.clone()).collect() };
            let (info, capacity) = self
                .known
                .entry(name.clone())
                .or_insert_with(|| {
                    let cap: f64 = pdh_letters.iter().filter_map(|l| volume_size(l)).sum();
                    let info = disk_info(index);
                    let size = info.size.or((cap > 0.0).then_some(cap));
                    (info, size)
                })
                .clone();
            // USB-Gehäuse geben die Temperatur meist nicht weiter → nicht fragen
            let temp_c = if info.kind.as_deref() == Some("USB") {
                None
            } else {
                let e = self.temps.entry(index).or_insert((None, std::time::Instant::now() - std::time::Duration::from_secs(60)));
                if e.1.elapsed().as_secs() >= 10 {
                    *e = (disk_temp(index), std::time::Instant::now());
                }
                e.0
            };
            disks.push(DiskOne {
                volumes,
                index,
                letters,
                active_pct: Some((100.0 - idle_v).clamp(0.0, 100.0)),
                read_bps: read.get(name).copied(),
                write_bps: write.get(name).copied(),
                info,
                capacity,
                temp_c,
            });
        }
        sort_disks(&mut disks);
        DiskSample {
            active_pct: self.idle.and_then(|c| self.q.value(c)).map(|idle| (100.0 - idle).clamp(0.0, 100.0)),
            read_bps: self.read.and_then(|c| self.q.value(c)),
            write_bps: self.write.and_then(|c| self.q.value(c)),
            disks,
        }
    }
}

// ------------------------------------------------------------------ Linux: /proc/meminfo, /proc/diskstats, /sys/block

#[cfg_attr(windows, allow(dead_code))]
/// Wert aus /proc/meminfo in Bytes, z. B. "MemTotal:  32768000 kB"
pub fn meminfo_value(text: &str, key: &str) -> Option<f64> {
    text.lines().find(|l| l.starts_with(key) && l[key.len()..].starts_with(':')).and_then(|l| {
        let n: f64 = l.split_whitespace().nth(1)?.parse().ok()?;
        Some(if l.trim_end().ends_with("kB") { n * 1024.0 } else { n })
    })
}

#[cfg(target_os = "linux")]
pub fn sample_mem() -> MemSample {
    let Ok(t) = std::fs::read_to_string("/proc/meminfo") else { return MemSample::default() };
    let total = meminfo_value(&t, "MemTotal").unwrap_or(0.0);
    let avail = meminfo_value(&t, "MemAvailable").unwrap_or(0.0);
    if total <= 0.0 {
        return MemSample::default();
    }
    let used = (total - avail).max(0.0);
    MemSample {
        used_pct: Some(used / total * 100.0),
        used_bytes: used,
        total_bytes: total,
        commit_used: meminfo_value(&t, "Committed_AS").unwrap_or(used),
        commit_limit: meminfo_value(&t, "CommitLimit").unwrap_or(total),
    }
}

#[cfg_attr(windows, allow(dead_code))]
/// Zeile aus /proc/diskstats → (Name, gelesene Sektoren, geschriebene Sektoren, ms aktiv)
pub fn parse_diskstats_line(line: &str) -> Option<(String, u64, u64, u64)> {
    let f: Vec<&str> = line.split_whitespace().collect();
    if f.len() < 13 {
        return None;
    }
    let n = |i: usize| f[i].parse::<u64>().ok();
    Some((f[2].to_string(), n(5)?, n(9)?, n(12)?))
}

#[cfg_attr(windows, allow(dead_code))]
/// Echte Laufwerke (keine loop-, ram-, zram- oder Device-Mapper-Geräte)
pub fn is_physical_disk(name: &str) -> bool {
    !(name.starts_with("loop") || name.starts_with("ram") || name.starts_with("zram") || name.starts_with("dm-") || name.starts_with("sr") || name.starts_with("md"))
}

#[cfg(target_os = "linux")]
fn linux_disk_info(name: &str) -> (DiskInfo, Option<f64>, Vec<String>) {
    use super::sysfs::{read, read_u64};
    let base = format!("/sys/block/{name}");
    let vendor = read(format!("{base}/device/vendor")).filter(|v| !v.is_empty() && v != "ATA");
    let model = read(format!("{base}/device/model")).filter(|m| !m.is_empty());
    let model = match (vendor, model) {
        (Some(v), Some(m)) if !m.to_lowercase().starts_with(&v.to_lowercase()) => Some(format!("{v} {m}")),
        (_, Some(m)) => Some(m),
        (v, None) => v,
    };
    let link = std::fs::read_link(&base).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let kind = if link.contains("/usb") || read(format!("{base}/removable")).as_deref() == Some("1") {
        Some("USB".to_string())
    } else if name.starts_with("nvme") {
        Some("NVMe-SSD".to_string())
    } else {
        match read(format!("{base}/queue/rotational")).as_deref() {
            Some("0") => Some("SSD".to_string()),
            Some("1") => Some("HDD".to_string()),
            _ => None,
        }
    };
    let capacity = read_u64(format!("{base}/size")).map(|s| s as f64 * 512.0).filter(|c| *c > 0.0);
    // Einhängepunkte aller Partitionen dieses Laufwerks (statt Laufwerksbuchstaben)
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let mut points: Vec<String> = mounts
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let dev = it.next()?;
            let mp = it.next()?;
            let d = dev.strip_prefix("/dev/")?;
            (d == name || (d.starts_with(name) && d[name.len()..].trim_start_matches('p').chars().all(|c| c.is_ascii_digit()))).then(|| mp.replace("\\040", " "))
        })
        .collect();
    points.sort();
    points.dedup();
    let model = model.map(|m| format!("{m} ({name})")).or_else(|| Some(name.to_string()));
    (DiskInfo { model, kind, size: capacity }, capacity, points)
}

/// Linux: Einhängepunkt → Laufwerk (Name = Einhängepunkt, Belegung über statvfs)
#[cfg(target_os = "linux")]
fn linux_volume(mount: &str) -> Volume {
    let mut v = Volume { letter: mount.to_string(), ..Default::default() };
    if let Ok(c) = std::ffi::CString::new(mount) {
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0 {
            let bs = st.f_frsize as f64;
            v.total = Some(st.f_blocks as f64 * bs).filter(|t| *t > 0.0);
            v.free = Some(st.f_bfree as f64 * bs);
        }
    }
    v
}

/// Linux: Temperatur über hwmon – NVMe (Treiber nvme) bzw. SATA (Treiber drivetemp)
#[cfg(target_os = "linux")]
fn linux_disk_temp(name: &str) -> Option<f64> {
    let dev = std::path::PathBuf::from(format!("/sys/block/{name}/device"));
    // NVMe: hwmon hängt am Controller (device), SATA: unter device/hwmon
    let mut dirs = super::sysfs::dirs(dev.join("hwmon"));
    dirs.extend(super::sysfs::dirs(&dev).into_iter().filter(|d| d.file_name().map(|n| n.to_string_lossy().starts_with("hwmon")).unwrap_or(false)));
    dirs.iter().find_map(|d| super::sysfs::read_f64(d.join("temp1_input"))).map(|m| m / 1000.0).filter(|t| (-40.0..150.0).contains(t) && *t != 0.0)
}

#[cfg(target_os = "linux")]
pub struct Disk {
    prev: HashMap<String, (u64, u64, u64)>,
    at: std::time::Instant,
    known: HashMap<String, (DiskInfo, Option<f64>, Vec<String>)>,
}

#[cfg(target_os = "linux")]
impl Disk {
    pub fn new() -> Option<Disk> {
        let mut d = Disk { prev: HashMap::new(), at: std::time::Instant::now(), known: HashMap::new() };
        let _ = d.sample();
        Some(d)
    }

    pub fn sample(&mut self) -> DiskSample {
        let Ok(t) = std::fs::read_to_string("/proc/diskstats") else { return DiskSample::default() };
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.at).as_secs_f64();
        self.at = now;
        let mut names: Vec<String> = Vec::new();
        let mut cur: HashMap<String, (u64, u64, u64)> = HashMap::new();
        for l in t.lines() {
            if let Some((name, r, w, ms)) = parse_diskstats_line(l) {
                // nur ganze Laufwerke (in /sys/block), keine Partitionen
                if is_physical_disk(&name) && std::path::Path::new(&format!("/sys/block/{name}")).exists() {
                    names.push(name.clone());
                    cur.insert(name, (r, w, ms));
                }
            }
        }
        names.sort();
        let mut disks = Vec::new();
        let (mut sum_r, mut sum_w, mut max_active): (f64, f64, Option<f64>) = (0.0, 0.0, None);
        for (index, name) in names.iter().enumerate() {
            let (r, w, ms) = cur[name];
            let (info, capacity, letters) = self.known.entry(name.clone()).or_insert_with(|| linux_disk_info(name)).clone();
            let (mut active, mut rb, mut wb) = (None, None, None);
            if let Some((r0, w0, ms0)) = self.prev.get(name) {
                if dt > 0.05 {
                    active = Some((ms.saturating_sub(*ms0) as f64 / (dt * 1000.0) * 100.0).clamp(0.0, 100.0));
                    rb = Some(r.saturating_sub(*r0) as f64 * 512.0 / dt);
                    wb = Some(w.saturating_sub(*w0) as f64 * 512.0 / dt);
                }
            }
            sum_r += rb.unwrap_or(0.0);
            sum_w += wb.unwrap_or(0.0);
            if let Some(a) = active {
                max_active = Some(max_active.map_or(a, |m: f64| m.max(a)));
            }
            let temp_c = linux_disk_temp(name);
            let volumes = letters.iter().map(|m| linux_volume(m)).collect();
            disks.push(DiskOne { index: index as u32, letters, active_pct: active, read_bps: rb, write_bps: wb, info, capacity, temp_c, volumes });
        }
        sort_disks(&mut disks);
        let first = self.prev.is_empty();
        self.prev = cur;
        DiskSample {
            // „alle“: höchste aktive Zeit eines Laufwerks (wie der Task-Manager je Laufwerk)
            active_pct: max_active,
            read_bps: (!first).then_some(sum_r),
            write_bps: (!first).then_some(sum_w),
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
    fn linux_files() {
        let mi = "MemTotal:       32768000 kB\nMemFree:  1000 kB\nMemAvailable:   16384000 kB\n";
        assert_eq!(meminfo_value(mi, "MemTotal"), Some(32768000.0 * 1024.0));
        assert_eq!(meminfo_value(mi, "MemAvailable"), Some(16384000.0 * 1024.0));
        assert_eq!(meminfo_value(mi, "Mem"), None);
        let ds = " 259       0 nvme0n1 1000 0 2048 50 2000 0 4096 80 0 120 130 0 0 0 0";
        assert_eq!(parse_diskstats_line(ds), Some(("nvme0n1".to_string(), 2048, 4096, 120)));
        assert!(is_physical_disk("sda") && is_physical_disk("nvme0n1"));
        assert!(!is_physical_disk("loop3") && !is_physical_disk("zram0") && !is_physical_disk("dm-1"));
    }

    #[test]
    fn titles() {
        let d = DiskOne { index: 1, letters: vec!["D:".into(), "E:".into()], ..Default::default() };
        assert_eq!(d.number(), "Datenträger 1 (D: E:)");
        assert_eq!(DiskOne { index: 3, ..Default::default() }.title(), "Datenträger 3");
        let mut d = d;
        d.volumes = vec![
            Volume { letter: "D:".into(), label: "Spiele".into(), ..Default::default() },
            Volume { letter: "E:".into(), ..Default::default() },
        ];
        if cfg!(windows) {
            assert_eq!(d.title(), "Spiele (D:) · Lokaler Datenträger (E:)");
            // Verbund über vier Datenträger: Datenträger 2 ist Teil 3 von 4
            let g = Volume { letter: "G:".into(), label: "games HDD".into(), disks: vec![0, 1, 2, 3], ..Default::default() };
            let x = DiskOne { index: 2, volumes: vec![g], ..Default::default() };
            assert_eq!(x.title(), "games HDD (G:) – Verbund 3/4");
            assert_eq!(Volume { letter: "F:".into(), removable: true, ..Default::default() }.explorer_name(), "USB-Laufwerk (F:)");
        }
    }

    #[test]
    fn disks_sorted_by_letter() {
        let d = |index: u32, l: &[&str]| DiskOne { index, letters: l.iter().map(|x| x.to_string()).collect(), ..Default::default() };
        let mut v = vec![d(6, &["C:"]), d(8, &[]), d(3, &["G:"]), d(4, &["D:"]), d(7, &["J:", "F:"]), d(0, &["G:"]), d(1, &[])];
        sort_disks(&mut v);
        let order: Vec<u32> = v.iter().map(|x| x.index).collect();
        assert_eq!(order, vec![6, 4, 7, 0, 3, 1, 8]);
        // Leerer USB-Adapter
        let mut e = d(8, &[]);
        e.info.kind = Some("USB".into());
        assert!(e.is_empty_adapter());
        assert_eq!(e.title(), "Datenträger 8 (leer)");
        e.capacity = Some(1e12);
        assert!(!e.is_empty_adapter());
    }

    #[test]
    fn disk_extents() {
        let mut b = vec![0u8; 8 + 48];
        b[0] = 2;
        b[8] = 3; // Datenträger 3
        b[24..32].copy_from_slice(&(931i64 << 30).to_le_bytes());
        b[32] = 1; // Datenträger 1
        b[48..56].copy_from_slice(&(298i64 << 30).to_le_bytes());
        assert_eq!(parse_extents(&b), vec![(3, 931.0 * 1073741824.0), (1, 298.0 * 1073741824.0)]);
        assert!(parse_extents(&b[..4]).is_empty());
    }

    #[test]
    fn temperature_descriptor() {
        let mut b = vec![0u8; 56];
        b[0] = 1;
        b[4] = 56;
        b[12] = 2; // zwei Sensoren
        b[26..28].copy_from_slice(&41i16.to_le_bytes());
        b[42..44].copy_from_slice(&55i16.to_le_bytes());
        assert_eq!(parse_temperature_descriptor(&b), Some(41.0));
        b[26..28].copy_from_slice(&0i16.to_le_bytes());
        assert_eq!(parse_temperature_descriptor(&b), Some(55.0));
        assert_eq!(parse_temperature_descriptor(&b[..20]), None);
        // SMART: Attribut 194 mit Rohwert 38 °C an zweiter Stelle
        let mut d = vec![0u8; 362];
        d[2] = 9;
        d[14] = 194;
        d[14 + 5] = 38;
        assert_eq!(parse_smart_temperature(&d), Some(38.0));
        d[14] = 5;
        assert_eq!(parse_smart_temperature(&d), None);
        // NVMe-Health-Log: 311 K = 38 °C
        let mut n = vec![0u8; 8 + 40 + 512];
        n[24..28].copy_from_slice(&40u32.to_le_bytes());
        n[28..32].copy_from_slice(&512u32.to_le_bytes());
        n[48 + 1..48 + 3].copy_from_slice(&311u16.to_le_bytes());
        assert_eq!(parse_nvme_health_temp(&n), Some(38.0));
        assert_eq!(parse_nvme_health_temp(&n[..20]), None);
    }
}
