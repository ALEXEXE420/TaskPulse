//! NVIDIA: Hot-Spot und Speichertemperatur über NVAPI (nvapi64.dll, kommt mit jedem NVIDIA-Treiber).
//!
//! NVML meldet beides bei GeForce-Karten nicht. NVAPI hat dafür eine (nicht offiziell dokumentierte)
//! Funktion „GPU_ThermalGetSensors“, die auch LibreHardwareMonitor (Open Source) nutzt. Sie braucht
//! **keine Adminrechte** und keinen Zusatztreiber.
//! - RTX 30 und RTX 40 (Ampere, Ada): Hot-Spot; Speicher bei GDDR6X (z. B. 3080/3090, 4070 Ti–4090)
//! - RTX 50 (Blackwell): Feld 1 ist dort nur noch die normale GPU-Temperatur, einen Hot-Spot meldet
//!   der Treiber nicht mehr – dort liest TaskPulse per PawnIO (nvtemp.rs) und nutzt NVAPI nicht.
//!
//! Gelesen werden nur Werte, es wird nichts eingestellt.

/// Feld 1 = Hot-Spot, Feld 9 = Speicher (Memory Junction), Einheit 1/256 °C; 0 = nicht vorhanden
pub fn decode_sensors(values: &[i32; 32], mask: u32) -> (Option<f64>, Option<f64>) {
    let get = |i: usize| -> Option<f64> {
        if mask & (1 << i) == 0 {
            return None;
        }
        let t = values[i] as f64 / 256.0;
        (1.0..150.0).contains(&t).then_some(t)
    };
    (get(1), get(9))
}

#[cfg(windows)]
pub use win::NvApiTemps;

#[cfg(windows)]
mod win {
    use super::decode_sensors;
    use std::ffi::c_void;
    use windows::core::{s, w};
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    // Kennungen der Funktionen für nvapi_QueryInterface
    const ID_INITIALIZE: u32 = 0x0150_E828;
    const ID_ENUM_PHYSICAL_GPUS: u32 = 0xE5AC_921F;
    const ID_GPU_GET_BUS_ID: u32 = 0x1BE0_B8E5;
    const ID_GPU_THERMAL_GET_SENSORS: u32 = 0x65FE_3AAD;

    /// Version 2 (Größe 0xA8): 8 reservierte Werte vor den 32 Temperaturen.
    /// Ältere Treiber nahmen auch die kürzere Form ohne Reserve an, aktuelle (ab ca. 2024) nicht mehr.
    #[repr(C)]
    struct ThermalSensors {
        version: u32,
        mask: u32,
        reserved: [i32; 8],
        values: [i32; 32],
    }

    const _: () = assert!(std::mem::size_of::<ThermalSensors>() == 0xA8);

    impl ThermalSensors {
        fn new(mask: u32) -> ThermalSensors {
            // Version 2, Größe der Struktur in den unteren 16 Bit
            ThermalSensors { version: std::mem::size_of::<ThermalSensors>() as u32 | (2 << 16), mask, reserved: [0; 8], values: [0; 32] }
        }
    }

    type FnQuery = unsafe extern "C" fn(u32) -> *mut c_void;
    type FnInit = unsafe extern "C" fn() -> i32;
    type FnEnum = unsafe extern "C" fn(*mut [*mut c_void; 64], *mut u32) -> i32;
    type FnBusId = unsafe extern "C" fn(*mut c_void, *mut u32) -> i32;
    type FnThermal = unsafe extern "C" fn(*mut c_void, *mut ThermalSensors) -> i32;

    pub struct NvApiTemps {
        gpu: *mut c_void,
        thermal: FnThermal,
        /// Bits der Sensoren, die die Karte meldet
        mask: u32,
        _lib: HMODULE,
    }

    // Der Handle wird nur vom Messthread benutzt.
    unsafe impl Send for NvApiTemps {}

    impl NvApiTemps {
        /// Öffnet NVAPI für die Karte am PCI-Bus `bus`. `None`, wenn es keine Werte gibt.
        pub fn open(bus: u64) -> Option<NvApiTemps> {
            // nvapi64.dll gibt es nur für x64 (unter ARM-Windows nicht)
            if crate::winutil::native_arm64() {
                return None;
            }
            unsafe {
                let lib = LoadLibraryW(w!("nvapi64.dll")).ok()?;
                let q = GetProcAddress(lib, s!("nvapi_QueryInterface"))?;
                let query: FnQuery = std::mem::transmute(q);
                let get = |id: u32| {
                    let p = query(id);
                    (!p.is_null()).then_some(p)
                };
                let init: FnInit = std::mem::transmute(get(ID_INITIALIZE)?);
                let enum_gpus: FnEnum = std::mem::transmute(get(ID_ENUM_PHYSICAL_GPUS)?);
                let bus_id: FnBusId = std::mem::transmute(get(ID_GPU_GET_BUS_ID)?);
                let thermal: FnThermal = std::mem::transmute(get(ID_GPU_THERMAL_GET_SENSORS)?);
                if init() != 0 {
                    return None;
                }
                let mut gpus = [std::ptr::null_mut(); 64];
                let mut n = 0u32;
                if enum_gpus(&mut gpus, &mut n) != 0 {
                    return None;
                }
                let gpu = gpus.iter().take(n.min(64) as usize).copied().find(|g| {
                    let mut b = 0u32;
                    bus_id(*g, &mut b) == 0 && b as u64 == bus
                })?;
                // Höchstes Sensor-Bit suchen, das die Karte annimmt (wie LibreHardwareMonitor)
                let mut mask = 0u32;
                for bit in 0..32u32 {
                    let mut t = ThermalSensors::new(1u32 << bit);
                    if thermal(gpu, &mut t) != 0 {
                        break;
                    }
                    mask = if bit == 31 { u32::MAX } else { (1u32 << (bit + 1)) - 1 };
                }
                if mask == 0 {
                    crate::diag::log("GPU-Temperaturen (NVAPI): Treiber meldet keine Zusatzsensoren");
                    return None;
                }
                let me = NvApiTemps { gpu, thermal, mask, _lib: lib };
                let (hot, mem) = me.read();
                crate::diag::log(format!(
                    "GPU-Temperaturen (NVAPI): Maske 0x{mask:X}, Hot-Spot {}, Speicher {} – Felder {}",
                    hot.map(|t| format!("{t:.1} °C")).unwrap_or("nein".into()),
                    mem.map(|t| format!("{t:.1} °C")).unwrap_or("nein".into()),
                    me.raw()
                ));
                (hot.is_some() || mem.is_some()).then_some(me)
            }
        }

        /// (Hot-Spot, Speicher) in °C
        pub fn read(&self) -> (Option<f64>, Option<f64>) {
            let mut t = ThermalSensors::new(self.mask);
            if unsafe { (self.thermal)(self.gpu, &mut t) } != 0 {
                return (None, None);
            }
            decode_sensors(&t.values, self.mask)
        }

        /// Alle gemeldeten Felder in °C (für Protokoll und Diagnosebericht)
        pub fn raw(&self) -> String {
            let mut t = ThermalSensors::new(self.mask);
            if unsafe { (self.thermal)(self.gpu, &mut t) } != 0 {
                return "nicht lesbar".into();
            }
            (0..32)
                .filter(|i| self.mask & (1 << i) != 0)
                .map(|i| format!("[{i}] {:.1}", t.values[i] as f64 / 256.0))
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

/// Linux: NVAPI gibt es nicht – keine Anzeige.
#[cfg(target_os = "linux")]
pub struct NvApiTemps;

#[cfg(target_os = "linux")]
impl NvApiTemps {
    pub fn open(_bus: u64) -> Option<NvApiTemps> {
        None
    }
    pub fn read(&self) -> (Option<f64>, Option<f64>) {
        (None, None)
    }
    pub fn raw(&self) -> String {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensors() {
        let mut v = [0i32; 32];
        v[0] = 50 * 256;
        v[1] = 62 * 256 + 128; // 62,5 °C
        v[9] = 74 * 256;
        assert_eq!(decode_sensors(&v, 0x3FF), (Some(62.5), Some(74.0)));
        // Speicher ohne Sensor (GDDR6): 0
        v[9] = 0;
        assert_eq!(decode_sensors(&v, 0x3FF), (Some(62.5), None));
        // Maske deckt Feld 9 nicht ab
        v[9] = 74 * 256;
        assert_eq!(decode_sensors(&v, 0x3), (Some(62.5), None));
    }
}
