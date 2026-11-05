//! Texte und Spaltenaufteilung der Taskleisten-Anzeige (ohne Windows-Abhängigkeit).

use crate::config::{Config, Field, GpuMemMode, NetUnit};
use crate::format;
use crate::sensors::Basic;

/// Haupttext und optionaler Zusatz (Leitungsauslastung) eines Felds.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub text: String,
    pub extra: Option<String>,
}

fn line_enabled(f: Field, cfg: &Config) -> bool {
    if !cfg.taskbar.show_line_usage {
        return false;
    }
    match f {
        Field::Up => cfg.line_up_mbit > 0.0,
        Field::Down => cfg.line_down_mbit > 0.0,
        _ => false,
    }
}

pub fn cell(f: Field, b: &Basic, cfg: &Config) -> Cell {
    let unit = cfg.taskbar.net_unit;
    let text = match f {
        Field::Up => format!("↑: {}", format::net_rate(b.net.up_bps, unit)),
        Field::Down => format!("↓: {}", format::net_rate(b.net.down_bps, unit)),
        Field::CpuLoad => format!("CPU: {}", format::pct0(b.cpu.load_pct)),
        Field::CpuTemp => format!("CPU: {}", format::temp(b.cpu_temp_c)),
        Field::CpuFreq => format!("CPU Freq: {}", format::ghz(b.cpu_freq_mhz)),
        Field::Mem => match cfg.taskbar.cpu_mem_mode {
            GpuMemMode::Percent => format!("CPU RAM: {}", format::pct0(b.mem.used_pct)),
            GpuMemMode::Gigabytes => format!("CPU RAM: {}", format::gb_dot((b.mem.total_bytes > 0.0).then_some(b.mem.used_bytes))),
        },
        Field::GpuTemp => format!("GPU: {}", format::temp(b.gpu.temp_c)),
        Field::GpuLoad => format!("GPU: {}", format::pct0(b.gpu.load_pct)),
        Field::GpuFreq => format!("GPU Freq: {}", format::ghz(b.gpu.clock_mhz)),
        Field::GpuMem => match cfg.taskbar.gpu_mem_mode {
            GpuMemMode::Percent => format!("GPU VRAM: {}", format::pct0(b.gpu.mem_pct())),
            GpuMemMode::Gigabytes => format!("GPU VRAM: {}", format::gb_dot(b.gpu.mem_used)),
        },
        Field::CpuPower => format!("CPU: {}", format::watts(b.cpu_power_w)),
        Field::GpuPower => format!("GPU: {}", format::watts(b.gpu.power_w)),
    };
    let extra = if line_enabled(f, cfg) {
        let v = if f == Field::Up { b.line_up_pct } else { b.line_down_pct };
        Some(format::pct0(v))
    } else {
        None
    };
    Cell { text, extra }
}

/// Breitester möglicher Text eines Felds – bestimmt die feste Spaltenbreite,
/// damit die Anzeige bei wechselnden Werten nicht springt.
pub fn template(f: Field, cfg: &Config) -> Cell {
    let unit = format::net_unit_label(cfg.taskbar.net_unit);
    let net = match cfg.taskbar.net_unit {
        NetUnit::MBps => "888.88",
        NetUnit::Mbitps => "8888.8",
    };
    let text = match f {
        Field::Up => format!("↑: {net} {unit}"),
        Field::Down => format!("↓: {net} {unit}"),
        Field::CpuLoad => "CPU: 100 %".into(),
        Field::CpuTemp => "CPU: 188 °C".into(),
        Field::CpuFreq => "CPU Freq: 8.88 GHz".into(),
        Field::Mem => match cfg.taskbar.cpu_mem_mode {
            GpuMemMode::Percent => "CPU RAM: 100 %".into(),
            GpuMemMode::Gigabytes => "CPU RAM: 888.8 GB".into(),
        },
        Field::GpuTemp => "GPU: 188 °C".into(),
        Field::GpuLoad => "GPU: 100 %".into(),
        Field::GpuFreq => "GPU Freq: 8.88 GHz".into(),
        Field::GpuMem => match cfg.taskbar.gpu_mem_mode {
            GpuMemMode::Percent => "GPU VRAM: 100 %".into(),
            GpuMemMode::Gigabytes => "GPU VRAM: 88.8 GB".into(),
        },
        Field::CpuPower => "CPU: 888 W".into(),
        Field::GpuPower => "GPU: 888 W".into(),
    };
    let extra = line_enabled(f, cfg).then(|| "888 %".to_string());
    Cell { text, extra }
}

/// Je zwei sichtbare Felder bilden eine Spalte (oben/unten), wie in der bisherigen Anzeige.
pub fn columns(fields: &[Field]) -> Vec<Vec<Field>> {
    fields.chunks(2).map(|c| c.to_vec()).collect()
}

/// Spaltenpositionen aus gemessenen Breiten. `measure` liefert die Textbreite in Pixeln.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnLayout {
    pub x: f32,
    pub text_w: f32,
    pub extra_x: Option<f32>,
    /// Trennstrich vor dieser Spalte (x-Position der Mitte).
    pub sep_x: Option<f32>,
    pub fields: Vec<Field>,
}

fn is_net(col: &[Field]) -> bool {
    col.iter().all(|f| matches!(f, Field::Up | Field::Down))
}

pub fn layout(cfg: &Config, measure: &mut dyn FnMut(&str) -> f32, gap: f32, pad: f32) -> (Vec<ColumnLayout>, f32) {
    let mut out = Vec::new();
    let mut x = pad;
    let mut prev_net: Option<bool> = None;
    for col in columns(&cfg.taskbar.visible_fields()) {
        // Wechsel zwischen Internet- und PC-Werten: Trennstrich in einer etwas breiteren Lücke
        let net = is_net(&col);
        let mut sep_x = None;
        if cfg.taskbar.separator && prev_net.is_some_and(|p| p != net) {
            x += gap * 0.5;
            sep_x = Some(x - gap * 0.75);
        }
        prev_net = Some(net);
        let mut text_w: f32 = 0.0;
        let mut extra_w: f32 = 0.0;
        for f in &col {
            let t = template(*f, cfg);
            text_w = text_w.max(measure(&t.text));
            if let Some(e) = &t.extra {
                extra_w = extra_w.max(measure(e));
            }
        }
        let extra_x = (extra_w > 0.0).then(|| x + text_w + gap * 0.5);
        out.push(ColumnLayout { x, text_w, extra_x, sep_x, fields: col });
        x += text_w + if extra_w > 0.0 { gap * 0.5 + extra_w } else { 0.0 } + gap;
    }
    let width = (x - gap + pad).max(pad * 2.0);
    (out, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Basic {
        let mut b = Basic::default();
        b.net.up_bps = Some(0.0);
        b.net.down_bps = Some(3_000_000.0);
        b.cpu.load_pct = Some(6.0);
        b.cpu_temp_c = Some(47.0);
        b.cpu_freq_mhz = Some(4370.0);
        b.mem.used_pct = Some(40.0);
        b.gpu.temp_c = Some(39.0);
        b.gpu.load_pct = Some(0.0);
        b.gpu.clock_mhz = Some(210.0);
        b.gpu.mem_used = Some(2.0);
        b.gpu.mem_total = Some(16.0);
        b.line_up_pct = Some(0.0);
        b.line_down_pct = Some(48.0);
        b.cpu_power_w = Some(84.6);
        b.gpu.power_w = Some(41.8);
        b
    }

    #[test]
    fn texts_match_existing_display() {
        let mut cfg = Config::default();
        let b = sample();
        assert_eq!(cell(Field::Up, &b, &cfg).text, "↑: 0.00 MB/s");
        assert_eq!(cell(Field::CpuLoad, &b, &cfg).text, "CPU: 6 %");
        assert_eq!(cell(Field::CpuTemp, &b, &cfg).text, "CPU: 47 °C");
        assert_eq!(cell(Field::CpuFreq, &b, &cfg).text, "CPU Freq: 4.37 GHz");
        assert_eq!(cell(Field::Mem, &b, &cfg).text, "CPU RAM: 40 %");
        assert_eq!(cell(Field::GpuTemp, &b, &cfg).text, "GPU: 39 °C");
        assert_eq!(cell(Field::GpuLoad, &b, &cfg).text, "GPU: 0 %");
        assert_eq!(cell(Field::GpuFreq, &b, &cfg).text, "GPU Freq: 0.21 GHz");
        assert_eq!(cell(Field::GpuMem, &b, &cfg).text, "GPU VRAM: 12 %");
        {
            let mut c = cfg.clone();
            c.taskbar.cpu_mem_mode = GpuMemMode::Gigabytes;
            let mut b2 = sample();
            b2.mem.used_bytes = 12.8 * 1073741824.0;
            b2.mem.total_bytes = 32.0 * 1073741824.0;
            assert_eq!(cell(Field::Mem, &b2, &c).text, "CPU RAM: 12.8 GB");
        }
        assert_eq!(cell(Field::CpuPower, &b, &cfg).text, "CPU: 85 W");
        assert_eq!(cell(Field::GpuPower, &b, &cfg).text, "GPU: 42 W");
        // ohne hinterlegte Leitung keine Prozentangabe
        assert_eq!(cell(Field::Down, &b, &cfg).extra, None);
        cfg.line_down_mbit = 50.0;
        assert_eq!(cell(Field::Down, &b, &cfg).extra.as_deref(), Some("48 %"));
        cfg.taskbar.show_line_usage = false;
        assert_eq!(cell(Field::Down, &b, &cfg).extra, None);
    }

    #[test]
    fn missing_sensor_shows_dash() {
        let cfg = Config::default();
        let b = Basic::default();
        assert_eq!(cell(Field::CpuTemp, &b, &cfg).text, "CPU: – °C");
        assert_eq!(cell(Field::GpuFreq, &b, &cfg).text, "GPU Freq: – GHz");
    }

    #[test]
    fn default_columns_like_concept() {
        let cfg = Config::default();
        let cols = columns(&cfg.taskbar.visible_fields());
        assert_eq!(cols.len(), 6);
        assert_eq!(cols[0], vec![Field::Up, Field::Down]);
        assert_eq!(cols[4], vec![Field::GpuFreq, Field::GpuMem]);
        assert_eq!(cols[5], vec![Field::CpuPower, Field::GpuPower]);
    }

    #[test]
    fn separator_between_net_and_pc() {
        let mut cfg = Config::default();
        let mut m = |s: &str| s.chars().count() as f32 * 7.0;
        let (cols, w_with) = layout(&cfg, &mut m, 10.0, 4.0);
        assert!(cols[0].sep_x.is_none());
        let s = cols[1].sep_x.expect("Trennstrich vor der ersten PC-Spalte");
        let net_end = cols[0].x + cols[0].text_w;
        assert!(s > net_end && s < cols[1].x);
        assert!(cols[2..].iter().all(|c| c.sep_x.is_none()));
        cfg.taskbar.separator = false;
        let (cols, w_without) = layout(&cfg, &mut m, 10.0, 4.0);
        assert!(cols.iter().all(|c| c.sep_x.is_none()));
        assert!(w_without < w_with);
    }

    #[test]
    fn layout_is_value_independent() {
        let mut cfg = Config::default();
        cfg.line_up_mbit = 40.0;
        let mut m = |s: &str| s.chars().count() as f32 * 7.0;
        let (cols, w) = layout(&cfg, &mut m, 10.0, 4.0);
        assert_eq!(cols.len(), 6);
        assert!(cols[0].extra_x.is_some());
        assert!(cols[1].extra_x.is_none());
        assert!(w > 0.0);
        // Template ist mindestens so breit wie echte Werte
        let b = sample();
        for c in &cols {
            for f in &c.fields {
                assert!(m(&cell(*f, &b, &cfg).text) <= c.text_w + 0.01, "{f:?}");
            }
        }
    }
}
