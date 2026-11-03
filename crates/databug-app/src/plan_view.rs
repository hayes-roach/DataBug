//! Query plan viewer: an indented tree with cost bars, plus the original
//! plan in the database's native format.

use crate::theme::{bold, Palette};
use eframe::egui::{self, CornerRadius, FontId, RichText, Sense};
use std::time::Duration;
use databug_core::{Plan, PlanNode};

pub struct PlanView {
    pub plan: Plan,
    pub analyze: bool,
    pub elapsed: Duration,
    show_raw: bool,
    selected: Option<usize>,
}

impl PlanView {
    pub fn new(plan: Plan, analyze: bool, elapsed: Duration) -> Self {
        let show_raw = plan.root.is_none();
        PlanView { plan, analyze, elapsed, show_raw, selected: None }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, p: &Palette) -> Option<String> {
        let mut toast = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(if self.analyze { "Explain analyze" } else { "Explain plan" }).font(bold(13.0)));
            ui.label(RichText::new(crate::grid::fmt_duration(self.elapsed)).color(p.muted));
            if self.plan.root.is_some() {
                ui.selectable_value(&mut self.show_raw, false, "Tree");
            }
            ui.selectable_value(&mut self.show_raw, true, format!("Raw {}", self.plan.format.to_uppercase()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Copy original plan").clicked() {
                    ui.ctx().copy_text(self.plan.raw.clone());
                    toast = Some(format!("Plan copied as {}", self.plan.format.to_uppercase()));
                }
            });
        });
        ui.add_space(4.0);
        if self.show_raw || self.plan.root.is_none() {
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                let mut raw = self.plan.raw.as_str();
                ui.add(egui::TextEdit::multiline(&mut raw).font(FontId::monospace(12.0)).desired_width(f32::INFINITY).code_editor());
            });
            return toast;
        }
        let root = self.plan.root.as_ref().unwrap();
        let use_time = root.actual_ms.is_some();
        let mut flat: Vec<(usize, &PlanNode)> = Vec::new();
        fn walk<'a>(n: &'a PlanNode, d: usize, out: &mut Vec<(usize, &'a PlanNode)>) {
            out.push((d, n));
            for c in &n.children {
                walk(c, d + 1, out);
            }
        }
        walk(root, 0, &mut flat);
        // bars are relative to the most expensive node, in whichever unit is shown
        let max = flat.iter().filter_map(|(_, n)| if use_time { n.actual_ms } else { n.total_cost }).fold(0.0_f64, f64::max).max(1e-9);

        let mut clicked = None;
        let detail_w = if self.selected.is_some() { 300.0 } else { 0.0 };
        ui.horizontal_top(|ui| {
            let tree_w = (ui.available_width() - detail_w).max(200.0);
            ui.allocate_ui_with_layout(egui::vec2(tree_w, ui.available_height()), egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::ScrollArea::vertical().id_salt("plan-tree").auto_shrink([false, false]).show(ui, |ui| {
                    // header
                    let (hr, _) = ui.allocate_exact_size(egui::vec2(tree_w - 12.0, 20.0), Sense::hover());
                    let col_x = hr.left() + tree_w - 280.0;
                    let hdr = |x: f32, align: egui::Align2, t: &str| {
                        ui.painter().text(egui::pos2(x, hr.center().y), align, t, FontId::proportional(11.5), p.muted);
                    };
                    hdr(hr.left() + 8.0, egui::Align2::LEFT_CENTER, "Operation");
                    hdr(col_x + 86.0, egui::Align2::RIGHT_CENTER, if use_time { "Time" } else { "Cost" });
                    hdr(col_x + 156.0, egui::Align2::RIGHT_CENTER, "Rows");
                    hdr(col_x + 166.0, egui::Align2::LEFT_CENTER, "Share");
                    for (i, (depth, n)) in flat.iter().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(tree_w - 12.0, 26.0), Sense::click());
                        if self.selected == Some(i) {
                            ui.painter().rect_filled(rect, CornerRadius::same(4), p.selection);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::same(4), p.hover);
                        }
                        let x = rect.left() + 8.0 + *depth as f32 * 18.0;
                        if *depth > 0 {
                            ui.painter().line_segment([egui::pos2(x - 10.0, rect.top()), egui::pos2(x - 10.0, rect.center().y)], egui::Stroke::new(1.0_f32, p.border));
                            ui.painter().line_segment([egui::pos2(x - 10.0, rect.center().y), egui::pos2(x - 3.0, rect.center().y)], egui::Stroke::new(1.0_f32, p.border));
                        }
                        let label_w = (tree_w - 280.0 - (x - rect.left())).max(40.0);
                        let label: String = {
                            let full = &n.label;
                            let max_chars = (label_w / 7.0) as usize;
                            if full.chars().count() > max_chars { full.chars().take(max_chars.saturating_sub(1)).collect::<String>() + "…" } else { full.clone() }
                        };
                        ui.painter().text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, label, FontId::proportional(13.0), p.text);
                        let metric = if use_time { n.actual_ms } else { n.total_cost };
                        let col_x = rect.left() + tree_w - 280.0;
                        if let Some(m) = metric {
                            let txt = if use_time { format!("{m:.2} ms") } else { format!("{m:.2}") };
                            ui.painter().text(egui::pos2(col_x + 86.0, rect.center().y), egui::Align2::RIGHT_CENTER, txt, FontId::monospace(12.0), p.muted);
                            let frac = (m / max).clamp(0.0, 1.0) as f32;
                            let bar = egui::Rect::from_min_size(egui::pos2(col_x + 166.0, rect.center().y - 4.0), egui::vec2(96.0, 8.0));
                            ui.painter().rect_filled(bar, CornerRadius::same(3), p.raised);
                            let color = if frac > 0.6 { p.danger } else if frac > 0.25 { p.warning } else { p.accent };
                            ui.painter().rect_filled(egui::Rect::from_min_size(bar.min, egui::vec2(96.0 * frac, 8.0)), CornerRadius::same(3), color);
                        }
                        if let Some(r) = n.rows {
                            ui.painter().text(egui::pos2(col_x + 156.0, rect.center().y), egui::Align2::RIGHT_CENTER, crate::grid::group(r as usize), FontId::monospace(12.0), p.muted);
                        }
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                    }
                });
            });
            if let Some(sel) = self.selected.and_then(|i| flat.get(i)) {
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(detail_w - 16.0);
                    let n = sel.1;
                    ui.label(RichText::new(&n.label).font(bold(14.0)));
                    ui.add_space(6.0);
                    let kv = |ui: &mut egui::Ui, k: &str, v: String| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(k).color(p.muted));
                            ui.label(RichText::new(v).font(FontId::monospace(12.0)));
                        });
                    };
                    if let Some(v) = n.startup_cost {
                        kv(ui, "Startup cost", format!("{v:.2}"));
                    }
                    if let Some(v) = n.total_cost {
                        kv(ui, "Total cost", format!("{v:.2}"));
                    }
                    if let Some(v) = n.rows {
                        kv(ui, "Rows", crate::grid::group(v as usize));
                    }
                    if let Some(v) = n.actual_ms {
                        kv(ui, "Actual time", format!("{v:.3} ms"));
                    }
                    if !n.detail.is_empty() {
                        ui.add_space(6.0);
                        for part in n.detail.split("  ·  ") {
                            ui.label(RichText::new(part).size(12.0).color(p.text));
                        }
                    }
                });
            }
        });
        if let Some(i) = clicked {
            self.selected = if self.selected == Some(i) { None } else { Some(i) };
        }
        toast
    }
}
