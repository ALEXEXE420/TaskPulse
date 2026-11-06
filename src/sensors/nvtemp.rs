//! NVIDIA: Hot-Spot- und Speichertemperatur (GDDR6X/GDDR7).
//!
//! NVIDIA meldet beides über die öffentliche Schnittstelle (NVML) bei GeForce-Karten nicht.
//! Gelesen wird deshalb – nur lesend – direkt aus den Registern der Grafikkarte über das signierte
//! PawnIO-Modul „Nvidia“ (von CapFrameX, wie es auch LibreHardwareMonitor nutzt). Braucht Adminrechte.
//! - Hot-Spot: Blackwell (RTX 50: GB202/203/205/206/207)
//! - Speicher je Chip: Ampere, Ada, Blackwell (RTX 30/40/50)

/// Hot-Spot-Rohwert (Kanal 0): gültig, wenn Bit 30 gesetzt; Temperatur = (Wert & 0xFFFF) / 256 °C
pub fn decode_hotspot(raw: u64) -> Option<f64> {
    let raw = raw as u32;
    if raw & (1 << 30) == 0 {
        return None;
    }
    let t = (raw & 0xFFFF) as f64 / 256.0;
    (1.0..150.0).contains(&t).then_some(t)
}

/// Speichertemperaturen: out[0] = Aufbau, out[1] = Anzahl, out[2..] = °C je Chip (INT32_MIN = keiner)
pub fn decode_memory(out: &[u64]) -> Vec<f64> {
    let n = out.get(1).copied().unwrap_or(0).min(48) as usize;
    out.iter()
        .skip(2)
        .take(n)
        .map(|v| *v as i64)
        .filter(|v| *v != i32::MIN as i64)
        .map(|v| v as f64)
        .filter(|t| (-20.0..150.0).contains(t))
        .collect()
}

/// PCI-Adresse aus der NVML-Kennung "00000000:0A:00.0" → (Bus, Gerät, Funktion)
pub fn parse_bus_id(id: &str) -> Option<(u64, u64, u64)> {
    let mut parts = id.trim().rsplitn(3, ':');
    let dev_fn = parts.next()?;
    let bus = u64::from_str_radix(parts.next()?, 16).ok()?;
    let (dev, func) = dev_fn.split_once('.')?;
    Some((bus, u64::from_str_radix(dev, 16).ok()?, u64::from_str_radix(func, 16).ok()?))
}

/// Zusätzliche Temperaturen der NVIDIA-Karte.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct NvExtraTemps {
    pub hotspot_c: Option<f64>,
    /// Heißester Speicherchip
    pub mem_max_c: Option<f64>,
    /// Alle Speicherchips
    pub mem_chips: Vec<f64>,
}

#[cfg(windows)]
pub use win::NvRegTemps;

#[cfg(windows)]
mod win {
    use super::*;
    use crate::sensors::cputemp::{pawnio_dll_path, FnClose, FnExec, FnLoad, FnOpen};
    use windows::core::{s, HSTRING, PCSTR};
    use windows::Win32::Foundation::{HANDLE, HMODULE};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    static NVIDIA_MODULE: &[u8] = include_bytes!("../../resources/pawnio/Nvidia.bin");

    pub struct NvRegTemps {
        handle: HANDLE,
        exec: FnExec,
        close: FnClose,
        bdf: [u64; 3],
        /// Gibt die Karte den jeweiligen Wert her? (nach dem ersten Versuch bekannt)
        hotspot_ok: bool,
        memory_ok: bool,
        _lib: HMODULE,
    }

    unsafe impl Send for NvRegTemps {}

    impl NvRegTemps {
        /// Öffnet das PawnIO-Modul für die NVIDIA-Karte mit der NVML-Kennung `bus_id`.
        pub fn open(bus_id: &str) -> Option<NvRegTemps> {
            let (b, d, f) = parse_bus_id(bus_id)?;
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
                let hr = load(h, NVIDIA_MODULE.as_ptr(), NVIDIA_MODULE.len());
                if hr < 0 {
                    close(h);
                    crate::diag::log(format!("GPU-Temperaturen: PawnIO-Modul „Nvidia“ nicht geladen 0x{:08X}", hr as u32));
                    return None;
                }
                let mut me = NvRegTemps { handle: h, exec, close, bdf: [b, d, f], hotspot_ok: true, memory_ok: true, _lib: lib };
                let first = me.read();
                me.hotspot_ok = first.hotspot_c.is_some();
                me.memory_ok = !first.mem_chips.is_empty();
                crate::diag::log(format!("GPU-Temperaturen (PawnIO): Hot-Spot {}, Speicher {} Chips", if me.hotspot_ok { "ja" } else { "nein" }, first.mem_chips.len()));
                (me.hotspot_ok || me.memory_ok).then_some(me)
            }
        }

        fn call(&self, name: PCSTR, out: &mut [u64]) -> bool {
            let mut ret = 0usize;
            let hr = unsafe { (self.exec)(self.handle, name, self.bdf.as_ptr(), 3, out.as_mut_ptr(), out.len(), &mut ret) };
            hr >= 0 && ret >= out.len().min(1)
        }

        pub fn read(&self) -> NvExtraTemps {
            let mut t = NvExtraTemps::default();
            if self.hotspot_ok {
                let mut out = [0u64; 6];
                if self.call(s!("ioctl_read_thermal_registers"), &mut out) {
                    t.hotspot_c = decode_hotspot(out[0]);
                }
            }
            if self.memory_ok {
                let mut out = [0u64; 50];
                if self.call(s!("ioctl_read_memory_temperatures"), &mut out) {
                    t.mem_chips = decode_memory(&out);
                    t.mem_max_c = t.mem_chips.iter().copied().reduce(f64::max);
                }
            }
            t
        }
    }

    impl Drop for NvRegTemps {
        fn drop(&mut self) {
            unsafe {
                (self.close)(self.handle);
            }
        }
    }
}

/// Linux: nouveau/NVIDIA-Treiber melden diese Werte nicht – keine Anzeige.
#[cfg(target_os = "linux")]
pub struct NvRegTemps;

#[cfg(target_os = "linux")]
impl NvRegTemps {
    pub fn open(_bus_id: &str) -> Option<NvRegTemps> {
        None
    }
    pub fn read(&self) -> NvExtraTemps {
        NvExtraTemps::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotspot_values() {
        // 52,5 °C = 52,5 × 256 = 13440 = 0x3480, mit Gültig-Bit 30
        assert_eq!(decode_hotspot((1 << 30) | 0x3480), Some(52.5));
        assert_eq!(decode_hotspot(0x3480), None);
        assert_eq!(decode_hotspot((1 << 30) | 0xFFFF), None); // 255,99 °C: unmöglich
    }

    #[test]
    fn memory_values() {
        let mut out = vec![0u64; 50];
        out[1] = 48;
        for v in out.iter_mut().skip(2) {
            *v = i32::MIN as i64 as u64;
        }
        out[2] = 56;
        out[3] = 60;
        out[9] = 58;
        assert_eq!(decode_memory(&out), vec![56.0, 60.0, 58.0]);
        assert!(decode_memory(&[0, 0]).is_empty());
    }

    #[test]
    fn bus_ids() {
        assert_eq!(parse_bus_id("00000000:0A:00.0"), Some((0x0A, 0, 0)));
        assert_eq!(parse_bus_id("0000:01:00.1"), Some((1, 0, 1)));
        assert_eq!(parse_bus_id("kaputt"), None);
    }
}
