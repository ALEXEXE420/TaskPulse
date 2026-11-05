//! Seite "Leistung": Verlaufsgrafiken und Kennzahlen für CPU, Arbeitsspeicher, Datenträger,
//! Netzwerk und GPU; logische Prozessoren einzeln.

use super::{TaskPulseApp, HISTORY};
use crate::format;
use eframe::egui::{self, epaint, Color32, Pos2, RichText, Sense, Stroke, Vec2};
use std::collections::VecDeque;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Item {
    #[default]
    Cpu,
    Mem,
    Disk,
    Net,
    Gpu,
}

#[derive(Default)]
pub struct PerfState {
    pub item: Item,
    pub per_core: bool,
}

const C_CPU: Color32 = Color32::from_rgb(17, 125, 187);
const C_MEM: Color32 = Color32::from_rgb(139, 18, 174);
const C_DISK: Color32 = Color32::from_rgb(77, 166, 12);
const C_NET: Color32 = Color32::from_rgb(167, 79, 1);
const C_NET2: Color32 = Color32::from_rgb(220, 150, 60);
const C_GPU: Color32 = Color32::from_rgb(10, 140, 120);

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

fn stat(ui: &mut egui::Ui, label: &str, value: String) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).small().weak());
        ui.label(RichText::new(value).heading());
    });
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

pub fn show(app: &mut TaskPulseApp, ui: &mut egui::Ui) {
    let f = &app.frame;
    let h = &app.history;
    let b = &f.basic;
    let st = &mut app.perf;
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
            if side_entry(ui, st.item == Item::Disk, "Datenträger (alle)", &format::pct0(f.disk.active_pct), &h.disk, 100.0, C_DISK) {
                st.item = Item::Disk;
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
            let gpu_sub = format!("{}  {}", format::pct0(f.gpu.total_pct.or(b.gpu.load_pct)), format::temp(b.gpu.temp_c));
            if side_entry(ui, st.item == Item::Gpu, &format!("GPU 0 – {gpu_name}"), &gpu_sub, &h.gpu, 100.0, C_GPU) {
                st.item = Item::Gpu;
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
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut st.per_core, false, "Gesamtauslastung");
                        ui.radio_value(&mut st.per_core, true, "Logische Prozessoren");
                    });
                    if st.per_core && !h.cores.is_empty() {
                        let n = h.cores.len();
                        let cols = ((n as f32).sqrt().ceil() as usize).max(1);
                        let cw = (w - 6.0 * cols as f32) / cols as f32;
                        let ch = (cw * 0.55).clamp(40.0, 120.0);
                        egui::Grid::new("cores").spacing([6.0, 6.0]).show(ui, |ui| {
                            for (i, q) in h.cores.iter().enumerate() {
                                graph(ui, Vec2::new(cw, ch), &[(q, C_CPU)], 100.0).on_hover_text(format!(
                                    "Logischer Prozessor {i}: {}",
                                    format::pct0(q.back().copied())
                                ));
                                if (i + 1) % cols == 0 {
                                    ui.end_row();
                                }
                            }
                        });
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
                    ui.label(RichText::new(format!("Temperatur: {}", b.cpu_temp_status)).small().weak());
                }
                Item::Mem => {
                    ui.heading("Arbeitsspeicher");
                    graph(ui, Vec2::new(w, 300.0), &[(&h.mem, C_MEM)], 100.0);
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
                    });
                }
                Item::Disk => {
                    ui.heading("Datenträger (alle)");
                    graph(ui, Vec2::new(w, 300.0), &[(&h.disk, C_DISK)], 100.0);
                    ui.label(RichText::new("Aktive Zeit in %").small().weak());
                    ui.horizontal_wrapped(|ui| {
                        stat(ui, "Aktive Zeit", format::pct0(f.disk.active_pct));
                        stat(ui, "Lesen", f.disk.read_bps.map(|v| format::rate_de(v, 1)).unwrap_or("–".into()));
                        stat(ui, "Schreiben", f.disk.write_bps.map(|v| format::rate_de(v, 1)).unwrap_or("–".into()));
                    });
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
                }
            }
            ui.add_space(10.0);
            if ui.link("Ressourcenmonitor öffnen").clicked() {
                crate::winutil::shell_open("resmon.exe");
            }
        });
    });
}
