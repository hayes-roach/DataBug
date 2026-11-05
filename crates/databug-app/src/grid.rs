//! The results grid: virtualized (only visible rows are laid out), sortable,
//! filterable, with in-place editing for single-table queries.

use crate::theme::{bold, Palette};
use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Key, Layout, Rect, RichText, Sense, Stroke};
use egui_extras::{Column, TableBuilder};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::time::Duration;
use databug_core::{ColumnMeta, Dialect, ResultSet};

#[derive(Clone, Debug)]
pub struct EditTarget {
    pub qualified: String,
    pub pk_cols: Vec<usize>,
    pub numeric: Vec<bool>,
}

pub struct ResultView {
    pub title: String,
    pub columns: Vec<ColumnMeta>,
    pub data: ResultSet,
    pub truncated: bool,
    pub elapsed: Option<Duration>,
    pub edit_target: Option<EditTarget>,
    pub dialect: Dialect,
    order: Vec<u32>,
    order_dirty: bool,
    sort: Option<(usize, bool)>,
    filter: String,
    widths: Vec<f32>,
    widths_from_rows: usize,
    /// Per column: right-align and sort numerically.
    numeric: Vec<bool>,
    selected: Option<(usize, usize)>,
    edits: BTreeMap<(u32, u16), Option<String>>,
    editing: Option<(u32, u16, String)>,
    table_id: u64,
    scroll_to_row: Option<usize>,
    /// Whole rows selected by clicking the row numbers (data row indexes).
    selected_rows: BTreeSet<u32>,
    /// Visual row where a Shift-click range starts.
    anchor: Option<usize>,
    /// Rows marked for deletion (data row indexes).
    deleted: BTreeSet<u32>,
    /// Rows from this index on were added in the grid and aren't in the table yet.
    inserted_from: Option<u32>,
}

pub enum GridAction {
    None,
    Run(Vec<String>),
    Toast(String),
}

#[derive(Clone, Copy)]
enum ExportFormat {
    Csv,
    Tsv,
    Json,
    SqlInsert,
    Markdown,
}

impl ResultView {
    pub fn new(title: String, columns: Vec<ColumnMeta>, dialect: Dialect, table_id: u64) -> Self {
        let width = columns.len();
        ResultView {
            title,
            columns,
            data: ResultSet::new(width),
            truncated: false,
            elapsed: None,
            edit_target: None,
            dialect,
            order: Vec::new(),
            order_dirty: true,
            sort: None,
            filter: String::new(),
            widths: Vec::new(),
            widths_from_rows: 0,
            numeric: Vec::new(),
            selected: None,
            edits: BTreeMap::new(),
            editing: None,
            table_id,
            scroll_to_row: None,
            selected_rows: BTreeSet::new(),
            anchor: None,
            deleted: BTreeSet::new(),
            inserted_from: None,
        }
    }

    fn is_inserted(&self, r: u32) -> bool {
        self.inserted_from.map(|f| r >= f).unwrap_or(false)
    }

    /// Pending changes: (rows added, rows deleted, rows edited).
    fn pending(&self) -> (usize, usize, usize) {
        let added = self.inserted_from.map(|f| (f..self.data.rows() as u32).filter(|r| !self.deleted.contains(r)).count()).unwrap_or(0);
        let deleted = self.deleted.iter().filter(|r| !self.is_inserted(**r)).count();
        let edited = self.edits.keys().map(|k| k.0).filter(|r| !self.is_inserted(*r) && !self.deleted.contains(r)).collect::<BTreeSet<_>>().len();
        (added, deleted, edited)
    }

    fn has_changes(&self) -> bool {
        let (a, d, e) = self.pending();
        a + d + e > 0
    }

    /// Add an empty row (or a copy of `from` without its primary key) at the
    /// end and start editing its first cell.
    pub(crate) fn add_row(&mut self, from: Option<u32>) {
        let width = self.columns.len();
        let r = self.data.rows() as u32;
        for _ in 0..width {
            self.data.push(None);
        }
        if self.inserted_from.is_none() {
            self.inserted_from = Some(r);
        }
        let pk: Vec<usize> = self.edit_target.as_ref().map(|t| t.pk_cols.clone()).unwrap_or_default();
        if let Some(src) = from {
            for c in 0..width {
                if pk.contains(&c) {
                    continue;
                }
                if let Some(v) = self.cell(src, c).map(str::to_string) {
                    self.edits.insert((r, c as u16), Some(v));
                }
            }
        }
        self.order_dirty = true;
        self.rebuild_order();
        let vr = self.order.iter().position(|x| *x == r).unwrap_or(0);
        let first = (0..width).find(|c| !pk.contains(c)).unwrap_or(0);
        self.selected = Some((vr, first));
        self.selected_rows = [r].into();
        self.scroll_to_row = Some(vr);
        if from.is_none() {
            self.editing = Some((r, first as u16, String::new()));
        }
    }

    /// Select whole rows (data row indexes), as clicking their numbers would.
    pub(crate) fn select_rows(&mut self, rows: &[u32]) {
        self.selected_rows = rows.iter().copied().collect();
    }

    /// Mark rows for deletion, or restore them if they all already are.
    pub(crate) fn toggle_delete(&mut self, rows: &BTreeSet<u32>) {
        if rows.is_empty() {
            return;
        }
        if rows.iter().all(|r| self.deleted.contains(r)) {
            for r in rows {
                self.deleted.remove(r);
            }
        } else {
            self.deleted.extend(rows.iter().copied());
        }
        self.order_dirty = true;
    }

    /// Undo every pending change: edits, deletions and added rows.
    pub(crate) fn revert(&mut self) {
        self.edits.clear();
        self.deleted.clear();
        self.editing = None;
        if let Some(f) = self.inserted_from.take() {
            while self.data.rows() as u32 > f {
                self.data.pop_row();
            }
        }
        self.selected_rows.clear();
        self.order_dirty = true;
    }

    /// Rows the row actions apply to: the selected rows, or the row of the
    /// selected cell.
    fn target_rows(&self) -> BTreeSet<u32> {
        if !self.selected_rows.is_empty() {
            return self.selected_rows.clone();
        }
        self.selected.and_then(|(vr, _)| self.order.get(vr).copied()).into_iter().collect()
    }

    pub fn append(&mut self, rows: ResultSet) {
        self.data.append(rows);
        self.order_dirty = true;
    }

    pub fn row_count(&self) -> usize {
        self.data.rows()
    }

    pub fn heap_bytes(&self) -> usize {
        self.data.heap_bytes() + self.order.capacity() * 4
    }

    fn cell(&self, row: u32, col: usize) -> Option<&str> {
        if let Some(v) = self.edits.get(&(row, col as u16)) {
            return v.as_deref();
        }
        self.data.cell(row as usize, col)
    }

    fn rebuild_order(&mut self) {
        let n = self.data.rows() as u32;
        let f = self.filter.to_lowercase();
        self.order.clear();
        if f.is_empty() {
            self.order.extend(0..n);
        } else {
            for r in 0..n {
                if (0..self.columns.len()).any(|c| self.cell(r, c).map(|v| contains_ci(v, &f)).unwrap_or(f == "null")) {
                    self.order.push(r);
                }
            }
        }
        if let Some((col, asc)) = self.sort {
            let numeric = self.is_numeric(col);
            let data = &self.data;
            self.order.sort_by(|&a, &b| {
                let (x, y) = (data.cell(a as usize, col), data.cell(b as usize, col));
                let o = match (x, y) {
                    (None, None) => std::cmp::Ordering::Equal,
                    (None, _) => std::cmp::Ordering::Greater,
                    (_, None) => std::cmp::Ordering::Less,
                    (Some(x), Some(y)) => {
                        if numeric {
                            match (x.parse::<f64>(), y.parse::<f64>()) {
                                (Ok(a), Ok(b)) => a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal),
                                _ => x.cmp(y),
                            }
                        } else {
                            natural_cmp(x, y)
                        }
                    }
                };
                if asc || x.is_none() || y.is_none() {
                    o
                } else {
                    o.reverse()
                }
            });
        }
        // rows added in the grid always sit at the end, in the order added
        if let Some(f) = self.inserted_from {
            self.order.retain(|r| *r < f);
            let deleted = &self.deleted;
            self.order.extend((f..n).filter(|r| !deleted.contains(r)));
        }
        self.order_dirty = false;
    }

    fn ensure_widths(&mut self, ui: &egui::Ui) {
        let rows = self.data.rows();
        if self.widths.len() == self.columns.len() && (self.widths_from_rows >= 200 || self.widths_from_rows == rows) {
            return;
        }
        let char_w = ui.fonts(|f| f.glyph_width(&FontId::monospace(12.5), '0')).max(6.0);
        let sample = rows.min(200);
        self.widths = self
            .columns
            .iter()
            .enumerate()
            .map(|(c, m)| {
                let header = (m.name.chars().count().max(m.type_name.chars().count() + 2)) as f32 * 7.2 + 30.0;
                let longest = (0..sample).map(|r| self.data.cell(r, c).map(|v| v.chars().take(60).count()).unwrap_or(6)).max().unwrap_or(4) as f32;
                (longest * char_w + 18.0).max(header).clamp(56.0, 380.0)
            })
            .collect();
        self.widths_from_rows = sample;
        // columns without a declared type (expressions in SQLite, Mongo fields) are
        // numeric when every sampled value parses as a number
        self.numeric = self
            .columns
            .iter()
            .enumerate()
            .map(|(c, m)| {
                if m.is_numeric() {
                    return true;
                }
                if !m.type_name.is_empty() && !matches!(m.type_name.as_str(), "int" | "long" | "double" | "decimal" | "mixed") {
                    return false;
                }
                let mut seen = 0;
                for r in 0..sample {
                    if let Some(v) = self.data.cell(r, c) {
                        if v.parse::<f64>().is_err() {
                            return false;
                        }
                        seen += 1;
                    }
                }
                seen > 0
            })
            .collect();
    }

    fn is_numeric(&self, c: usize) -> bool {
        self.numeric.get(c).copied().unwrap_or_else(|| self.columns[c].is_numeric())
    }

    pub fn show(&mut self, ui: &mut egui::Ui, p: &Palette) -> GridAction {
        let mut action = GridAction::None;
        if self.order_dirty {
            self.rebuild_order();
        }
        self.ensure_widths(ui);

        // ---- toolbar ----
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let total = self.data.rows();
            let shown = self.order.len();
            let mut summary = if shown == total { format!("{} rows", group(total)) } else { format!("{} of {} rows", group(shown), group(total)) };
            if let Some(e) = self.elapsed {
                summary.push_str(&format!("  ·  {}", fmt_duration(e)));
            }
            ui.label(RichText::new(summary).color(p.muted));
            if self.truncated {
                ui.label(RichText::new("limit reached").color(p.warning)).on_hover_text("DataBug stopped fetching at the row limit. Change it in settings, or add a LIMIT/WHERE.");
            }
            let filter = ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter rows…").desired_width(180.0));
            if filter.changed() {
                self.order_dirty = true;
            }
            let editable = self.edit_target.is_some();
            let complete = self.elapsed.is_some();
            ui.separator();
            let why = "Editing rows needs a query on one table that includes its primary key";
            let add = ui.add_enabled(editable && complete, egui::Button::new(RichText::new("+ Row").color(if editable { p.success } else { p.faint })));
            if add.on_hover_text(if editable { "Add a row  (Alt + Insert)" } else { why }).on_disabled_hover_text(why).clicked() {
                self.add_row(None);
            }
            let targets = self.target_rows();
            let all_deleted = !targets.is_empty() && targets.iter().all(|r| self.deleted.contains(r));
            let del_label = if all_deleted { "Restore" } else { "Delete" };
            let del = ui.add_enabled(editable && !targets.is_empty(), egui::Button::new(RichText::new(format!("− {del_label}")).color(if editable && !targets.is_empty() { p.danger } else { p.faint })));
            if del.on_hover_text(if all_deleted { "Keep the selected rows" } else { "Delete the selected rows  (Delete)" }).on_disabled_hover_text(if editable { "Select rows by clicking their numbers" } else { why }).clicked() {
                self.toggle_delete(&targets);
            }
            let dup = ui.add_enabled(editable && complete && targets.len() == 1, egui::Button::new("Duplicate"));
            if dup.on_hover_text("Add a copy of the selected row (without its primary key)").on_disabled_hover_text(if editable { "Select one row" } else { why }).clicked() {
                if let Some(&r) = targets.iter().next() {
                    self.add_row(Some(r));
                }
            }
            if self.has_changes() {
                let (added, deleted, edited) = self.pending();
                let mut parts = Vec::new();
                if added > 0 {
                    parts.push(format!("{added} added"));
                }
                if edited > 0 {
                    parts.push(format!("{edited} changed"));
                }
                if deleted > 0 {
                    parts.push(format!("{deleted} deleted"));
                }
                ui.separator();
                ui.label(RichText::new(parts.join(" · ")).color(p.warning));
                if ui.add(egui::Button::new(RichText::new("Submit").color(Color32::WHITE)).fill(p.accent)).on_hover_text("Save the changes: DELETE, UPDATE and INSERT statements, run in order").clicked() {
                    match self.update_statements() {
                        Ok(stmts) => action = GridAction::Run(stmts),
                        Err(e) => action = GridAction::Toast(e),
                    }
                }
                if ui.button("Preview SQL").on_hover_text("Copy the statements to the clipboard").clicked() {
                    match self.update_statements() {
                        Ok(stmts) => {
                            ui.ctx().copy_text(stmts.join(";\n") + ";");
                            action = GridAction::Toast("Statements copied to the clipboard".into());
                        }
                        Err(e) => action = GridAction::Toast(e),
                    }
                }
                if ui.button("Revert").on_hover_text("Undo all pending changes").clicked() {
                    self.revert();
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button("Export", |ui| {
                    for (label, fmt, ext) in [
                        ("CSV…", ExportFormat::Csv, "csv"),
                        ("JSON…", ExportFormat::Json, "json"),
                        ("SQL INSERT statements…", ExportFormat::SqlInsert, "sql"),
                        ("Markdown table…", ExportFormat::Markdown, "md"),
                    ] {
                        if ui.button(label).clicked() {
                            ui.close_menu();
                            action = self.export_to_file(fmt, ext);
                        }
                    }
                    ui.separator();
                    if ui.button("Copy as TSV").clicked() {
                        ui.close_menu();
                        let mut s = String::new();
                        self.export(ExportFormat::Tsv, &mut s);
                        ui.ctx().copy_text(s);
                        action = GridAction::Toast(format!("Copied {} rows", group(self.order.len())));
                    }
                    if ui.button("Copy as CSV").clicked() {
                        ui.close_menu();
                        let mut s = String::new();
                        self.export(ExportFormat::Csv, &mut s);
                        ui.ctx().copy_text(s);
                        action = GridAction::Toast(format!("Copied {} rows", group(self.order.len())));
                    }
                });
                if self.edit_target.is_some() {
                    ui.label(RichText::new("editable").color(p.faint).small()).on_hover_text("Double-click a cell (or F2) to edit it, click row numbers to select rows, Delete to remove them, Alt + Insert to add one. Nothing is saved until you Submit.");
                }
            });
        });
        ui.add_space(2.0);

        // ---- keyboard navigation ----
        let no_text_focus = ui.ctx().memory(|m| m.focused().is_none());
        // row shortcuts while the grid has a selection and no text field has focus
        if no_text_focus && self.editing.is_none() && self.edit_target.is_some() && (self.selected.is_some() || !self.selected_rows.is_empty()) {
            let (del, add) = ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, Key::Delete) || i.consume_key(egui::Modifiers::NONE, Key::Backspace), i.consume_key(egui::Modifiers::ALT, Key::Insert)));
            if del {
                let t = self.target_rows();
                self.toggle_delete(&t);
            }
            if add && self.elapsed.is_some() {
                self.add_row(None);
            }
        }
        if let (Some((r, c)), true, None) = (self.selected, no_text_focus, &self.editing) {
            let (mut r, mut c) = (r, c);
            let (up, down, left, right, copy, enter) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(egui::Modifiers::NONE, Key::ArrowLeft),
                    i.consume_key(egui::Modifiers::NONE, Key::ArrowRight),
                    i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                    i.consume_key(egui::Modifiers::NONE, Key::F2),
                )
            });
            if up {
                r = r.saturating_sub(1);
            }
            if down && r + 1 < self.order.len() {
                r += 1;
            }
            if left {
                c = c.saturating_sub(1);
            }
            if right && c + 1 < self.columns.len() {
                c += 1;
            }
            self.selected = Some((r, c));
            if copy {
                if let Some(&dr) = self.order.get(r) {
                    ui.ctx().copy_text(self.cell(dr, c).unwrap_or("").to_string());
                }
            }
            if enter && self.edit_target.is_some() {
                if let Some(&dr) = self.order.get(r) {
                    self.editing = Some((dr, c as u16, self.cell(dr, c).unwrap_or("").to_string()));
                }
            }
        }

        // ---- table ----
        let row_h = 25.0;
        // seamless grid: no spacing between cells, thin dividers drawn by hand
        let divider = Stroke::new(1.0_f32, p.border.gamma_multiply(0.55));
        let n_rows = self.order.len();
        let gutter_w = ((n_rows.max(1) as f32).log10().floor() + 1.0) * 7.5 + 18.0;
        let mut clicked: Option<(usize, usize)> = None;
        let mut start_edit: Option<(usize, usize)> = None;
        let mut commit: Option<(u32, u16, Option<String>)> = None;
        let mut cancel_edit = false;
        let mut header_click: Option<usize> = None;
        let mut ctx_action: Option<(usize, usize, &'static str)> = None;
        let mut gutter_click: Option<(usize, egui::Modifiers)> = None;

        let sel = self.selected;
        let editable = self.edit_target.is_some();
        egui::ScrollArea::horizontal().id_salt(("grid-h", self.table_id)).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            // resize handles: invisible until hovered, so they don't read as gaps
            ui.visuals_mut().widgets.noninteractive.bg_stroke = Stroke::NONE;
            ui.visuals_mut().widgets.hovered.bg_stroke = Stroke::new(1.5_f32, p.accent);
            ui.visuals_mut().widgets.active.bg_stroke = Stroke::new(1.5_f32, p.accent);
            let mut tb = TableBuilder::new(ui)
                .id_salt(("grid", self.table_id))
                .striped(true)
                .resizable(true)
                .auto_shrink([false, false])
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::exact(gutter_w));
            for w in &self.widths {
                tb = tb.column(Column::initial(*w).at_least(36.0).clip(true));
            }
            // virtualized rows: ask the table to scroll a new row into view
            if let Some(r) = self.scroll_to_row.take() {
                tb = tb.scroll_to_row(r, Some(Align::Center));
            }
            tb.header(28.0, |mut h| {
                h.col(|ui| {
                    let rect = ui.max_rect();
                    ui.painter().rect_filled(rect, CornerRadius { nw: 6, ..CornerRadius::ZERO }, p.raised);
                    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0_f32, p.border));
                    ui.painter().vline(rect.right() - 0.5, rect.y_range(), divider);
                });
                let last = self.columns.len().saturating_sub(1);
                for (ci, col) in self.columns.iter().enumerate() {
                    h.col(|ui| {
                        let rect = ui.max_rect();
                        let round = if ci == last { CornerRadius { ne: 6, ..CornerRadius::ZERO } } else { CornerRadius::ZERO };
                        let resp = ui.interact(rect, ui.id().with(("hdr", ci)), Sense::click());
                        ui.painter().rect_filled(rect, round, if resp.hovered() { p.hover } else { p.raised });
                        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0_f32, p.border));
                        ui.painter().vline(rect.right() - 0.5, rect.y_range(), divider);
                        ui.add_space(8.0);
                        ui.label(RichText::new(&col.name).font(bold(12.5)).color(p.text));
                        if !col.type_name.is_empty() {
                            ui.add_space(6.0);
                            ui.label(RichText::new(&col.type_name).size(11.0).color(p.faint));
                        }
                        if let Some((sc, asc)) = self.sort {
                            if sc == ci {
                                ui.label(RichText::new(if asc { "▲" } else { "▼" }).size(9.0).color(p.accent));
                            }
                        }
                        if resp.clicked() {
                            header_click = Some(ci);
                        }
                        resp.on_hover_text(format!("{} · {}\nClick to sort", col.name, if col.type_name.is_empty() { "unknown type" } else { &col.type_name }));
                    });
                }
            })
            .body(|body| {
                body.rows(row_h, n_rows, |mut row| {
                    let vr = row.index();
                    let dr = self.order[vr];
                    let row_selected = self.selected_rows.contains(&dr);
                    let row_deleted = self.deleted.contains(&dr);
                    let row_inserted = self.is_inserted(dr);
                    row.col(|ui| {
                        let rect = ui.max_rect();
                        let resp = ui.interact(rect, ui.id().with(("gutter", vr)), Sense::click());
                        let fill = if row_selected { p.accent.gamma_multiply(0.35) } else if resp.hovered() { p.hover } else { p.raised.gamma_multiply(0.5) };
                        ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
                        if row_selected {
                            ui.painter().rect_filled(Rect::from_min_size(rect.min, egui::vec2(2.5, rect.height())), CornerRadius::ZERO, p.accent);
                        }
                        ui.painter().vline(rect.right() - 0.5, rect.y_range(), divider);
                        let (mark, color) = if row_inserted {
                            ("+".to_string(), p.success)
                        } else if row_deleted {
                            ("−".to_string(), p.danger)
                        } else {
                            ((vr + 1).to_string(), if row_selected { p.text } else { p.faint })
                        };
                        ui.painter().text(egui::pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, mark, FontId::proportional(11.0), color);
                        if resp.clicked() {
                            gutter_click = Some((vr, ui.input(|i| i.modifiers)));
                        }
                        if editable {
                            resp.context_menu(|ui| {
                                for label in ["Add row", "Duplicate row", if row_deleted { "Restore row" } else { "Delete row" }] {
                                    if ui.button(label).clicked() {
                                        ctx_action = Some((vr, 0, label));
                                        ui.close_menu();
                                    }
                                }
                            });
                        }
                    });
                    for ci in 0..self.columns.len() {
                        row.col(|ui| {
                            let rect = ui.max_rect();
                            ui.painter().vline(rect.right() - 0.5, rect.y_range(), divider);
                            let is_sel = sel == Some((vr, ci));
                            let edited = self.edits.contains_key(&(dr, ci as u16));
                            if row_deleted {
                                ui.painter().rect_filled(rect, CornerRadius::ZERO, p.danger.gamma_multiply(0.16));
                            } else if row_inserted {
                                ui.painter().rect_filled(rect, CornerRadius::ZERO, p.success.gamma_multiply(0.12));
                            }
                            if edited && !row_deleted {
                                ui.painter().rect_filled(rect, CornerRadius::ZERO, p.edited);
                            }
                            if row_selected && !is_sel {
                                ui.painter().rect_filled(rect, CornerRadius::ZERO, p.selection.gamma_multiply(0.35));
                            }
                            if is_sel {
                                ui.painter().rect_filled(rect, CornerRadius::ZERO, p.selection.gamma_multiply(0.55));
                                ui.painter().rect_stroke(rect.shrink(0.5), CornerRadius::ZERO, Stroke::new(1.0_f32, p.accent), egui::StrokeKind::Inside);
                            }
                            if let Some((er, ec, buf)) = self.editing.as_mut() {
                                if *er == dr && *ec as usize == ci {
                                    let te = ui.add(egui::TextEdit::singleline(buf).font(FontId::monospace(12.5)).frame(false).desired_width(rect.width() - 8.0));
                                    if !te.has_focus() && !te.lost_focus() {
                                        te.request_focus();
                                    }
                                    let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                                    if esc {
                                        cancel_edit = true;
                                    } else if te.lost_focus() || enter {
                                        let was_null = self.data.cell(dr as usize, ci).is_none() && !self.edits.contains_key(&(dr, ci as u16));
                                        commit = Some((dr, ci as u16, if buf.is_empty() && was_null { None } else { Some(buf.clone()) }));
                                    }
                                    return;
                                }
                            }
                            let value = self.cell(dr, ci);
                            let resp = ui.interact(rect, ui.id().with(("cell", vr, ci)), Sense::click());
                            ui.add_space(8.0);
                            match value {
                                None => {
                                    ui.label(RichText::new("<null>").italics().size(12.0).color(p.faint));
                                }
                                Some(v) => {
                                    let first = first_line(v, 400);
                                    let numeric = self.is_numeric(ci);
                                    let text = RichText::new(first).font(FontId::monospace(12.5)).color(p.text);
                                    if numeric {
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            ui.add_space(8.0);
                                            ui.add(egui::Label::new(text).truncate().selectable(false));
                                        });
                                    } else {
                                        ui.add(egui::Label::new(text).truncate().selectable(false));
                                    }
                                }
                            }
                            if row_deleted {
                                ui.painter().hline(rect.x_range().shrink(4.0), rect.center().y, Stroke::new(1.0_f32, p.danger.gamma_multiply(0.7)));
                            }
                            if resp.clicked() {
                                clicked = Some((vr, ci));
                            }
                            if resp.double_clicked() && editable && !row_deleted {
                                start_edit = Some((vr, ci));
                            }
                            if let Some(v) = value {
                                if v.len() > 40 || v.contains('\n') {
                                    let full: String = v.chars().take(3000).collect();
                                    resp.clone().on_hover_ui_at_pointer(|ui| {
                                        ui.set_max_width(520.0);
                                        ui.label(RichText::new(full).font(FontId::monospace(12.0)));
                                    });
                                }
                            }
                            resp.context_menu(|ui| {
                                for label in ["Copy value", "Copy row", "Copy column name", "Filter by this value"] {
                                    if ui.button(label).clicked() {
                                        ctx_action = Some((vr, ci, label));
                                        ui.close_menu();
                                    }
                                }
                                if editable {
                                    ui.separator();
                                    for label in ["Add row", "Duplicate row", if row_deleted { "Restore row" } else { "Delete row" }] {
                                        if ui.button(label).clicked() {
                                            ctx_action = Some((vr, ci, label));
                                            ui.close_menu();
                                        }
                                    }
                                    ui.separator();
                                    if ui.button("Edit value").clicked() {
                                        ctx_action = Some((vr, ci, "Edit value"));
                                        ui.close_menu();
                                    }
                                    if ui.button("Set NULL").clicked() {
                                        ctx_action = Some((vr, ci, "Set NULL"));
                                        ui.close_menu();
                                    }
                                }
                            });
                        });
                    }
                });
            });
        });

        if let Some(ci) = header_click {
            self.sort = match self.sort {
                Some((c, true)) if c == ci => Some((ci, false)),
                Some((c, false)) if c == ci => None,
                _ => Some((ci, true)),
            };
            self.order_dirty = true;
        }
        if let Some((vr, m)) = gutter_click {
            let Some(&dr) = self.order.get(vr) else { return action };
            if m.shift {
                let a = self.anchor.unwrap_or(vr);
                let (lo, hi) = (a.min(vr), a.max(vr));
                if !m.command {
                    self.selected_rows.clear();
                }
                for i in lo..=hi {
                    if let Some(&r) = self.order.get(i) {
                        self.selected_rows.insert(r);
                    }
                }
            } else if m.command {
                if !self.selected_rows.remove(&dr) {
                    self.selected_rows.insert(dr);
                }
                self.anchor = Some(vr);
            } else {
                self.selected_rows = [dr].into();
                self.anchor = Some(vr);
            }
            self.selected = Some((vr, self.selected.map(|s| s.1).unwrap_or(0)));
            ui.ctx().memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
        }
        if let Some(sel) = clicked {
            self.selected_rows.clear();
            self.anchor = Some(sel.0);
            self.selected = Some(sel);
            ui.ctx().memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
        }
        if let Some((vr, ci)) = start_edit {
            let dr = self.order[vr];
            self.editing = Some((dr, ci as u16, self.cell(dr, ci).unwrap_or("").to_string()));
        }
        if cancel_edit {
            self.editing = None;
        }
        if let Some((dr, ci, v)) = commit {
            self.editing = None;
            self.set_cell(dr, ci, v);
        }
        if let Some((vr, ci, what)) = ctx_action {
            let dr = self.order[vr];
            match what {
                "Copy value" => ui.ctx().copy_text(self.cell(dr, ci).unwrap_or("").to_string()),
                "Copy row" => {
                    let row: Vec<&str> = (0..self.columns.len()).map(|c| self.cell(dr, c).unwrap_or("")).collect();
                    ui.ctx().copy_text(row.join("\t"));
                }
                "Copy column name" => ui.ctx().copy_text(self.columns[ci].name.clone()),
                "Filter by this value" => {
                    self.filter = self.cell(dr, ci).unwrap_or("null").to_string();
                    self.order_dirty = true;
                }
                "Edit value" => self.editing = Some((dr, ci as u16, self.cell(dr, ci).unwrap_or("").to_string())),
                "Add row" => self.add_row(None),
                "Duplicate row" => self.add_row(Some(dr)),
                "Delete row" | "Restore row" => {
                    let rows = if self.selected_rows.contains(&dr) { self.selected_rows.clone() } else { [dr].into() };
                    self.toggle_delete(&rows);
                }
                "Set NULL" => self.set_cell(dr, ci as u16, None),
                _ => {}
            }
        }
        action
    }

    pub(crate) fn set_cell(&mut self, dr: u32, ci: u16, v: Option<String>) {
        let original = self.data.cell(dr as usize, ci as usize).map(str::to_string);
        if original == v {
            self.edits.remove(&(dr, ci));
        } else {
            self.edits.insert((dr, ci), v);
        }
    }

    pub fn edits_applied(&mut self) {
        // fold edits into display; the next refresh re-reads from the server
        let edits = std::mem::take(&mut self.edits);
        let deleted = std::mem::take(&mut self.deleted);
        self.inserted_from = None;
        self.selected_rows.clear();
        self.selected = None;
        let width = self.columns.len();
        let mut rs = ResultSet::with_capacity(width, self.data.rows());
        for r in 0..self.data.rows() as u32 {
            if deleted.contains(&r) {
                continue;
            }
            for c in 0..width {
                match edits.get(&(r, c as u16)) {
                    Some(v) => rs.push(v.as_deref()),
                    None => rs.push(self.data.cell(r as usize, c)),
                }
            }
        }
        self.data = rs;
        self.order_dirty = true;
    }

    fn literal(&self, v: Option<&str>, numeric: bool) -> String {
        match v {
            None => "NULL".into(),
            Some(s) if numeric && s.parse::<f64>().is_ok() => s.to_string(),
            Some(s) => self.dialect.quote_literal(s),
        }
    }

    /// The statements that save the pending changes: DELETEs, then UPDATEs,
    /// then INSERTs. Rows are matched on their original primary key values.
    pub(crate) fn update_statements(&self) -> Result<Vec<String>, String> {
        let t = self.edit_target.as_ref().ok_or("This result isn't editable")?;
        let q = |n: &str| self.dialect.quote_ident(n);
        let key = |r: u32| -> Result<String, String> {
            if t.pk_cols.is_empty() {
                return Err("Can't save: the table has no primary key in this result".into());
            }
            Ok(t.pk_cols
                .iter()
                .map(|&pk| {
                    let orig = self.data.cell(r as usize, pk);
                    match orig {
                        None => format!("{} IS NULL", q(&self.columns[pk].name)),
                        Some(_) => format!("{} = {}", q(&self.columns[pk].name), self.literal(orig, t.numeric[pk])),
                    }
                })
                .collect::<Vec<_>>()
                .join(" AND "))
        };
        let mut out = Vec::new();
        for &r in self.deleted.iter().filter(|r| !self.is_inserted(**r)) {
            out.push(format!("DELETE FROM {} WHERE {}", t.qualified, key(r)?));
        }
        let mut by_row: BTreeMap<u32, Vec<(u16, Option<&String>)>> = BTreeMap::new();
        for ((r, c), v) in &self.edits {
            if self.deleted.contains(r) {
                continue;
            }
            by_row.entry(*r).or_default().push((*c, v.as_ref()));
        }
        let mut inserts = Vec::new();
        for (r, cols) in &by_row {
            if self.is_inserted(*r) {
                continue;
            }
            let set: Vec<String> = cols.iter().map(|(c, v)| format!("{} = {}", q(&self.columns[*c as usize].name), self.literal(v.map(|s| s.as_str()), t.numeric[*c as usize]))).collect();
            out.push(format!("UPDATE {} SET {} WHERE {}", t.qualified, set.join(", "), key(*r)?));
        }
        if let Some(f) = self.inserted_from {
            for r in (f..self.data.rows() as u32).filter(|r| !self.deleted.contains(r)) {
                // only the columns you filled in, so the table's defaults apply to the rest
                let cols = by_row.get(&r).cloned().unwrap_or_default();
                if cols.is_empty() {
                    inserts.push(match self.dialect {
                        Dialect::MySql => format!("INSERT INTO {} () VALUES ()", t.qualified),
                        _ => format!("INSERT INTO {} DEFAULT VALUES", t.qualified),
                    });
                    continue;
                }
                let names: Vec<String> = cols.iter().map(|(c, _)| q(&self.columns[*c as usize].name)).collect();
                let vals: Vec<String> = cols.iter().map(|(c, v)| self.literal(v.map(|s| s.as_str()), t.numeric[*c as usize])).collect();
                inserts.push(format!("INSERT INTO {} ({}) VALUES ({})", t.qualified, names.join(", "), vals.join(", ")));
            }
        }
        out.extend(inserts);
        Ok(out)
    }

    fn export_to_file(&self, fmt: ExportFormat, ext: &str) -> GridAction {
        let name = format!("{}.{}", self.title.to_lowercase().replace(' ', "_"), ext);
        let Some(path) = rfd::FileDialog::new().set_file_name(&name).save_file() else { return GridAction::None };
        let mut s = String::with_capacity(self.data.heap_bytes() + 1024);
        self.export(fmt, &mut s);
        match std::fs::write(&path, s) {
            Ok(()) => GridAction::Toast(format!("Exported {} rows to {}", group(self.order.len()), path.display())),
            Err(e) => GridAction::Toast(format!("Couldn't write the file: {e}")),
        }
    }

    fn export(&self, fmt: ExportFormat, out: &mut String) {
        let names: Vec<&str> = self.columns.iter().map(|c| c.name.as_str()).collect();
        match fmt {
            ExportFormat::Csv | ExportFormat::Tsv => {
                let sep = if matches!(fmt, ExportFormat::Csv) { ',' } else { '\t' };
                let esc = |v: &str, out: &mut String| {
                    if matches!(fmt, ExportFormat::Csv) && (v.contains(sep) || v.contains('"') || v.contains('\n')) {
                        out.push('"');
                        out.push_str(&v.replace('"', "\"\""));
                        out.push('"');
                    } else if matches!(fmt, ExportFormat::Tsv) {
                        out.push_str(&v.replace(['\t', '\n'], " "));
                    } else {
                        out.push_str(v);
                    }
                };
                for (i, n) in names.iter().enumerate() {
                    if i > 0 {
                        out.push(sep);
                    }
                    esc(n, out);
                }
                out.push('\n');
                for &r in &self.order {
                    for c in 0..names.len() {
                        if c > 0 {
                            out.push(sep);
                        }
                        if let Some(v) = self.cell(r, c) {
                            esc(v, out);
                        }
                    }
                    out.push('\n');
                }
            }
            ExportFormat::Json => {
                out.push_str("[\n");
                for (i, &r) in self.order.iter().enumerate() {
                    let mut obj = serde_json::Map::new();
                    for (c, n) in names.iter().enumerate() {
                        let v = match self.cell(r, c) {
                            None => serde_json::Value::Null,
                            Some(s) if self.is_numeric(c) => s.parse::<serde_json::Number>().map(serde_json::Value::Number).unwrap_or_else(|_| s.into()),
                            Some(s) => s.into(),
                        };
                        obj.insert(n.to_string(), v);
                    }
                    out.push_str("  ");
                    out.push_str(&serde_json::Value::Object(obj).to_string());
                    out.push_str(if i + 1 < self.order.len() { ",\n" } else { "\n" });
                }
                out.push_str("]\n");
            }
            ExportFormat::SqlInsert => {
                let table = self.edit_target.as_ref().map(|t| t.qualified.clone()).unwrap_or_else(|| "my_table".into());
                let cols: Vec<String> = names.iter().map(|n| self.dialect.quote_ident(n)).collect();
                for &r in &self.order {
                    let vals: Vec<String> = (0..names.len()).map(|c| self.literal(self.cell(r, c), self.is_numeric(c))).collect();
                    let _ = writeln!(out, "INSERT INTO {} ({}) VALUES ({});", table, cols.join(", "), vals.join(", "));
                }
            }
            ExportFormat::Markdown => {
                let _ = writeln!(out, "| {} |", names.join(" | "));
                let _ = writeln!(out, "|{}|", names.iter().map(|_| " --- ").collect::<Vec<_>>().join("|"));
                for &r in &self.order {
                    let vals: Vec<String> = (0..names.len()).map(|c| self.cell(r, c).unwrap_or("NULL").replace('|', "\\|").replace('\n', " ")).collect();
                    let _ = writeln!(out, "| {} |", vals.join(" | "));
                }
            }
        }
    }
}

fn first_line(v: &str, max_chars: usize) -> String {
    let line = v.split('\n').next().unwrap_or("");
    let mut s: String = line.chars().take(max_chars).collect();
    if s.len() < v.len() {
        s.push('…');
    }
    s
}

fn contains_ci(hay: &str, needle_lower: &str) -> bool {
    if hay.is_ascii() && needle_lower.is_ascii() {
        hay.as_bytes().windows(needle_lower.len().max(1)).any(|w| w.eq_ignore_ascii_case(needle_lower.as_bytes()))
    } else {
        hay.to_lowercase().contains(needle_lower)
    }
}

/// Compare strings so that "item2" sorts before "item10".
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(|c| c.is_ascii_digit()) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(|c| c.is_ascii_digit()) {
                    nb.push(c);
                    bi.next();
                }
                let o = na.trim_start_matches('0').len().cmp(&nb.trim_start_matches('0').len()).then_with(|| na.trim_start_matches('0').cmp(nb.trim_start_matches('0')));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

pub fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn fmt_duration(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1.0 {
        format!("{:.2} ms", ms)
    } else if ms < 1000.0 {
        format!("{:.0} ms", ms)
    } else if ms < 60_000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else {
        format!("{}m {}s", (ms / 60_000.0) as u64, ((ms % 60_000.0) / 1000.0) as u64)
    }
}

pub fn painted_dot(ui: &mut egui::Ui, color: Color32, r: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(r * 2.0 + 2.0, r * 2.0 + 2.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), r, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_sort() {
        let mut v = vec!["item10", "item2", "Item1"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["Item1", "item2", "item10"]);
    }

    #[test]
    fn updates_use_pk() {
        let cols = vec![ColumnMeta::new("id", "int4"), ColumnMeta::new("name", "text")];
        let mut g = ResultView::new("r".into(), cols, Dialect::Postgres, 1);
        let mut rs = ResultSet::new(2);
        rs.push(Some("7"));
        rs.push(Some("old"));
        g.append(rs);
        g.edit_target = Some(EditTarget { qualified: "public.t".into(), pk_cols: vec![0], numeric: vec![true, false] });
        g.set_cell(0, 1, Some("it's".into()));
        assert_eq!(g.update_statements().unwrap(), vec!["UPDATE public.t SET name = 'it''s' WHERE id = 7"]);
    }

    fn three_rows() -> ResultView {
        let cols = vec![ColumnMeta::new("id", "int4"), ColumnMeta::new("name", "text"), ColumnMeta::new("qty", "int4")];
        let mut g = ResultView::new("r".into(), cols, Dialect::Postgres, 1);
        let mut rs = ResultSet::new(3);
        for (id, name) in [("1", "a"), ("2", "b"), ("3", "c")] {
            rs.push(Some(id));
            rs.push(Some(name));
            rs.push(Some("5"));
        }
        g.append(rs);
        g.elapsed = Some(Duration::from_millis(1));
        g.edit_target = Some(EditTarget { qualified: "public.t".into(), pk_cols: vec![0], numeric: vec![true, false, true] });
        g
    }

    #[test]
    fn row_changes_in_order() {
        let mut g = three_rows();
        g.toggle_delete(&[1u32].into());
        g.set_cell(2, 1, Some("z".into()));
        g.add_row(None);
        g.editing = None;
        g.set_cell(3, 1, Some("new".into()));
        g.set_cell(3, 2, Some("9".into()));
        g.add_row(Some(0));
        assert_eq!(
            g.update_statements().unwrap(),
            vec![
                "DELETE FROM public.t WHERE id = 2",
                "UPDATE public.t SET name = 'z' WHERE id = 3",
                "INSERT INTO public.t (name, qty) VALUES ('new', 9)",
                "INSERT INTO public.t (name, qty) VALUES ('a', 5)",
            ]
        );
        assert_eq!(g.pending(), (2, 1, 1));
    }

    #[test]
    fn deleting_a_new_row_discards_it() {
        let mut g = three_rows();
        g.add_row(None);
        g.editing = None;
        g.toggle_delete(&[3u32].into());
        assert!(g.update_statements().unwrap().is_empty());
        assert!(!g.has_changes());
        // an untouched new row uses the table's defaults
        g.add_row(None);
        assert_eq!(g.update_statements().unwrap(), vec!["INSERT INTO public.t DEFAULT VALUES"]);
    }

    #[test]
    fn revert_and_apply() {
        let mut g = three_rows();
        g.add_row(None);
        g.toggle_delete(&[0u32].into());
        g.revert();
        assert_eq!(g.row_count(), 3);
        assert!(!g.has_changes());
        g.toggle_delete(&[0u32].into());
        g.set_cell(1, 1, Some("bb".into()));
        g.edits_applied();
        assert_eq!(g.row_count(), 2);
        assert_eq!(g.data.cell(0, 1), Some("bb"));
    }

    #[test]
    fn grouping() {
        assert_eq!(group(1234567), "1,234,567");
        assert_eq!(group(12), "12");
    }
}
