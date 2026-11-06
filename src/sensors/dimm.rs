//! Arbeitsspeicher-Riegel: Steckplätze, Hersteller, Takt (SMBIOS), laufende Timings
//! (AMD: Speichercontroller UMC über SMN) und Temperatur der Riegel (Sensor auf dem Riegel über SMBus).
//!
//! Alle Zugriffe sind reine Lesezugriffe:
//! - SMBIOS-Tabelle „Memory Device“ (Typ 17) – liefert Windows ohne Treiber
//! - UMC-Register DramTiming1–6 (AMD PPR) über das schon genutzte PawnIO-Modul AMDFamily17
//! - DDR4: Temperatursensor nach JEDEC JC-42.4 (SMBus 0x18–0x1F, Register 5)
//!   DDR5: SPD-Hub SPD5118 (SMBus 0x50–0x57, Register MR49/MR50) – wie die Linux-Treiber jc42/spd5118.
//!   Über die PawnIO-Module SmbusPIIX4 (AMD) bzw. SmbusI801 (Intel), mit der gemeinsamen
//!   SMBus-Sperre „Access_SMBUS.HTP.Method“, damit es keine Kollision mit anderen Programmen gibt.

/// Ein Speicherriegel (bzw. Steckplatz) laut SMBIOS.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Dimm {
    /// Steckplatz, z. B. "DIMM_A2"
    pub slot: String,
    /// Bank, z. B. "P0 CHANNEL A"
    pub bank: String,
    pub size_bytes: f64,
    /// "DDR4", "DDR5" …
    pub kind: String,
    /// Höchster Takt laut Riegel (MT/s)
    pub rated_mts: Option<u32>,
    /// Eingestellter Takt (MT/s)
    pub configured_mts: Option<u32>,
    pub manufacturer: Option<String>,
    pub part_number: Option<String>,
    /// Eingestellte Spannung in V
    pub volts: Option<f64>,
    pub ranks: Option<u32>,
}

fn mem_type_name(t: u8) -> String {
    match t {
        0x12 => "DDR",
        0x13 => "DDR2",
        0x18 => "DDR3",
        0x1A => "DDR4",
        0x1B => "LPDDR",
        0x1C => "LPDDR2",
        0x1D => "LPDDR3",
        0x1E => "LPDDR4",
        0x22 => "DDR5",
        0x23 => "LPDDR5",
        _ => "",
    }
    .to_string()
}

/// JEDEC-Herstellerkennung (wie sie manche BIOS statt des Namens eintragen) → Name
fn jedec_name(s: &str) -> Option<&'static str> {
    let h = s.trim().to_uppercase();
    let h = h.trim_start_matches("0X");
    let known = [
        ("80CE", "Samsung"),
        ("CE00", "Samsung"),
        ("80AD", "SK hynix"),
        ("AD00", "SK hynix"),
        ("802C", "Micron"),
        ("2C00", "Micron"),
        ("859B", "Crucial"),
        ("04CD", "G.Skill"),
        ("CD04", "G.Skill"),
        ("029E", "Corsair"),
        ("9E02", "Corsair"),
        ("0198", "Kingston"),
        ("9801", "Kingston"),
    ];
    known.iter().find(|(k, _)| h.starts_with(k)).map(|x| x.1)
}

fn clean(s: Option<String>) -> Option<String> {
    let s = s?.trim().to_string();
    let l = s.to_lowercase();
    if s.is_empty() || l == "unknown" || l.starts_with("not specified") || l == "undefined" || s.chars().all(|c| c == '0' || c == ' ') {
        return None;
    }
    Some(s)
}

/// Ein Eintrag der SMBIOS-Tabelle: Typ, fester Teil (ab Byte 0) und Zeichenketten.
#[derive(Debug, Clone)]
pub struct SmbiosStruct {
    pub typ: u8,
    pub data: Vec<u8>,
    pub strings: Vec<String>,
}

impl SmbiosStruct {
    pub fn u16at(&self, o: usize) -> Option<u16> {
        (o + 2 <= self.data.len()).then(|| u16::from_le_bytes([self.data[o], self.data[o + 1]]))
    }
    /// Zeichenkette, auf die das Byte bei `o` zeigt (1-basiert), bereinigt
    pub fn str_at(&self, o: usize) -> Option<String> {
        let n = *self.data.get(o)? as usize;
        clean((n > 0).then(|| self.strings.get(n - 1).cloned()).flatten())
    }
}

/// SMBIOS-Tabelle (ohne Windows-Kopf) → alle Einträge.
pub fn parse_structures(table: &[u8]) -> Vec<SmbiosStruct> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 4 <= table.len() {
        let typ = table[i];
        let len = table[i + 1] as usize;
        if len < 4 || i + len > table.len() {
            break;
        }
        let f = &table[i..i + len];
        // Zeichenketten hinter dem festen Teil, jede mit 0 beendet, die Liste mit einer weiteren 0
        let mut strings: Vec<String> = Vec::new();
        let mut j = i + len;
        let next = if j + 1 < table.len() && table[j] == 0 && table[j + 1] == 0 {
            j + 2
        } else {
            loop {
                let start = j;
                while j < table.len() && table[j] != 0 {
                    j += 1;
                }
                if j >= table.len() {
                    break j;
                }
                strings.push(String::from_utf8_lossy(&table[start..j]).to_string());
                j += 1;
                if j >= table.len() || table[j] == 0 {
                    break j + 1;
                }
            }
        };
        if typ == 127 {
            break;
        }
        out.push(SmbiosStruct { typ, data: f.to_vec(), strings });
        i = next;
    }
    out
}

/// SMBIOS-Tabelle dieses Rechners, einmal gelesen.
pub fn structures() -> &'static [SmbiosStruct] {
    static T: std::sync::OnceLock<Vec<SmbiosStruct>> = std::sync::OnceLock::new();
    T.get_or_init(|| parse_structures(&smbios_table()))
}

/// SMBIOS-Tabelle (ohne Windows-Kopf) → belegte Speicherriegel.
pub fn parse_smbios(table: &[u8]) -> Vec<Dimm> {
    let mut out = Vec::new();
    for st in parse_structures(table) {
        let (typ, f, strings) = (st.typ, &st.data[..], &st.strings);
        let len = f.len();
        if typ == 17 && len >= 0x15 {
            let u16at = |o: usize| (o + 2 <= len).then(|| u16::from_le_bytes([f[o], f[o + 1]]));
            let u32at = |o: usize| (o + 4 <= len).then(|| u32::from_le_bytes([f[o], f[o + 1], f[o + 2], f[o + 3]]));
            let s = |o: usize| -> Option<String> {
                let n = *f.get(o)? as usize;
                (n > 0).then(|| strings.get(n - 1).cloned()).flatten()
            };
            let size = u16at(0x0C).unwrap_or(0);
            let bytes = match size {
                0 | 0xFFFF => 0.0,
                0x7FFF => u32at(0x1C).map(|mb| (mb & 0x7FFF_FFFF) as f64 * 1048576.0).unwrap_or(0.0),
                v if v & 0x8000 != 0 => (v & 0x7FFF) as f64 * 1024.0,
                v => v as f64 * 1048576.0,
            };
            if bytes > 0.0 {
                let speed = |o: usize, ext: usize| -> Option<u32> {
                    match u16at(o)? {
                        0 => None,
                        0xFFFF => u32at(ext).filter(|v| *v > 0),
                        v => Some(v as u32),
                    }
                };
                let manufacturer = clean(s(0x17)).map(|m| jedec_name(&m).map(String::from).unwrap_or(m));
                out.push(Dimm {
                    slot: clean(s(0x10)).unwrap_or_default(),
                    bank: clean(s(0x11)).unwrap_or_default(),
                    size_bytes: bytes,
                    kind: mem_type_name(f[0x12]),
                    rated_mts: speed(0x15, 0x54),
                    configured_mts: speed(0x20, 0x58),
                    manufacturer,
                    part_number: clean(s(0x1A)),
                    volts: u16at(0x26).filter(|v| *v > 0).map(|mv| mv as f64 / 1000.0),
                    ranks: f.get(0x1B).map(|a| (a & 0x0F) as u32).filter(|r| *r > 0),
                });
            }
        }
    }
    out
}

/// Belegte Speicherriegel dieses Rechners (einmal gelesen).
pub fn modules() -> &'static [Dimm] {
    static M: std::sync::OnceLock<Vec<Dimm>> = std::sync::OnceLock::new();
    M.get_or_init(|| {
        let t = smbios_table();
        parse_smbios(&t)
    })
}

#[cfg(windows)]
fn smbios_table() -> Vec<u8> {
    use windows::Win32::System::SystemInformation::{GetSystemFirmwareTable, RSMB};
    unsafe {
        let n = GetSystemFirmwareTable(RSMB, 0, None);
        if n < 8 {
            return Vec::new();
        }
        let mut buf = vec![0u8; n as usize];
        let got = GetSystemFirmwareTable(RSMB, 0, Some(&mut buf));
        if got < 8 || got > n {
            return Vec::new();
        }
        // RawSMBIOSData: 4 Byte Versionen, 4 Byte Länge, dann die Tabelle
        let len = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
        buf[8..(8 + len).min(got as usize)].to_vec()
    }
}

/// Linux: die Tabelle liegt unter /sys (meist nur für root lesbar).
#[cfg(target_os = "linux")]
fn smbios_table() -> Vec<u8> {
    std::fs::read("/sys/firmware/dmi/tables/DMI").unwrap_or_default()
}

// ------------------------------------------------------------------ Timings (AMD UMC)

/// Laufende Speicher-Timings eines Kanals (in Takten).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Timings {
    pub channel: usize,
    /// Takt laut Speichercontroller (MT/s), nur bei DDR4 sicher bekannt
    pub mts: Option<f64>,
    pub cl: u32,
    pub rcd_rd: u32,
    pub rcd_wr: u32,
    pub rp: u32,
    pub ras: u32,
    pub rc: u32,
    pub rrds: u32,
    pub rrdl: u32,
    pub faw: u32,
    pub wtrs: u32,
    pub wtrl: u32,
    pub wr: u32,
    pub rtp: u32,
    pub cwl: u32,
    pub rfc: Option<u32>,
    /// Command Rate 2T (sonst 1T) – nur DDR4
    pub cmd2t: Option<bool>,
    /// Gear Down Mode – nur DDR4
    pub gdm: Option<bool>,
}

impl Timings {
    /// "16-19-19-39"
    pub fn primary(&self) -> String {
        format!("{}-{}-{}-{}", self.cl, self.rcd_rd, self.rp, self.ras)
    }
}

fn bits(v: u32, lo: u32, n: u32) -> u32 {
    (v >> lo) & ((1u32 << n) - 1)
}

/// UMC-Register eines Kanals → Timings. `ddr4` = Zen 2/3 (Familie 17h/19h mit DDR4).
/// Register DramTiming1–6 und DramTiming… (tRFC) wie im AMD PPR; Werte werden auf Plausibilität geprüft.
pub fn decode_umc(channel: usize, ddr4: bool, cfg: u32, t: [u32; 6], rfc: [u32; 4]) -> Option<Timings> {
    // Nicht vorhandener Kanal liest sich als 0 oder lauter Einsen
    if t.iter().any(|v| *v == u32::MAX) || t[0] == 0 {
        return None;
    }
    let mut x = Timings {
        channel,
        cl: bits(t[0], 0, 6),
        ras: bits(t[0], 8, 7),
        rcd_rd: bits(t[0], 16, 6),
        rcd_wr: bits(t[0], 24, 6),
        rc: bits(t[1], 0, 8),
        rp: bits(t[1], 16, 6),
        rrds: bits(t[2], 0, 5),
        rrdl: bits(t[2], 8, 5),
        rtp: bits(t[2], 24, 5),
        faw: bits(t[3], 0, 8),
        cwl: bits(t[4], 0, 6),
        wtrs: bits(t[4], 8, 5),
        wtrl: bits(t[4], 16, 7),
        wr: bits(t[5], 0, 8),
        ..Default::default()
    };
    let ok = (6..=80).contains(&x.cl)
        && (6..=100).contains(&x.rcd_rd)
        && (6..=100).contains(&x.rp)
        && x.ras >= x.cl
        && x.ras <= 127
        && x.rc >= x.ras
        && x.cwl > 0
        && x.cwl <= x.cl + 10;
    if !ok {
        return None;
    }
    if ddr4 {
        let ratio = bits(cfg, 0, 7);
        x.mts = (ratio > 0).then(|| (ratio as f64 / 3.0 * 200.0).round());
        x.cmd2t = Some(cfg & (1 << 10) != 0);
        x.gdm = Some(cfg & (1 << 11) != 0);
        // Erster belegter tRFC-Satz (0x21060138 = Grundeinstellung eines leeren Satzes)
        x.rfc = rfc.iter().find(|v| **v != 0 && **v != 0x2106_0138 && **v != u32::MAX).map(|v| bits(*v, 0, 11)).filter(|r| (50..=2047).contains(r));
    }
    Some(x)
}

/// Liest die Timings aller Kanäle über `read_smn` (AMD Zen; UMC n liegt bei 0x50000 + n · 0x100000).
pub fn read_umc(family: u32, read_smn: impl Fn(u64) -> Option<u32>) -> Vec<Timings> {
    let ddr4 = modules().first().map(|d| d.kind == "DDR4").unwrap_or(family < 0x19);
    let mut out = Vec::new();
    for ch in 0..8usize {
        let base = 0x50000u64 + ch as u64 * 0x100000;
        let r = |o: u64| read_smn(base + o);
        let (Some(cfg), Some(t1)) = (r(0x200), r(0x204)) else { break };
        if t1 == 0 || t1 == u32::MAX {
            continue;
        }
        let t = [t1, r(0x208).unwrap_or(0), r(0x20C).unwrap_or(0), r(0x210).unwrap_or(0), r(0x214).unwrap_or(0), r(0x218).unwrap_or(0)];
        let rfc = [r(0x260).unwrap_or(0), r(0x264).unwrap_or(0), r(0x268).unwrap_or(0), r(0x26C).unwrap_or(0)];
        if let Some(x) = decode_umc(ch, ddr4, cfg, t, rfc) {
            out.push(x);
        }
    }
    out
}

/// tRFC in Nanosekunden
pub fn rfc_ns(rfc: u32, mts: f64) -> Option<f64> {
    (mts > 0.0).then(|| rfc as f64 * 2000.0 / mts)
}

// ------------------------------------------------------------------ Temperatur der Riegel

/// DDR4-Temperatursensor (JC-42.4), Register 5 – höherwertiges Byte zuerst. °C
pub fn jc42_temp(raw: u16) -> f64 {
    let mut t = (raw & 0x0FFF) as f64 / 16.0;
    if raw & 0x1000 != 0 {
        t -= 256.0;
    }
    t
}

/// DDR5-SPD-Hub (SPD5118), MR49 (niedrig) und MR50 (hoch): Bits 12:2 in 0,25 °C, mit Vorzeichen.
pub fn spd5118_temp(raw: u16) -> f64 {
    let v = ((raw & 0x1FFC) >> 2) as i32;
    let v = if v & 0x400 != 0 { v - 0x800 } else { v };
    v as f64 * 0.25
}

/// Temperatur eines Riegels: SMBus-Adresse (bzw. Linux-Sensor) und °C.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DimmTemp {
    /// Steckplatz-Nummer am SMBus (0–7)
    pub index: u8,
    pub temp_c: f64,
}

#[cfg(windows)]
pub use win::RamSensors;

#[cfg(windows)]
mod win {
    use super::*;
    use crate::sensors::cputemp::{pawnio_dll_path, FnClose, FnExec, FnLoad, FnOpen};
    use windows::core::{s, HSTRING, PCSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, WAIT_ABANDONED, WAIT_OBJECT_0};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

    static PIIX4_MODULE: &[u8] = include_bytes!("../../resources/pawnio/SmbusPIIX4.bin");
    static I801_MODULE: &[u8] = include_bytes!("../../resources/pawnio/SmbusI801.bin");

    const READ: u64 = 1;
    const BYTE_DATA: u64 = 2;
    const WORD_DATA: u64 = 3;

    /// Sensoren auf den Riegeln über den SMBus (nur lesen).
    pub struct RamSensors {
        handle: HANDLE,
        exec: FnExec,
        close: FnClose,
        mutex: HANDLE,
        /// gefundene Sensoren: (Steckplatz, Adresse, DDR5?)
        found: Vec<(u8, u8, bool)>,
        _lib: HMODULE,
    }

    unsafe impl Send for RamSensors {}

    impl RamSensors {
        /// Öffnet den SMBus und sucht die Sensoren. None = kein SMBus/keine Sensoren/keine Rechte.
        pub fn open(intel: bool) -> Option<RamSensors> {
            let module = if intel { I801_MODULE } else { PIIX4_MODULE };
            let lib = pawnio_dll_path().into_iter().find_map(|p| unsafe { LoadLibraryW(&HSTRING::from(p)).ok() })?;
            unsafe {
                let get = |name: PCSTR| GetProcAddress(lib, name);
                let (Some(o), Some(l), Some(e), Some(c)) = (get(s!("pawnio_open")), get(s!("pawnio_load")), get(s!("pawnio_execute")), get(s!("pawnio_close"))) else {
                    return None;
                };
                let open: FnOpen = std::mem::transmute(o);
                let load: FnLoad = std::mem::transmute(l);
                let exec: FnExec = std::mem::transmute(e);
                let close: FnClose = std::mem::transmute(c);
                let mut h = HANDLE::default();
                if open(&mut h) < 0 {
                    return None;
                }
                let hr = load(h, module.as_ptr(), module.len());
                if hr < 0 {
                    close(h);
                    crate::diag::log(format!("RAM-Sensoren: SMBus-Modul nicht geladen 0x{:08X}", hr as u32));
                    return None;
                }
                let Ok(mutex) = CreateMutexW(None, false, &HSTRING::from(r"Global\Access_SMBUS.HTP.Method")) else {
                    close(h);
                    return None;
                };
                let mut me = RamSensors { handle: h, exec, close, mutex, found: Vec::new(), _lib: lib };
                me.detect();
                if me.found.is_empty() {
                    return None;
                }
                Some(me)
            }
        }

        fn read(&self, addr: u8, cmd: u8, proto: u64) -> Option<u16> {
            let input = [addr as u64, READ, cmd as u64, proto];
            let mut out = [0u64; 1];
            let mut ret = 0usize;
            let hr = unsafe { (self.exec)(self.handle, s!("ioctl_smbus_xfer"), input.as_ptr(), input.len(), out.as_mut_ptr(), 1, &mut ret) };
            (hr >= 0).then_some(out[0] as u16)
        }

        /// Führt `f` unter der gemeinsamen SMBus-Sperre aus (andere Programme warten solange).
        fn locked<T>(&self, f: impl FnOnce() -> T) -> Option<T> {
            unsafe {
                let w = WaitForSingleObject(self.mutex, 200);
                if w != WAIT_OBJECT_0 && w != WAIT_ABANDONED {
                    return None;
                }
                let r = f();
                let _ = ReleaseMutex(self.mutex);
                Some(r)
            }
        }

        fn detect(&mut self) {
            let kind = modules().first().map(|d| d.kind.clone()).unwrap_or_default();
            let mut found = Vec::new();
            let _ = self.locked(|| {
                for n in 0..8u8 {
                    // DDR5: SPD-Hub meldet sich in MR0/MR1 mit 0x51 0x18 („5118“)
                    if kind != "DDR4" {
                        let a = 0x50 + n;
                        if let (Some(m0), Some(m1)) = (self.read(a, 0x00, BYTE_DATA), self.read(a, 0x01, BYTE_DATA)) {
                            // MR26 Bit 0 = Temperatursensor abgeschaltet
                            let ts_on = self.read(a, 0x1A, BYTE_DATA).map(|v| v & 1 == 0).unwrap_or(false);
                            if m0 & 0xFF == 0x51 && m1 & 0xFF == 0x18 && ts_on {
                                found.push((n, a, true));
                                continue;
                            }
                        }
                    }
                    // DDR4: JC-42.4-Sensor; Herstellerkennung (Register 6) darf nicht leer sein
                    if kind != "DDR5" {
                        let a = 0x18 + n;
                        if let (Some(m), Some(t)) = (self.read(a, 0x06, WORD_DATA), self.read(a, 0x05, WORD_DATA)) {
                            let m = m.swap_bytes();
                            let t = jc42_temp(t.swap_bytes());
                            if m != 0 && m != 0xFFFF && (0.0..110.0).contains(&t) {
                                found.push((n, a, false));
                            }
                        }
                    }
                }
            });
            self.found = found;
        }

        pub fn temps(&self) -> Vec<DimmTemp> {
            self.locked(|| {
                self.found
                    .iter()
                    .filter_map(|&(n, a, ddr5)| {
                        let t = if ddr5 { spd5118_temp(self.read(a, 0x31, WORD_DATA)?) } else { jc42_temp(self.read(a, 0x05, WORD_DATA)?.swap_bytes()) };
                        (-20.0..120.0).contains(&t).then_some(DimmTemp { index: n, temp_c: t })
                    })
                    .collect()
            })
            .unwrap_or_default()
        }
    }

    impl Drop for RamSensors {
        fn drop(&mut self) {
            unsafe {
                (self.close)(self.handle);
                let _ = CloseHandle(self.mutex);
            }
        }
    }
}

/// Linux: die Kernel-Treiber jc42 (DDR4) und spd5118 (DDR5) melden die Riegeltemperatur über hwmon.
#[cfg(target_os = "linux")]
pub struct RamSensors {
    dirs: Vec<std::path::PathBuf>,
}

#[cfg(target_os = "linux")]
impl RamSensors {
    pub fn open(_intel: bool) -> Option<RamSensors> {
        let mut dirs: Vec<std::path::PathBuf> = super::sysfs::dirs("/sys/class/hwmon")
            .into_iter()
            .filter(|d| matches!(super::sysfs::read(d.join("name")).as_deref().map(str::trim), Some("jc42") | Some("spd5118")))
            .collect();
        // nach Gerät (I²C-Adresse) sortieren, damit die Reihenfolge den Steckplätzen folgt
        dirs.sort_by_key(|d| std::fs::canonicalize(d.join("device")).unwrap_or_default());
        (!dirs.is_empty()).then_some(RamSensors { dirs })
    }

    pub fn temps(&self) -> Vec<DimmTemp> {
        self.dirs
            .iter()
            .enumerate()
            .filter_map(|(i, d)| super::sysfs::read_f64(d.join("temp1_input")).map(|m| DimmTemp { index: i as u8, temp_c: m / 1000.0 }))
            .collect()
    }
}

/// Ordnet gefundene Temperaturen den belegten Riegeln zu (beide nach Steckplatz-Reihenfolge).
/// Passt die Anzahl nicht, gibt es keine Zuordnung (dann zeigt die Oberfläche die Werte als Liste).
pub fn assign_temps(dimms: usize, temps: &[DimmTemp]) -> Option<Vec<f64>> {
    (dimms > 0 && temps.len() == dimms).then(|| {
        let mut t = temps.to_vec();
        t.sort_by_key(|x| x.index);
        t.iter().map(|x| x.temp_c).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Baut einen SMBIOS-Typ-17-Eintrag (Länge 0x5C, wie SMBIOS 3.3).
    fn type17(size_mb: u16, kind: u8, speed: u16, conf: u16, mv: u16, strings: &[&str]) -> Vec<u8> {
        let mut f = vec![0u8; 0x5C];
        f[0] = 17;
        f[1] = 0x5C;
        f[0x0C..0x0E].copy_from_slice(&size_mb.to_le_bytes());
        f[0x10] = 1; // Steckplatz
        f[0x11] = 2; // Bank
        f[0x12] = kind;
        f[0x15..0x17].copy_from_slice(&speed.to_le_bytes());
        f[0x17] = 3; // Hersteller
        f[0x1A] = 4; // Teilenummer
        f[0x1B] = 2; // 2 Ranks
        f[0x20..0x22].copy_from_slice(&conf.to_le_bytes());
        f[0x26..0x28].copy_from_slice(&mv.to_le_bytes());
        for s in strings {
            f.extend_from_slice(s.as_bytes());
            f.push(0);
        }
        if strings.is_empty() {
            f.push(0);
        }
        f.push(0);
        f
    }

    #[test]
    fn smbios_memory_devices() {
        let mut t = Vec::new();
        // Typ 0 (BIOS) ohne Zeichenketten
        t.extend_from_slice(&[0, 4, 0, 0, 0, 0]);
        t.extend(type17(16384, 0x1A, 3200, 3600, 1350, &["DIMM_A2", "P0 CHANNEL A", "G Skill Intl", "F4-3600C16-16GTZNC"]));
        t.extend(type17(0, 0x02, 0, 0, 0, &["DIMM_A1", "P0 CHANNEL A", "Unknown", "Unknown"])); // leer
        // 32 GB passt nicht ins kurze Feld → 0x7FFF und erweiterte Größe (MB) bei 0x1C
        let mut big = type17(0x7FFF, 0x22, 6000, 6000, 1350, &["DIMM_B2", "P0 CHANNEL B", "80CE000080CE", "M323R4GA3DB0-CQKOD"]);
        big[0x1C..0x20].copy_from_slice(&32768u32.to_le_bytes());
        t.extend(big);
        t.extend_from_slice(&[127, 4, 0, 0, 0, 0]);
        let d = parse_smbios(&t);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].slot, "DIMM_A2");
        assert_eq!(d[0].bank, "P0 CHANNEL A");
        assert_eq!(d[0].kind, "DDR4");
        assert_eq!(d[0].size_bytes, 16.0 * 1073741824.0);
        assert_eq!(d[0].rated_mts, Some(3200));
        assert_eq!(d[0].configured_mts, Some(3600));
        assert_eq!(d[0].volts, Some(1.35));
        assert_eq!(d[0].ranks, Some(2));
        assert_eq!(d[0].manufacturer.as_deref(), Some("G Skill Intl"));
        assert_eq!(d[1].kind, "DDR5");
        assert_eq!(d[1].size_bytes, 32.0 * 1073741824.0);
        assert_eq!(d[1].manufacturer.as_deref(), Some("Samsung"));
        assert_eq!(d[1].part_number.as_deref(), Some("M323R4GA3DB0-CQKOD"));
    }

    #[test]
    fn umc_timings_ddr4() {
        // DDR4-3600 16-19-19-39, tRC 58, tRRDS 4, tRRDL 6, tRTP 12, tFAW 16, tCWL 16, tWTRS 4, tWTRL 12, tWR 24, tRFC 630, 1T, GDM an
        let cfg = 54 | (1 << 11);
        let t1 = 16 | (39 << 8) | (19 << 16) | (19 << 24);
        let t2 = 58 | (19 << 16);
        let t3 = 4 | (6 << 8) | (12 << 24);
        let t4 = 16;
        let t5 = 16 | (4 << 8) | (12 << 16);
        let t6 = 24;
        let rfc = [0x2106_0138, 630 | (468 << 11) | (288 << 22), 0, 0];
        let x = decode_umc(0, true, cfg, [t1, t2, t3, t4, t5, t6], rfc).unwrap();
        assert_eq!(x.primary(), "16-19-19-39");
        assert_eq!((x.rcd_wr, x.rc, x.rrds, x.rrdl, x.rtp, x.faw), (19, 58, 4, 6, 12, 16));
        assert_eq!((x.cwl, x.wtrs, x.wtrl, x.wr), (16, 4, 12, 24));
        assert_eq!(x.mts, Some(3600.0));
        assert_eq!(x.cmd2t, Some(false));
        assert_eq!(x.gdm, Some(true));
        assert_eq!(x.rfc, Some(630));
        assert_eq!(rfc_ns(630, 3600.0).map(|n| n.round()), Some(350.0));
        // Unsinn (leerer Kanal) wird verworfen
        assert!(decode_umc(1, true, 0, [0, 0, 0, 0, 0, 0], [0; 4]).is_none());
        assert!(decode_umc(1, true, 0, [u32::MAX; 6], [0; 4]).is_none());
    }

    #[test]
    fn dimm_temperatures() {
        // JC-42.4: 0x01E8 = 30,5 °C; negatives Vorzeichen
        assert_eq!(jc42_temp(0x01E8), 30.5);
        assert_eq!(jc42_temp(0x1FF0), -1.0);
        // SPD5118: 40,25 °C = 161 × 0,25 → 161 << 2
        assert_eq!(spd5118_temp(161 << 2), 40.25);
        assert_eq!(spd5118_temp(((0x800 - 8) << 2) as u16), -2.0);
        let t = [DimmTemp { index: 3, temp_c: 41.0 }, DimmTemp { index: 1, temp_c: 39.0 }];
        assert_eq!(assign_temps(2, &t), Some(vec![39.0, 41.0]));
        assert_eq!(assign_temps(4, &t), None);
    }
}
