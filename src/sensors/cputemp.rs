//! CPU-Temperatur und -Leistung über den PawnIO-Treiber (nicht WinRing0).
//! Benötigt Adminrechte und ein installiertes PawnIO.
//!
//! AMD Ryzen (Familie 17h/19h/1Ah): Tctl im SMN-Register 0x00059800, Energie über RAPL-MSRs.
//! Intel (Core ab ca. 2. Generation): Package-Temperatur = TjMax − Abstand aus
//! IA32_PACKAGE_THERM_STATUS, Energie über MSR_PKG_ENERGY_STATUS – wie Linux coretemp/RAPL.

use windows::core::{s, HSTRING, PCSTR};
use windows::Win32::Foundation::{HANDLE, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

static AMD_FAMILY17_MODULE: &[u8] = include_bytes!("../../resources/pawnio/AMDFamily17.bin");
static INTEL_MSR_MODULE: &[u8] = include_bytes!("../../resources/pawnio/IntelMSR.bin");

const MSR_IA32_THERM_STATUS: u64 = 0x19C;
const MSR_IA32_TEMPERATURE_TARGET: u64 = 0x1A2;
const MSR_IA32_PACKAGE_THERM_STATUS: u64 = 0x1B1;
const MSR_RAPL_POWER_UNIT: u64 = 0x606;
const MSR_PKG_ENERGY_STATUS: u64 = 0x611;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Amd,
    Intel,
}

const SMN_THM_TCON_CUR_TMP: u64 = 0x0005_9800;
/// RAPL-Einheiten (Bits 12:8 = Energieeinheit 1/2^ESU Joule)
const MSR_PWR_UNIT: u64 = 0xC001_0299;
/// Aufsummierte Energie des gesamten Prozessors (32 Bit, läuft über)
const MSR_PKG_ENERGY_STAT: u64 = 0xC001_029B;
/// Aufsummierte Energie des Kerns, auf dem gelesen wird (AMD Zen, 32 Bit)
const MSR_CORE_ENERGY_STAT: u64 = 0xC001_029A;
/// Hardware-P-State-Status des Kerns: CurCpuVid in Bit 21:14 (AMD Zen)
const MSR_HW_PSTATE_STATUS: u64 = 0xC001_0293;
/// IA32_PERF_STATUS: aktuelle Kernspannung in Bit 47:32 (Einheit 1/8192 V, Intel)
const MSR_IA32_PERF_STATUS: u64 = 0x198;

/// Messwerte eines physischen Kerns.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CoreReading {
    /// Logische Prozessoren dieses Kerns (global nummeriert)
    pub lps: Vec<usize>,
    pub watts: Option<f64>,
    /// Kernspannung: AMD = angeforderte VID, Intel = gemeldete Spannung
    pub volts: Option<f64>,
    /// Temperatur des Kerns (nur Intel; AMD meldet sie je CCD, siehe `ccd_temps`)
    pub temp_c: Option<f64>,
}

/// Abstand der CCD-Temperaturregister (Tccd) zu 0x59800 je AMD-Generation und Höchstzahl der CCDs
/// – Werte wie im Linux-Treiber k10temp.
pub fn ccd_layout(family: u32, model: u32) -> Option<(u64, usize)> {
    Some(match (family, model) {
        (0x17, 0x01 | 0x08 | 0x11 | 0x18) => (0x154, 4),
        (0x17, 0x31 | 0x47 | 0x60 | 0x68 | 0x71) => (0x154, 8),
        (0x17, 0xA0..=0xAF) => (0x300, 8),
        (0x19, 0x00..=0x01 | 0x08 | 0x21 | 0x50..=0x5F) => (0x154, 8),
        (0x19, 0x40..=0x4F) => (0x300, 8),
        (0x19, 0x60..=0x7F) => (0x308, 8),
        (0x19, 0x10..=0x1F | 0xA0..=0xAF) => (0x300, 12),
        (0x1A, 0x00..=0x1F) => (0x1F0, 16),
        (0x1A, 0x40..=0x4F) => (0x308, 8),
        _ => return None,
    })
}

/// Tccd-Rohwert → °C: Bit 11 = gültig, Bit 10:0 in 1/8 °C, minus 49 °C.
pub fn decode_ccd(raw: u32) -> Option<f64> {
    if raw == 0xFFFF_FFFF || raw & (1 << 11) == 0 {
        return None;
    }
    let t = (raw & 0x7FF) as f64 * 0.125 - 49.0;
    (-20.0..150.0).contains(&t).then_some(t)
}

/// AMD: Spannung aus der VID. Zen bis Zen 3 nutzen SVI2 (1,55 V − VID × 6,25 mV),
/// Zen 4/Zen 5 SVI3 (0,245 V + VID × 5 mV).
pub fn amd_vid_volts(vid: u32, family: u32, model: u32) -> Option<f64> {
    if vid == 0 {
        return None;
    }
    let svi3 = family >= 0x1A || (family == 0x19 && matches!(model, 0x10..=0x1F | 0x60..=0x7F | 0xA0..=0xAF));
    let v = if svi3 { 0.245 + vid as f64 * 0.005 } else { 1.55 - vid as f64 * 0.00625 };
    (0.2..2.0).contains(&v).then_some(v)
}

/// Intel: IA32_PERF_STATUS Bit 47:32 / 8192 V
pub fn intel_core_volts(perf_status: u64) -> Option<f64> {
    let v = ((perf_status >> 32) & 0xFFFF) as f64 / 8192.0;
    (0.2..2.0).contains(&v).then_some(v)
}

/// Bindet den aufrufenden Thread für die Dauer eines Lesevorgangs an einen logischen Prozessor
/// (MSRs gelten je Kern; PawnIO liest auf dem Prozessor, auf dem der Thread gerade läuft).
struct PinGuard(Option<windows::Win32::System::SystemInformation::GROUP_AFFINITY>);

impl PinGuard {
    fn pin(lp: usize) -> PinGuard {
        use windows::Win32::System::SystemInformation::GROUP_AFFINITY;
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadGroupAffinity};
        let want = GROUP_AFFINITY { Mask: 1usize << (lp % 64), Group: (lp / 64) as u16, Reserved: [0; 3] };
        let mut old = GROUP_AFFINITY::default();
        let ok = unsafe { SetThreadGroupAffinity(GetCurrentThread(), &want, Some(&mut old)) }.as_bool();
        if ok {
            // Wechsel auf den neuen Prozessor abwarten
            std::thread::yield_now();
        }
        PinGuard(ok.then_some(old))
    }
}

impl Drop for PinGuard {
    fn drop(&mut self) {
        if let Some(old) = self.0 {
            unsafe {
                let _ = windows::Win32::System::Threading::SetThreadGroupAffinity(windows::Win32::System::Threading::GetCurrentThread(), &old, None);
            }
        }
    }
}

type FnOpen = unsafe extern "system" fn(*mut HANDLE) -> i32;
type FnLoad = unsafe extern "system" fn(HANDLE, *const u8, usize) -> i32;
type FnExec = unsafe extern "system" fn(HANDLE, PCSTR, *const u64, usize, *mut u64, usize, *mut usize) -> i32;
type FnClose = unsafe extern "system" fn(HANDLE) -> i32;

#[derive(Debug, Clone, serde::Serialize)]
pub enum TempStatus {
    Ok,
    Disabled,
    Unsupported,
    NoPawnIo,
    NeedsAdmin,
    Failed(String),
}

impl TempStatus {
    pub fn describe(&self) -> String {
        match self {
            TempStatus::Ok => "PawnIO aktiv".into(),
            TempStatus::Disabled => "In den Einstellungen abgeschaltet".into(),
            TempStatus::Unsupported if crate::winutil::native_arm64() => "Auf ARM-Prozessoren (z. B. NVIDIA RTX Spark, Snapdragon) liest TaskPulse keine CPU-Temperatur".into(),
            TempStatus::Unsupported => "Dieser Prozessor wird nicht unterstützt (nur AMD Ryzen und Intel Core)".into(),
            TempStatus::NoPawnIo => "PawnIO ist nicht installiert (pawnio.eu)".into(),
            TempStatus::NeedsAdmin => "Benötigt Adminrechte".into(),
            TempStatus::Failed(e) => format!("Fehler: {e}"),
        }
    }
}

#[allow(dead_code)]
pub struct CpuInfo {
    pub vendor: String,
    pub family: u32,
    pub model: u32,
    pub brand: String,
}

pub fn cpu_info() -> CpuInfo {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::__cpuid;
        let r0 = __cpuid(0);
        let mut v = Vec::new();
        for x in [r0.ebx, r0.edx, r0.ecx] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        let vendor = String::from_utf8_lossy(&v).to_string();
        let r1 = __cpuid(1);
        let base_family = (r1.eax >> 8) & 0xF;
        let ext_family = (r1.eax >> 20) & 0xFF;
        let family = if base_family == 0xF { base_family + ext_family } else { base_family };
        let model = ((r1.eax >> 4) & 0xF) | (((r1.eax >> 16) & 0xF) << 4);
        let mut brand_bytes = Vec::new();
        let maxext = __cpuid(0x8000_0000).eax;
        if maxext >= 0x8000_0004 {
            for leaf in 0x8000_0002u32..=0x8000_0004 {
                let r = __cpuid(leaf);
                for x in [r.eax, r.ebx, r.ecx, r.edx] {
                    brand_bytes.extend_from_slice(&x.to_le_bytes());
                }
            }
        }
        let brand = String::from_utf8_lossy(&brand_bytes).trim_matches(char::from(0)).trim().to_string();
        CpuInfo { vendor, family, model, brand }
    }
    #[cfg(not(target_arch = "x86_64"))]
    CpuInfo { vendor: String::new(), family: 0, model: 0, brand: String::new() }
}

/// Tctl-Offset älterer Ryzen-Modelle (aus Linux k10temp). Zen 2/3 (z. B. 5950X) haben keinen Offset.
pub fn tctl_offset(brand: &str) -> f64 {
    const TABLE: &[(&str, f64)] = &[
        ("1600X", 20.0),
        ("1700X", 20.0),
        ("1800X", 20.0),
        ("2700X", 10.0),
        ("Threadripper 19", 27.0),
        ("Threadripper 29", 27.0),
    ];
    for (k, off) in TABLE {
        if brand.contains(k) {
            return *off;
        }
    }
    0.0
}

/// Rechnet den SMN-Rohwert in °C um.
pub fn decode_tctl(raw: u32, offset: f64) -> f64 {
    let mut t = (raw >> 21) as f64 * 0.125;
    if raw & 0x80000 != 0 {
        t -= 49.0;
    }
    t - offset
}

pub struct CpuTemp {
    handle: HANDLE,
    exec: FnExec,
    close: FnClose,
    offset: f64,
    vendor: Vendor,
    tjmax: f64,
    energy_unit_j: Option<f64>,
    last_energy: Option<(u32, std::time::Instant)>,
    /// Letzte Energie je Kern (erster LP des Kerns → Zählerstand, Zeit)
    core_energy: std::collections::HashMap<usize, (u32, std::time::Instant)>,
    family: u32,
    model: u32,
    _lib: HMODULE,
}

// Der Handle wird nur vom Messthread benutzt.
unsafe impl Send for CpuTemp {}

fn pawnio_dll_path() -> Vec<String> {
    let mut v = Vec::new();
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        v.push(format!("{}\\PawnIO\\PawnIOLib.dll", pf.to_string_lossy()));
    }
    v.push(r"C:\Program Files\PawnIO\PawnIOLib.dll".into());
    v.push("PawnIOLib.dll".into());
    v
}

impl CpuTemp {
    pub fn open() -> Result<CpuTemp, TempStatus> {
        // ARM-Prozessoren: PawnIO-Module gibt es nur für AMD/Intel (MSR/SMN)
        if crate::winutil::native_arm64() {
            return Err(TempStatus::Unsupported);
        }
        let info = cpu_info();
        let (vendor, module) = match info.vendor.as_str() {
            "AuthenticAMD" if info.family >= 0x17 => (Vendor::Amd, AMD_FAMILY17_MODULE),
            "GenuineIntel" => (Vendor::Intel, INTEL_MSR_MODULE),
            _ => return Err(TempStatus::Unsupported),
        };
        let lib = pawnio_dll_path()
            .into_iter()
            .find_map(|p| unsafe { LoadLibraryW(&HSTRING::from(p)).ok() })
            .ok_or(TempStatus::NoPawnIo)?;
        unsafe {
            let get = |name: PCSTR| GetProcAddress(lib, name);
            let (Some(o), Some(l), Some(e), Some(c)) =
                (get(s!("pawnio_open")), get(s!("pawnio_load")), get(s!("pawnio_execute")), get(s!("pawnio_close")))
            else {
                return Err(TempStatus::Failed("PawnIOLib.dll unvollständig".into()));
            };
            let open: FnOpen = std::mem::transmute(o);
            let load: FnLoad = std::mem::transmute(l);
            let exec: FnExec = std::mem::transmute(e);
            let close: FnClose = std::mem::transmute(c);
            let mut h = HANDLE::default();
            let hr = open(&mut h);
            if hr < 0 {
                // E_ACCESSDENIED = 0x80070005
                return Err(if hr as u32 == 0x8007_0005 || !crate::winutil::is_elevated() {
                    TempStatus::NeedsAdmin
                } else {
                    TempStatus::Failed(format!("pawnio_open 0x{:08X}", hr as u32))
                });
            }
            let hr = load(h, module.as_ptr(), module.len());
            if hr < 0 {
                close(h);
                return Err(TempStatus::Failed(format!("Modul nicht geladen 0x{:08X}", hr as u32)));
            }
            let offset = if vendor == Vendor::Amd { tctl_offset(&info.brand) } else { 0.0 };
            let mut t = CpuTemp {
                handle: h,
                exec,
                close,
                offset,
                vendor,
                tjmax: 100.0,
                energy_unit_j: None,
                last_energy: None,
                core_energy: std::collections::HashMap::new(),
                family: info.family,
                model: info.model,
                _lib: lib,
            };
            match vendor {
                Vendor::Amd => {
                    t.energy_unit_j = t.read_msr(MSR_PWR_UNIT).map(|v| energy_unit(v as u32));
                    // Probe
                    t.read_smn(SMN_THM_TCON_CUR_TMP).ok_or_else(|| TempStatus::Failed("SMN-Lesen fehlgeschlagen".into()))?;
                }
                Vendor::Intel => {
                    t.tjmax = t.read_msr(MSR_IA32_TEMPERATURE_TARGET).map(|v| intel_tjmax(v)).unwrap_or(100.0);
                    t.energy_unit_j = t.read_msr(MSR_RAPL_POWER_UNIT).map(|v| energy_unit(v as u32));
                    t.read().ok_or_else(|| TempStatus::Failed("Intel-Temperatur nicht lesbar".into()))?;
                }
            }
            Ok(t)
        }
    }

    fn read_smn(&self, addr: u64) -> Option<u32> {
        let input = [addr];
        let mut out = [0u64; 1];
        let mut ret = 0usize;
        let hr = unsafe { (self.exec)(self.handle, s!("ioctl_read_smn"), input.as_ptr(), 1, out.as_mut_ptr(), 1, &mut ret) };
        (hr >= 0 && ret >= 1).then_some(out[0] as u32)
    }

    fn read_msr(&self, msr: u64) -> Option<u64> {
        let input = [msr];
        let mut out = [0u64; 1];
        let mut ret = 0usize;
        let hr = unsafe { (self.exec)(self.handle, s!("ioctl_read_msr"), input.as_ptr(), 1, out.as_mut_ptr(), 1, &mut ret) };
        (hr >= 0 && ret >= 1).then_some(out[0])
    }

    /// Leistungsaufnahme des ganzen Prozessors (Package) in Watt, aus der Energiedifferenz.
    pub fn power(&mut self) -> Option<f64> {
        let unit = self.energy_unit_j?;
        let msr = if self.vendor == Vendor::Amd { MSR_PKG_ENERGY_STAT } else { MSR_PKG_ENERGY_STATUS };
        let raw = self.read_msr(msr)? as u32;
        let now = std::time::Instant::now();
        let prev = self.last_energy.replace((raw, now));
        let (e0, t0) = prev?;
        package_watts(e0, raw, now.duration_since(t0).as_secs_f64(), unit)
    }

    /// Watt und Spannung je physischem Kern. `cores` = logische Prozessoren je Kern.
    /// AMD: Energie je Kern (Watt) und VID; Intel: nur Spannung – Watt je Kern meldet Intel nicht.
    pub fn per_core(&mut self, cores: &[Vec<usize>]) -> Vec<CoreReading> {
        let mut out = Vec::with_capacity(cores.len());
        for lps in cores {
            let Some(&lp) = lps.first() else { continue };
            let mut r = CoreReading { lps: lps.clone(), ..Default::default() };
            let pin = PinGuard::pin(lp);
            if pin.0.is_none() {
                out.push(r);
                continue;
            }
            match self.vendor {
                Vendor::Amd => {
                    if let (Some(unit), Some(raw)) = (self.energy_unit_j, self.read_msr(MSR_CORE_ENERGY_STAT)) {
                        let now = std::time::Instant::now();
                        if let Some((e0, t0)) = self.core_energy.insert(lp, (raw as u32, now)) {
                            r.watts = package_watts(e0, raw as u32, now.duration_since(t0).as_secs_f64(), unit).filter(|w| *w < 1000.0);
                        }
                    }
                    if let Some(st) = self.read_msr(MSR_HW_PSTATE_STATUS) {
                        r.volts = amd_vid_volts(((st >> 14) & 0xFF) as u32, self.family, self.model);
                    }
                }
                Vendor::Intel => {
                    r.volts = self.read_msr(MSR_IA32_PERF_STATUS).and_then(intel_core_volts);
                    // Kerntemperatur: Bit 31 = gültig
                    r.temp_c = self
                        .read_msr(MSR_IA32_THERM_STATUS)
                        .filter(|v| v & (1 << 31) != 0)
                        .map(|v| intel_temp(self.tjmax, v))
                        .filter(|t| (-20.0..150.0).contains(t));
                }
            }
            drop(pin);
            out.push(r);
        }
        out
    }

    /// AMD: Temperatur je CCD (nur die vorhandenen, in Hardware-Reihenfolge). Intel: leer.
    pub fn ccd_temps(&self) -> Vec<f64> {
        if self.vendor != Vendor::Amd {
            return Vec::new();
        }
        let Some((offset, max)) = ccd_layout(self.family, self.model) else { return Vec::new() };
        (0..max).filter_map(|i| self.read_smn(SMN_THM_TCON_CUR_TMP + offset + 4 * i as u64).and_then(decode_ccd)).collect()
    }

    pub fn read(&self) -> Option<f64> {
        let t = match self.vendor {
            Vendor::Amd => decode_tctl(self.read_smn(SMN_THM_TCON_CUR_TMP)?, self.offset),
            Vendor::Intel => {
                // Package-Sensor, sonst Kernsensor des aktuellen Kerns (Bit 31 = Wert gültig)
                match self.read_msr(MSR_IA32_PACKAGE_THERM_STATUS) {
                    Some(v) if v != 0 => intel_temp(self.tjmax, v),
                    _ => {
                        let v = self.read_msr(MSR_IA32_THERM_STATUS)?;
                        if v & (1 << 31) == 0 {
                            return None;
                        }
                        intel_temp(self.tjmax, v)
                    }
                }
            }
        };
        (-20.0..150.0).contains(&t).then_some(t)
    }
}

impl Drop for CpuTemp {
    fn drop(&mut self) {
        unsafe {
            (self.close)(self.handle);
        }
    }
}

/// TjMax aus IA32_TEMPERATURE_TARGET (Bits 23:16), Rückfall 100 °C.
pub fn intel_tjmax(v: u64) -> f64 {
    let t = ((v >> 16) & 0xFF) as f64;
    if (60.0..=130.0).contains(&t) {
        t
    } else {
        100.0
    }
}

/// Temperatur = TjMax − Abstand (Bits 22:16 des Therm-Status-Registers).
pub fn intel_temp(tjmax: f64, therm_status: u64) -> f64 {
    tjmax - ((therm_status >> 16) & 0x7F) as f64
}

/// Energieeinheit in Joule aus MSR_PWR_UNIT.
pub fn energy_unit(pwr_unit: u32) -> f64 {
    1.0 / (1u64 << ((pwr_unit >> 8) & 0x1F)) as f64
}

/// Watt aus zwei Zählerständen; berücksichtigt den Überlauf des 32-Bit-Zählers.
pub fn package_watts(e0: u32, e1: u32, dt_s: f64, unit_j: f64) -> Option<f64> {
    if !(dt_s > 0.05) {
        return None;
    }
    let delta = e1.wrapping_sub(e0) as f64 * unit_j;
    let w = delta / dt_s;
    (0.0..2000.0).contains(&w).then_some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode() {
        // 47,0 °C -> 376 * 0.125
        let raw = 376u32 << 21;
        assert!((decode_tctl(raw, 0.0) - 47.0).abs() < 1e-9);
        // Mit Range-Select-Bit: −49
        let raw = (800u32 << 21) | 0x80000;
        assert!((decode_tctl(raw, 0.0) - (100.0 - 49.0)).abs() < 1e-9);
        assert!((decode_tctl(376u32 << 21, 20.0) - 27.0).abs() < 1e-9);
    }

    #[test]
    fn rapl_power() {
        // Zen: ESU = 16 -> 15,26 µJ
        let u = energy_unit(0x000A_1003);
        assert!((u - 1.0 / 65536.0).abs() < 1e-12);
        // 100 W über 1 s = 100 J = 6 553 600 Einheiten
        let w = package_watts(1000, 1000 + 6_553_600, 1.0, u).unwrap();
        assert!((w - 100.0).abs() < 1e-6);
        // Überlauf des 32-Bit-Zählers
        let w = package_watts(u32::MAX - 100, 6_553_500 - 1, 1.0, u).unwrap();
        assert!((w - 100.0).abs() < 1e-6);
        assert_eq!(package_watts(0, 10, 0.0, u), None);
    }

    #[test]
    fn ccd_temperatures() {
        // 5950X (19h/21h): Offset 0x154, bis 8 CCDs → Register 0x59954, 0x59958 …
        assert_eq!(ccd_layout(0x19, 0x21), Some((0x154, 8)));
        assert_eq!(ccd_layout(0x19, 0x61), Some((0x308, 8))); // Zen 4 Desktop
        assert_eq!(ccd_layout(0x1A, 0x44), Some((0x308, 8))); // Zen 5 Desktop
        assert_eq!(ccd_layout(0x19, 0x11), Some((0x300, 12))); // Genoa
        assert_eq!(ccd_layout(0x06, 0x97), None);
        // gültig, 0x2F0 = 752 → 752/8 − 49 = 45 °C
        assert_eq!(decode_ccd(0x800 | 0x2F0), Some(45.0));
        assert_eq!(decode_ccd(0x2F0), None); // Bit 11 fehlt
        assert_eq!(decode_ccd(0xFFFF_FFFF), None);
    }

    #[test]
    fn core_voltages() {
        // Zen 3 (5950X, Familie 19h Modell 21h): SVI2; VID 0x30 = 48 → 1,55 − 0,3 = 1,25 V
        assert!((amd_vid_volts(0x30, 0x19, 0x21).unwrap() - 1.25).abs() < 1e-9);
        // Zen 4 (7950X, Modell 61h): SVI3; VID 200 → 0,245 + 1,0 = 1,245 V
        assert!((amd_vid_volts(200, 0x19, 0x61).unwrap() - 1.245).abs() < 1e-9);
        // Zen 5 (Familie 1Ah): SVI3
        assert!((amd_vid_volts(180, 0x1A, 0x44).unwrap() - 1.145).abs() < 1e-9);
        assert_eq!(amd_vid_volts(0, 0x19, 0x21), None);
        // Intel: 0x2800 / 8192 = 1,25 V in Bit 47:32
        assert!((intel_core_volts(0x2800u64 << 32).unwrap() - 1.25).abs() < 1e-9);
        assert_eq!(intel_core_volts(0), None);
    }

    #[test]
    fn intel_decode() {
        // TjMax 100 °C, Abstand 52 -> 48 °C
        assert_eq!(intel_tjmax(0x0064_0000), 100.0);
        assert_eq!(intel_tjmax(0x005F_0A00), 95.0);
        assert_eq!(intel_tjmax(0), 100.0);
        assert_eq!(intel_temp(100.0, 0x8834_0000), 48.0);
        // Intel-RAPL: ESU 14 -> 61 µJ
        assert!((energy_unit(0x000A_0E03) - 1.0 / 16384.0).abs() < 1e-12);
    }

    #[test]
    fn offsets() {
        assert_eq!(tctl_offset("AMD Ryzen 9 5950X 16-Core Processor"), 0.0);
        assert_eq!(tctl_offset("AMD Ryzen 7 1700X Eight-Core Processor"), 20.0);
    }

    #[test]
    fn module_embedded() {
        assert!(AMD_FAMILY17_MODULE.len() > 1000);
        assert!(INTEL_MSR_MODULE.len() > 1000);
    }
}
