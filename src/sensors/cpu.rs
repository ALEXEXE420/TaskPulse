//! CPU-Auslastung (gesamt und je logischem Prozessor) und aktueller Takt über Leistungsindikatoren.
//!
//! Takt je Kern = "Processor Frequency" (Nenntakt) × "% Processor Performance" / 100 –
//! die gleiche Berechnung, die der Task-Manager für "Geschwindigkeit" verwendet.

#[cfg(windows)]
use super::pdh::{Counter, Query};

#[cfg(windows)]
pub struct Cpu {
    q: Query,
    total: Counter,
    per_core: Option<Counter>,
    perf: Option<Counter>,
    nominal: Option<Counter>,
    pub logical_count: usize,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CpuSample {
    pub load_pct: Option<f64>,
    /// Auslastung je logischem Prozessor in Systemreihenfolge.
    pub cores_pct: Vec<f64>,
    /// Effektiver Takt je logischem Prozessor in MHz.
    pub cores_mhz: Vec<f64>,
    pub nominal_mhz: Option<f64>,
}

/// "0,5" -> Some(5), "1,3" -> Some(67), "_Total"/"0,_Total" -> None
pub fn parse_core_instance(name: &str) -> Option<usize> {
    let (g, n) = name.split_once(',')?;
    let g: usize = g.trim().parse().ok()?;
    let n: usize = n.trim().parse().ok()?;
    Some(g * 64 + n)
}

#[cfg(windows)]
fn sorted_cores(v: Vec<(String, f64)>) -> Vec<f64> {
    let mut items: Vec<(usize, f64)> = v.into_iter().filter_map(|(n, v)| parse_core_instance(&n).map(|i| (i, v))).collect();
    items.sort_by_key(|(i, _)| *i);
    items.into_iter().map(|(_, v)| v).collect()
}

#[cfg(windows)]
impl Cpu {
    pub fn new() -> Option<Cpu> {
        let q = Query::new()?;
        // "% Processor Utility" entspricht dem Task-Manager ab Windows 8; Rückfall auf "% Processor Time".
        let total = q
            .add(r"\Processor Information(_Total)\% Processor Utility")
            .or_else(|| q.add(r"\Processor Information(_Total)\% Processor Time"))?;
        let per_core = q
            .add(r"\Processor Information(*)\% Processor Utility")
            .or_else(|| q.add(r"\Processor Information(*)\% Processor Time"));
        let perf = q.add(r"\Processor Information(*)\% Processor Performance");
        let nominal = q.add(r"\Processor Information(*)\Processor Frequency");
        q.collect();
        let logical_count = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        Some(Cpu { q, total, per_core, perf, nominal, logical_count })
    }

    pub fn sample(&mut self) -> CpuSample {
        if !self.q.collect() {
            return CpuSample::default();
        }
        let load_pct = self.q.value(self.total).map(|v| v.clamp(0.0, 100.0));
        let cores_pct: Vec<f64> = self
            .per_core
            .map(|c| sorted_cores(self.q.array(c)).into_iter().map(|v| v.clamp(0.0, 100.0)).collect())
            .unwrap_or_default();
        let perf = self.perf.map(|c| sorted_cores(self.q.array(c))).unwrap_or_default();
        let nom = self.nominal.map(|c| sorted_cores(self.q.array(c))).unwrap_or_default();
        let cores_mhz: Vec<f64> = perf.iter().zip(nom.iter()).map(|(p, n)| p / 100.0 * n).collect();
        let nominal_mhz = nom.first().copied().filter(|v| *v > 0.0);
        if !cores_pct.is_empty() {
            self.logical_count = cores_pct.len();
        }
        CpuSample { load_pct, cores_pct, cores_mhz, nominal_mhz }
    }
}

// ------------------------------------------------------------------ Linux: /proc/stat und cpufreq

#[cfg_attr(windows, allow(dead_code))]
/// Zeiten einer Zeile aus /proc/stat: (aktiv, gesamt) in Ticks
pub fn parse_stat_line(line: &str) -> Option<(String, u64, u64)> {
    let mut it = line.split_whitespace();
    let name = it.next()?.to_string();
    if !name.starts_with("cpu") {
        return None;
    }
    let v: Vec<u64> = it.filter_map(|x| x.parse().ok()).collect();
    if v.len() < 4 {
        return None;
    }
    // user nice system idle iowait irq softirq steal (guest zählt schon in user)
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    let total: u64 = v.iter().take(8).sum();
    Some((name, total - idle, total))
}

#[cfg(target_os = "linux")]
pub struct Cpu {
    prev: Vec<(u64, u64)>,
    nominal: Option<f64>,
    pub logical_count: usize,
}

#[cfg(target_os = "linux")]
impl Cpu {
    pub fn new() -> Option<Cpu> {
        let logical_count = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        // Grundtakt: base_frequency (Intel/AMD mit amd-pstate), sonst Höchsttakt laut cpufreq
        let base = "/sys/devices/system/cpu/cpu0/cpufreq";
        let nominal = super::sysfs::read_f64(format!("{base}/base_frequency"))
            .or_else(|| super::sysfs::read_f64(format!("{base}/cpuinfo_max_freq")))
            .map(|khz| khz / 1000.0);
        let mut c = Cpu { prev: Vec::new(), nominal, logical_count };
        let _ = c.sample();
        Some(c)
    }

    pub fn sample(&mut self) -> CpuSample {
        let Ok(stat) = std::fs::read_to_string("/proc/stat") else { return CpuSample::default() };
        // Zeile "cpu" = gesamt, danach "cpu0", "cpu1" …
        let mut rows: Vec<(Option<usize>, u64, u64)> = Vec::new();
        for l in stat.lines() {
            if let Some((name, busy, total)) = parse_stat_line(l) {
                rows.push((name[3..].parse().ok(), busy, total));
            }
        }
        let now: Vec<(u64, u64)> = rows.iter().map(|r| (r.1, r.2)).collect();
        let pct = |i: usize| -> Option<f64> {
            let (b1, t1) = now.get(i)?;
            let (b0, t0) = self.prev.get(i)?;
            let dt = t1.saturating_sub(*t0);
            (dt > 0).then(|| (b1.saturating_sub(*b0) as f64 / dt as f64 * 100.0).clamp(0.0, 100.0))
        };
        let load_pct = pct(0);
        let mut cores: Vec<(usize, f64)> = rows.iter().enumerate().skip(1).filter_map(|(i, r)| Some((r.0?, pct(i)?))).collect();
        cores.sort_by_key(|c| c.0);
        let cores_pct: Vec<f64> = cores.iter().map(|c| c.1).collect();
        let cores_mhz: Vec<f64> = (0..rows.len().saturating_sub(1))
            .map(|i| super::sysfs::read_f64(format!("/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_cur_freq")).map(|k| k / 1000.0).unwrap_or(0.0))
            .collect();
        self.prev = now;
        if !cores_pct.is_empty() {
            self.logical_count = cores_pct.len();
        }
        CpuSample { load_pct, cores_pct, cores_mhz, nominal_mhz: self.nominal }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_instances() {
        assert_eq!(parse_core_instance("0,0"), Some(0));
        assert_eq!(parse_core_instance("0,31"), Some(31));
        assert_eq!(parse_core_instance("1,2"), Some(66));
        assert_eq!(parse_core_instance("_Total"), None);
        assert_eq!(parse_core_instance("0,_Total"), None);
    }

    #[test]
    fn proc_stat_lines() {
        // user nice system idle iowait irq softirq steal guest guest_nice
        let (n, busy, total) = parse_stat_line("cpu0 100 0 50 800 50 0 0 0 0 0").unwrap();
        assert_eq!(n, "cpu0");
        assert_eq!((busy, total), (150, 1000));
        assert!(parse_stat_line("intr 1 2 3").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn cores_sorted_numerically() {
        let v = vec![("0,10".to_string(), 10.0), ("0,2".to_string(), 2.0), ("_Total".to_string(), 99.0), ("0,0".to_string(), 0.0)];
        assert_eq!(sorted_cores(v), vec![0.0, 2.0, 10.0]);
    }
}
