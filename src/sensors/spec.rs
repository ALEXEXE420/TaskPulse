//! Spezifikation von Grafikkarte und Prozessor zum Aufklappen (in der Art von GPU-Z / CPU-Z).
//! Alles stammt aus dem, was Treiber, Betriebssystem, BIOS (SMBIOS) und die CPU selbst (CPUID)
//! melden – ohne Modelldatenbank. Was nicht gemeldet wird, fehlt einfach.

use super::gpu_proc::DxgiAdapter;
use super::nvgpu::{arch_name, NvSpec};

/// Ein Abschnitt, z. B. "Speicher", mit Zeilen (Bezeichnung, Wert).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct SpecSection {
    pub title: String,
    pub rows: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Spec {
    pub sections: Vec<SpecSection>,
}

impl Spec {
    fn section(&mut self, title: &str) -> Section<'_> {
        self.sections.push(SpecSection { title: title.into(), rows: Vec::new() });
        Section(self.sections.last_mut().unwrap())
    }

    fn tidy(mut self) -> Spec {
        self.sections.retain(|s| !s.rows.is_empty());
        self
    }
}

struct Section<'a>(&'a mut SpecSection);

impl Section<'_> {
    fn row(&mut self, label: &str, value: Option<String>) -> &mut Self {
        if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
            self.0.rows.push((label.into(), v));
        }
        self
    }
}

fn de(v: f64, d: usize) -> String {
    crate::format::de(v, d)
}

// ------------------------------------------------------------------ PCIe

/// PCI-Express-Verbindung einer Karte.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize)]
pub struct PcieLink {
    /// Aktuell (Generation 1–6, Breite x1–x16)
    pub cur_gen: u32,
    pub cur_width: u32,
    /// Was die Karte selbst kann
    pub max_gen: u32,
    pub max_width: u32,
}

/// Generation → Bezeichnung wie in GPU-Z ("1.1", "2.0" …)
pub fn gen_name(g: u32) -> String {
    match g {
        1 => "1.1".into(),
        0 => "?".into(),
        g => format!("{g}.0"),
    }
}

/// Linux-Angabe "16.0 GT/s PCIe" → Generation 4
#[cfg_attr(windows, allow(dead_code))]
pub fn gen_from_gts(s: &str) -> Option<u32> {
    let v: f64 = s.split_whitespace().next()?.parse().ok()?;
    Some(match v {
        x if x < 4.0 => 1,
        x if x < 7.0 => 2,
        x if x < 12.0 => 3,
        x if x < 24.0 => 4,
        x if x < 48.0 => 5,
        _ => 6,
    })
}

impl PcieLink {
    /// "4.0 x16"
    pub fn short(&self) -> String {
        format!("{} x{}", gen_name(self.cur_gen), self.cur_width)
    }

    /// "PCIe 5.0 x16 @ 4.0 x16" (Karte kann … @ läuft mit …)
    pub fn describe(&self) -> String {
        if self.max_gen == self.cur_gen && self.max_width == self.cur_width {
            format!("PCIe {} x{}", gen_name(self.max_gen), self.max_width)
        } else {
            format!("PCIe {} x{} @ {} x{}", gen_name(self.max_gen), self.max_width, gen_name(self.cur_gen), self.cur_width)
        }
    }
}

/// Erklärung, warum die Verbindung langsamer läuft als die Karte kann.
/// `host_gen` = höchste mit Mainboard/CPU mögliche Generation (NVIDIA meldet sie).
pub fn pcie_note(l: &PcieLink, host_gen: Option<u32>) -> Option<String> {
    if let Some(h) = host_gen.filter(|h| *h < l.max_gen) {
        return Some(format!("Die Karte kann PCIe {}, Mainboard/CPU höchstens PCIe {}", gen_name(l.max_gen), gen_name(h)));
    }
    if l.cur_gen < l.max_gen {
        return Some("Im Leerlauf schaltet die Karte die Verbindung zum Stromsparen herunter – unter Last prüfen".into());
    }
    if l.cur_width < l.max_width {
        return Some(format!("Nur mit x{} angebunden (Steckplatz oder geteilte Lanes)", l.cur_width));
    }
    None
}

/// Angaben aus dem Windows-Geräte-Manager zur Grafikkarte.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PnpInfo {
    pub link: Option<PcieLink>,
    pub driver_date: Option<String>,
}

/// FILETIME (100 ns seit 1601) → "30.09.2026"
pub fn filetime_date(ft: u64) -> Option<String> {
    let days = (ft / 864_000_000_000) as i64 - 134_774; // Tage seit 1970-01-01
    if !(0..100_000).contains(&days) {
        return None;
    }
    // Umrechnung Tage → Datum (Algorithmus von H. Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    Some(format!("{d:02}.{m:02}.{y:04}"))
}

#[cfg(windows)]
pub fn pnp_info(a: &DxgiAdapter) -> PnpInfo {
    use windows::core::GUID;
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows::Win32::Devices::Properties::{DEVPKEY_Device_DriverDate, DEVPROPKEY, DEVPROPTYPE};
    // DEVPKEY_PciDevice_*: CurrentLinkSpeed 9, CurrentLinkWidth 10, MaxLinkSpeed 11, MaxLinkWidth 12
    const PCI: GUID = GUID::from_u128(0x3ab22e31_8264_4b4e_9af5_a8d2d8e33e62);
    let key = |pid: u32| DEVPROPKEY { fmtid: PCI, pid };
    let want_full = format!("VEN_{:04X}&DEV_{:04X}&SUBSYS_{:08X}", a.vendor_id, a.device_id, a.subsys_id);
    let want_dev = format!("VEN_{:04X}&DEV_{:04X}", a.vendor_id, a.device_id);
    let mut out = PnpInfo::default();
    unsafe {
        let Ok(set) = SetupDiGetClassDevsW(Some(&GUID_DEVCLASS_DISPLAY), None, None, DIGCF_PRESENT) else { return out };
        let mut best: Option<(bool, SP_DEVINFO_DATA)> = None;
        for i in 0..32 {
            let mut data = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInfo(set, i, &mut data).is_err() {
                break;
            }
            let mut id = [0u16; 512];
            if SetupDiGetDeviceInstanceIdW(set, &data, Some(&mut id), None).is_err() {
                continue;
            }
            let id = String::from_utf16_lossy(&id[..id.iter().position(|c| *c == 0).unwrap_or(0)]).to_uppercase();
            if id.contains(&want_full) {
                best = Some((true, data));
                break;
            }
            if id.contains(&want_dev) && best.is_none() {
                best = Some((false, data));
            }
        }
        if let Some((_, data)) = best {
            let prop = |k: &DEVPROPKEY| -> Option<Vec<u8>> {
                let mut t = DEVPROPTYPE::default();
                let mut buf = [0u8; 64];
                let mut need = 0u32;
                SetupDiGetDevicePropertyW(set, &data, k, &mut t, Some(&mut buf), Some(&mut need), 0).ok()?;
                Some(buf[..(need as usize).min(buf.len())].to_vec())
            };
            let u32p = |pid: u32| prop(&key(pid)).filter(|b| b.len() >= 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            if let (Some(cg), Some(cw), Some(mg), Some(mw)) = (u32p(9), u32p(10), u32p(11), u32p(12)) {
                if (1..=7).contains(&cg) && (1..=7).contains(&mg) && cw > 0 && mw > 0 {
                    out.link = Some(PcieLink { cur_gen: cg, cur_width: cw, max_gen: mg, max_width: mw });
                }
            }
            out.driver_date = prop(&DEVPKEY_Device_DriverDate).filter(|b| b.len() >= 8).and_then(|b| filetime_date(u64::from_le_bytes(b[..8].try_into().unwrap())));
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
    out
}

/// Linux: PCIe-Verbindung aus /sys (current_link_speed "16.0 GT/s PCIe", current_link_width "16" …)
#[cfg(target_os = "linux")]
pub fn pnp_info(a: &DxgiAdapter) -> PnpInfo {
    use super::sysfs::read;
    let dev = format!("/sys/class/drm/{}/device", a.luid);
    let g = |f: &str| read(format!("{dev}/{f}")).and_then(|s| gen_from_gts(s.trim()));
    let w = |f: &str| read(format!("{dev}/{f}")).and_then(|s| s.trim().parse::<u32>().ok());
    let link = match (g("current_link_speed"), w("current_link_width"), g("max_link_speed"), w("max_link_width")) {
        (Some(cg), Some(cw), Some(mg), Some(mw)) if cw > 0 => Some(PcieLink { cur_gen: cg, cur_width: cw, max_gen: mg, max_width: mw }),
        _ => None,
    };
    PnpInfo { link, driver_date: None }
}

// ------------------------------------------------------------------ Grafikkarte

/// PCIe-Verbindung: Fähigkeit der Karte aus Windows/Linux, aktueller Stand bei NVIDIA aus NVML
/// (genauer im Moment des Abfragens).
pub fn effective_link(pnp: &PnpInfo, nv: Option<&NvSpec>) -> Option<PcieLink> {
    pnp.link.map(|mut l| {
        if let Some((g, w)) = nv.and_then(|n| n.pcie_cur) {
            l.cur_gen = g;
            l.cur_width = w;
        }
        l
    })
}

/// Shader-Einheiten je Streaming-Multiprozessor nach Compute Capability (NVIDIA)
fn cores_per_sm(cc: (i32, i32)) -> Option<u32> {
    match cc {
        (7, 5) | (8, 0) => Some(64),
        (8, 6) | (8, 7) | (8, 9) | (9, _) | (10, _) | (12, _) => Some(128),
        (6, 1) | (6, 2) => Some(128),
        (6, 0) | (7, 0) | (7, 2) => Some(64),
        _ => None,
    }
}

/// Tensor-Leistung je SM und Takt (Rechenoperationen, ohne Sparsity) für GeForce/RTX-Chips:
/// (FP16, INT8 oder FP8, FP4, Bezeichnung der 2. Stufe). Werte aus den Architektur-Whitepapern
/// (z. B. RTX 2080 Ti 107 TFLOPS FP16, RTX 3090 284 TOPS INT8, RTX 4090 660 TFLOPS FP8, RTX 5090 3352 KI-TOPS FP4 mit Sparsity).
fn tensor_per_sm(cc: (i32, i32)) -> Option<(f64, f64, Option<f64>, &'static str)> {
    match cc {
        (7, 5) => Some((1024.0, 2048.0, None, "INT8")),
        (8, 6) | (8, 7) => Some((1024.0, 2048.0, None, "INT8")),
        (8, 9) => Some((1024.0, 2048.0, None, "FP8")),
        (12, _) => Some((1024.0, 2048.0, Some(4096.0), "FP8")),
        _ => None,
    }
}

/// Theoretische Rechenleistung bei `mhz`: (FP32 TFLOPS, KI-TOPS wie NVIDIA sie angibt, Art der KI-TOPS,
/// Tensor-Einzelwerte FP16/zweite Stufe/FP4 in TFLOPS bzw. TOPS)
pub fn compute_power(cores: u32, cc: (i32, i32), mhz: f64) -> Option<(f64, Option<(f64, &'static str, String)>)> {
    let f = mhz * 1e6;
    let fp32 = cores as f64 * 2.0 * f / 1e12;
    let ai = cores_per_sm(cc).zip(tensor_per_sm(cc)).map(|(per, (fp16, second, fp4, name))| {
        let sm = (cores / per) as f64;
        let t = |ops: f64| sm * ops * f / 1e12;
        // NVIDIA-„KI-TOPS“: Blackwell FP4 mit Sparsity, Ada FP8 mit Sparsity, Turing/Ampere INT8
        let (tops, kind) = match (fp4, name) {
            (Some(x), _) => (t(x) * 2.0, "FP4 mit Sparsity"),
            (None, "FP8") => (t(second) * 2.0, "FP8 mit Sparsity"),
            _ => (t(second), "INT8"),
        };
        let mut detail = format!("FP16 {} TFLOPS · {name} {} {}", de(t(fp16), 0), de(t(second), 0), if name == "INT8" { "TOPS" } else { "TFLOPS" });
        if let Some(x) = fp4 {
            detail += &format!(" · FP4 {} TFLOPS", de(t(x), 0));
        }
        (tops, kind, detail + " (ohne Sparsity)")
    });
    (fp32 > 0.0).then_some((fp32, ai))
}

/// Spezifikation der Grafikkarte. `nv` nur bei NVIDIA. `cur_mhz` = aktueller GPU-Takt.
pub fn gpu_spec(a: &DxgiAdapter, nv: Option<&NvSpec>, pnp: &PnpInfo, shared_bytes: Option<f64>, cur_mhz: Option<f64>) -> Spec {
    let mut s = Spec::default();
    let host = nv.and_then(|n| n.pcie_host_max.map(|h| h.0));
    let link = effective_link(pnp, nv);
    s.section("Grafikkarte")
        .row("Name", Some(a.name.clone()))
        .row("Chiphersteller", Some(a.vendor_name().to_string()))
        .row("Architektur", nv.and_then(|n| arch_name(n.arch, n.compute)).map(String::from))
        .row("Kartenhersteller", a.board_vendor().map(String::from))
        .row("Teilenummer", nv.and_then(|n| n.part_number.clone()))
        .row("PCI-Kennung", Some(a.pci_id()))
        .row("Bus-Schnittstelle", link.map(|l| l.describe()))
        .row("Hinweis", link.and_then(|l| pcie_note(&l, host)))
        .row(
            "Resizable BAR",
            nv.and_then(|n| n.bar1_total.map(|b| (b, n.mem_total.unwrap_or(0)))).map(|(b, m)| {
                if b >= 1 << 30 && (m == 0 || b as f64 >= m as f64 * 0.5) {
                    format!("aktiv (BAR1 {} GB)", de(b as f64 / 1073741824.0, 0))
                } else {
                    format!("aus (BAR1 {} MB)", b / 1048576)
                }
            }),
        )
        .row("BIOS-Version", nv.and_then(|n| n.vbios.clone()));

    let bw = nv.and_then(|n| Some(n.max_mem_mhz? as f64 * 2.0 * n.bus_width? as f64 / 8.0 / 1000.0));
    s.section("Speicher")
        .row("Grafikspeicher", (a.dedicated > 0.0).then(|| format!("{} GB", de(a.dedicated / 1073741824.0, 0))))
        .row("Speicherbus", nv.and_then(|n| n.bus_width).map(|b| format!("{b} Bit")))
        .row("Speichertakt", nv.and_then(|n| n.max_mem_mhz).map(|m| format!("{m} MHz ({} Gbit/s je Leitung)", de(m as f64 * 2.0 / 1000.0, 1))))
        .row("Bandbreite", bw.map(|b| format!("{} GB/s", de(b, 1))))
        .row("Gemeinsamer Speicher", shared_bytes.filter(|b| *b > 0.0).map(|b| format!("{} GB (vom Arbeitsspeicher)", de(b / 1073741824.0, 1))));

    s.section("Recheneinheiten und Takt")
        .row("Shader-Einheiten", nv.and_then(|n| n.cores).map(|c| format!("{c} CUDA-Kerne")))
        .row("Höchster GPU-Takt", nv.and_then(|n| n.max_gpu_mhz).map(|m| format!("{m} MHz")))
        .row("Boost-Takt (Werk)", nv.and_then(|n| n.boost_mhz).map(|m| format!("{m} MHz")))
        .row(
            "Leistungsgrenze",
            nv.and_then(|n| {
                let d = n.power_default_w?;
                Some(match (n.power_min_w, n.power_max_w) {
                    (Some(lo), Some(hi)) if (hi - lo).abs() > 1.0 => format!("{} W (einstellbar {}–{} W)", de(d, 0), de(lo, 0), de(hi, 0)),
                    _ => format!("{} W", de(d, 0)),
                })
            }),
        )
        .row(
            "Temperaturgrenzen",
            nv.and_then(|n| match (n.temp_slowdown, n.temp_shutdown) {
                (Some(a), Some(b)) => Some(format!("drosselt ab {a} °C, schaltet ab bei {b} °C")),
                (Some(a), None) => Some(format!("drosselt ab {a} °C")),
                (None, Some(b)) => Some(format!("schaltet ab bei {b} °C")),
                _ => None,
            }),
        )
        .row("Lüfter", nv.and_then(|n| n.fans).filter(|f| *f > 0).map(|f| f.to_string()));

    // Theoretische Rechenleistung (Spitzenwert beim höchsten Takt, unter Last auch beim aktuellen)
    if let Some((cores, cc, max)) = nv.and_then(|n| Some((n.cores?, n.compute?, n.max_gpu_mhz? as f64))) {
        if let Some((fp32, ai)) = compute_power(cores, cc, max) {
            let sms = cores_per_sm(cc).map(|p| cores / p);
            let mut r = s.section("Rechenleistung (theoretisch)");
            r.row(
                "Recheneinheiten",
                sms.map(|n| if tensor_per_sm(cc).is_some() { format!("{n} SM · {} Tensor-Kerne", n * 4) } else { format!("{n} SM") }),
            );
            r.row("GPU-Rechenleistung", Some(format!("{} TFLOPS (FP32) bei {} MHz", de(fp32, 1), max)));
            if let Some((tops, kind, detail)) = &ai {
                r.row("KI-Rechenleistung", Some(format!("{} KI-TOPS ({kind}) bei {} MHz", de(*tops, 0), max)));
                r.row("Tensor-Kerne", Some(detail.clone()));
            }
            // Unter Last: Werte beim gerade anliegenden Takt
            if let Some(c) = cur_mhz.filter(|c| *c >= max * 0.5 && *c < max) {
                if let Some((f2, ai2)) = compute_power(cores, cc, c) {
                    let ai_txt = ai2.map(|(t, _, _)| format!(" · {} KI-TOPS", de(t, 0))).unwrap_or_default();
                    r.row("Jetzt", Some(format!("{} TFLOPS{ai_txt} bei {} MHz", de(f2, 1), de(c, 0))));
                }
            }
            r.row(
                "Hinweis",
                Some("Theoretische Spitzenwerte aus Recheneinheiten × Takt. NVIDIA rechnet mit dem offiziellen Boost-Takt, deshalb sind die Herstellerangaben etwas niedriger; im Spiel liegt der Takt meist dazwischen.".into()),
            );
        }
    }

    s.section("Treiber")
        .row("Treiberversion", nv.and_then(|n| n.driver.clone()).map(|d| format!("{} {d}", a.vendor_name())))
        .row("Windows-Treiberversion", if cfg!(windows) { a.driver.clone() } else { None })
        .row("Treiberdatum", pnp.driver_date.clone())
        .row("CUDA", nv.and_then(|n| n.cuda).map(|c| format!("{}.{}", c / 1000, (c % 1000) / 10)))
        .row("Compute Capability", nv.and_then(|n| n.compute).map(|(a, b)| format!("{a}.{b}")));
    s.tidy()
}

// ------------------------------------------------------------------ Prozessor

/// Befehlssätze laut CPUID (was die CPU kann).
#[cfg(target_arch = "x86_64")]
fn instruction_sets() -> (Vec<&'static str>, Option<&'static str>) {
    use std::arch::x86_64::__cpuid;
    let mut v = Vec::new();
    macro_rules! f {
        ($name:tt, $label:literal) => {
            if std::arch::is_x86_feature_detected!($name) {
                v.push($label);
            }
        };
    }
    f!("mmx", "MMX");
    f!("sse", "SSE");
    f!("sse2", "SSE2");
    f!("sse3", "SSE3");
    f!("ssse3", "SSSE3");
    f!("sse4.1", "SSE4.1");
    f!("sse4.2", "SSE4.2");
    f!("sse4a", "SSE4A");
    v.push("x86-64");
    f!("avx", "AVX");
    f!("avx2", "AVX2");
    f!("avx512f", "AVX-512");
    f!("fma", "FMA3");
    f!("aes", "AES");
    f!("sha", "SHA");
    f!("bmi1", "BMI1");
    f!("bmi2", "BMI2");
    f!("f16c", "F16C");
    f!("popcnt", "POPCNT");
    f!("adx", "ADX");
    // Virtualisierung: Intel VT-x (CPUID 1, ECX Bit 5), AMD-V (CPUID 0x80000001, ECX Bit 2)
    let virt = if __cpuid(1).ecx & (1 << 5) != 0 {
        Some("VT-x")
    } else if __cpuid(0x8000_0000).eax >= 0x8000_0001 && __cpuid(0x8000_0001).ecx & (1 << 2) != 0 {
        Some("AMD-V")
    } else {
        None
    };
    (v, virt)
}

#[cfg(not(target_arch = "x86_64"))]
fn instruction_sets() -> (Vec<&'static str>, Option<&'static str>) {
    let mut v = vec!["ARMv8"];
    #[cfg(target_arch = "aarch64")]
    {
        macro_rules! f {
            ($name:tt, $label:literal) => {
                if std::arch::is_aarch64_feature_detected!($name) {
                    v.push($label);
                }
            };
        }
        f!("neon", "NEON");
        f!("aes", "AES");
        f!("sha2", "SHA2");
        f!("crc", "CRC32");
        f!("dotprod", "DotProd");
        f!("sve", "SVE");
        f!("sve2", "SVE2");
    }
    (v, None)
}

/// Ist die Virtualisierung im BIOS eingeschaltet? (Windows weiß es, Linux über /proc/cpuinfo nicht sicher)
#[cfg(windows)]
fn virt_enabled() -> Option<bool> {
    // PF_VIRT_FIRMWARE_ENABLED = 21
    Some(unsafe { windows::Win32::System::Threading::IsProcessorFeaturePresent(windows::Win32::System::Threading::PROCESSOR_FEATURE_ID(21)) }.as_bool())
}

#[cfg(not(windows))]
fn virt_enabled() -> Option<bool> {
    None
}

/// Microcode-Version: Windows-Registrierung („Update Revision“), Linux /proc/cpuinfo
#[cfg(windows)]
fn microcode() -> Option<String> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY};
    let mut buf = [0u8; 16];
    let mut size = buf.len() as u32;
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &HSTRING::from(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
            &HSTRING::from("Update Revision"),
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    if size < 4 {
        return None;
    }
    // Intel: Revision in den oberen 32 Bit (8 Byte), AMD (Patch-Level): 4 Byte bzw. die unteren
    let lo = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    let hi = if size >= 8 { u32::from_le_bytes(buf[4..8].try_into().unwrap()) } else { 0 };
    let v = if hi != 0 { hi } else { lo };
    (v != 0).then(|| format!("0x{v:X}"))
}

#[cfg(not(windows))]
fn microcode() -> Option<String> {
    let t = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    t.lines().find(|l| l.starts_with("microcode")).and_then(|l| l.split(':').nth(1)).map(|v| v.trim().to_string())
}

/// CPUID-Signatur (Leaf 1, EAX) → (Familie, Modell, Stepping, Rohwert)
#[cfg(target_arch = "x86_64")]
fn signature() -> Option<(u32, u32, u32, u32)> {
    let eax = std::arch::x86_64::__cpuid(1).eax;
    let base_family = (eax >> 8) & 0xF;
    let family = if base_family == 0xF { base_family + ((eax >> 20) & 0xFF) } else { base_family };
    let model = ((eax >> 4) & 0xF) | (((eax >> 16) & 0xF) << 4);
    Some((family, model, eax & 0xF, eax))
}

#[cfg(not(target_arch = "x86_64"))]
fn signature() -> Option<(u32, u32, u32, u32)> {
    None
}

fn size_text(b: u64) -> String {
    if b >= 1 << 20 {
        format!("{} MB", de(b as f64 / 1048576.0, 0))
    } else {
        format!("{} KB", b / 1024)
    }
}

/// Caches zusammenfassen: (Ebene, Art, Größe) → "16 × 32 KB"
pub fn cache_rows(caches: &[(u8, u8, u64)]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (level, kind, label) in [(1u8, 2u8, "L1 Daten"), (1, 1, "L1 Befehle"), (1, 0, "L1"), (2, 0, "L2"), (3, 0, "L3")] {
        let mut sizes: Vec<u64> = caches.iter().filter(|c| c.0 == level && c.1 == kind).map(|c| c.2).collect();
        if sizes.is_empty() {
            continue;
        }
        sizes.sort();
        // gleiche Größen zusammenfassen: "16 × 32 KB" bzw. "8 × 32 KB + 8 × 48 KB"
        let mut parts: Vec<String> = Vec::new();
        let mut i = 0;
        while i < sizes.len() {
            let n = sizes[i..].iter().take_while(|s| **s == sizes[i]).count();
            parts.push(if n > 1 { format!("{n} × {}", size_text(sizes[i])) } else { size_text(sizes[i]) });
            i += n;
        }
        out.push((label.to_string(), parts.join(" + ")));
    }
    out
}

/// Spezifikation des Prozessors. `nominal_mhz` = Grundtakt laut Windows/Linux.
pub fn cpu_spec(nominal_mhz: Option<f64>) -> Spec {
    let ci = super::cputemp::cpu_info();
    let topo = super::topology::query();
    let amd = ci.vendor.contains("AMD") && !crate::winutil::native_arm64();
    let groups = super::topology::groups(&topo, amd, ci.family);
    let cores = topo.cores.len();
    let threads: usize = topo.cores.iter().map(|c| c.lps.len()).sum();
    let smb = super::dimm::structures();
    let t4 = smb.iter().find(|s| s.typ == 4);
    let t2 = smb.iter().find(|s| s.typ == 2);
    let t0 = smb.iter().find(|s| s.typ == 0);
    let vendor = match ci.vendor.as_str() {
        "AuthenticAMD" => "AMD".to_string(),
        "GenuineIntel" => "Intel".to_string(),
        "" => t4.and_then(|t| t.str_at(0x07)).unwrap_or_default(),
        v => v.to_string(),
    };
    let brand = if ci.brand.is_empty() { t4.and_then(|t| t.str_at(0x10)).unwrap_or_default() } else { ci.brand.clone() };
    let mut s = Spec::default();
    {
        let mut p = s.section("Prozessor");
        p.row("Name", Some(brand))
            .row("Hersteller", Some(vendor))
            .row("Sockel", t4.and_then(|t| t.str_at(0x04)))
            .row(
                "Familie / Modell / Stepping",
                signature().map(|(f, m, st, raw)| format!("{f:X}h / {m:X}h / {st} (CPUID {raw:X})")),
            )
            .row("Microcode", microcode())
            .row("Kerne / Threads", (cores > 0).then(|| format!("{cores} / {threads}")))
            .row("Grundtakt", nominal_mhz.filter(|m| *m > 0.0).map(|m| format!("{} GHz", de(m / 1000.0, 2))))
            .row("Höchsttakt laut BIOS", t4.and_then(|t| t.u16at(0x14)).filter(|m| *m > 0 && *m < 20000).map(|m| format!("{} GHz", de(m as f64 / 1000.0, 2))))
            .row("Referenztakt", t4.and_then(|t| t.u16at(0x12)).filter(|m| *m > 0).map(|m| format!("{m} MHz")));
        if groups.len() > 1 {
            p.row("Aufbau", Some(groups.iter().map(|g| format!("{} ({})", g.title, g.detail)).collect::<Vec<_>>().join(" · ")));
        }
    }
    {
        let mut c = s.section("Cache");
        for (l, v) in cache_rows(&topo.all_caches) {
            c.row(&l, Some(v));
        }
    }
    let (sets, virt) = instruction_sets();
    s.section("Befehlssätze").row("Unterstützt", Some(sets.join(", "))).row(
        "Virtualisierung",
        virt.map(|v| match virt_enabled() {
            Some(true) => format!("{v} – im BIOS eingeschaltet"),
            Some(false) => format!("{v} – im BIOS ausgeschaltet"),
            None => v.to_string(),
        }),
    );
    let dimms = super::dimm::modules();
    let ram = dimms.first().map(|d| {
        let total: f64 = dimms.iter().map(|d| d.size_bytes).sum();
        format!(
            "{} GB {}{} · {} Riegel",
            de(total / 1073741824.0, 0),
            d.kind,
            d.configured_mts.map(|m| format!("-{m}")).unwrap_or_default(),
            dimms.len()
        )
    });
    s.section("Mainboard und BIOS")
        .row(
            "Mainboard",
            t2.map(|t| [t.str_at(0x04), t.str_at(0x05)].into_iter().flatten().collect::<Vec<_>>().join(" ")),
        )
        .row("Mainboard-Version", t2.and_then(|t| t.str_at(0x06)))
        .row("BIOS", t0.map(|t| [t.str_at(0x04), t.str_at(0x05)].into_iter().flatten().collect::<Vec<_>>().join(" ")))
        .row("BIOS-Datum", t0.and_then(|t| t.str_at(0x08)))
        .row("Arbeitsspeicher", ram);
    s.tidy()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcie_text() {
        let l = PcieLink { cur_gen: 4, cur_width: 16, max_gen: 5, max_width: 16 };
        assert_eq!(l.describe(), "PCIe 5.0 x16 @ 4.0 x16");
        assert_eq!(l.short(), "4.0 x16");
        assert!(pcie_note(&l, Some(4)).unwrap().contains("höchstens PCIe 4.0"));
        assert!(pcie_note(&l, Some(5)).unwrap().contains("Leerlauf"));
        let full = PcieLink { cur_gen: 4, cur_width: 16, max_gen: 4, max_width: 16 };
        assert_eq!(full.describe(), "PCIe 4.0 x16");
        assert_eq!(pcie_note(&full, None), None);
        assert_eq!(gen_name(1), "1.1");
        assert_eq!(gen_from_gts("16.0 GT/s PCIe"), Some(4));
        assert_eq!(gen_from_gts("2.5 GT/s PCIe"), Some(1));
        assert_eq!(gen_from_gts("32.0 GT/s PCIe"), Some(5));
    }

    #[test]
    fn dates_and_caches() {
        // 30.09.2026 00:00 UTC als FILETIME
        assert_eq!(filetime_date(134_352_000_000_000_000), Some("30.09.2026".into()));
        let c = vec![(1, 2, 32 << 10), (1, 2, 32 << 10), (1, 1, 32 << 10), (1, 1, 32 << 10), (2, 0, 512 << 10), (2, 0, 512 << 10), (3, 0, 32 << 20), (3, 0, 32 << 20)];
        let r = cache_rows(&c);
        assert_eq!(r[0], ("L1 Daten".into(), "2 × 32 KB".into()));
        assert_eq!(r[1], ("L1 Befehle".into(), "2 × 32 KB".into()));
        assert_eq!(r[2], ("L2".into(), "2 × 512 KB".into()));
        assert_eq!(r[3], ("L3".into(), "2 × 32 MB".into()));
    }

    #[test]
    fn compute_numbers() {
        // RTX 5090: 21760 Kerne, Boost 2407 MHz → 104,8 TFLOPS FP32, 3352 KI-TOPS (FP4, Sparsity) laut NVIDIA
        let (fp32, ai) = compute_power(21760, (12, 0), 2407.0).unwrap();
        assert!((fp32 - 104.8).abs() < 0.2, "{fp32}");
        let (tops, kind, _) = ai.unwrap();
        assert!((tops - 3352.0).abs() < 5.0, "{tops}");
        assert_eq!(kind, "FP4 mit Sparsity");
        // RTX 4090: 16384 Kerne, 2520 MHz → 82,6 TFLOPS, 1321 KI-TOPS (FP8, Sparsity)
        let (fp32, ai) = compute_power(16384, (8, 9), 2520.0).unwrap();
        assert!((fp32 - 82.6).abs() < 0.2);
        assert!((ai.unwrap().0 - 1321.0).abs() < 3.0);
        // RTX 3090: 10496 Kerne, 1695 MHz → 35,6 TFLOPS, 285 TOPS INT8
        let (fp32, ai) = compute_power(10496, (8, 6), 1695.0).unwrap();
        assert!((fp32 - 35.6).abs() < 0.2);
        assert!((ai.unwrap().0 - 284.6).abs() < 2.0);
        // RTX 5080 beim Boost 2617 MHz: 56,3 TFLOPS und 1801 KI-TOPS laut NVIDIA
        let (fp32, ai) = compute_power(10752, (12, 0), 2617.0).unwrap();
        assert!((fp32 - 56.3).abs() < 0.2);
        assert!((ai.unwrap().0 - 1801.0).abs() < 3.0);
        // Unbekannte Architektur: nur FP32
        assert!(compute_power(4096, (5, 0), 1000.0).unwrap().1.is_none());
    }

    #[test]
    fn gpu_spec_rows() {
        let a = DxgiAdapter { name: "NVIDIA GeForce RTX 5080".into(), vendor_id: 0x10DE, device_id: 0x2C02, subsys_id: 0x89D7_1043, dedicated: 16.0 * 1073741824.0, ..Default::default() };
        let nv = NvSpec { arch: Some(10), bus_width: Some(256), max_mem_mhz: Some(15001), bar1_total: Some(16 << 30), mem_total: Some(16 << 30), pcie_cur: Some((4, 16)), pcie_host_max: Some((4, 16)), ..Default::default() };
        let pnp = PnpInfo { link: Some(PcieLink { cur_gen: 1, cur_width: 16, max_gen: 5, max_width: 16 }), driver_date: None };
        let s = gpu_spec(&a, Some(&nv), &pnp, None, None);
        let find = |l: &str| s.sections.iter().flat_map(|x| x.rows.iter()).find(|r| r.0 == l).map(|r| r.1.clone());
        assert_eq!(find("Architektur").as_deref(), Some("Blackwell"));
        assert_eq!(find("Bus-Schnittstelle").as_deref(), Some("PCIe 5.0 x16 @ 4.0 x16"));
        assert_eq!(find("Bandbreite").as_deref(), Some("960,1 GB/s"));
        assert!(find("Resizable BAR").unwrap().starts_with("aktiv"));
        assert!(s.sections.iter().all(|x| !x.rows.is_empty()));
    }
}
