//! Schema compare: pick a source and a target (connection + schema), see
//! what differs, and get a migration script for the target.

use crate::theme::{bold, Palette};
use databug_core::{Catalog, ConnectionConfig, Dialect, ObjectKind};
use databug_sql::diff::{self, DiffItem, DiffKind, Sources};
use eframe::egui::{self, Color32, CornerRadius, FontId, RichText, Sense};
use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Idle,
    Catalogs,
    Definitions,
    Done,
}

pub struct CompareView {
    pub left: (String, String),
    pub right: (String, String),
    pub with_definitions: bool,
    pub phase: Phase,
    pub items: Vec<DiffItem>,
    checked: Vec<bool>,
    selected: Option<usize>,
    pub src_sources: Sources,
    pub dst_sources: Sources,
    pub wait_gens: (u64, u64),
    pub pending: HashSet<u64>,
    pub status: String,
    pub target_dialect: Dialect,
}

pub enum CompareAction {
    None,
    Start,
    OpenScript(String),
    RunScript(String),
}

impl CompareView {
    pub fn new(left: (String, String), right: (String, String)) -> Self {
        CompareView {
            left,
            right,
            with_definitions: true,
            phase: Phase::Idle,
            items: Vec::new(),
            checked: Vec::new(),
            selected: None,
            src_sources: Sources::new(),
            dst_sources: Sources::new(),
            wait_gens: (0, 0),
            pending: HashSet::new(),
            status: String::new(),
            target_dialect: Dialect::Postgres,
        }
    }

    pub fn finish(&mut self, src: &Catalog, dst: &Catalog, dialect: Dialect) {
        self.target_dialect = dialect;
        self.items = diff::diff(src, &self.left.1, dst, &self.right.1, dialect, &self.src_sources, &self.dst_sources);
        // destructive changes start unchecked so nothing is dropped by accident
        self.checked = self.items.iter().map(|i| !i.destructive).collect();
        self.selected = (!self.items.is_empty()).then_some(0);
        self.phase = Phase::Done;
        self.status = if self.items.is_empty() { "The schemas match".into() } else { format!("{} differences", self.items.len()) };
    }

    fn script(&self) -> String {
        let chosen: Vec<&DiffItem> = self.items.iter().zip(&self.checked).filter(|(_, c)| **c).map(|(i, _)| i).collect();
        diff::migration_script(self.target_dialect, &chosen)
    }

    pub fn show(&mut self, ui: &mut egui::Ui, p: &Palette, connections: &[ConnectionConfig], catalog: &dyn Fn(&str) -> Option<Catalog>) -> CompareAction {
        let mut action = CompareAction::None;
        let busy = matches!(self.phase, Phase::Catalogs | Phase::Definitions);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Schema compare").font(bold(14.0)));
            ui.add_space(10.0);
            for (label, side) in [("Source", 0), ("Target", 1)] {
                ui.label(RichText::new(label).color(p.muted));
                let (conn, schema) = if side == 0 { &mut self.left } else { &mut self.right };
                let name = connections.iter().find(|c| &c.id == conn).map(|c| c.name.clone()).unwrap_or_else(|| "choose…".into());
                egui::ComboBox::from_id_salt(("cmp-conn", side)).selected_text(name).width(170.0).show_ui(ui, |ui| {
                    for c in connections {
                        if ui.selectable_label(&c.id == conn, &c.name).clicked() {
                            *conn = c.id.clone();
                            if let Some(cat) = catalog(&c.id) {
                                *schema = cat.default_schema.clone();
                            }
                        }
                    }
                });
                let schemas: Vec<String> = catalog(conn).map(|c| c.schemas.iter().map(|s| s.name.clone()).collect()).unwrap_or_default();
                egui::ComboBox::from_id_salt(("cmp-schema", side)).selected_text(schema.as_str()).width(120.0).show_ui(ui, |ui| {
                    for s in schemas {
                        ui.selectable_value(schema, s.clone(), s);
                    }
                });
                if side == 0 {
                    ui.label(RichText::new("→").color(p.muted));
                }
            }
            ui.checkbox(&mut self.with_definitions, "Compare definitions").on_hover_text("Also fetch and compare the text of views, procedures, functions and triggers");
            if ui.add_enabled(!busy, egui::Button::new(RichText::new("Compare").color(Color32::WHITE)).fill(p.accent)).clicked() {
                action = CompareAction::Start;
            }
            if busy {
                ui.spinner();
            }
            ui.label(RichText::new(&self.status).color(p.muted));
        });
        ui.add_space(6.0);
        if self.phase != Phase::Done {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Choose a source and a target, then Compare.").color(p.muted));
                ui.label(RichText::new("The script changes the target so it matches the source.").size(12.0).color(p.faint));
            });
            return action;
        }
        if self.items.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| ui.label(RichText::new("No differences").size(15.0).color(p.success)));
            return action;
        }

        let avail = ui.available_size();
        ui.horizontal_top(|ui| {
            // ---- list ----
            ui.allocate_ui_with_layout(egui::vec2(avail.x * 0.42, avail.y - 40.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::ScrollArea::vertical().id_salt("cmp-list").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    for group in [DiffKind::Added, DiffKind::Changed, DiffKind::Removed] {
                        let idx: Vec<usize> = (0..self.items.len()).filter(|&i| self.items[i].kind == group).collect();
                        if idx.is_empty() {
                            continue;
                        }
                        let (title, color) = match group {
                            DiffKind::Added => ("Only in source · create in target", p.success),
                            DiffKind::Changed => ("Different · alter target", p.warning),
                            DiffKind::Removed => ("Only in target · drop", p.danger),
                        };
                        ui.add_space(6.0);
                        ui.label(RichText::new(format!("{title} ({})", idx.len())).size(12.0).color(color));
                        for i in idx {
                            let item = &self.items[i];
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut self.checked[i], "");
                                let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), Sense::click());
                                if self.selected == Some(i) {
                                    ui.painter().rect_filled(rect, CornerRadius::same(4), p.tree_selected);
                                } else if resp.hovered() {
                                    ui.painter().rect_filled(rect, CornerRadius::same(4), p.hover);
                                }
                                let icon = egui::Rect::from_center_size(egui::pos2(rect.left() + 10.0, rect.center().y), egui::vec2(14.0, 14.0));
                                crate::icons::paint_kind(ui.painter(), icon, item.object, p);
                                ui.painter().text(egui::pos2(rect.left() + 24.0, rect.center().y), egui::Align2::LEFT_CENTER, &item.name, FontId::proportional(13.0), p.text);
                                if item.destructive {
                                    ui.painter().text(egui::pos2(rect.right() - 6.0, rect.center().y), egui::Align2::RIGHT_CENTER, "may lose data", FontId::proportional(10.5), p.danger);
                                }
                                if resp.clicked() {
                                    self.selected = Some(i);
                                }
                            });
                        }
                    }
                });
            });
            ui.separator();
            // ---- detail ----
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), avail.y - 40.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                if let Some(item) = self.selected.and_then(|i| self.items.get(i)) {
                    ui.label(RichText::new(format!("{} {}", item.object.label(), item.name)).font(bold(14.0)));
                    for d in &item.details {
                        ui.label(RichText::new(format!("• {d}")).size(12.5).color(p.muted));
                    }
                    ui.add_space(6.0);
                    let mut sql = databug_sql::ddl::script(self.target_dialect, &item.stmts);
                    egui::ScrollArea::both().id_salt("cmp-sql").auto_shrink([false, false]).show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut sql).font(FontId::monospace(12.0)).desired_width(f32::INFINITY).code_editor());
                    });
                }
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.small_button("Select all").clicked() {
                self.checked.iter_mut().for_each(|c| *c = true);
            }
            if ui.small_button("Select none").clicked() {
                self.checked.iter_mut().for_each(|c| *c = false);
            }
            let n = self.checked.iter().filter(|c| **c).count();
            ui.label(RichText::new(format!("{n} of {} selected", self.items.len())).color(p.muted));
            if self.items.iter().zip(&self.checked).any(|(i, c)| *c && i.destructive) {
                ui.label(RichText::new("includes changes that can lose data").color(p.danger));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(n > 0, egui::Button::new(RichText::new("Run on target").color(Color32::WHITE)).fill(p.danger)).on_hover_text("Runs the migration on the target connection (you'll be asked to confirm drops)").clicked() {
                    action = CompareAction::RunScript(self.script());
                }
                if ui.add_enabled(n > 0, egui::Button::new("Open migration in console")).clicked() {
                    action = CompareAction::OpenScript(self.script());
                }
            });
        });
        action
    }

    /// The objects whose definitions to fetch from one side.
    pub fn definition_requests(cat: &Catalog, schema: &str) -> Vec<databug_drivers::SourceRef> {
        let Some(s) = cat.schema(schema) else { return vec![] };
        let mut v: Vec<databug_drivers::SourceRef> = s.tables.iter().filter(|t| matches!(t.kind, ObjectKind::View | ObjectKind::MaterializedView)).map(|t| databug_drivers::SourceRef::relation(&t.name, t.kind)).collect();
        v.extend(s.objects.iter().filter(|o| o.kind.has_source() && o.kind != ObjectKind::Assembly).map(databug_drivers::SourceRef::from_object));
        v
    }
}
