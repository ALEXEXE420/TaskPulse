//! Seite "Prozesse" mit GPU-, Engine-, VRAM- und VRAM-Anteil-Spalten.

use super::TaskPulseApp;
use crate::config::Threshold;
use crate::format;
use crate::sensors::gpu_proc::{group_gpu_pct, ProcGpu};
use crate::sensors::procs::{Category, Proc};
use crate::winutil;
use eframe::egui::{self, Align, Color32, Layout, RichText, Sense};
use egui_extras::{Column, TableBuilder};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortCol {
    Name,
    Pid,
    Cpu,
    Mem,
    Disk,
    Gpu,
    Engine,
    Vram,
    VramShare,
    Shared,
}

pub struct ProcState {
    pub search: String,
    pub sort: SortCol,
    pub desc: bool,
    pub frozen: bool,
    pub frozen_rank: HashMap<String, usize>,
    pub expanded: HashSet<String>,
    pub collapsed: HashSet<Category>,
    pub selected: Option<String>,
    pub confirm: Option<Confirm>,
}

impl Default for ProcState {
    fn default() -> Self {
        ProcState {
            search: String::new(),
            sort: SortCol::Cpu,
            desc: true,
            frozen: false,
            frozen_rank: HashMap::new(),
            expanded: HashSet::new(),
            collapsed: HashSet::new(),
            selected: None,
            confirm: None,
        }
    }
}

pub struct Confirm {
    pub title: String,
    pub pids: Vec<u32>,
}

/// Eine Zeile (Gruppe oder Einzelprozess) mit aggregierten Werten.
#[derive(Clone, Debug)]
pub struct Unit {
    pub key: String,
    pub name: String,
    pub image: String,
    pub pid: u32,
    pub pids: Vec<u32>,
    pub path: Option<String>,
    pub category: Category,
    pub cpu: f64,
    pub mem: f64,
    pub disk: f64,
    pub gpu: f64,
    pub engine: String,
    pub vram: f64,
    pub shared: f64,
    pub vram_bad: bool,
    pub members: Vec<Unit>,
}

fn proc_key(p: &Proc) -> String {
    format!("p{}:{}", p.pid, p.create_time)
}

fn single(p: &Proc, g: Option<&ProcGpu>) -> Unit {
    Unit {
        key: proc_key(p),
        name: p.display.clone(),
        image: p.image.clone(),
        pid: p.pid,
        pids: vec![p.pid],
        path: p.path.clone(),
        category: p.category,
        cpu: p.cpu_pct,
        mem: p.mem_private,
        disk: p.io_bps,
        gpu: g.map(|g| g.max_pct).unwrap_or(0.0),
        engine: g.map(|g| g.engine_label.clone()).unwrap_or_default(),
        vram: g.map(|g| g.dedicated).unwrap_or(0.0),
        shared: g.map(|g| g.shared).unwrap_or(0.0),
        vram_bad: g.map(|g| g.vram_implausible).unwrap_or(false),
        members: vec![],
    }
}

/// Gruppiert Prozesse: ein Prozess gehört zur Gruppe seines obersten Vorfahren mit gleichem
/// Programmnamen (z. B. alle Chrome-Prozesse unter dem Browserprozess).
pub fn build_units(procs: &[Proc], gpu: &HashMap<u32, ProcGpu>, group: bool) -> Vec<Unit> {
    if !group {
        return procs.iter().map(|p| single(p, gpu.get(&p.pid))).collect();
    }
    let by_pid: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let root_of = |p: &Proc| -> u32 {
        let mut cur = p;
        for _ in 0..64 {
            match by_pid.get(&cur.ppid) {
                Some(par)
                    if par.pid != cur.pid
                        && par.create_time <= cur.create_time
                        && par.image.eq_ignore_ascii_case(&cur.image) =>
                {
                    cur = par
                }
                _ => break,
            }
        }
        cur.pid
    };
    let mut groups: HashMap<u32, Vec<&Proc>> = HashMap::new();
    let mut order: Vec<u32> = Vec::new();
    for p in procs {
        let r = root_of(p);
        let e = groups.entry(r).or_insert_with(|| {
            order.push(r);
            Vec::new()
        });
        e.push(p);
    }
    let mut out = Vec::new();
    for r in order {
        let members = &groups[&r];
        let root = by_pid.get(&r).copied().unwrap_or(members[0]);
        if members.len() == 1 {
            out.push(single(root, gpu.get(&root.pid)));
            continue;
        }
        let category = if members.iter().any(|m| m.has_window) { Category::App } else { root.category };
        let gs: Vec<&ProcGpu> = members.iter().filter_map(|m| gpu.get(&m.pid)).collect();
        let engine = gs.iter().filter(|g| g.max_pct > 0.0).max_by(|a, b| a.max_pct.total_cmp(&b.max_pct)).map(|g| g.engine_label.clone());
        let mut subs: Vec<Unit> = members.iter().map(|m| single(m, gpu.get(&m.pid))).collect();
        subs.iter_mut().for_each(|u| u.category = category);
        out.push(Unit {
            key: format!("g{}:{}", root.pid, root.create_time),
            name: format!("{} ({})", root.display, members.len()),
            image: root.image.clone(),
            pid: root.pid,
            pids: members.iter().map(|m| m.pid).collect(),
            path: root.path.clone(),
            category,
            cpu: members.iter().map(|m| m.cpu_pct).sum::<f64>().min(100.0),
            mem: members.iter().map(|m| m.mem_private).sum(),
            disk: members.iter().map(|m| m.io_bps).sum(),
            gpu: group_gpu_pct(gs.iter().copied()),
            engine: engine.unwrap_or_default(),
            vram: gs.iter().map(|g| g.dedicated).sum(),
            shared: gs.iter().map(|g| g.shared).sum(),
            vram_bad: gs.iter().any(|g| g.vram_implausible),
            members: subs,
        });
    }
    out
}

fn cmp_units(a: &Unit, b: &Unit, col: SortCol, vram_total: f64) -> std::cmp::Ordering {
    let _ = vram_total;
    match col {
        SortCol::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        SortCol::Pid => a.pid.cmp(&b.pid),
        SortCol::Cpu => a.cpu.total_cmp(&b.cpu),
        SortCol::Mem => a.mem.total_cmp(&b.mem),
        SortCol::Disk => a.disk.total_cmp(&b.disk),
        SortCol::Gpu => a.gpu.total_cmp(&b.gpu),
        SortCol::Engine => a.engine.cmp(&b.engine),
        SortCol::Vram | SortCol::VramShare => a.vram.total_cmp(&b.vram),
        SortCol::Shared => a.shared.total_cmp(&b.shared),
    }
}

pub fn sort_units(v: &mut [Unit], col: SortCol, desc: bool, frozen: Option<&HashMap<String, usize>>) {
    if let Some(rank) = frozen {
        // Eingefrorene Reihenfolge: bekannte Zeilen bleiben, neue kommen ans Ende
        v.sort_by_key(|u| rank.get(&u.key).copied().unwrap_or(usize::MAX));
        for u in v.iter_mut() {
            sort_units(&mut u.members, col, desc, frozen);
        }
        return;
    }
    v.sort_by(|a, b| {
        let o = cmp_units(a, b, col, 0.0);
        let o = if desc { o.reverse() } else { o };
        o.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then(a.pid.cmp(&b.pid))
    });
    for u in v.iter_mut() {
        sort_units(&mut u.members, col, desc, None);
    }
}

fn matches(u: &Unit, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let q = q.to_lowercase();
    u.name.to_lowercase().contains(&q)
        || u.image.to_lowercase().contains(&q)
        || u.pid.to_string() == q
        || u.members.iter().any(|m| matches(m, &q))
}

enum RowKind {
    Header(Category, usize),
    Unit { depth: u8, expandable: bool, expanded: bool },
}

struct Row<'a> {
    kind: RowKind,
    unit: Option<&'a Unit>,
}

fn heat(ui: &egui::Ui, t: &Threshold, v: f64, on: bool) -> Option<Color32> {
    if !on {
        return None;
    }
    let i = t.intensity(v);
    if i <= 0.0 {
        return None;
    }
    let a = (40.0 + 150.0 * i) as u8;
    Some(if ui.visuals().dark_mode {
        Color32::from_rgba_unmultiplied(200, 120, 0, (a as f32 * 0.6) as u8)
    } else {
        Color32::from_rgba_unmultiplied(255, 190, 40, a)
    })
}

/// Skalierungsfaktor gegenüber der ursprünglichen Schriftgröße 13,5.
fn scale(ui: &egui::Ui) -> f32 {
    ui.style().text_styles.get(&egui::TextStyle::Body).map(|f| f.size / 13.5).unwrap_or(1.0)
}

fn num_cell(ui: &mut egui::Ui, text: String, bg: Option<Color32>) {
    if let Some(c) = bg {
        ui.painter().rect_filled(ui.max_rect().expand2(egui::vec2(4.0, 2.0)), 0.0, c);
    }
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(text);
    });
}

fn header_cell(ui: &mut egui::Ui, st: &mut ProcState, col: SortCol, total: &str, label: &str, right: bool, tip: Option<&str>) {
    let body = ui.style().text_styles.get(&egui::TextStyle::Body).map(|f| f.size).unwrap_or(16.0);
    let sorted = st.sort == col && !st.frozen;
    let accent = ui.visuals().hyperlink_color;
    let layout = if right { Layout::top_down(Align::Max) } else { Layout::top_down(Align::Min) };
    let r = ui
        .with_layout(layout, |ui| {
            ui.label(RichText::new(if total.is_empty() { " " } else { total }).size(body * 1.2).strong());
            // Sortierte Spalte: großer, farbiger Pfeil und hervorgehobene Beschriftung
            let dir = if right { Layout::right_to_left(Align::Center) } else { Layout::left_to_right(Align::Center) };
            ui.with_layout(dir, |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let text = RichText::new(label).size(body * 0.9);
                let text = if sorted { text.strong().color(accent) } else { text };
                let arrow = sorted.then(|| RichText::new(if st.desc { "▼" } else { "▲" }).size(body * 1.05).strong().color(accent));
                // right_to_left: zuerst hinzugefügtes Element steht rechts
                if let Some(a) = arrow.clone().filter(|_| right) {
                    ui.label(a);
                }
                ui.label(text);
                if let Some(a) = arrow.filter(|_| !right) {
                    ui.label(a);
                }
            });
        })
        .response;
    // Ganze Zelle anklickbar (nicht nur die Schrift)
    let resp = ui.interact(ui.max_rect().union(r.rect), ui.id().with(label), Sense::click());
    let resp = if let Some(t) = tip { resp.on_hover_text(t) } else { resp };
    if resp.clicked() {
        if st.sort == col {
            st.desc = !st.desc;
        } else {
            st.sort = col;
            st.desc = !matches!(col, SortCol::Name | SortCol::Engine | SortCol::Pid);
        }
        st.frozen = false;
    }
}

const VRAM_TIP: &str = "Dedizierter Grafikspeicher je Prozess (Leistungsindikator „GPU Process Memory“).\n\
Prozesswerte können geteilten Speicher enthalten – ihre Summe kann über dem echten Gesamtverbrauch liegen. \
Die Kopfzeile stammt deshalb separat vom Adapter.\n\
Microsoft dokumentiert, dass der prozessbezogene VRAM-Wert unter Windows 10 ab Version 1709 zu hoch ausfallen kann.";

const DISK_TIP: &str = "Lese- und Schreibrate aller E/A-Vorgänge des Prozesses (Näherung). \
Die Kopfzeile zeigt die aktive Zeit aller Datenträger.";

pub fn show(app: &mut TaskPulseApp, ui: &mut egui::Ui) {
    let cfg = app.cfg.clone();
    let f = &app.frame;
    let vram_total = f.vram_total.unwrap_or(0.0);
    let mut units = build_units(&f.procs, &f.gpu.per_pid, cfg.group_processes);
    let sel = selected_unit(app);
    let mut end_now: Option<Unit> = None;
    let st = &mut app.procs;

    // Werkzeugleiste
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut st.search).hint_text("🔍 Name, Programm oder PID suchen").desired_width(260.0));
        if !st.search.is_empty() && ui.small_button("✕").clicked() {
            st.search.clear();
        }
        ui.separator();
        let was = st.frozen;
        ui.toggle_value(&mut st.frozen, "❄ Sortierung einfrieren")
            .on_hover_text("Hält die Reihenfolge an, damit beim Anklicken kein anderer Prozess unter den Mauszeiger rutscht.");
        if st.frozen && !was {
            st.frozen_rank.clear();
            let mut tmp = units.clone();
            sort_units(&mut tmp, st.sort, st.desc, None);
            let mut i = 0;
            let mut add = |k: &str, m: &mut HashMap<String, usize>| {
                m.insert(k.to_string(), i);
                i += 1;
            };
            for u in &tmp {
                add(&u.key, &mut st.frozen_rank);
                for m in &u.members {
                    add(&m.key, &mut st.frozen_rank);
                }
            }
        }
        ui.toggle_value(&mut app.cfg.heatmap.enabled, "Heatmap");
        ui.toggle_value(&mut app.cfg.group_processes, "Gruppieren");
        ui.menu_button("Spalten ▾", |ui| {
            let c = &mut app.cfg.columns;
            ui.checkbox(&mut c.pid, "PID");
            ui.checkbox(&mut c.cpu, "CPU");
            ui.checkbox(&mut c.mem, "Arbeitsspeicher");
            ui.checkbox(&mut c.disk, "Datenträger");
            ui.checkbox(&mut c.gpu, "GPU");
            ui.checkbox(&mut c.gpu_engine, "GPU-Engine");
            ui.checkbox(&mut c.vram, "VRAM");
            ui.checkbox(&mut c.vram_share, "VRAM-Anteil");
            ui.checkbox(&mut c.shared_gpu_mem, "Gemeinsamer GPU-Speicher");
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label = RichText::new("✖ Task beenden").strong();
            let btn = egui::Button::new(if sel.is_some() { label.color(Color32::WHITE) } else { label });
            let btn = if sel.is_some() { btn.fill(Color32::from_rgb(196, 43, 28)) } else { btn };
            let tip = match &sel {
                Some(u) if u.pids.len() > 1 => format!("„{}“ beenden ({} Prozesse) – Taste Entf", u.name, u.pids.len()),
                Some(u) => format!("„{}“ beenden – Taste Entf", u.name),
                None => "Erst einen Prozess in der Liste anklicken".to_string(),
            };
            if ui.add_enabled(sel.is_some(), btn).on_hover_text(tip).on_disabled_hover_text("Erst einen Prozess in der Liste anklicken").clicked() {
                end_now = sel.clone();
            }
            if ui.button("Neue Aufgabe ausführen …").clicked() {
                st.confirm = None;
                app.settings.run_dialog = Some(String::new());
            }
        });
    });
    ui.add_space(4.0);
    let typing = ui.ctx().memory(|m| m.focused().is_some());
    if !typing && sel.is_some() && ui.input(|i| i.key_pressed(egui::Key::Delete)) {
        end_now = sel.clone();
    }

    let rank = st.frozen.then_some(&st.frozen_rank);
    sort_units(&mut units, st.sort, st.desc, rank);
    let q = st.search.trim().to_string();
    units.retain(|u| matches(u, &q));

    // Zeilen aufbauen
    let mut rows: Vec<Row> = Vec::new();
    let cats: &[Category] = if cfg.group_processes { &[Category::App, Category::Background, Category::Windows] } else { &[Category::App] };
    for cat in cats {
        let list: Vec<&Unit> = if cfg.group_processes { units.iter().filter(|u| u.category == *cat).collect() } else { units.iter().collect() };
        if list.is_empty() {
            continue;
        }
        if cfg.group_processes {
            rows.push(Row { kind: RowKind::Header(*cat, list.len()), unit: None });
            if st.collapsed.contains(cat) {
                continue;
            }
        }
        for u in list {
            let expandable = !u.members.is_empty();
            let expanded = expandable && (st.expanded.contains(&u.key) || !q.is_empty());
            rows.push(Row { kind: RowKind::Unit { depth: 0, expandable, expanded }, unit: Some(u) });
            if expanded {
                for m in &u.members {
                    if q.is_empty() || matches(m, &q) || matches(u, &q) {
                        rows.push(Row { kind: RowKind::Unit { depth: 1, expandable: false, expanded: false }, unit: Some(m) });
                    }
                }
            }
        }
    }

    // Kopfzeilen-Gesamtwerte (vom System bzw. Adapter, nicht aus den Zeilen addiert)
    let b = &f.basic;
    let t_cpu = b.cpu.load_pct.map(|v| format!("{v:.0} %")).unwrap_or_default();
    let t_mem = b.mem.used_pct.map(|v| format!("{v:.0} %")).unwrap_or_default();
    let t_disk = f.disk.active_pct.map(|v| format!("{v:.0} %")).unwrap_or_default();
    let t_gpu = f.gpu.total_pct.or(b.gpu.load_pct).map(|v| format!("{v:.0} %")).unwrap_or_default();
    let t_vram = f.vram_used.map(|v| format!("{} GB", format::de(v / format::GIB, 1))).unwrap_or_default();
    let t_vshare = match (f.vram_used, f.vram_total) {
        (Some(u), Some(t)) if t > 0.0 => format!("{:.0} %", u / t * 100.0),
        _ => String::new(),
    };
    let t_shared = f.gpu.adapter_shared_used.map(|v| format!("{} GB", format::de(v / format::GIB, 1))).unwrap_or_default();

    let c = cfg.columns.clone();
    let hm = &cfg.heatmap;
    let dec = cfg.decimals;
    let mut actions: Vec<Action> = Vec::new();
    let selected = st.selected.clone();

    let k = scale(ui);
    // Schrift nicht markierbar – sonst fängt der Text den Klick ab und die Zeile wird nicht ausgewählt
    ui.style_mut().interaction.selectable_labels = false;
    let mut tb = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::initial(300.0 * k).at_least(160.0).clip(true));
    let mut ncols = 1;
    for (on, w) in [
        (c.pid, 60.0),
        (c.cpu, 70.0),
        (c.mem, 85.0),
        (c.disk, 85.0),
        (c.gpu, 62.0),
        (c.gpu_engine, 150.0),
        (c.vram, 80.0),
        (c.vram_share, 85.0),
        (c.shared_gpu_mem, 95.0),
    ] {
        if on {
            tb = tb.column(Column::initial(w * k).at_least(40.0).clip(true));
            ncols += 1;
        }
    }
    let _ = ncols;

    tb.header(50.0 * k, |mut h| {
        h.col(|ui| header_cell(ui, st, SortCol::Name, "", "Name", false, None));
        if c.pid {
            h.col(|ui| header_cell(ui, st, SortCol::Pid, "", "PID", true, None));
        }
        if c.cpu {
            h.col(|ui| header_cell(ui, st, SortCol::Cpu, &t_cpu, "CPU", true, None));
        }
        if c.mem {
            h.col(|ui| header_cell(ui, st, SortCol::Mem, &t_mem, "Arbeitsspeicher", true, Some("Privater Arbeitssatz")));
        }
        if c.disk {
            h.col(|ui| header_cell(ui, st, SortCol::Disk, &t_disk, "Datenträger", true, Some(DISK_TIP)));
        }
        if c.gpu {
            h.col(|ui| {
                header_cell(ui, st, SortCol::Gpu, &t_gpu, "GPU", true, Some("Auslastung der am stärksten genutzten Engine (Engines werden nicht addiert)."))
            });
        }
        if c.gpu_engine {
            h.col(|ui| header_cell(ui, st, SortCol::Engine, "", "GPU-Engine", false, None));
        }
        if c.vram {
            h.col(|ui| header_cell(ui, st, SortCol::Vram, &t_vram, "VRAM", true, Some(VRAM_TIP)));
        }
        if c.vram_share {
            h.col(|ui| {
                header_cell(ui, st, SortCol::VramShare, &t_vshare, "VRAM-Anteil", true, Some("VRAM des Prozesses geteilt durch den gesamten Grafikspeicher der Karte."))
            });
        }
        if c.shared_gpu_mem {
            h.col(|ui| header_cell(ui, st, SortCol::Shared, &t_shared, "Gem. GPU-Speicher", true, Some("Vom Grafiksystem genutzter Arbeitsspeicher.")));
        }
    })
    .body(|body| {
        body.rows(24.0 * k, rows.len(), |mut row| {
            let r = &rows[row.index()];
            match (&r.kind, r.unit) {
                (RowKind::Header(cat, n), _) => {
                    let collapsed = st.collapsed.contains(cat);
                    row.col(|ui| {
                        let arrow = if collapsed { "▶" } else { "▼" };
                        ui.label(RichText::new(format!("{arrow} {} ({n})", cat.title())).strong().color(ui.visuals().hyperlink_color));
                    });
                    if row.response().clicked() {
                        actions.push(Action::ToggleCat(*cat));
                    }
                }
                (RowKind::Unit { depth, expandable, expanded }, Some(u)) => {
                    row.set_selected(selected.as_deref() == Some(u.key.as_str()));
                    let mut toggle = false;
                    row.col(|ui| {
                        ui.add_space(if *depth > 0 { 26.0 } else { 4.0 });
                        if *expandable {
                            let a = if *expanded { "▼" } else { "▶" };
                            if ui.add(egui::Label::new(a).sense(Sense::click())).clicked() {
                                toggle = true;
                            }
                        } else if *depth == 0 {
                            ui.add_space(14.0);
                        }
                        ui.label(&u.name).on_hover_text(u.path.clone().unwrap_or_else(|| u.image.clone()));
                    });
                    if c.pid {
                        row.col(|ui| num_cell(ui, if u.members.is_empty() { u.pid.to_string() } else { String::new() }, None));
                    }
                    if c.cpu {
                        row.col(|ui| {
                            let bg = heat(ui, &hm.cpu, u.cpu, hm.enabled);
                            num_cell(ui, format::pct_de(u.cpu, dec), bg)
                        });
                    }
                    if c.mem {
                        row.col(|ui| {
                            let bg = heat(ui, &hm.mem_mb, u.mem / format::MIB, hm.enabled);
                            num_cell(ui, format::mem(u.mem, cfg.mem_unit, dec), bg)
                        });
                    }
                    if c.disk {
                        row.col(|ui| {
                            let bg = heat(ui, &hm.disk_mbps, u.disk / 1e6, hm.enabled);
                            num_cell(ui, format::rate_de(u.disk, dec), bg)
                        });
                    }
                    if c.gpu {
                        row.col(|ui| {
                            let bg = heat(ui, &hm.gpu, u.gpu, hm.enabled);
                            num_cell(ui, format::pct_de(u.gpu, 0), bg)
                        });
                    }
                    if c.gpu_engine {
                        row.col(|ui| {
                            ui.label(if u.engine.is_empty() { format::DASH.to_string() } else { u.engine.clone() });
                        });
                    }
                    let share = if vram_total > 0.0 { u.vram / vram_total * 100.0 } else { f64::NAN };
                    if c.vram {
                        row.col(|ui| {
                            let bg = heat(ui, &hm.vram_pct, share, hm.enabled);
                            let t = format::mem(u.vram, crate::config::MemUnit::GB, 2);
                            if u.vram_bad {
                                num_cell(ui, format!("⚠ {t}"), bg);
                                ui.interact(ui.max_rect(), ui.id().with(("vb", &u.key)), Sense::hover()).on_hover_text(
                                    "Windows meldet für diesen Prozess einen unmöglichen VRAM-Wert (größer als der Grafikspeicher der Karte). Der Wert wurde verworfen.",
                                );
                            } else {
                                num_cell(ui, t, bg);
                            }
                        });
                    }
                    if c.vram_share {
                        row.col(|ui| {
                            let bg = heat(ui, &hm.vram_pct, share, hm.enabled);
                            num_cell(ui, if share.is_finite() { format::pct_de(share, 0) } else { format::DASH.into() }, bg)
                        });
                    }
                    if c.shared_gpu_mem {
                        row.col(|ui| num_cell(ui, format::mem(u.shared, cfg.mem_unit, dec), None));
                    }
                    let resp = row.response();
                    if resp.clicked() {
                        actions.push(Action::Select(u.key.clone()));
                    }
                    if resp.double_clicked() && *expandable {
                        toggle = true;
                    }
                    if toggle {
                        actions.push(Action::ToggleExpand(u.key.clone()));
                    }
                    resp.context_menu(|ui| {
                        context_menu(ui, u, &mut actions);
                    });
                }
                _ => {}
            }
        });
    });

    if let Some(u) = end_now {
        actions.push(Action::End(u));
    }
    for a in actions {
        apply(app, a);
    }
}

enum Action {
    Select(String),
    ToggleExpand(String),
    ToggleCat(Category),
    End(Unit),
    EndTree(Unit),
    Efficiency(Vec<u32>, bool),
    OpenPath(String),
    Search(String),
    RestartExplorer,
}

fn context_menu(ui: &mut egui::Ui, u: &Unit, actions: &mut Vec<Action>) {
    ui.label(RichText::new(&u.name).strong());
    ui.separator();
    if ui.button("Task beenden").clicked() {
        actions.push(Action::End(u.clone()));
        ui.close_menu();
    }
    if ui.button("Prozessstruktur beenden").clicked() {
        actions.push(Action::EndTree(u.clone()));
        ui.close_menu();
    }
    ui.separator();
    if ui.button("Effizienzmodus ein").clicked() {
        actions.push(Action::Efficiency(u.pids.clone(), true));
        ui.close_menu();
    }
    if ui.button("Effizienzmodus aus").clicked() {
        actions.push(Action::Efficiency(u.pids.clone(), false));
        ui.close_menu();
    }
    ui.separator();
    if let Some(p) = &u.path {
        if ui.button("Dateipfad öffnen").clicked() {
            actions.push(Action::OpenPath(p.clone()));
            ui.close_menu();
        }
    }
    if ui.button("Online suchen").clicked() {
        actions.push(Action::Search(u.image.clone()));
        ui.close_menu();
    }
    if u.image.eq_ignore_ascii_case("explorer.exe") {
        ui.separator();
        if ui.button("Explorer neu starten").clicked() {
            actions.push(Action::RestartExplorer);
            ui.close_menu();
        }
    }
}

fn descendants(procs: &[Proc], roots: &[u32]) -> Vec<u32> {
    let mut out: Vec<u32> = roots.to_vec();
    let mut i = 0;
    while i < out.len() {
        let cur = out[i];
        let ct = procs.iter().find(|p| p.pid == cur).map(|p| p.create_time).unwrap_or(i64::MIN);
        for p in procs {
            if p.ppid == cur && p.pid != cur && p.create_time >= ct && !out.contains(&p.pid) {
                out.push(p.pid);
            }
        }
        i += 1;
    }
    out
}

fn kill_all(app: &mut TaskPulseApp, pids: &[u32]) {
    let mut ok = 0;
    let mut errs = Vec::new();
    for pid in pids {
        match winutil::terminate(*pid) {
            Ok(()) => ok += 1,
            Err(e) => errs.push(format!("PID {pid}: {}", e.trim())),
        }
    }
    if errs.is_empty() {
        app.set_status(format!("{ok} Prozess(e) beendet"));
    } else {
        let hint = if app.admin { " (geschützter Prozess)" } else { " – evtl. Adminrechte nötig" };
        app.set_status(format!("{ok} beendet, {} fehlgeschlagen{hint}: {}", errs.len(), errs.join("; ")));
    }
}

fn request_end(app: &mut TaskPulseApp, title: String, category: Category, pids: Vec<u32>) {
    if category == Category::Windows {
        app.procs.confirm = Some(Confirm { title, pids });
    } else {
        kill_all(app, &pids);
    }
}

fn apply(app: &mut TaskPulseApp, a: Action) {
    match a {
        Action::Select(k) => app.procs.selected = Some(k),
        Action::ToggleExpand(k) => {
            if !app.procs.expanded.remove(&k) {
                app.procs.expanded.insert(k);
            }
        }
        Action::ToggleCat(c) => {
            if !app.procs.collapsed.remove(&c) {
                app.procs.collapsed.insert(c);
            }
        }
        Action::End(u) => {
            request_end(app, u.name.clone(), u.category, u.pids.clone());
            if app.procs.confirm.is_none() {
                app.procs.selected = None;
            }
        }
        Action::EndTree(u) => {
            let pids = descendants(&app.frame.procs, &u.pids);
            request_end(app, format!("{} (mit {} Unterprozessen)", u.name, pids.len().saturating_sub(u.pids.len())), u.category, pids);
        }
        Action::Efficiency(pids, on) => {
            let errs: Vec<String> = pids.iter().filter_map(|p| winutil::set_efficiency_mode(*p, on).err()).collect();
            if errs.is_empty() {
                app.set_status(if on { "Effizienzmodus eingeschaltet" } else { "Effizienzmodus ausgeschaltet" });
            } else {
                app.set_status(format!("Effizienzmodus: {}", errs[0].trim()));
            }
        }
        Action::OpenPath(p) => winutil::open_in_explorer(&p),
        Action::Search(t) => winutil::search_online(&t),
        Action::RestartExplorer => {
            std::thread::spawn(winutil::restart_explorer);
            app.set_status("Explorer wird neu gestartet …");
        }
    }
}

fn selected_unit(app: &TaskPulseApp) -> Option<Unit> {
    let key = app.procs.selected.as_ref()?;
    let units = build_units(&app.frame.procs, &app.frame.gpu.per_pid, app.cfg.group_processes);
    for u in units {
        if &u.key == key {
            return Some(u);
        }
        if let Some(m) = u.members.iter().find(|m| &m.key == key) {
            return Some(m.clone());
        }
    }
    None
}

pub fn end_task_button(app: &mut TaskPulseApp, ui: &mut egui::Ui) {
    let sel = selected_unit(app);
    if ui.add_enabled(sel.is_some(), egui::Button::new("Task beenden")).clicked() {
        if let Some(u) = sel {
            request_end(app, u.name.clone(), u.category, u.pids.clone());
        }
    }
}

/// Bestätigungs- und "Neue Aufgabe"-Dialoge.
pub fn dialogs(app: &mut TaskPulseApp, ctx: &egui::Context) {
    if let Some(c) = &app.procs.confirm {
        let mut close = false;
        let mut kill = false;
        egui::Window::new("Windows-Prozess beenden?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(
            ctx,
            |ui| {
                ui.label(format!("„{}“ ist ein Windows-Prozess.", c.title));
                ui.label("Das Beenden kann Windows instabil machen oder einen Neustart auslösen.");
                ui.horizontal(|ui| {
                    if ui.button("Trotzdem beenden").clicked() {
                        kill = true;
                    }
                    if ui.button("Abbrechen").clicked() {
                        close = true;
                    }
                });
            },
        );
        if kill {
            let pids = c.pids.clone();
            app.procs.confirm = None;
            kill_all(app, &pids);
        } else if close {
            app.procs.confirm = None;
        }
    }
    super::settings::run_dialog(app, ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, image: &str, ct: i64, win: bool, cpu: f64) -> Proc {
        Proc {
            pid,
            ppid,
            image: image.into(),
            display: image.trim_end_matches(".exe").into(),
            path: None,
            session: 1,
            threads: 1,
            handles: 1,
            base_priority: 8,
            cpu_pct: cpu,
            mem_private: 100.0,
            working_set: 100.0,
            io_bps: 0.0,
            cpu_time_100ns: 0,
            create_time: ct,
            has_window: win,
            category: if win { Category::App } else { Category::Background },
        }
    }

    #[test]
    fn chrome_children_grouped_under_browser() {
        let procs = vec![
            p(10, 1, "chrome.exe", 100, true, 1.0),
            p(11, 10, "chrome.exe", 101, false, 2.0),
            p(12, 10, "chrome.exe", 102, false, 3.0),
            p(13, 10, "crashpad.exe", 103, false, 0.5),
            p(20, 1, "obs64.exe", 100, true, 1.0),
        ];
        let units = build_units(&procs, &HashMap::new(), true);
        let chrome = units.iter().find(|u| u.image == "chrome.exe").unwrap();
        assert_eq!(chrome.members.len(), 3);
        assert_eq!(chrome.name, "chrome (3)");
        assert!((chrome.cpu - 6.0).abs() < 1e-9);
        assert_eq!(chrome.mem, 300.0);
        assert_eq!(chrome.category, Category::App);
        assert!(units.iter().any(|u| u.image == "crashpad.exe" && u.members.is_empty()));
        assert_eq!(units.len(), 3);
    }

    #[test]
    fn pid_reuse_not_grouped() {
        // Elternprozess jünger als Kind -> PID wurde wiederverwendet
        let procs = vec![p(10, 1, "a.exe", 500, false, 0.0), p(11, 10, "a.exe", 100, false, 0.0)];
        let units = build_units(&procs, &HashMap::new(), true);
        assert_eq!(units.len(), 2);
    }

    #[test]
    fn frozen_sort_keeps_order() {
        let procs = vec![p(1, 0, "a.exe", 1, false, 1.0), p(2, 0, "b.exe", 1, false, 5.0)];
        let mut units = build_units(&procs, &HashMap::new(), false);
        sort_units(&mut units, SortCol::Cpu, true, None);
        assert_eq!(units[0].pid, 2);
        let rank: HashMap<String, usize> = units.iter().enumerate().map(|(i, u)| (u.key.clone(), i)).collect();
        // Werte ändern sich, Reihenfolge bleibt
        let procs = vec![p(1, 0, "a.exe", 1, false, 50.0), p(2, 0, "b.exe", 1, false, 0.0), p(3, 0, "c.exe", 1, false, 99.0)];
        let mut units = build_units(&procs, &HashMap::new(), false);
        sort_units(&mut units, SortCol::Cpu, true, Some(&rank));
        assert_eq!(units.iter().map(|u| u.pid).collect::<Vec<_>>(), vec![2, 1, 3]);
    }

    #[test]
    fn tree_descendants() {
        let procs = vec![p(1, 0, "a", 1, false, 0.0), p(2, 1, "b", 2, false, 0.0), p(3, 2, "c", 3, false, 0.0), p(4, 9, "d", 1, false, 0.0)];
        let mut d = descendants(&procs, &[1]);
        d.sort();
        assert_eq!(d, vec![1, 2, 3]);
    }
}
