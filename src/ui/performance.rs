//! Seite "Leistung": Verlaufsgrafiken und Kennzahlen für CPU, Arbeitsspeicher, Datenträger,
//! Netzwerk und GPU; logische Prozessoren einzeln.

use super::{TaskPulseApp, HISTORY};
use crate::format;
use eframe::egui::{self, epaint, Color32, Pos2, RichText, Sense, Stroke, Vec2};
use std::collections::VecDeque;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Item {
    #[default]
    Cpu,
    Mem,
    Disk,
    /// Ein einzelner Datenträger (Nummer)
    DiskOne(u32),
    Net,
    Gpu,
}

#[derive(Default)]
pub struct PerfState {
    pub item: Item,
}

const C_CPU: Color32 = Color32::from_rgb(17, 125, 187);
const C_MEM: Color32 = Color32::from_rgb(139, 18, 174);
const C_DISK: Color32 = Color32::from_rgb(77, 166, 12);
const C_DISK2: Color32 = Color32::from_rgb(150, 205, 90);
const C_NET: Color32 = Color32::from_rgb(167, 79, 1);
const C_NET2: Color32 = Color32::from_rgb(220, 150, 60);
const C_GPU: Color32 = Color32::from_rgb(10, 140, 120);

/// Start-Unterseite: `--page gpu` oder `--perf cpu|mem|disk|disk0|disk1…|net|gpu` (Tests, Bildschirmfotos).
pub fn start_item() -> Item {
    let a: Vec<String> = std::env::args().collect();
    let v = a.iter().position(|x| x == "--perf").and_then(|i| a.get(i + 1)).map(|s| s.to_lowercase());
    match v.as_deref() {
        Some("mem") => Item::Mem,
        Some("disk") => Item::Disk,
        Some("net") => Item::Net,
        Some("gpu") => Item::Gpu,
        Some(d) if d.starts_with("disk") => d[4..].parse().map(Item::DiskOne).unwrap_or(Item::Disk),
        _ if a.iter().any(|x| x == "gpu") => Item::Gpu,
        _ => Item::Cpu,
    }
}

/// Zeichnet eine Verlaufsgrafik. `max` = Skalenendwert.
fn graph(ui: &mut egui::Ui, size: Vec2, series: &[(&VecDeque<f64>, Color32)], max: f64) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect);
    let grid = if ui.visuals().dark_mode { Color32::from_gray(60) } else { Color32::from_gray(215) };
    let border = series.first().map(|s| s.1).unwrap_or(grid);
    p.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    for i in 1..10 {
        let x = rect.left() + rect.width() * i as f32 / 10.0;
        p.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], Stroke::new(1.0_f32, grid));
    }
    for i in 1..5 {
        let y = rect.top() + rect.height() * i as f32 / 5.0;
        p.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], Stroke::new(1.0_f32, grid));
    }
    let max = if max > 0.0 { max } else { 1.0 };
    for (s, col) in series {
        if s.len() < 2 {
            continue;
        }
        let n = s.len();
        let pts: Vec<Pos2> = s
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let x = rect.right() - rect.width() * (n - 1 - i) as f32 / (HISTORY - 1) as f32;
                let y = rect.bottom() - rect.height() * (v / max).clamp(0.0, 1.0) as f32;
                Pos2::new(x, y)
            })
            .collect();
        // Fläche unter der Kurve
        let mut mesh = epaint::Mesh::default();
        let fill = Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 40);
        for w in pts.windows(2) {
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(w[0], fill);
            mesh.colored_vertex(w[1], fill);
            mesh.colored_vertex(Pos2::new(w[1].x, rect.bottom()), fill);
            mesh.colored_vertex(Pos2::new(w[0].x, rect.bottom()), fill);
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        p.add(egui::Shape::mesh(mesh));
        p.add(egui::Shape::line(pts, Stroke::new(1.5_f32, *col)));
    }
    p.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, border));
    resp
}

fn side_entry(ui: &mut egui::Ui, selected: bool, title: &str, sub: &str, s: &VecDeque<f64>, max: f64, col: Color32) -> bool {
    let fill = if selected { ui.visuals().selection.bg_fill.linear_multiply(0.4) } else { Color32::TRANSPARENT };
    let r = egui::Frame::none()
        .fill(fill)
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                graph(ui, Vec2::new(80.0, 50.0), &[(s, col)], max);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).strong());
                    ui.label(RichText::new(sub).small());
                });
            });
        })
        .response;
    ui.interact(r.rect, ui.id().with(title), Sense::click()).clicked()
}

/// Breite, die nie schrumpft – so verschieben wechselnde Zahlen (z. B. 9,8 W → 10,2 W) nichts daneben.
fn grow_width(ui: &egui::Ui, id: egui::Id, w: f32) -> f32 {
    ui.data_mut(|d| {
        let e = d.get_temp_mut_or(id, 0.0f32);
        if w > *e {
            *e = w;
        }
        *e
    })
}

fn stat(ui: &mut egui::Ui, label: &str, value: String) {
    let id = ui.id().with(("stat_w", label));
    let min = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
    // In einer umbrechenden Zeile: passt der Wert nicht mehr hinein, in die nächste Zeile
    // (sonst wird er am rechten Rand abgeschnitten)
    // (sichtbare Breite: der Inhalt einer Scrollfläche kann breiter sein als ihr Ausschnitt)
    let room = ui.available_width().min(ui.clip_rect().right() - ui.cursor().left());
    if min > 0.0 && room < min + 4.0 && ui.layout().main_wrap {
        ui.end_row();
    }
    let r = ui
        .vertical(|ui| {
            ui.set_min_width(min);
            ui.add(egui::Label::new(RichText::new(label).small().weak()).extend());
            ui.add(egui::Label::new(RichText::new(value).heading()).extend());
        })
        .response;
    grow_width(ui, id, r.rect.width());
    ui.add_space(6.0);
}

/// Text in einem Feld, das nur wächst (rechtsbündig bei `right`), damit Nachbarn stehen bleiben.
fn steady(ui: &mut egui::Ui, key: impl std::hash::Hash, text: RichText, right: bool) -> egui::Response {
    let id = ui.id().with(("steady", key));
    let galley = egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body);
    let w = grow_width(ui, id, galley.size().x);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, galley.size().y), Sense::hover());
    let x = if right { rect.right() - galley.size().x } else { rect.left() };
    ui.painter().galley(egui::pos2(x, rect.top()), galley, ui.visuals().text_color());
    resp
}

/// Skala für Übertragungsraten der Datenträger: ab 1 MB/s, verdoppelt bis der Höchstwert passt.
fn disk_max(h: &VecDeque<f64>, h2: &VecDeque<f64>) -> f64 {
    let m = h.iter().chain(h2.iter()).copied().fold(0.0, f64::max);
    let mut scale = 1_000_000.0;
    while scale < m {
        scale *= 2.0;
    }
    scale
}

/// Spaltenzahl für das Kern-Raster: in die Breite (etwa doppelt so viele Spalten wie Zeilen),
/// z. B. 32 -> 8 x 4, 16 -> 6 x 3, 8 -> 4 x 2.
pub fn core_cols(n: usize) -> usize {
    if n <= 1 {
        return 1;
    }
    ((2.0 * n as f32).sqrt().ceil() as usize).clamp(1, n)
}

/// Aufteilung der CPU (CCDs/CCX bei AMD, P-/E-Kerne bei Intel), einmal ermittelt.
fn cpu_groups() -> &'static [crate::sensors::topology::CoreGroup] {
    static G: std::sync::OnceLock<Vec<crate::sensors::topology::CoreGroup>> = std::sync::OnceLock::new();
    G.get_or_init(|| {
        let ci = crate::sensors::cputemp::cpu_info();
        // Auf ARM nicht der (nachgebildeten) CPUID glauben – dort gibt es keine CCDs
        let amd = ci.vendor.contains("AMD") && !crate::winutil::native_arm64();
        crate::sensors::topology::groups(&crate::sensors::topology::query(), amd, ci.family)
    })
}

/// Spalten innerhalb einer Gruppe: Vielfache von 8 als volle Achterreihen, sonst breites Raster.
fn group_cols(n: usize) -> usize {
    match n {
        0..=8 => n.max(1),
        _ if n % 8 == 0 => 8,
        _ => core_cols(n).max(8.min(n)),
    }
}

type Reading = crate::sensors::cputemp::CoreReading;

/// Messwerte des physischen Kerns, zu dem ein logischer Prozessor gehört.
fn reading_of(readings: &[Reading], lp: usize) -> Option<&Reading> {
    readings.iter().find(|r| r.lps.contains(&lp))
}

fn ghz(mhz: Option<f64>) -> Option<String> {
    mhz.filter(|m| *m > 0.0).map(|m| format!("{} GHz", format::de(m / 1000.0, 2)))
}

/// Text mit halbdurchsichtigem Hintergrund (lesbar auch über der Kurve). Gibt die Größe zurück.
fn label_at(ui: &egui::Ui, pos: Pos2, text: String, size: f32, color: Color32) -> Vec2 {
    let g = ui.painter().layout_no_wrap(text, egui::FontId::proportional(size), color);
    let bg = if ui.visuals().dark_mode { Color32::from_black_alpha(150) } else { Color32::from_white_alpha(190) };
    let sz = g.size();
    ui.painter().rect_filled(egui::Rect::from_min_size(pos - Vec2::new(2.0, 0.0), sz + Vec2::new(4.0, 0.0)), 3.0, bg);
    ui.painter().galley(pos, g, color);
    sz
}

fn temp_text(t: f64) -> String {
    format!("{} °C", format::de(t, 0))
}

/// Temperatur einer Gruppe: Intel = heißester Kern; AMD = Tccd des passenden CCD
/// (bei Zen 2 teilen sich zwei CCX ein CCD).
fn group_temp(gi: usize, n_groups: usize, lps: &[usize], ccd_temps: &[f64], readings: &[Reading]) -> Option<f64> {
    let core = readings.iter().filter(|r| r.lps.iter().any(|l| lps.contains(l))).filter_map(|r| r.temp_c).reduce(f64::max);
    if core.is_some() {
        return core;
    }
    match ccd_temps.len() {
        0 => None,
        n if n == n_groups => ccd_temps.get(gi).copied(),
        n if n * 2 == n_groups => ccd_temps.get(gi / 2).copied(),
        _ if n_groups == 1 => ccd_temps.iter().copied().reduce(f64::max),
        _ => None,
    }
}

/// Eine Kachel je logischem Prozessor: oben Takt und (groß) Auslastung, darunter Watt und Spannung
/// des Kerns, unten rechts die Nummer.
fn core_tile(ui: &mut egui::Ui, size: Vec2, lp: usize, q: &VecDeque<f64>, mhz: Option<f64>, rd: Option<&Reading>) {
    let r = graph(ui, size, &[(q, C_CPU)], 100.0);
    let v = q.back().copied();
    let strong = ui.visuals().strong_text_color();
    let text = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    let big = (size.y * 0.27).clamp(15.0, 22.0);
    let small = (big * 0.68).clamp(11.0, 14.0);
    let pct = format::pct0(v);
    let clock = ghz(mhz);
    // Feste Plätze nach der breitesten möglichen Anzeige („100 %“, „0,00 GHz“ …), damit nichts springt,
    // wenn sich Takt, Last oder Watt ändern.
    let width_of = |t: &str, sz: f32| ui.painter().layout_no_wrap(t.to_string(), egui::FontId::proportional(sz), text).size().x;
    let w_pct = width_of("100 %", big);
    let w_clk = width_of("0,00 GHz", small);
    let clock_first = clock.is_some() && w_clk + 6.0 + w_pct + 12.0 <= size.x;
    let p = r.rect.left_top() + Vec2::new(5.0, 3.0);
    let mut x = 0.0;
    if clock_first {
        label_at(ui, p + Vec2::new(0.0, (big - small) * 0.55), clock.clone().unwrap(), small, text);
        x = w_clk + 6.0;
    }
    let line_h = label_at(ui, p + Vec2::new(x, 0.0), pct, big, strong).y;
    // zweite Zeile: (Takt,) Watt, Spannung – jeweils an fester Stelle
    let mut second: Vec<(String, f32)> = Vec::new();
    if !clock_first {
        if let Some(c) = clock.clone() {
            second.push((c, w_clk));
        }
    }
    if let Some(rd) = rd {
        if let Some(wt) = rd.watts {
            second.push((format!("{} W", format::de(wt, 1)), width_of("00,0 W", small)));
        }
        if let Some(vt) = rd.volts {
            second.push((format!("{} V", format::de(vt, 3)), width_of("0,000 V", small)));
        }
    }
    // In Zeilen aufteilen, die in die Kachel passen (feste Plätze, kein Abschneiden)
    let avail = size.x - 10.0;
    let mut lines: Vec<Vec<(String, f32)>> = vec![Vec::new()];
    let mut used = 0.0;
    for (t, w) in second {
        let need = if used == 0.0 { w } else { used + 8.0 + w };
        if need > avail && used > 0.0 {
            lines.push(Vec::new());
            used = w;
        } else {
            used = need;
        }
        lines.last_mut().unwrap().push((t, w));
    }
    for (k, line) in lines.iter().enumerate().filter(|(_, l)| !l.is_empty()) {
        let y = 4.0 + line_h + k as f32 * (small + 3.0);
        if y + small + 2.0 > size.y {
            break;
        }
        let mut q = r.rect.left_top() + Vec2::new(5.0, y);
        for (t, w) in line {
            let got = label_at(ui, q, t.clone(), small, text);
            q.x += got.x.max(*w) + 8.0;
        }
    }
    // Kerntemperatur unten links (weiß)
    if let Some(t) = rd.and_then(|r| r.temp_c) {
        let g = ui.painter().layout_no_wrap(temp_text(t), egui::FontId::proportional(small + 1.0), Color32::WHITE);
        let pos = r.rect.left_bottom() + Vec2::new(5.0, -3.0 - g.size().y);
        ui.painter().rect_filled(egui::Rect::from_min_size(pos - Vec2::new(3.0, 0.0), g.size() + Vec2::new(6.0, 0.0)), 3.0, Color32::from_black_alpha(190));
        ui.painter().galley(pos, g, Color32::WHITE);
    }
    // Nummer unten rechts
    let num = ui.painter().layout_no_wrap(format!("#{lp}"), egui::FontId::proportional(11.0), weak);
    let npos = r.rect.right_bottom() - Vec2::new(5.0 + num.size().x, 3.0 + num.size().y);
    ui.painter().galley(npos, num, weak);
    let mut tip = format!("Logischer Prozessor {lp}: {}", format::pct0(v));
    if let Some(c) = clock {
        tip += &format!(" · {c}");
    }
    if let Some(rd) = rd {
        if rd.watts.is_some() || rd.volts.is_some() || rd.temp_c.is_some() {
            let others: Vec<String> = rd.lps.iter().filter(|l| **l != lp).map(|l| format!("#{l}")).collect();
            tip += &format!(
                "\nKern{}: {}{}{}",
                if others.is_empty() { String::new() } else { format!(" (zusammen mit {})", others.join(", ")) },
                rd.watts.map(|w| format!("{} W", format::de(w, 2))).unwrap_or_default(),
                rd.volts.map(|v| format!("  {} V", format::de(v, 3))).unwrap_or_default(),
                rd.temp_c.map(|t| format!("  {}", temp_text(t))).unwrap_or_default()
            );
        }
    }
    r.on_hover_text(tip);
}

/// Kennzahlen einer Gruppe: (Ø Last, höchster LP, Ø Takt MHz, Σ Watt, Ø Spannung, max Spannung)
fn group_stats(lps: &[usize], cores: &[VecDeque<f64>], mhz: &[f64], readings: &[Reading]) -> (Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>) {
    let vals: Vec<f64> = lps.iter().filter_map(|l| cores.get(*l).and_then(|q| q.back().copied())).collect();
    let avg = (!vals.is_empty()).then(|| vals.iter().sum::<f64>() / vals.len() as f64);
    let max = vals.iter().copied().reduce(f64::max);
    let clocks: Vec<f64> = lps.iter().filter_map(|l| mhz.get(*l).copied()).filter(|m| *m > 0.0).collect();
    let clk = (!clocks.is_empty()).then(|| clocks.iter().sum::<f64>() / clocks.len() as f64);
    let mine: Vec<&Reading> = readings.iter().filter(|r| r.lps.iter().any(|l| lps.contains(l))).collect();
    let w: Vec<f64> = mine.iter().filter_map(|r| r.watts).collect();
    let watts = (!w.is_empty()).then(|| w.iter().sum::<f64>());
    let v: Vec<f64> = mine.iter().filter_map(|r| r.volts).collect();
    let vavg = (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
    let vmax = v.iter().copied().reduce(f64::max);
    (avg, max, clk, watts, vavg, vmax)
}

/// Gruppen, wie sie Leistung → CPU zeigt (ohne leere).
fn visible_groups(n_lps: usize) -> Vec<crate::sensors::topology::CoreGroup> {
    let mut g: Vec<crate::sensors::topology::CoreGroup> = cpu_groups().iter().filter(|g| g.lps.iter().any(|l| *l < n_lps)).cloned().collect();
    if g.is_empty() {
        g.push(crate::sensors::topology::CoreGroup { title: "Alle Kerne".into(), detail: String::new(), lps: (0..n_lps).collect() });
    }
    g
}

/// Name des dritten Reiters, falls die CPU mehrere Gruppen hat: "CCD-Auslastung", "P/E-Kern-Auslastung" …
pub fn group_view_label() -> Option<String> {
    label_for(cpu_groups())
}

fn label_for(g: &[crate::sensors::topology::CoreGroup]) -> Option<String> {
    if g.len() < 2 {
        return None;
    }
    let bare = |t: &str| t.rsplit(" · ").next().unwrap_or(t).to_string();
    let first = bare(&g[0].title);
    if first.starts_with("CCD") {
        return Some("CCD-Auslastung".into());
    }
    if first.starts_with("CCX") {
        return Some("CCX-Auslastung".into());
    }
    let mut kinds: Vec<String> = Vec::new();
    for x in g {
        if let Some(k) = bare(&x.title).strip_suffix("-Kerne") {
            if !kinds.iter().any(|y| y == k) {
                kinds.push(k.to_string());
            }
        }
    }
    Some(match kinds.len() {
        0 => "Auslastung je Sockel".into(),
        // nur eine Kernart (z. B. Server mit zwei Sockeln, nur P-Kerne) → je Sockel
        1 => "Auslastung je Sockel".into(),
        _ => format!("{}-Kern-Auslastung", kinds.join("/")),
    })
}

/// Temperatur als weiße Marke im Textfluss.
fn temp_label(ui: &mut egui::Ui, t: Option<f64>, size: f32) {
    if let Some(t) = t {
        egui::Frame::none().fill(Color32::from_black_alpha(190)).rounding(3.0).inner_margin(egui::Margin::symmetric(5.0, 0.0)).show(ui, |ui| {
            ui.label(RichText::new(temp_text(t)).strong().size(size).color(Color32::WHITE));
        });
    }
}

fn group_header(ui: &mut egui::Ui, g: &crate::sensors::topology::CoreGroup, st: (Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>), temp: Option<f64>) {
    let (avg, max, clk, watts, _vavg, vmax) = st;
    ui.horizontal(|ui| {
        ui.label(RichText::new(&g.title).strong().size(17.0).color(C_CPU));
        temp_label(ui, temp, 15.0);
        ui.label(RichText::new(&g.detail).weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut parts = vec![format!("Ø {}", format::pct0(avg)), format!("max {}", format::pct0(max))];
            if let Some(c) = ghz(clk) {
                parts.push(format!("Ø {c}"));
            }
            if let Some(w) = watts {
                parts.push(format!("{} W", format::de(w, 1)));
            }
            if let Some(v) = vmax {
                parts.push(format!("bis {} V", format::de(v, 3)));
            }
            ui.spacing_mut().item_spacing.x = 16.0;
            for (i, p) in parts.into_iter().enumerate().rev() {
                steady(ui, (&g.title, i), RichText::new(p).strong(), true);
            }
        });
    });
}

/// Logische Prozessoren nach CCD bzw. P-/E-Kernen gruppiert.
fn core_groups_view(ui: &mut egui::Ui, w: f32, cores: &[VecDeque<f64>], mhz: &[f64], readings: &[Reading], ccd_temps: &[f64]) {
    let groups = visible_groups(cores.len());
    let single = groups.len() == 1;
    let gap = 6.0;
    let total_rows: usize = groups.iter().map(|g| g.lps.len().div_ceil(if single { core_cols(g.lps.len()) } else { group_cols(g.lps.len()) })).sum();
    let avail_h = (ui.available_height() - 230.0 - groups.len() as f32 * 34.0).max(56.0 * total_rows as f32);
    for (gi, g) in groups.iter().enumerate() {
        let lps: Vec<usize> = g.lps.iter().copied().filter(|l| *l < cores.len()).collect();
        let n = lps.len();
        if !single || !g.detail.is_empty() {
            ui.add_space(if gi == 0 { 2.0 } else { 8.0 });
            group_header(ui, g, group_stats(&lps, cores, mhz, readings), group_temp(gi, groups.len(), &lps, ccd_temps, readings));
        }
        let cols = if single { core_cols(n) } else { group_cols(n) };
        let cw = ((w - gap * (cols - 1) as f32) / cols as f32).floor() - 1.0;
        let min_h = if readings.iter().any(|r| r.temp_c.is_some()) { 100.0 } else if readings.is_empty() { 50.0 } else { 90.0 };
        let ch = (cw * 0.58).clamp(min_h, 140.0).min((avail_h / total_rows as f32).max(min_h));
        egui::Grid::new(("cores", gi)).spacing([gap, gap]).show(ui, |ui| {
            for (k, lp) in lps.iter().enumerate() {
                core_tile(ui, Vec2::new(cw, ch), *lp, &cores[*lp], mhz.get(*lp).copied(), reading_of(readings, *lp));
                if (k + 1) % cols == 0 {
                    ui.end_row();
                }
            }
        });
    }
}

/// Dritter Reiter: Auslastung je CCD bzw. P-/E-Kern-Gruppe als eigene Verlaufsgrafik.
fn group_load_view(ui: &mut egui::Ui, w: f32, cores: &[VecDeque<f64>], mhz: &[f64], readings: &[Reading], ccd_temps: &[f64]) {
    let groups = visible_groups(cores.len());
    let cols = if groups.len() <= 1 { 1 } else { 2 };
    let gap = 14.0;
    let cw = (w - gap * (cols - 1) as f32) / cols as f32 - 1.0;
    let rows = groups.len().div_ceil(cols);
    let gh = ((ui.available_height() - 260.0) / rows as f32 - 70.0).clamp(110.0, 260.0);
    egui::Grid::new("cpu_groups").spacing([gap, 12.0]).show(ui, |ui| {
        for (gi, g) in groups.iter().enumerate() {
            let lps: Vec<usize> = g.lps.iter().copied().filter(|l| *l < cores.len()).collect();
            // Verlauf: Durchschnitt der logischen Prozessoren je Messpunkt
            let len = lps.iter().map(|l| cores[*l].len()).min().unwrap_or(0);
            let series: VecDeque<f64> = (0..len)
                .map(|t| {
                    let sum: f64 = lps.iter().map(|l| {
                        let q = &cores[*l];
                        q[q.len() - len + t]
                    }).sum();
                    sum / lps.len().max(1) as f64
                })
                .collect();
            let (avg, max, clk, watts, vavg, vmax) = group_stats(&lps, cores, mhz, readings);
            ui.vertical(|ui| {
                ui.set_width(cw);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&g.title).strong().size(18.0).color(C_CPU));
                    steady(ui, (&g.title, "avg"), RichText::new(format::pct0(avg)).strong().size(18.0), true);
                    temp_label(ui, group_temp(gi, groups.len(), &lps, ccd_temps, readings), 16.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(&g.detail).weak());
                    });
                });
                graph(ui, Vec2::new(cw, gh), &[(&series, C_CPU)], 100.0);
                ui.horizontal_wrapped(|ui| {
                    stat(ui, "Auslastung", format::pct0(avg));
                    stat(ui, "Stärkster Thread", format::pct0(max));
                    stat(ui, "Ø Takt", ghz(clk).unwrap_or("–".into()));
                    if let Some(w) = watts {
                        stat(ui, "Leistung", format!("{} W", format::de(w, 1)));
                    }
                    if let (Some(a), Some(m)) = (vavg, vmax) {
                        stat(ui, "Spannung", format!("{}–{} V", format::de(a, 2), format::de(m, 2)));
                    }
                });
            });
            if (gi + 1) % cols == 0 {
                ui.end_row();
            }
        }
    });
}

/// Hinweis, woher Takt/Watt/Spannung kommen bzw. warum sie fehlen.
fn per_core_hint(ui: &mut egui::Ui, readings: &[Reading]) {
    let intel = crate::sensors::cputemp::cpu_info().vendor.contains("Intel");
    let any_w = readings.iter().any(|r| r.watts.is_some());
    let any_v = readings.iter().any(|r| r.volts.is_some());
    let t = if readings.is_empty() {
        "Takt je Kern aus Windows. Temperatur, Watt und Spannung je Kern gibt es nur mit Adminrechten und PawnIO.".to_string()
    } else if intel {
        format!("Takt aus Windows, Temperatur und Spannung je Kern über PawnIO{}. Intel meldet keine Watt je Kern, nur für die ganze CPU.", if any_v { "" } else { " (Spannung von dieser CPU nicht gemeldet)" })
    } else {
        format!(
            "Takt aus Windows; Watt je Kern{} und Spannung (angeforderte VID{}) über PawnIO. AMD meldet die Temperatur je CCD, nicht je Kern – sie steht in der CCD-Überschrift. Bei SMT teilen sich zwei logische Prozessoren einen Kern – beide zeigen dessen Werte.",
            if any_w { "" } else { " (nicht lesbar)" },
            if any_v { "" } else { ", nicht lesbar" }
        )
    };
    ui.label(RichText::new(t).small().weak());
}

/// Aktive Zeit und Übertragungsrate eines Datenträgers (oder aller).
#[allow(clippy::too_many_arguments)]
fn disk_page(ui: &mut egui::Ui, w: f32, title: &str, right: &str, active: &VecDeque<f64>, read: &VecDeque<f64>, write: &VecDeque<f64>, stats: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.heading(title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(right);
        });
    });
    ui.label(RichText::new("Aktive Zeit in %").small().weak());
    graph(ui, Vec2::new(w, 240.0), &[(active, C_DISK)], 100.0);
    let max = disk_max(read, write);
    ui.add_space(6.0);
    ui.label(RichText::new(format!("Übertragungsrate (Skala {}) · dunkel = Lesen, hell = Schreiben", format::rate_de(max, 0))).small().weak());
    graph(ui, Vec2::new(w, 140.0), &[(read, C_DISK), (write, C_DISK2)], max);
    ui.add_space(6.0);
    ui.horizontal_wrapped(stats);
}

fn net_max(h: &VecDeque<f64>, h2: &VecDeque<f64>) -> f64 {
    let m = h.iter().chain(h2.iter()).copied().fold(0.0, f64::max);
    // auf "runde" Skala aufrunden
    let mut scale = 125_000.0; // 1 Mbit/s
    while scale < m {
        scale *= 2.0;
    }
    scale
}

/// Weiße Temperaturmarke an fester Stelle (oben rechts in einer Karte).
fn temp_badge(ui: &egui::Ui, rect: egui::Rect, t: f64) {
    let g = ui.painter().layout_no_wrap(temp_text(t), egui::FontId::proportional(15.0), Color32::WHITE);
    let pos = rect.right_top() + Vec2::new(-8.0 - g.size().x, 8.0);
    ui.painter().rect_filled(egui::Rect::from_min_size(pos - Vec2::new(5.0, 1.0), g.size() + Vec2::new(10.0, 2.0)), 3.0, Color32::from_black_alpha(190));
    ui.painter().galley(pos, g, Color32::WHITE);
}

/// Leistung → Arbeitsspeicher: je Riegel eine Karte (Steckplatz, Größe, Hersteller, Takt, Temperatur),
/// darunter die laufenden Timings.
fn ram_modules_view(ui: &mut egui::Ui, w: f32, dimms: &[crate::sensors::dimm::Dimm], temps: &[crate::sensors::dimm::DimmTemp], timings: &[crate::sensors::dimm::Timings]) {
    use crate::sensors::dimm::{assign_temps, rfc_ns};
    if dimms.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("Die Speicherriegel konnten nicht gelesen werden (SMBIOS-Tabelle fehlt).").small().weak());
        return;
    }
    ui.add_space(10.0);
    ui.label(RichText::new("Speicherriegel").strong().size(17.0).color(C_MEM));
    let per_dimm = assign_temps(dimms.len(), temps);
    let gap = 10.0;
    let cols = dimms.len().clamp(1, ((w + gap) / 300.0).floor().max(1.0) as usize);
    let cw = ((w - gap * (cols - 1) as f32) / cols as f32).floor() - 1.0;
    egui::Grid::new("dimms").spacing([gap, gap]).show(ui, |ui| {
        for (i, d) in dimms.iter().enumerate() {
            let r = egui::Frame::none()
                .fill(ui.visuals().faint_bg_color)
                .stroke(egui::Stroke::new(1.0_f32, C_MEM.linear_multiply(0.5)))
                .rounding(6.0)
                .inner_margin(egui::Margin::same(10.0))
                .show(ui, |ui| {
                    ui.set_width(cw - 20.0);
                    ui.vertical(|ui| {
                    ui.label(RichText::new(if d.slot.is_empty() { format!("Riegel {}", i + 1) } else { d.slot.clone() }).strong().color(C_MEM));
                    let speed = d.configured_mts.map(|m| format!("-{m}")).unwrap_or_default();
                    ui.label(RichText::new(format!("{} GB {}{}", format::de(d.size_bytes / format::GIB, 0), d.kind, speed)).heading());
                    let name = [d.manufacturer.clone(), d.part_number.clone()].into_iter().flatten().collect::<Vec<_>>().join(" ");
                    if !name.is_empty() {
                        ui.label(name);
                    }
                    let mut det: Vec<String> = Vec::new();
                    if !d.bank.is_empty() {
                        det.push(d.bank.clone());
                    }
                    if let Some(r) = d.ranks {
                        det.push(format!("{r} Rank"));
                    }
                    if let Some(v) = d.volts {
                        det.push(format!("{} V", format::de(v, 2)));
                    }
                    if let Some(m) = d.rated_mts {
                        det.push(format!("Riegel bis {m} MT/s"));
                    }
                    ui.label(RichText::new(det.join(" · ")).small().weak());
                    });
                })
                .response;
            if let Some(t) = per_dimm.as_ref().and_then(|v| v.get(i)) {
                temp_badge(ui, r.rect, *t);
            }
            if (i + 1) % cols == 0 {
                ui.end_row();
            }
        }
    });
    if per_dimm.is_none() && !temps.is_empty() {
        let list: Vec<String> = temps.iter().map(|t| format!("Sensor {}: {}", t.index, temp_text(t.temp_c))).collect();
        ui.label(format!("Temperaturen der Riegel: {}", list.join(" · ")));
    }
    let mut hint = String::from("Eine Auslastung je Riegel gibt es nicht: Der Speichercontroller verteilt die Daten gleichmäßig über alle Riegel (Interleaving) – die Belegung oben gilt für alle zusammen.");
    if temps.is_empty() {
        hint += if cfg!(windows) {
            " Temperatur je Riegel: mit Adminrechten und PawnIO, wenn die Riegel einen Sensor haben (DDR5 immer, DDR4 nur manche)."
        } else {
            " Temperatur je Riegel: über die Kernel-Treiber jc42 (DDR4) bzw. spd5118 (DDR5), wenn die Riegel einen Sensor haben."
        };
    }
    ui.add(egui::Label::new(RichText::new(hint).small().weak()).wrap());

    // Timings
    ui.add_space(10.0);
    ui.label(RichText::new("Timings").strong().size(17.0).color(C_MEM));
    let Some(t0) = timings.first() else {
        let why = if !cfg!(windows) {
            "Unter Linux liest TaskPulse die Timings noch nicht aus."
        } else if crate::sensors::cputemp::cpu_info().vendor.contains("Intel") {
            "Timings liest TaskPulse bisher nur bei AMD Ryzen aus dem Speichercontroller – Intel folgt."
        } else {
            "Die laufenden Timings kommen direkt aus dem Speichercontroller – dafür braucht TaskPulse Adminrechte und PawnIO."
        };
        ui.add(egui::Label::new(RichText::new(why).small().weak()).wrap());
        return;
    };
    let mts = t0.mts.or(dimms.first().and_then(|d| d.configured_mts).map(|m| m as f64));
    let same = timings.iter().all(|t| timings_eq(t, t0));
    let shown: Vec<&crate::sensors::dimm::Timings> = if same { vec![t0] } else { timings.iter().collect() };
    ui.horizontal(|ui| {
        ui.label(RichText::new(t0.primary()).size(26.0).strong());
        ui.vertical(|ui| {
            ui.label(RichText::new("CL-tRCD-tRP-tRAS").small().weak());
            let mut extra: Vec<String> = Vec::new();
            if let Some(c) = t0.cmd2t {
                extra.push(if c { "2T".into() } else { "1T".into() });
            }
            if let Some(g) = t0.gdm {
                extra.push(format!("Gear Down {}", if g { "an" } else { "aus" }));
            }
            if let Some(m) = mts {
                extra.push(format!("{} MT/s (MCLK {} MHz)", m.round(), (m / 2.0).round()));
            }
            ui.label(extra.join(" · "));
        });
    });
    let rows: Vec<(&str, Box<dyn Fn(&crate::sensors::dimm::Timings) -> String>)> = vec![
        ("tCL", Box::new(|t| t.cl.to_string())),
        ("tRCD (Lesen)", Box::new(|t| t.rcd_rd.to_string())),
        ("tRCD (Schreiben)", Box::new(|t| t.rcd_wr.to_string())),
        ("tRP", Box::new(|t| t.rp.to_string())),
        ("tRAS", Box::new(|t| t.ras.to_string())),
        ("tRC", Box::new(|t| t.rc.to_string())),
        ("tRRD_S", Box::new(|t| t.rrds.to_string())),
        ("tRRD_L", Box::new(|t| t.rrdl.to_string())),
        ("tFAW", Box::new(|t| t.faw.to_string())),
        ("tWTR_S", Box::new(|t| t.wtrs.to_string())),
        ("tWTR_L", Box::new(|t| t.wtrl.to_string())),
        ("tWR", Box::new(|t| t.wr.to_string())),
        ("tRTP", Box::new(|t| t.rtp.to_string())),
        ("tCWL", Box::new(|t| t.cwl.to_string())),
        (
            "tRFC",
            Box::new(move |t| match t.rfc {
                Some(r) => match mts.and_then(|m| rfc_ns(r, m)) {
                    Some(ns) => format!("{r} ({} ns)", format::de(ns, 0)),
                    None => r.to_string(),
                },
                None => "–".into(),
            }),
        ),
    ];
    // In Spalten nebeneinander, damit es nicht zu lang wird
    let per_col = if same { rows.len().div_ceil(((w / 230.0).floor() as usize).clamp(1, 5)) } else { rows.len() };
    ui.horizontal_top(|ui| {
        for chunk in rows.chunks(per_col.max(1)) {
            egui::Grid::new(("timings", chunk[0].0)).num_columns(1 + shown.len()).spacing([18.0, 3.0]).striped(true).show(ui, |ui| {
                if !same {
                    ui.label("");
                    for t in &shown {
                        ui.label(RichText::new(format!("Kanal {}", (b'A' + t.channel as u8) as char)).weak());
                    }
                    ui.end_row();
                }
                for (name, f) in chunk {
                    ui.label(RichText::new(*name).weak());
                    for t in &shown {
                        ui.label(RichText::new(f(t)).strong());
                    }
                    ui.end_row();
                }
            });
            ui.add_space(24.0);
        }
    });
    ui.add(egui::Label::new(RichText::new("Laufende Werte aus dem Speichercontroller (in Takten), nur gelesen. Die Riegel-Angaben kommen aus der SMBIOS-Tabelle des BIOS.").small().weak()).wrap());
}


fn timings_eq(a: &crate::sensors::dimm::Timings, b: &crate::sensors::dimm::Timings) -> bool {
    let mut a = a.clone();
    a.channel = b.channel;
    a == *b
}

/// Karte im Stil der Speicherriegel: Rahmen, Titel in Farbe, Inhalt untereinander.
fn info_card(ui: &mut egui::Ui, width: f32, color: Color32, add: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    egui::Frame::none()
        .fill(ui.visuals().faint_bg_color)
        .stroke(egui::Stroke::new(1.0_f32, color.linear_multiply(0.5)))
        .rounding(6.0)
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            ui.set_width(width - 20.0);
            ui.set_max_width(width - 20.0);
            ui.vertical(add);
        })
        .response
}

/// Einzelner Datenträger: Karte mit dem Laufwerk selbst (Modell, Typ, Größe, Temperatur)
/// und je Laufwerk (Partition) eine Karte mit Name wie im Explorer und Belegung.
fn disk_cards(ui: &mut egui::Ui, w: f32, d: &crate::sensors::mem::DiskOne) {
    ui.add_space(10.0);
    ui.label(RichText::new("Laufwerk").strong().size(17.0).color(C_DISK));
    let gap = 10.0;
    let n = 1 + d.volumes.len();
    let cols = n.clamp(1, ((w + gap) / 300.0).floor().max(1.0) as usize);
    let cw = ((w - gap * (cols - 1) as f32) / cols as f32).floor() - 1.0;
    egui::Grid::new(("disk_cards", d.index)).spacing([gap, gap]).show(ui, |ui| {
        let r = info_card(ui, cw, C_DISK, |ui| {
            ui.label(RichText::new(format!("Datenträger {}", d.index)).strong().color(C_DISK));
            ui.label(RichText::new(d.info.model.clone().unwrap_or("Unbekanntes Modell".into())).heading());
            let mut det: Vec<String> = Vec::new();
            if let Some(k) = &d.info.kind {
                det.push(k.clone());
            }
            if let Some(c) = d.capacity {
                det.push(format::mem(c, crate::config::MemUnit::GB, 0));
            }
            ui.label(det.join(" · "));
            ui.label(
                RichText::new(if d.is_empty_adapter() {
                    "Keine Festplatte angeschlossen – z. B. leerer SATA-auf-USB-Adapter oder Kartenleser".to_string()
                } else if d.volumes.is_empty() {
                    "kein Laufwerksbuchstabe".to_string()
                } else {
                    format!("{} Laufwerk(e)", d.volumes.len())
                })
                .small()
                .weak(),
            );
        });
        if let Some(t) = d.temp_c {
            temp_badge(ui, r.rect, t);
        }
        for (i, v) in d.volumes.iter().enumerate() {
            if (i + 1) % cols == 0 {
                ui.end_row();
            }
            info_card(ui, cw, C_DISK, |ui| {
                ui.label(RichText::new(&v.letter).strong().color(C_DISK));
                ui.label(RichText::new(v.explorer_name()).heading());
                if let (Some(t), Some(f)) = (v.total, v.free) {
                    let used = (t - f).max(0.0);
                    let frac = (used / t) as f32;
                    // Belegungsbalken wie im Explorer (rot ab 90 %)
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 10.0), Sense::hover());
                    ui.painter().rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
                    let col = if frac >= 0.9 { Color32::from_rgb(218, 38, 38) } else { C_DISK };
                    ui.painter().rect_filled(egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * frac.clamp(0.0, 1.0), rect.height())), 2.0, col);
                    ui.label(format!(
                        "{} frei von {}",
                        format::mem(f, crate::config::MemUnit::GB, 1),
                        format::mem(t, crate::config::MemUnit::GB, 1)
                    ));
                }
                if v.disks.len() > 1 {
                    let list = v.disks.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ");
                    ui.label(RichText::new(format!("Verbund aus {} Datenträgern ({list})", v.disks.len())).strong());
                    if let Some(h) = v.here {
                        ui.label(format!("davon auf diesem Datenträger: {}", format::mem(h, crate::config::MemUnit::GB, 1)));
                    }
                }
                ui.label(RichText::new(v.fs.clone().unwrap_or_default()).small().weak());
            });
        }
    });
}


/// Ausklappbare Spezifikation (wie GPU-Z / CPU-Z): je Abschnitt eine Karte mit Bezeichnung und Wert.
fn spec_view(ui: &mut egui::Ui, w: f32, id: &str, spec: &crate::sensors::spec::Spec, color: Color32) {
    ui.add_space(8.0);
    let screenshot = std::env::var_os("TASKPULSE_SPEC_OFFEN").is_some();
    let r = egui::CollapsingHeader::new(RichText::new("Spezifikation").strong().size(17.0).color(color)).id_salt(id).default_open(screenshot).show(ui, |ui| {
        if spec.sections.is_empty() {
            ui.label(RichText::new("Noch keine Angaben – einen Moment …").weak());
            return;
        }
        let gap = 10.0;
        let w = ui.available_width().min(w) - 4.0;
        let cols = if w >= 820.0 { 2 } else { 1 };
        let cw = ((w - gap * (cols - 1) as f32) / cols as f32).floor() - 1.0;
        let label_w = (cw * 0.36).clamp(120.0, 190.0);
        egui::Grid::new((id, "spec")).spacing([gap, gap]).show(ui, |ui| {
            for (i, sec) in spec.sections.iter().enumerate() {
                info_card(ui, cw, color, |ui| {
                    ui.label(RichText::new(&sec.title).strong().color(color));
                    for (k, v) in &sec.rows {
                        ui.horizontal_top(|ui| {
                            ui.allocate_ui_with_layout(Vec2::new(label_w, 18.0), egui::Layout::left_to_right(egui::Align::Min), |ui| {
                                ui.set_width(label_w);
                                ui.add(egui::Label::new(RichText::new(k).weak()).truncate());
                            });
                            ui.add(egui::Label::new(RichText::new(v).strong()).wrap());
                        });
                    }
                });
                if (i + 1) % cols == 0 {
                    ui.end_row();
                }
            }
        });
        ui.add(
            egui::Label::new(
                RichText::new("Angaben so, wie Treiber, Betriebssystem, BIOS und CPU sie melden – ohne Modelldatenbank. Was nicht gemeldet wird, fehlt.")
                    .small()
                    .weak(),
            )
            .wrap(),
        );
    });
    // Nur für Bildschirmfotos: in den ersten Sekunden zur Spezifikation scrollen
    if screenshot && !spec.sections.is_empty() && ui.ctx().input(|i| i.time) < 30.0 {
        r.header_response.scroll_to_me(Some(egui::Align::Min));
    }
}

/// Datenträger (alle): Liste mit Name, Typ, Größe, Temperatur und Aktivität.
fn disk_list(ui: &mut egui::Ui, disks: &[crate::sensors::mem::DiskOne]) {
    if disks.is_empty() {
        return;
    }
    ui.add_space(10.0);
    egui::Grid::new("disk_list").num_columns(7).spacing([18.0, 4.0]).striped(true).show(ui, |ui| {
        for h in ["Nr.", "Name", "Modell", "Typ", "Kapazität", "Temperatur", "Aktive Zeit"] {
            ui.label(RichText::new(h).weak());
        }
        ui.end_row();
        for d in disks {
            ui.label(d.index.to_string());
            ui.label(RichText::new(d.title()).strong());
            ui.label(d.info.model.clone().unwrap_or("–".into()));
            ui.label(d.info.kind.clone().unwrap_or("–".into()));
            ui.label(d.capacity.map(|c| format::mem(c, crate::config::MemUnit::GB, 0)).unwrap_or(if d.is_empty_adapter() { "leer".into() } else { "–".into() }));
            ui.label(d.temp_c.map(temp_text).unwrap_or("–".into()));
            ui.label(format::pct0(d.active_pct));
            ui.end_row();
        }
    });
    if disks.iter().any(|d| d.temp_c.is_none() && d.info.kind.as_deref() != Some("USB")) {
        ui.add(egui::Label::new(
            RichText::new(if cfg!(windows) {
                "Temperaturen: NVMe-SSDs auch ohne Adminrechte, SATA-SSDs und Festplatten (über SMART) nur mit Adminrechten. Schlafende Festplatten werden dafür nicht geweckt; USB-Gehäuse melden meist keine Temperatur."
            } else {
                "Temperaturen kommen von den Kernel-Treibern nvme bzw. drivetemp (SATA)."
            })
            .small()
            .weak(),
        ).wrap());
    }
}

pub fn show(app: &mut TaskPulseApp, ui: &mut egui::Ui) {
    let f = &app.frame;
    let h = &app.history;
    let b = &f.basic;
    let st = &mut app.perf;
    let view = &mut app.cfg.cpu_view;
    // Ausgewählter einzelner Datenträger verschwunden (z. B. USB abgezogen) -> alle
    if let Item::DiskOne(i) = st.item {
        // (erst prüfen, wenn Messwerte da sind – im ersten Moment ist die Liste noch leer)
        if !f.disk.disks.is_empty() && !f.disk.disks.iter().any(|d| d.index == i) {
            st.item = Item::Disk;
        }
    }
    let gpu_name = b.gpu.name.clone().or_else(|| f.adapters.first().map(|a| a.name.clone())).unwrap_or_else(|| "GPU".into());
    let netmax = net_max(&h.net_down, &h.net_up);

    egui::SidePanel::left("perf_side").resizable(false).exact_width(290.0).show_inside(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            let cpu_sub = format!("{} {}", format::pct0(b.cpu.load_pct), b.cpu_freq_mhz.map(|m| format!("{} GHz", format::de(m / 1000.0, 2))).unwrap_or_default());
            if side_entry(ui, st.item == Item::Cpu, "CPU", &cpu_sub, &h.cpu, 100.0, C_CPU) {
                st.item = Item::Cpu;
            }
            let mem_sub = format!(
                "{}/{} ({})",
                format::de(b.mem.used_bytes / format::GIB, 1),
                format::mem(b.mem.total_bytes, crate::config::MemUnit::GB, 1),
                format::pct0(b.mem.used_pct)
            );
            if side_entry(ui, st.item == Item::Mem, "Arbeitsspeicher", &mem_sub, &h.mem, 100.0, C_MEM) {
                st.item = Item::Mem;
            }
            // Grafikkarte direkt unter dem Arbeitsspeicher
            let gpu_sub = format!("{}  {}", format::pct0(f.gpu.total_pct.or(b.gpu.load_pct)), format::temp(b.gpu.temp_c));
            if side_entry(ui, st.item == Item::Gpu, &format!("GPU 0 – {gpu_name}"), &gpu_sub, &h.gpu, 100.0, C_GPU) {
                st.item = Item::Gpu;
            }
            if side_entry(ui, st.item == Item::Disk, "Datenträger (alle)", &format::pct0(f.disk.active_pct), &h.disk, 100.0, C_DISK) {
                st.item = Item::Disk;
            }
            for d in &f.disk.disks {
                let Some(hd) = h.disks.iter().find(|x| x.index == d.index) else { continue };
                // Ohne Laufwerksbuchstaben heißt der Eintrag schon „Datenträger N“ → dann das Modell darunter
                let first = if d.is_empty_adapter() {
                    format!("{} · keine Festplatte", d.info.model.clone().unwrap_or("USB".into()))
                } else if d.volumes.is_empty() {
                    d.info.model.clone().unwrap_or_default()
                } else {
                    format!("Datenträger {}", d.index)
                };
                let sub = format!(
                    "{first}\n{}{}{}",
                    d.info.kind.as_deref().map(|k| format!("{k}  ")).unwrap_or_default(),
                    format::pct0(d.active_pct),
                    d.temp_c.map(|t| format!("  {}", temp_text(t))).unwrap_or_default()
                );
                if side_entry(ui, st.item == Item::DiskOne(d.index), &d.title(), &sub, &hd.active, 100.0, C_DISK) {
                    st.item = Item::DiskOne(d.index);
                }
            }
            let net_sub = format!(
                "{}  ↓ {} ↑ {}",
                b.net.adapter_name.clone().unwrap_or_default(),
                format::net_rate(b.net.down_bps, crate::config::NetUnit::Mbitps),
                format::net_rate(b.net.up_bps, crate::config::NetUnit::Mbitps)
            );
            if side_entry(ui, st.item == Item::Net, "Netzwerk", &net_sub, &h.net_down, netmax, C_NET) {
                st.item = Item::Net;
            }
        });
    });

    egui::CentralPanel::default().show_inside(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            let w = ui.available_width().max(200.0);
            match st.item {
                Item::Cpu => {
                    let brand = crate::sensors::cputemp::cpu_info().brand;
                    ui.horizontal(|ui| {
                        ui.heading("CPU");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(brand);
                        });
                    });
                    use crate::config::CpuView;
                    let group_label = group_view_label();
                    // Gespeicherte Gruppenansicht, aber diese CPU hat keine Gruppen → Gesamt
                    if *view == CpuView::Groups && group_label.is_none() {
                        *view = CpuView::Total;
                    }
                    ui.horizontal(|ui| {
                        ui.radio_value(view, CpuView::Total, "Gesamtauslastung");
                        ui.radio_value(view, CpuView::Logical, "Logische Prozessoren");
                        if let Some(l) = &group_label {
                            ui.radio_value(view, CpuView::Groups, l.as_str());
                        }
                    });
                    if *view == CpuView::Logical && !h.cores.is_empty() {
                        core_groups_view(ui, w, &h.cores, &b.cpu.cores_mhz, &f.core_readings, &f.ccd_temps);
                        per_core_hint(ui, &f.core_readings);
                    } else if *view == CpuView::Groups && !h.cores.is_empty() {
                        group_load_view(ui, w, &h.cores, &b.cpu.cores_mhz, &f.core_readings, &f.ccd_temps);
                        per_core_hint(ui, &f.core_readings);
                    } else {
                        graph(ui, Vec2::new(w, 300.0), &[(&h.cpu, C_CPU)], 100.0);
                    }
                    ui.label(RichText::new("% Auslastung über 60 Messungen").small().weak());
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        stat(ui, "Auslastung", format::pct0(b.cpu.load_pct));
                        stat(ui, "Geschwindigkeit", b.cpu_freq_mhz.map(|m| format!("{} GHz", format::de(m / 1000.0, 2))).unwrap_or("–".into()));
                        stat(ui, "Grundtakt", b.cpu.nominal_mhz.map(|m| format!("{} GHz", format::de(m / 1000.0, 2))).unwrap_or("–".into()));
                        stat(ui, "Temperatur", format::temp(b.cpu_temp_c));
                        stat(ui, "Leistung", format::watts(b.cpu_power_w));
                        stat(ui, "Logische Prozessoren", b.cpu.cores_pct.len().to_string());
                        let threads: u32 = f.procs.iter().map(|p| p.threads).sum();
                        let handles: u32 = f.procs.iter().map(|p| p.handles).sum();
                        stat(ui, "Prozesse", f.procs.len().to_string());
                        stat(ui, "Threads", threads.to_string());
                        stat(ui, "Handles", handles.to_string());
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(format!("Temperatur: {}", b.cpu_temp_status)).small().weak());
                        // Ohne PawnIO gibt es keine CPU-Temperatur/-Watt (und keine RAM-Temperaturen) – direkt zum Download
                        if b.cpu_temp_status.contains("pawnio.eu") || crate::ui::pawnio_state_done() {
                            crate::ui::pawnio_install_button(ui);
                        }
                    });
                    spec_view(ui, w, "cpu", &f.cpu_spec, C_CPU);
                }
                Item::Mem => {
                    let dimms = crate::sensors::dimm::modules();
                    ui.horizontal(|ui| {
                        ui.heading("Arbeitsspeicher");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if let Some(d) = dimms.first() {
                                let total: f64 = dimms.iter().map(|d| d.size_bytes).sum();
                                ui.label(format!(
                                    "{} GB {}{}",
                                    format::de(total / format::GIB, 0),
                                    d.kind,
                                    d.configured_mts.map(|m| format!(" · {m} MT/s")).unwrap_or_default()
                                ));
                            }
                        });
                    });
                    graph(ui, Vec2::new(w, 260.0), &[(&h.mem, C_MEM)], 100.0);
                    ui.horizontal_wrapped(|ui| {
                        stat(ui, "In Verwendung", format::mem(b.mem.used_bytes, crate::config::MemUnit::GB, 1));
                        stat(ui, "Verfügbar", format::mem(b.mem.total_bytes - b.mem.used_bytes, crate::config::MemUnit::GB, 1));
                        stat(ui, "Gesamt", format::mem(b.mem.total_bytes, crate::config::MemUnit::GB, 1));
                        stat(
                            ui,
                            "Zugesichert",
                            format!(
                                "{}/{}",
                                format::de(b.mem.commit_used / format::GIB, 1),
                                format::mem(b.mem.commit_limit, crate::config::MemUnit::GB, 1)
                            ),
                        );
                        if let Some(d) = dimms.first() {
                            stat(ui, "Geschwindigkeit", d.configured_mts.map(|m| format!("{m} MT/s")).unwrap_or("–".into()));
                            stat(ui, "Steckplätze belegt", dimms.len().to_string());
                        }
                    });
                    ram_modules_view(ui, w, dimms, &f.dimm_temps, &f.ram_timings);
                }
                Item::Disk => {
                    let n = f.disk.disks.len();
                    disk_page(ui, w, "Datenträger (alle)", &format!("{n} Datenträger"), &h.disk, &h.disk_read, &h.disk_write, |ui| {
                        stat(ui, "Aktive Zeit", format::pct0(f.disk.active_pct));
                        stat(ui, "Lesen", f.disk.read_bps.map(|v| format::rate_de(v, 1)).unwrap_or("–".into()));
                        stat(ui, "Schreiben", f.disk.write_bps.map(|v| format::rate_de(v, 1)).unwrap_or("–".into()));
                    });
                    disk_list(ui, &f.disk.disks);
                }
                Item::DiskOne(i) => {
                    if let (Some(d), Some(hd)) = (f.disk.disks.iter().find(|d| d.index == i), h.disks.iter().find(|x| x.index == i)) {
                        disk_page(ui, w, &d.title(), &format!("Datenträger {}", d.index), &hd.active, &hd.read, &hd.write, |ui| {
                            stat(ui, "Aktive Zeit", format::pct0(d.active_pct));
                            stat(ui, "Lesen", d.read_bps.map(|v| format::rate_de(v, 1)).unwrap_or("–".into()));
                            stat(ui, "Schreiben", d.write_bps.map(|v| format::rate_de(v, 1)).unwrap_or("–".into()));
                            stat(ui, "Typ", d.info.kind.clone().unwrap_or("–".into()));
                            stat(ui, "Kapazität", d.capacity.map(|c| format::mem(c, crate::config::MemUnit::GB, 0)).unwrap_or("–".into()));
                            stat(ui, "Temperatur", d.temp_c.map(temp_text).unwrap_or("–".into()));
                            stat(ui, "Laufwerke", if d.letters.is_empty() { "–".into() } else { d.letters.join(" ") });
                        });
                        disk_cards(ui, w, d);
                    }
                }
                Item::Net => {
                    ui.heading(format!("Netzwerk – {}", b.net.adapter_name.clone().unwrap_or("kein Adapter".into())));
                    graph(ui, Vec2::new(w, 300.0), &[(&h.net_down, C_NET), (&h.net_up, C_NET2)], netmax);
                    ui.label(RichText::new(format!("Skala: {}  ·  dunkel = Empfangen, hell = Senden", format::net_rate(Some(netmax), crate::config::NetUnit::Mbitps))).small().weak());
                    ui.horizontal_wrapped(|ui| {
                        stat(ui, "Empfangen", format::net_rate(b.net.down_bps, crate::config::NetUnit::Mbitps));
                        stat(ui, "Senden", format::net_rate(b.net.up_bps, crate::config::NetUnit::Mbitps));
                        stat(ui, "Leitung ↓", b.line_down_pct.map(|v| format!("{v:.0} %")).unwrap_or("nicht hinterlegt".into()));
                        stat(ui, "Leitung ↑", b.line_up_pct.map(|v| format!("{v:.0} %")).unwrap_or("nicht hinterlegt".into()));
                        stat(ui, "Verbindung", if b.net.rx_link_bps > 0 { format!("{} Mbit/s", b.net.rx_link_bps / 1_000_000) } else { "–".into() });
                    });
                }
                Item::Gpu => {
                    ui.heading(format!("GPU 0 – {gpu_name}"));
                    let engines: Vec<&(String, VecDeque<f64>)> = h.engines.iter().filter(|(n, _)| !n.is_empty()).collect();
                    if engines.is_empty() {
                        graph(ui, Vec2::new(w, 260.0), &[(&h.gpu, C_GPU)], 100.0);
                    } else {
                        // Wichtigste Engines zuerst, dann die übrigen nach Last; Raster 2 × 3
                        let prio = ["3D", "Copy", "Video Encode", "Video Decode", "Compute", "Compute 0"];
                        let peak = |q: &VecDeque<f64>| q.iter().copied().fold(0.0, f64::max);
                        let mut sorted = engines.clone();
                        sorted.sort_by(|a, b| {
                            let pa = prio.iter().position(|p| *p == a.0).unwrap_or(99);
                            let pb = prio.iter().position(|p| *p == b.0).unwrap_or(99);
                            pa.cmp(&pb).then_with(|| peak(&b.1).total_cmp(&peak(&a.1)))
                        });
                        let show: Vec<&(String, VecDeque<f64>)> = sorted.into_iter().take(6).collect();
                        let cols = 2;
                        let gap = 14.0;
                        let cw = (w - gap - 4.0) / cols as f32;
                        let gh = (cw * 0.42).clamp(120.0, 220.0);
                        egui::Grid::new("engines").spacing([gap, 10.0]).show(ui, |ui| {
                            for (i, (name, q)) in show.iter().enumerate() {
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(name.as_str()).strong());
                                        ui.label(format::pct0(q.back().copied()));
                                    });
                                    graph(ui, Vec2::new(cw, gh), &[(q, C_GPU)], 100.0);
                                });
                                if (i + 1) % cols == 0 {
                                    ui.end_row();
                                }
                            }
                        });
                    }
                    ui.add_space(4.0);
                    ui.label(RichText::new("Dedizierter GPU-Speicher in %").small().weak());
                    graph(ui, Vec2::new(w, 110.0), &[(&h.vram, C_GPU)], 100.0);
                    ui.horizontal_wrapped(|ui| {
                        stat(ui, "Auslastung", format::pct0(f.gpu.total_pct.or(b.gpu.load_pct)));
                        stat(ui, "Temperatur", format::temp(b.gpu.temp_c));
                        if b.gpu.hotspot_c.is_some() || b.gpu.mem_temp_c.is_some() {
                            stat(ui, "Hot-Spot", format::temp(b.gpu.hotspot_c));
                            stat(ui, "Speichertemperatur", format::temp(b.gpu.mem_temp_c));
                        }
                        stat(ui, "GPU-Takt", b.gpu.clock_mhz.map(|m| format!("{m:.0} MHz")).unwrap_or("–".into()));
                        stat(ui, "Speichertakt", b.gpu.mem_clock_mhz.map(|m| format!("{m:.0} MHz")).unwrap_or("–".into()));
                        stat(
                            ui,
                            "Dedizierter Speicher",
                            match (f.vram_used, f.vram_total) {
                                (Some(u), Some(t)) => format!("{}/{} GB", format::de(u / format::GIB, 1), format::de(t / format::GIB, 1)),
                                _ => "–".into(),
                            },
                        );
                        stat(ui, "Gemeinsamer Speicher", f.gpu.adapter_shared_used.map(|v| format::mem(v, crate::config::MemUnit::GB, 1)).unwrap_or("–".into()));
                        stat(ui, "Leistungsaufnahme", b.gpu.power_w.map(|v| format!("{v:.0} W")).unwrap_or("–".into()));
                    });
                    // Erkennung der Karte (wie HWiNFO/GPU-Z: PCI-Kennungen, Kartenhersteller, Treiber)
                    let ad = f.adapters.iter().find(|a| Some(&a.name) == b.gpu.name.as_ref()).or(f.adapters.first());
                    if let Some(ad) = ad {
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            stat(ui, "Chip", format!("{} {}", ad.vendor_name(), ad.name.trim_start_matches(ad.vendor_name()).trim()));
                            stat(ui, "Kartenhersteller", ad.board_vendor().map(String::from).unwrap_or_else(|| format!("unbekannt ({:04X})", ad.subsys_id & 0xFFFF)));
                            stat(ui, "Treiber", ad.driver_display().unwrap_or("–".into()));
                            stat(ui, "PCIe-Verbindung", f.gpu_link.map(|l| format!("PCIe {}", l.short())).unwrap_or("–".into()));
                        });
                        ui.label(RichText::new(format!("PCI {} · Quelle: {}", ad.pci_id(), b.gpu.source.clone().unwrap_or_default())).small().weak());
                    }
                    // Hinweis, warum Hot-Spot/Speichertemperatur fehlen (nur NVIDIA)
                    if b.gpu.hotspot_c.is_none() && b.gpu.mem_temp_c.is_none() && b.gpu.source.as_deref().map(|s| s.contains("NVML")).unwrap_or(false) {
                        ui.label(
                            RichText::new(if cfg!(windows) {
                                "Hot-Spot und Speichertemperatur: RTX 30/40 meldet der Treiber selbst (Speicher nur bei GDDR6X), RTX 50 nur mit Adminrechten und PawnIO. Ältere Karten haben diese Sensoren nicht."
                            } else {
                                "Hot-Spot und Speichertemperatur meldet der NVIDIA-Treiber unter Linux nicht."
                            })
                            .small()
                            .weak(),
                        );
                    }
                    spec_view(ui, w, "gpu", &f.gpu_spec, C_GPU);
                }
            }
            ui.add_space(10.0);
            if ui.link("Ressourcenmonitor öffnen").clicked() {
                crate::winutil::shell_open("resmon.exe");
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_grid_is_wide() {
        assert_eq!(core_cols(32), 8); // 8 x 4
        assert_eq!(core_cols(16), 6); // 6 x 3
        assert_eq!(core_cols(8), 4); // 4 x 2
        assert_eq!(core_cols(12), 5); // 5 x 3
        assert_eq!(core_cols(1), 1);
        assert_eq!(group_cols(16), 8); // CCD mit 16 Threads: 2 Achterreihen
        assert_eq!(group_cols(8), 8);
        assert_eq!(group_cols(6), 6);
        assert_eq!(group_cols(12), 8);
        let g = |t: &str| crate::sensors::topology::CoreGroup { title: t.into(), detail: String::new(), lps: vec![0] };
        assert_eq!(label_for(&[g("CCD 1"), g("CCD 2")]).as_deref(), Some("CCD-Auslastung"));
        assert_eq!(label_for(&[g("P-Kerne"), g("E-Kerne")]).as_deref(), Some("P/E-Kern-Auslastung"));
        assert_eq!(label_for(&[g("P-Kerne"), g("E-Kerne"), g("LP-E-Kerne")]).as_deref(), Some("P/E/LP-E-Kern-Auslastung"));
        assert_eq!(label_for(&[g("CPU 1 · CCD 1"), g("CPU 2 · CCD 1")]).as_deref(), Some("CCD-Auslastung"));
        // Intel nur mit P-Kernen bzw. Ryzen mit einem CCD: kein dritter Reiter
        assert_eq!(label_for(&[g("Alle Kerne")]), None);
        // Gruppentemperatur: AMD je CCD, Zen 2 (CCX) teilt sich ein CCD, Intel heißester Kern
        assert_eq!(group_temp(1, 2, &[16], &[45.0, 52.0], &[]), Some(52.0));
        assert_eq!(group_temp(3, 4, &[12], &[45.0, 52.0], &[]), Some(52.0));
        let rd = |lps: Vec<usize>, t: f64| Reading { lps, temp_c: Some(t), ..Default::default() };
        assert_eq!(group_temp(0, 2, &[0, 1, 2, 3], &[], &[rd(vec![0, 1], 61.0), rd(vec![2, 3], 67.0), rd(vec![8], 90.0)]), Some(67.0));
        assert_eq!(group_temp(0, 2, &[0], &[], &[]), None);
        for n in 1..=128 {
            let c = core_cols(n);
            assert!(c >= n.div_ceil(c), "{n}: Spalten {c} < Zeilen");
        }
    }
}
