//! CPU-Auslastung (gesamt und je logischem Prozessor) und aktueller Takt über Leistungsindikatoren.
//!
//! Takt je Kern = "Processor Frequency" (Nenntakt) × "% Processor Performance" / 100 –
//! die gleiche Berechnung, die der Task-Manager für "Geschwindigkeit" verwendet.

use super::pdh::{Counter, Query};

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

fn sorted_cores(v: Vec<(String, f64)>) -> Vec<f64> {
    let mut items: Vec<(usize, f64)> = v.into_iter().filter_map(|(n, v)| parse_core_instance(&n).map(|i| (i, v))).collect();
    items.sort_by_key(|(i, _)| *i);
    items.into_iter().map(|(_, v)| v).collect()
}

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
    fn cores_sorted_numerically() {
        let v = vec![("0,10".to_string(), 10.0), ("0,2".to_string(), 2.0), ("_Total".to_string(), 99.0), ("0,0".to_string(), 0.0)];
        assert_eq!(sorted_cores(v), vec![0.0, 2.0, 10.0]);
    }
}
