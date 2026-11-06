//! Aufbau der CPU: welche logischen Prozessoren zu welchem Kern, CCD/CCX (AMD) oder
//! P-/E-Kern-Gruppe (Intel) gehören.
//!
//! Quelle ist allein Windows (GetLogicalProcessorInformationEx), keine Modellliste:
//! - AMD: Logische Prozessoren, die sich einen L3-Cache teilen, bilden ein CCD (ab Zen 3: ein
//!   CCX je CCD) bzw. ein CCX (Zen/Zen 2: zwei CCX je CCD). Das passt damit automatisch für
//!   Ryzen, Threadripper und EPYC, auch mit mehreren Sockeln.
//! - Intel: Windows meldet je Kern eine „Effizienzklasse“ (höher = schneller). Bei Hybrid-CPUs
//!   (ab 12. Generation) ergeben sich daraus P-Kerne (höchste Klasse) und E-Kerne. Core-Ultra-Laptop-CPUs
//!   (Meteor Lake, Lunar Lake, Arrow Lake H/U) haben zusätzlich Low-Power-E-Kerne auf dem SoC-Teil:
//!   Sie haben keinen L3-Cache bzw. eine eigene, niedrigste Effizienzklasse.
//! - ARM (z. B. NVIDIA RTX Spark N1X, Snapdragon X): ebenfalls über die Effizienzklassen; dort
//!   liegen P- und E-Kerne verschränkt (N1X: E-Maske 0x07C1F, P-Maske 0xF83E0).
//!
//! Die Nummern der logischen Prozessoren sind global: Gruppe × 64 + Index – genauso wie die
//! Leistungsindikatoren sie liefern (siehe cpu.rs).

use std::collections::BTreeMap;

const RELATION_PROCESSOR_CORE: u32 = 0;
const RELATION_CACHE: u32 = 2;
const RELATION_PROCESSOR_PACKAGE: u32 = 3;
const RELATION_ALL: i32 = 0xFFFF;

/// Ein physischer Kern.
#[derive(Debug, Clone, PartialEq)]
pub struct Core {
    /// Logische Prozessoren (global nummeriert)
    pub lps: Vec<usize>,
    pub efficiency: u8,
}

/// Ein Cache (nur L3 wird ausgewertet).
#[derive(Debug, Clone, PartialEq)]
pub struct Cache {
    pub level: u8,
    pub size: u64,
    pub lps: Vec<usize>,
}

/// Rohdaten von Windows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawTopology {
    pub cores: Vec<Core>,
    pub caches: Vec<Cache>,
    /// Sockel: logische Prozessoren je Paket
    pub packages: Vec<Vec<usize>>,
}

/// Eine angezeigte Gruppe, z. B. "CCD 1" oder "P-Kerne".
#[derive(Debug, Clone, PartialEq)]
pub struct CoreGroup {
    pub title: String,
    /// z. B. "8 Kerne · 16 Threads · L3 32 MB"
    pub detail: String,
    pub lps: Vec<usize>,
}

/// Logische Prozessoren aus einer GROUP_AFFINITY-Maske.
fn mask_lps(mask: u64, group: u16) -> Vec<usize> {
    (0..64).filter(|b| mask & (1u64 << b) != 0).map(|b| group as usize * 64 + b).collect()
}

fn rd16(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|x| u16::from_le_bytes([x[0], x[1]]))
}
fn rd32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|x| u32::from_le_bytes(x.try_into().unwrap()))
}
fn rd64(b: &[u8], o: usize) -> Option<u64> {
    b.get(o..o + 8).map(|x| u64::from_le_bytes(x.try_into().unwrap()))
}

/// GROUP_AFFINITY-Liste ab `off` (je 16 Byte: Mask u64, Group u16, Reserved[3] u16).
fn affinities(rec: &[u8], off: usize, count: usize) -> Vec<usize> {
    let mut v = Vec::new();
    for i in 0..count {
        let o = off + i * 16;
        if let (Some(m), Some(g)) = (rd64(rec, o), rd16(rec, o + 8)) {
            v.extend(mask_lps(m, g));
        }
    }
    v.sort_unstable();
    v
}

/// Zerlegt den Puffer von GetLogicalProcessorInformationEx (Layout aus winnt.h, x64).
pub fn parse(buf: &[u8]) -> RawTopology {
    let mut t = RawTopology::default();
    let mut o = 0;
    while o + 8 <= buf.len() {
        let rel = rd32(buf, o).unwrap_or(u32::MAX);
        let size = rd32(buf, o + 4).unwrap_or(0) as usize;
        if size < 8 || o + size > buf.len() {
            break;
        }
        let rec = &buf[o..o + size];
        match rel {
            // PROCESSOR_RELATIONSHIP: Flags @8, EfficiencyClass @9, Reserved[20], GroupCount @30, GroupMask[] @32
            RELATION_PROCESSOR_CORE | RELATION_PROCESSOR_PACKAGE => {
                let eff = rec.get(9).copied().unwrap_or(0);
                let n = rd16(rec, 30).unwrap_or(1).max(1) as usize;
                let lps = affinities(rec, 32, n);
                if !lps.is_empty() {
                    if rel == RELATION_PROCESSOR_CORE {
                        t.cores.push(Core { lps, efficiency: eff });
                    } else {
                        t.packages.push(lps);
                    }
                }
            }
            // CACHE_RELATIONSHIP: Level @8, CacheSize @12, Type @16, Reserved[18], GroupCount @38, GroupMask[] @40
            // (ältere Windows-Versionen: GroupCount = 0, genau eine Maske)
            RELATION_CACHE => {
                let level = rec.get(8).copied().unwrap_or(0);
                let csize = rd32(rec, 12).unwrap_or(0) as u64;
                let n = rd16(rec, 38).unwrap_or(0).max(1) as usize;
                let lps = affinities(rec, 40, n);
                if level == 3 && !lps.is_empty() {
                    t.caches.push(Cache { level, size: csize, lps });
                }
            }
            _ => {}
        }
        o += size;
    }
    t.cores.sort_by_key(|c| c.lps.first().copied().unwrap_or(0));
    t.caches.sort_by_key(|c| c.lps.first().copied().unwrap_or(0));
    t.packages.sort_by_key(|p| p.first().copied().unwrap_or(0));
    t
}

/// Fragt Windows nach dem Aufbau der CPU.
pub fn query() -> RawTopology {
    use windows::Win32::System::SystemInformation::{GetLogicalProcessorInformationEx, LOGICAL_PROCESSOR_RELATIONSHIP, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX};
    unsafe {
        let mut len = 0u32;
        let _ = GetLogicalProcessorInformationEx(LOGICAL_PROCESSOR_RELATIONSHIP(RELATION_ALL), None, &mut len);
        if len == 0 {
            return RawTopology::default();
        }
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        if GetLogicalProcessorInformationEx(LOGICAL_PROCESSOR_RELATIONSHIP(RELATION_ALL), Some(buf.as_mut_ptr() as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX), &mut len).is_err() {
            return RawTopology::default();
        }
        let bytes = std::slice::from_raw_parts(buf.as_ptr() as *const u8, len as usize);
        parse(bytes)
    }
}

fn mb(bytes: u64) -> String {
    format!("{} MB", (bytes as f64 / 1048576.0).round() as u64)
}

fn counts(t: &RawTopology, lps: &[usize]) -> String {
    let cores = t.cores.iter().filter(|c| c.lps.iter().any(|l| lps.contains(l))).count();
    if cores == 0 || cores == lps.len() {
        format!("{} Kerne", lps.len())
    } else {
        format!("{cores} Kerne · {} Threads", lps.len())
    }
}

/// Bildet die angezeigten Gruppen. `amd`/`family` aus CPUID.
pub fn groups(t: &RawTopology, amd: bool, family: u32) -> Vec<CoreGroup> {
    let all: Vec<usize> = {
        let mut v: Vec<usize> = t.cores.iter().flat_map(|c| c.lps.iter().copied()).collect();
        v.sort_unstable();
        v
    };
    if all.is_empty() {
        return Vec::new();
    }
    let multi_socket = t.packages.len() > 1;
    let socket_of = |lp: usize| t.packages.iter().position(|p| p.contains(&lp)).unwrap_or(0);
    let prefix = |lp: usize| if multi_socket { format!("CPU {} · ", socket_of(lp) + 1) } else { String::new() };

    // Intel (und andere) mit unterschiedlichen Effizienzklassen: P-/E-Kerne
    let classes: Vec<u8> = {
        let mut c: Vec<u8> = t.cores.iter().map(|c| c.efficiency).collect();
        c.sort_unstable();
        c.dedup();
        c
    };
    if !amd && classes.len() > 1 {
        // Höchste Effizienzklasse = P-Kerne. Low-Power-E-Kerne: ohne L3-Cache, oder bei drei
        // Klassen die niedrigste. Alle übrigen = E-Kerne.
        let top = *classes.last().unwrap();
        let in_l3 = |c: &Core| t.caches.iter().any(|l3| c.lps.iter().any(|l| l3.lps.contains(l)));
        let kind = |c: &Core| -> u8 {
            if c.efficiency == top {
                0
            } else if (classes.len() >= 3 && c.efficiency == classes[0]) || (!t.caches.is_empty() && !in_l3(c)) {
                2
            } else {
                1
            }
        };
        let mut out = Vec::new();
        for (name, k) in [("P-Kerne", 0u8), ("E-Kerne", 1), ("LP-E-Kerne", 2)] {
            let mut by_socket: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
            for c in t.cores.iter().filter(|c| kind(c) == k) {
                for l in &c.lps {
                    by_socket.entry(socket_of(*l)).or_default().push(*l);
                }
            }
            for (_, mut lps) in by_socket {
                lps.sort_unstable();
                let detail = counts(t, &lps);
                out.push(CoreGroup { title: format!("{}{name}", prefix(lps[0])), detail, lps });
            }
        }
        return out;
    }

    // Mehrere L3-Caches: AMD-CCDs/CCX bzw. Sockel
    if t.caches.len() > 1 {
        let unit = if amd && family == 0x17 { "CCX" } else if amd { "CCD" } else { "Cluster" };
        let biggest = t.caches.iter().map(|c| c.size).max().unwrap_or(0);
        let smallest = t.caches.iter().map(|c| c.size).min().unwrap_or(0);
        let mut n_in_socket: BTreeMap<usize, usize> = BTreeMap::new();
        return t
            .caches
            .iter()
            .map(|c| {
                let s = socket_of(c.lps[0]);
                let n = n_in_socket.entry(s).or_insert(0);
                *n += 1;
                // X3D: ein CCD mit deutlich größerem L3 als die anderen
                let vcache = amd && biggest > smallest && c.size == biggest && biggest >= 64 << 20;
                let detail = format!("{} · L3 {}{}", counts(t, &c.lps), mb(c.size), if vcache { " · 3D V-Cache" } else { "" });
                CoreGroup { title: format!("{}{unit} {}", prefix(c.lps[0]), n), detail, lps: c.lps.clone() }
            })
            .collect();
    }

    // Mehrere Sockel ohne getrennte L3-Angaben
    if multi_socket {
        return t.packages.iter().enumerate().map(|(i, p)| CoreGroup { title: format!("CPU {}", i + 1), detail: counts(t, p), lps: p.clone() }).collect();
    }
    let l3 = t.caches.first().map(|c| format!(" · L3 {}", mb(c.size))).unwrap_or_default();
    vec![CoreGroup { title: "Alle Kerne".into(), detail: format!("{}{l3}", counts(t, &all)), lps: all }]
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Testpuffer im Windows-Layout bauen ---
    fn affinity(mask: u64, group: u16) -> Vec<u8> {
        let mut v = mask.to_le_bytes().to_vec();
        v.extend(group.to_le_bytes());
        v.extend([0u8; 6]);
        v
    }
    fn core(lps: &[usize], eff: u8) -> Vec<u8> {
        let mask = lps.iter().fold(0u64, |m, l| m | 1 << (l % 64));
        let mut r = vec![0u8; 32];
        r[0..4].copy_from_slice(&RELATION_PROCESSOR_CORE.to_le_bytes());
        r[8] = if lps.len() > 1 { 1 } else { 0 };
        r[9] = eff;
        r[30..32].copy_from_slice(&1u16.to_le_bytes());
        r.extend(affinity(mask, (lps[0] / 64) as u16));
        let n = r.len() as u32;
        r[4..8].copy_from_slice(&n.to_le_bytes());
        r
    }
    fn l3(lps: std::ops::Range<usize>, size_mb: u32) -> Vec<u8> {
        let mask = lps.clone().fold(0u64, |m, l| m | 1 << (l % 64));
        let mut r = vec![0u8; 40];
        r[0..4].copy_from_slice(&RELATION_CACHE.to_le_bytes());
        r[8] = 3;
        r[12..16].copy_from_slice(&(size_mb << 20).to_le_bytes());
        r[38..40].copy_from_slice(&1u16.to_le_bytes());
        r.extend(affinity(mask, (lps.start / 64) as u16));
        let n = r.len() as u32;
        r[4..8].copy_from_slice(&n.to_le_bytes());
        r
    }
    /// n Kerne mit SMT ab LP `start`
    fn smt_cores(start: usize, n: usize, eff: u8) -> Vec<u8> {
        (0..n).flat_map(|i| core(&[start + 2 * i, start + 2 * i + 1], eff)).collect()
    }

    #[test]
    fn ryzen_5950x_two_ccds() {
        let mut b = smt_cores(0, 16, 0);
        b.extend(l3(0..16, 32));
        b.extend(l3(16..32, 32));
        let t = parse(&b);
        assert_eq!(t.cores.len(), 16);
        let g = groups(&t, true, 0x19);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].title, "CCD 1");
        assert_eq!(g[1].title, "CCD 2");
        assert_eq!(g[0].lps, (0..16).collect::<Vec<_>>());
        assert_eq!(g[1].lps, (16..32).collect::<Vec<_>>());
        assert_eq!(g[0].detail, "8 Kerne · 16 Threads · L3 32 MB");
    }

    #[test]
    fn ryzen_x3d_marks_vcache_ccd() {
        let mut b = smt_cores(0, 16, 0);
        b.extend(l3(0..16, 96));
        b.extend(l3(16..32, 32));
        let g = groups(&parse(&b), true, 0x19);
        assert!(g[0].detail.ends_with("3D V-Cache"));
        assert!(!g[1].detail.contains("V-Cache"));
    }

    #[test]
    fn zen2_is_ccx() {
        let mut b = smt_cores(0, 8, 0);
        b.extend(l3(0..8, 16));
        b.extend(l3(8..16, 16));
        let g = groups(&parse(&b), true, 0x17);
        assert_eq!(g[0].title, "CCX 1");
        assert_eq!(g[1].title, "CCX 2");
    }

    #[test]
    fn threadripper_eight_ccds_over_two_groups() {
        // 64 Kerne / 128 Threads: Windows-Gruppen 0 und 1, 8 CCDs à 16 Threads
        let mut b = smt_cores(0, 64, 0);
        for i in 0..8 {
            b.extend(l3(i * 16..i * 16 + 16, 32));
        }
        let g = groups(&parse(&b), true, 0x19);
        assert_eq!(g.len(), 8);
        assert_eq!(g[7].title, "CCD 8");
        assert_eq!(g[4].lps.first(), Some(&64));
        assert!(g.iter().all(|x| x.lps.len() == 16));
    }

    #[test]
    fn intel_hybrid_p_and_e_cores() {
        // i7-13700K-ähnlich: 8 P-Kerne mit HT (LP 0–15), 8 E-Kerne ohne HT (LP 16–23)
        let mut b = smt_cores(0, 8, 1);
        for lp in 16..24 {
            b.extend(core(&[lp], 0));
        }
        b.extend(l3(0..24, 30));
        let g = groups(&parse(&b), false, 6);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].title, "P-Kerne");
        assert_eq!(g[0].detail, "8 Kerne · 16 Threads");
        assert_eq!(g[1].title, "E-Kerne");
        assert_eq!(g[1].detail, "8 Kerne");
        assert_eq!(g[1].lps, (16..24).collect::<Vec<_>>());
    }

    #[test]
    fn intel_core_ultra_lp_e_cores() {
        // Meteor Lake (z. B. Core Ultra 7 155H): 6 P mit HT, 8 E, 2 LP-E ohne L3, drei Effizienzklassen
        let mut b = smt_cores(0, 6, 2);
        for lp in 12..20 {
            b.extend(core(&[lp], 1));
        }
        for lp in 20..22 {
            b.extend(core(&[lp], 0));
        }
        b.extend(l3(0..20, 24));
        let g = groups(&parse(&b), false, 6);
        let titles: Vec<&str> = g.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, ["P-Kerne", "E-Kerne", "LP-E-Kerne"]);
        assert_eq!(g[0].detail, "6 Kerne · 12 Threads");
        assert_eq!(g[1].lps, (12..20).collect::<Vec<_>>());
        assert_eq!(g[2].lps, vec![20, 21]);
    }

    #[test]
    fn lp_e_by_missing_l3_with_two_classes() {
        // Meldet Windows nur zwei Klassen, erkennt man LP-E-Kerne am fehlenden L3-Cache
        let mut b = smt_cores(0, 4, 1);
        for lp in 8..12 {
            b.extend(core(&[lp], 0)); // E mit L3
        }
        for lp in 12..14 {
            b.extend(core(&[lp], 0)); // LP-E ohne L3
        }
        b.extend(l3(0..12, 12));
        let g = groups(&parse(&b), false, 6);
        let titles: Vec<&str> = g.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, ["P-Kerne", "E-Kerne", "LP-E-Kerne"]);
        assert_eq!(g[2].lps, vec![12, 13]);
    }

    #[test]
    fn rtx_spark_n1x_interleaved_cores() {
        // NVIDIA RTX Spark N1X: 10 P + 10 E ohne SMT, verschränkt (E-Maske 0x07C1F, P-Maske 0xF83E0), 2 Cluster mit L3
        let mut b = Vec::new();
        for lp in 0..20 {
            let p = 0xF83E0u32 & (1 << lp) != 0;
            b.extend(core(&[lp], if p { 1 } else { 0 }));
        }
        b.extend(l3(0..10, 16));
        b.extend(l3(10..20, 16));
        let g = groups(&parse(&b), false, 0);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].title, "P-Kerne");
        assert_eq!(g[0].lps, vec![5, 6, 7, 8, 9, 15, 16, 17, 18, 19]);
        assert_eq!(g[1].title, "E-Kerne");
        assert_eq!(g[1].lps, vec![0, 1, 2, 3, 4, 10, 11, 12, 13, 14]);
        assert_eq!(g[0].detail, "10 Kerne");
    }

    #[test]
    fn plain_cpu_single_group() {
        let mut b = smt_cores(0, 6, 0);
        b.extend(l3(0..12, 12));
        let g = groups(&parse(&b), false, 6);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].detail, "6 Kerne · 12 Threads · L3 12 MB");
    }

    #[test]
    fn this_pc_reports_cores() {
        // Echter Aufruf: jede CPU hat mindestens einen Kern; alle LPs sind genau einem Kern zugeordnet
        let t = query();
        assert!(!t.cores.is_empty());
        let mut lps: Vec<usize> = t.cores.iter().flat_map(|c| c.lps.clone()).collect();
        let n = lps.len();
        lps.sort_unstable();
        lps.dedup();
        assert_eq!(lps.len(), n);
    }
}
