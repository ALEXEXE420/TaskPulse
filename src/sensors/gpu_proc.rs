//! GPU-Werte pro Prozess über die herstellerunabhängigen Windows-Leistungsindikatoren
//! "GPU Engine" und "GPU Process Memory" (wie der Task-Manager), plus Adapterliste über DXGI.

use super::pdh::{Counter, Query};
use std::collections::HashMap;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EngineKey {
    pub luid: String,
    pub phys: u32,
    pub eng: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EngineInstance {
    pub pid: u32,
    pub key: EngineKey,
    pub engtype: String,
}

/// "pid_1234_luid_0x00000000_0x0000C44E_phys_0_eng_3_engtype_VideoDecode"
pub fn parse_engine_instance(name: &str) -> Option<EngineInstance> {
    let pid: u32 = between(name, "pid_", "_")?.parse().ok()?;
    let luid = luid_of(name)?;
    let phys: u32 = between(name, "_phys_", "_")?.parse().ok()?;
    let eng: u32 = between(name, "_eng_", "_")?.parse().ok()?;
    let engtype = name.split("_engtype_").nth(1).unwrap_or("").to_string();
    Some(EngineInstance { pid, key: EngineKey { luid, phys, eng }, engtype })
}

/// "pid_1234_luid_0x00000000_0x0000C44E_phys_0" -> (1234, luid)
pub fn parse_memory_instance(name: &str) -> Option<(u32, String)> {
    let pid: u32 = between(name, "pid_", "_")?.parse().ok()?;
    Some((pid, luid_of(name)?))
}

/// "luid_0x00000000_0x0000D1B5_phys_0" (GPU Adapter Memory) -> luid
pub fn parse_memory_luid(name: &str) -> Option<String> {
    luid_of(name)
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = s.find(start)? + start.len();
    let rest = &s[i..];
    Some(match rest.find(end) {
        Some(j) => &rest[..j],
        None => rest,
    })
}

fn luid_of(name: &str) -> Option<String> {
    let i = name.find("luid_")? + 5;
    let rest = &name[i..];
    // zwei Hex-Teile: 0x00000000_0x0000C44E
    let mut parts = rest.splitn(3, '_');
    let hi = parts.next()?;
    let lo = parts.next()?;
    Some(format!("{}_{}", hi.to_ascii_lowercase(), lo.to_ascii_lowercase()))
}

/// Lesbarer Engine-Name: "VideoDecode" -> "Video Decode"
pub fn pretty_engtype(t: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in t.chars() {
        if c.is_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = c.is_lowercase();
        out.push(c);
    }
    if out.is_empty() {
        "Engine".into()
    } else {
        out.replace('_', " ")
    }
}

#[derive(Debug, Clone)]
pub struct DxgiAdapter {
    pub index: usize,
    pub luid: String,
    pub luid_low: u32,
    pub luid_high: i32,
    pub vendor_id: u32,
    pub name: String,
    pub dedicated: f64,
}

impl DxgiAdapter {
    pub fn vendor_name(&self) -> &'static str {
        match self.vendor_id {
            0x10DE => "NVIDIA",
            0x1002 | 0x1022 => "AMD",
            0x8086 => "Intel",
            0x5143 => "Qualcomm",
            _ => "Unbekannt",
        }
    }
}

pub fn dxgi_adapters() -> Vec<DxgiAdapter> {
    let mut out: Vec<DxgiAdapter> = Vec::new();
    unsafe {
        let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
        let mut i = 0;
        while let Ok(a) = f.EnumAdapters1(i) {
            i += 1;
            let Ok(d) = a.GetDesc1() else { continue };
            if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let name = String::from_utf16_lossy(&d.Description[..d.Description.iter().position(|c| *c == 0).unwrap_or(128)]);
            let luid = format!("0x{:08x}_0x{:08x}", d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart);
            if out.iter().any(|x| x.luid == luid) {
                continue;
            }
            out.push(DxgiAdapter {
                index: out.len(),
                luid,
                luid_low: d.AdapterLuid.LowPart,
                luid_high: d.AdapterLuid.HighPart,
                vendor_id: d.VendorId,
                name,
                dedicated: d.DedicatedVideoMemory as f64,
            });
        }
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct ProcGpu {
    /// Auslastung je Engine (für die Gruppenberechnung).
    pub engines: Vec<(EngineKey, f64)>,
    pub max_pct: f64,
    pub engine_label: String,
    pub dedicated: f64,
    pub shared: f64,
    /// Windows meldete einen unmöglichen VRAM-Wert (größer als der Grafikspeicher der Karte).
    pub vram_implausible: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GpuProcSample {
    pub per_pid: HashMap<u32, ProcGpu>,
    /// Gesamtauslastung wie im Task-Manager: höchste Engine (Summe über Prozesse).
    pub total_pct: Option<f64>,
    /// Auslastung je Engine-Typ (gesamt) für die Leistungsseite.
    pub by_engtype: Vec<(String, f64)>,
    pub adapter_dedicated_used: Option<f64>,
    pub adapter_shared_used: Option<f64>,
}

struct Counters {
    q: Query,
    engine: Option<Counter>,
    ded: Option<Counter>,
    shared: Option<Counter>,
    ad_ded: Option<Counter>,
    ad_shared: Option<Counter>,
}

impl Counters {
    fn build() -> Option<Counters> {
        let q = Query::new()?;
        let engine = q.add(r"\GPU Engine(*)\Utilization Percentage");
        let ded = q.add(r"\GPU Process Memory(*)\Dedicated Usage");
        let shared = q.add(r"\GPU Process Memory(*)\Shared Usage");
        let ad_ded = q.add(r"\GPU Adapter Memory(*)\Dedicated Usage");
        let ad_shared = q.add(r"\GPU Adapter Memory(*)\Shared Usage");
        q.collect();
        Some(Counters { q, engine, ded, shared, ad_ded, ad_shared })
    }
}

pub struct GpuProc {
    c: Counters,
    pending: Option<Counters>,
    pub adapters: Vec<DxgiAdapter>,
    since_rebuild: u32,
}

impl GpuProc {
    pub fn new() -> Option<GpuProc> {
        Some(GpuProc { c: Counters::build()?, pending: None, adapters: dxgi_adapters(), since_rebuild: 0 })
    }

    fn adapter_memory(&self, luid: &str) -> Option<f64> {
        self.adapters.iter().find(|a| a.luid == luid).map(|a| a.dedicated).filter(|d| *d > 0.0)
    }

    fn gpu_index(&self, luid: &str) -> usize {
        self.adapters.iter().find(|a| a.luid == luid).map(|a| a.index).unwrap_or(0)
    }

    pub fn sample(&mut self) -> GpuProcSample {
        // Platzhalter-Instanzen regelmäßig neu auflösen, damit neue Prozesse sicher erscheinen.
        // Die neue Abfrage sammelt einen Takt vor, damit ihre erste Rate ein volles Intervall umfasst.
        if let Some(p) = self.pending.take() {
            self.c = p;
        }
        self.since_rebuild += 1;
        if self.since_rebuild >= 15 {
            self.since_rebuild = 0;
            self.pending = Counters::build();
        }
        let c = &self.c;
        if !c.q.collect() {
            return GpuProcSample::default();
        }
        let mut out = GpuProcSample::default();
        let mut engine_totals: HashMap<EngineKey, (f64, String)> = HashMap::new();
        if let Some(e) = c.engine {
            for (name, v) in c.q.array(e) {
                let Some(inst) = parse_engine_instance(&name) else { continue };
                let v = v.max(0.0);
                let t = engine_totals.entry(inst.key.clone()).or_insert((0.0, inst.engtype.clone()));
                t.0 += v;
                let p = out.per_pid.entry(inst.pid).or_default();
                if let Some(x) = p.engines.iter_mut().find(|(k, _)| *k == inst.key) {
                    x.1 += v;
                } else {
                    p.engines.push((inst.key, v));
                }
            }
            for p in out.per_pid.values_mut() {
                let best = p.engines.iter().filter(|(_, v)| *v > 0.0).max_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((k, v)) = best {
                    p.max_pct = v.min(100.0);
                    let t = engine_totals.get(k).map(|x| x.1.as_str()).unwrap_or("");
                    p.engine_label = format!("GPU {} – {}", self.gpu_index(&k.luid), pretty_engtype(t));
                }
            }
            out.total_pct = engine_totals.values().map(|(v, _)| *v).reduce(f64::max).map(|v| v.min(100.0));
            let mut by_type: HashMap<String, f64> = HashMap::new();
            for (v, t) in engine_totals.values() {
                let e = by_type.entry(pretty_engtype(t)).or_insert(0.0);
                *e = e.max(*v);
            }
            let mut bt: Vec<(String, f64)> = by_type.into_iter().map(|(k, v)| (k, v.min(100.0))).collect();
            bt.sort_by(|a, b| a.0.cmp(&b.0));
            out.by_engtype = bt;
        }
        if let Some(e) = c.ded {
            for (name, v) in c.q.array(e) {
                if let Some((pid, luid)) = parse_memory_instance(&name) {
                    let p = out.per_pid.entry(pid).or_default();
                    if plausible(v, self.adapter_memory(&luid)) {
                        p.dedicated += v.max(0.0);
                    } else {
                        p.vram_implausible = true;
                    }
                }
            }
        }
        if let Some(e) = c.shared {
            for (name, v) in c.q.array(e) {
                if let Some((pid, _)) = parse_memory_instance(&name) {
                    out.per_pid.entry(pid).or_default().shared += v.max(0.0);
                }
            }
        }
        out.adapter_dedicated_used = c.ad_ded.map(|e| c.q.array(e).iter().map(|(_, v)| *v).sum());
        out.adapter_shared_used = c.ad_shared.map(|e| c.q.array(e).iter().map(|(_, v)| *v).sum());
        out
    }
}

/// Ein Prozess kann nicht mehr dedizierten Speicher belegen, als die Karte hat (+5 % Toleranz).
pub fn plausible(v: f64, adapter_total: Option<f64>) -> bool {
    match adapter_total {
        Some(t) => v.is_finite() && v <= t * 1.05,
        None => v.is_finite() && v < 1024.0 * 1024.0 * 1024.0 * 1024.0,
    }
}

/// GPU-Wert einer Gruppe: Engines aufsummieren, dann die stärkste nehmen (nicht alle addieren).
pub fn group_gpu_pct<'a>(members: impl Iterator<Item = &'a ProcGpu>) -> f64 {
    let mut sums: HashMap<&EngineKey, f64> = HashMap::new();
    for m in members {
        for (k, v) in &m.engines {
            *sums.entry(k).or_insert(0.0) += *v;
        }
    }
    sums.values().copied().fold(0.0, f64::max).min(100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_instance() {
        let i = parse_engine_instance("pid_1234_luid_0x00000000_0x0000C44E_phys_0_eng_3_engtype_VideoDecode").unwrap();
        assert_eq!(i.pid, 1234);
        assert_eq!(i.key.luid, "0x00000000_0x0000c44e");
        assert_eq!(i.key.phys, 0);
        assert_eq!(i.key.eng, 3);
        assert_eq!(i.engtype, "VideoDecode");
        assert_eq!(pretty_engtype(&i.engtype), "Video Decode");
        assert_eq!(pretty_engtype("3D"), "3D");
        assert_eq!(pretty_engtype("JPEG_Decode_1"), "JPEG Decode 1");
        assert!(parse_engine_instance("_Total").is_none());
    }

    #[test]
    fn memory_instance() {
        assert_eq!(
            parse_memory_instance("pid_42_luid_0x00000000_0x0000D1B5_phys_0"),
            Some((42, "0x00000000_0x0000d1b5".to_string()))
        );
    }

    #[test]
    fn implausible_vram_rejected() {
        let gb = 1024.0 * 1024.0 * 1024.0;
        assert!(plausible(6.4 * gb, Some(16.0 * gb)));
        assert!(!plausible(587.4 * gb, Some(16.0 * gb)));
        assert!(!plausible(f64::NAN, Some(16.0 * gb)));
        assert!(plausible(2.0 * gb, None));
    }

    #[test]
    fn group_takes_strongest_engine_not_sum() {
        let k3d = EngineKey { luid: "a".into(), phys: 0, eng: 0 };
        let kvid = EngineKey { luid: "a".into(), phys: 0, eng: 3 };
        let a = ProcGpu { engines: vec![(k3d.clone(), 30.0), (kvid.clone(), 20.0)], ..Default::default() };
        let b = ProcGpu { engines: vec![(k3d.clone(), 10.0)], ..Default::default() };
        // 3D: 30 + 10 = 40, Video: 20 -> 40 (nicht 60)
        assert_eq!(group_gpu_pct([a, b].iter()), 40.0);
    }
}
