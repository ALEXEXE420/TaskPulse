//! Zahlenformate und Berechnungen ohne Windows-Abhängigkeit (vollständig getestet).

use crate::config::{MemUnit, NetUnit};

pub const DASH: &str = "–";

/// Leitungsauslastung in Prozent: MB/s × 8 ÷ gebuchte MBit/s × 100.
/// `None`, wenn keine Leitung hinterlegt ist. Werte über 100 % werden nicht abgeschnitten.
pub fn line_usage_pct(bytes_per_sec: f64, booked_mbit: f64) -> Option<f64> {
    if !(booked_mbit > 0.0) || !bytes_per_sec.is_finite() {
        return None;
    }
    let mbit = bytes_per_sec * 8.0 / 1_000_000.0;
    Some(mbit / booked_mbit * 100.0)
}

/// Netzwerkrate für die Taskleiste, z. B. "0.00 MB/s".
pub fn net_rate(bytes_per_sec: Option<f64>, unit: NetUnit) -> String {
    match bytes_per_sec {
        None => format!("{DASH} {}", net_unit_label(unit)),
        Some(b) => {
            let v = match unit {
                NetUnit::MBps => b / 1_000_000.0,
                NetUnit::Mbitps => b * 8.0 / 1_000_000.0,
            };
            let s = if v >= 100.0 { format!("{v:.1}") } else { format!("{v:.2}") };
            format!("{s} {}", net_unit_label(unit))
        }
    }
}

pub fn net_unit_label(unit: NetUnit) -> &'static str {
    match unit {
        NetUnit::MBps => "MB/s",
        NetUnit::Mbitps => "Mbit/s",
    }
}

/// Ganzzahliger Prozentwert, "–" bei fehlendem Wert.
pub fn pct0(v: Option<f64>) -> String {
    match v {
        Some(v) if v.is_finite() => format!("{:.0} %", v.max(0.0)),
        _ => format!("{DASH} %"),
    }
}

pub fn temp(v: Option<f64>) -> String {
    match v {
        Some(v) if v.is_finite() => format!("{v:.0} °C"),
        _ => format!("{DASH} °C"),
    }
}

pub fn watts(w: Option<f64>) -> String {
    match w {
        Some(v) if v.is_finite() => format!("{v:.0} W"),
        _ => format!("{DASH} W"),
    }
}

pub fn ghz(mhz: Option<f64>) -> String {
    match mhz {
        Some(v) if v.is_finite() && v > 0.0 => format!("{:.2} GHz", v / 1000.0),
        _ => format!("{DASH} GHz"),
    }
}

pub fn gb_dot(bytes: Option<f64>) -> String {
    match bytes {
        Some(v) if v.is_finite() => format!("{:.1} GB", v / GIB),
        _ => format!("{DASH} GB"),
    }
}

pub const MIB: f64 = 1024.0 * 1024.0;
pub const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Deutsches Zahlenformat mit Komma.
pub fn de(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    // „-0,00“ vermeiden: -0.0 (z. B. Summe einer leeren Liste) oder winzige negative Rundungsreste
    let s = match s.strip_prefix('-') {
        Some(rest) if rest.chars().all(|c| c == '0' || c == '.') => rest.to_string(),
        _ => s,
    };
    s.replace('.', ",")
}

/// Speicher (Bytes) im Hauptfenster: "6,2 GB" / "512,0 MB".
pub fn mem(bytes: f64, unit: MemUnit, decimals: usize) -> String {
    if !bytes.is_finite() {
        return DASH.into();
    }
    let gb = match unit {
        MemUnit::GB => true,
        MemUnit::MB => false,
        MemUnit::Auto => bytes >= GIB,
    };
    if gb {
        format!("{} GB", de(bytes / GIB, decimals.max(1)))
    } else {
        format!("{} MB", de(bytes / MIB, decimals))
    }
}

/// Rate im Hauptfenster: "1,2 MB/s".
pub fn rate_de(bytes_per_sec: f64, decimals: usize) -> String {
    if !bytes_per_sec.is_finite() {
        return DASH.into();
    }
    format!("{} MB/s", de(bytes_per_sec / 1_000_000.0, decimals.max(1)))
}

pub fn pct_de(v: f64, decimals: usize) -> String {
    if !v.is_finite() {
        return DASH.into();
    }
    format!("{} %", de(v, decimals))
}

/// Ruft das Ergebnis für einen Mittelwert/Höchstwert über Kerntakte ab.
pub fn freq_from_cores(cores_mhz: &[f64], average: bool) -> Option<f64> {
    let valid: Vec<f64> = cores_mhz.iter().copied().filter(|v| v.is_finite() && *v > 0.0).collect();
    if valid.is_empty() {
        return None;
    }
    if average {
        Some(valid.iter().sum::<f64>() / valid.len() as f64)
    } else {
        valid.iter().copied().reduce(f64::max)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn de_no_negative_zero() {
        let empty: f64 = Vec::<f64>::new().into_iter().sum();
        assert_eq!(de(empty, 2), "0,00");
        assert_eq!(de(-0.001, 2), "0,00");
        assert_eq!(de(-1.5, 1), "-1,5");
        assert_eq!(pct_de(-0.0, 0), "0 %");
    }

    use super::*;

    #[test]
    fn line_usage_example_from_concept() {
        // 3,00 MB/s = 24 MBit/s = 48 % einer 50-MBit/s-Leitung
        let p = line_usage_pct(3_000_000.0, 50.0).unwrap();
        assert!((p - 48.0).abs() < 1e-9);
    }

    #[test]
    fn line_usage_not_capped_and_hidden_without_value() {
        assert!(line_usage_pct(10_000_000.0, 50.0).unwrap() > 100.0);
        assert_eq!(line_usage_pct(1.0, 0.0), None);
        assert_eq!(line_usage_pct(1.0, -5.0), None);
    }

    #[test]
    fn net_format() {
        assert_eq!(net_rate(Some(0.0), NetUnit::MBps), "0.00 MB/s");
        assert_eq!(net_rate(Some(1_234_567.0), NetUnit::MBps), "1.23 MB/s");
        assert_eq!(net_rate(Some(123_456_789.0), NetUnit::MBps), "123.5 MB/s");
        assert_eq!(net_rate(Some(1_000_000.0), NetUnit::Mbitps), "8.00 Mbit/s");
        assert_eq!(net_rate(None, NetUnit::MBps), "– MB/s");
    }

    #[test]
    fn simple_formats() {
        assert_eq!(pct0(Some(6.4)), "6 %");
        assert_eq!(pct0(None), "– %");
        assert_eq!(temp(Some(47.2)), "47 °C");
        assert_eq!(temp(None), "– °C");
        assert_eq!(ghz(Some(4370.0)), "4.37 GHz");
        assert_eq!(ghz(Some(210.0)), "0.21 GHz");
        assert_eq!(ghz(None), "– GHz");
        assert_eq!(watts(Some(84.6)), "85 W");
        assert_eq!(watts(None), "– W");
    }

    #[test]
    fn german_formats() {
        assert_eq!(mem(6.2 * GIB, MemUnit::Auto, 1), "6,2 GB");
        assert_eq!(mem(512.0 * MIB, MemUnit::Auto, 1), "512,0 MB");
        assert_eq!(mem(512.0 * MIB, MemUnit::GB, 2), "0,50 GB");
        assert_eq!(rate_de(1_200_000.0, 1), "1,2 MB/s");
        assert_eq!(pct_de(8.44, 1), "8,4 %");
    }

    #[test]
    fn freq_modes() {
        let c = [3600.0, 4370.0, 0.0, f64::NAN, 4000.0];
        assert_eq!(freq_from_cores(&c, false), Some(4370.0));
        let avg = freq_from_cores(&c, true).unwrap();
        assert!((avg - (3600.0 + 4370.0 + 4000.0) / 3.0).abs() < 1e-9);
        assert_eq!(freq_from_cores(&[], false), None);
    }
}
