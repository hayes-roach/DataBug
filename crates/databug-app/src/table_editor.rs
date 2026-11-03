//! Create or modify a table: columns, indexes and foreign keys, with a live
//! preview of the DDL that will run.

use crate::theme::{bold, Palette};
use databug_core::{ColumnInfo, Dialect, ForeignKey, IndexInfo, ObjectKind, TableInfo};
use databug_sql::ddl::{self, ColumnEdit};
use eframe::egui::{self, Color32, FontId, RichText};

struct ColRow {
    original: Option<String>,
    name: String,
    ty: String,
    nullable: bool,
    pk: bool,
    default: String,
    dropped: bool,
}

struct IdxRow {
    name: String,
    columns: String,
    unique: bool,
    dropped: bool,
}

struct FkRow {
    name: String,
    columns: String,
    ref_table: String,
    ref_columns: String,
    on_delete: String,
    dropped: bool,
}

#[derive(PartialEq, Clone, Copy)]
enum Section {
    Columns,
    Indexes,
    Keys,
}

pub struct TableEditor {
    pub conn_id: String,
    pub schema: String,
    pub dialect: Dialect,
    original: Option<TableInfo>,
    name: String,
    cols: Vec<ColRow>,
    idx: Vec<IdxRow>,
    fks: Vec<FkRow>,
    primary_index: Option<IndexInfo>,
    tables: Vec<String>,
    section: Section,
}

pub enum EditorOutcome {
    Open,
    Cancel,
    Execute(String),
    OpenInConsole(String),
}

fn split(s: &str) -> Vec<String> {
    s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

fn types(d: Dialect) -> &'static [&'static str] {
    match d {
        Dialect::Postgres => &["integer", "bigint", "smallint", "serial", "bigserial", "numeric(12,2)", "real", "double precision", "boolean", "text", "varchar(255)", "uuid", "date", "timestamp", "timestamptz", "jsonb", "bytea"],
        Dialect::MySql => &["INT", "BIGINT", "SMALLINT", "TINYINT(1)", "DECIMAL(12,2)", "DOUBLE", "VARCHAR(255)", "TEXT", "DATE", "DATETIME", "TIMESTAMP", "JSON", "BLOB"],
        Dialect::MsSql => &["int", "bigint", "smallint", "bit", "decimal(12,2)", "float", "nvarchar(255)", "nvarchar(max)", "varchar(255)", "uniqueidentifier", "date", "datetime2", "datetimeoffset", "varbinary(max)"],
        Dialect::Sqlite => &["INTEGER", "REAL", "TEXT", "NUMERIC", "BLOB"],
        Dialect::Mongo => &[],
    }
}

impl TableEditor {
    pub fn new(conn_id: String, schema: String, dialect: Dialect, table: Option<TableInfo>, tables: Vec<String>) -> Self {
        let (name, cols, idx, fks, primary_index) = match &table {
            Some(t) => (
                t.name.clone(),
                t.columns.iter().map(|c| ColRow { original: Some(c.name.clone()), name: c.name.clone(), ty: c.data_type.clone(), nullable: c.nullable, pk: c.primary_key, default: c.default.clone().unwrap_or_default(), dropped: false }).collect(),
                t.indexes.iter().filter(|i| !i.primary).map(|i| IdxRow { name: i.name.clone(), columns: i.columns.join(", "), unique: i.unique, dropped: false }).collect(),
                t.foreign_keys.iter().map(|f| FkRow { name: f.name.clone(), columns: f.columns.join(", "), ref_table: f.ref_table.clone(), ref_columns: f.ref_columns.join(", "), on_delete: f.on_delete.clone(), dropped: false }).collect(),
                t.primary_index().cloned(),
            ),
            None => (
                "new_table".to_string(),
                vec![ColRow {
                    original: None,
                    name: "id".into(),
                    ty: types(dialect).first().copied().unwrap_or("integer").to_string(),
                    nullable: false,
                    pk: true,
                    default: String::new(),
                    dropped: false,
                }],
                vec![],
                vec![],
                None,
            ),
        };
        TableEditor { conn_id, schema, dialect, original: table, name, cols, idx, fks, primary_index, tables, section: Section::Columns }
    }

    /// Used by the screenshot scenes to show a pending change.
    pub fn add_column(&mut self, name: &str, ty: &str) {
        self.cols.push(ColRow { original: None, name: name.into(), ty: ty.into(), nullable: true, pk: false, default: String::new(), dropped: false });
    }

    fn fk_list(&self) -> Vec<ForeignKey> {
        self.fks
            .iter()
            .filter(|f| !f.dropped)
            .map(|f| ForeignKey {
                name: f.name.trim().to_string(),
                columns: split(&f.columns),
                ref_schema: self.schema.clone(),
                ref_table: f.ref_table.trim().to_string(),
                ref_columns: split(&f.ref_columns),
                on_delete: if f.on_delete == "NO ACTION" { String::new() } else { f.on_delete.clone() },
            })
            .collect()
    }

    fn index_list(&self) -> Vec<IndexInfo> {
        let mut v: Vec<IndexInfo> = self.primary_index.iter().cloned().collect();
        v.extend(self.idx.iter().filter(|i| !i.dropped).map(|i| IndexInfo { name: i.name.trim().to_string(), columns: split(&i.columns), unique: i.unique, primary: false }));
        v
    }

    fn validate(&self) -> Option<String> {
        if self.name.trim().is_empty() {
            return Some("Enter a table name".into());
        }
        let live: Vec<&ColRow> = self.cols.iter().filter(|c| !c.dropped).collect();
        if live.is_empty() {
            return Some("A table needs at least one column".into());
        }
        for c in &live {
            if c.name.trim().is_empty() || c.ty.trim().is_empty() {
                return Some("Every column needs a name and a type".into());
            }
        }
        let mut names: Vec<String> = live.iter().map(|c| c.name.trim().to_lowercase()).collect();
        names.sort();
        if names.windows(2).any(|w| w[0] == w[1]) {
            return Some("Column names must be unique".into());
        }
        for i in self.idx.iter().filter(|i| !i.dropped) {
            if i.name.trim().is_empty() || split(&i.columns).is_empty() {
                return Some("Every index needs a name and at least one column".into());
            }
        }
        for f in self.fks.iter().filter(|f| !f.dropped) {
            if f.ref_table.trim().is_empty() || split(&f.columns).len() != split(&f.ref_columns).len() || split(&f.columns).is_empty() {
                return Some("Each foreign key needs a referenced table and the same number of columns on both sides".into());
            }
        }
        None
    }

    /// The DDL for the current state of the editor.
    pub fn script(&self) -> String {
        let d = self.dialect;
        let cols: Vec<ColumnInfo> = self
            .cols
            .iter()
            .map(|c| ColumnInfo { name: c.name.trim().to_string(), data_type: c.ty.trim().to_string(), nullable: c.nullable && !c.pk, primary_key: c.pk, default: Some(c.default.trim().to_string()).filter(|s| !s.is_empty()) })
            .collect();
        match &self.original {
            None => {
                let mut t = TableInfo::new(self.name.trim(), ObjectKind::Table);
                t.columns = cols.into_iter().zip(&self.cols).filter(|(_, r)| !r.dropped).map(|(c, _)| c).collect();
                t.indexes = self.index_list();
                t.foreign_keys = self.fk_list();
                ddl::script(d, &ddl::create_table(d, &self.schema, &t))
            }
            Some(old) => {
                let edits: Vec<ColumnEdit> = cols.into_iter().zip(&self.cols).map(|(c, r)| ColumnEdit { original: r.original.clone(), column: c, dropped: r.dropped }).collect();
                ddl::script(d, &ddl::alter_table(d, &self.schema, old, self.name.trim(), &edits, &self.index_list(), &self.fk_list()))
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, p: &Palette) -> EditorOutcome {
        let mut outcome = EditorOutcome::Open;
        let title = match &self.original {
            Some(t) => format!("Modify table {}.{}", self.schema, t.name),
            None => format!("New table in {}", self.schema),
        };
        let mut open = true;
        let script = self.script();
        let error = self.validate();
        egui::Window::new(RichText::new(title).font(bold(15.0)))
            .id(egui::Id::new("table-editor"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([820.0, 600.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Name").color(p.muted));
                    ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(280.0));
                    ui.add_space(16.0);
                    let live = self.cols.iter().filter(|c| !c.dropped).count();
                    for (s, label) in [(Section::Columns, format!("Columns {live}")), (Section::Indexes, format!("Indexes {}", self.idx.iter().filter(|i| !i.dropped).count())), (Section::Keys, format!("Foreign keys {}", self.fks.iter().filter(|f| !f.dropped).count()))] {
                        ui.selectable_value(&mut self.section, s, label);
                    }
                });
                ui.add_space(6.0);
                let list_h = (ui.available_height() - 230.0).max(160.0);
                egui::ScrollArea::vertical().id_salt("te-list").max_height(list_h).auto_shrink([false, true]).show(ui, |ui| match self.section {
                    Section::Columns => self.columns_ui(ui, p),
                    Section::Indexes => self.indexes_ui(ui, p),
                    Section::Keys => self.keys_ui(ui, p),
                });
                ui.add_space(6.0);
                ui.separator();
                ui.label(RichText::new("DDL preview").size(12.0).color(p.muted));
                egui::ScrollArea::vertical().id_salt("te-ddl").max_height(150.0).auto_shrink([false, true]).show(ui, |ui| {
                    let mut text = if script.trim().is_empty() { "-- no changes".to_string() } else { script.clone() };
                    ui.add(egui::TextEdit::multiline(&mut text).font(FontId::monospace(12.0)).desired_width(f32::INFINITY).interactive(true).code_editor());
                });
                if let Some(e) = &error {
                    ui.label(RichText::new(e).color(p.warning));
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let ok = error.is_none() && !script.trim().is_empty();
                        if ui.add_enabled(ok, egui::Button::new(RichText::new("Execute").color(Color32::WHITE)).fill(p.accent)).on_hover_text("Run the DDL now (you'll be asked to confirm on production)").clicked() {
                            outcome = EditorOutcome::Execute(script.clone());
                        }
                        if ui.add_enabled(ok, egui::Button::new("Open in console")).on_hover_text("Review and run it yourself").clicked() {
                            outcome = EditorOutcome::OpenInConsole(script.clone());
                        }
                        if ui.button("Cancel").clicked() {
                            outcome = EditorOutcome::Cancel;
                        }
                    });
                });
            });
        if !open {
            outcome = EditorOutcome::Cancel;
        }
        outcome
    }

    fn columns_ui(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let d = self.dialect;
        egui::Grid::new("te-cols").num_columns(6).spacing([8.0, 6.0]).striped(false).show(ui, |ui| {
            for h in ["Name", "Type", "Not null", "Primary key", "Default", ""] {
                ui.label(RichText::new(h).size(11.5).color(p.muted));
            }
            ui.end_row();
            let mut remove: Option<usize> = None;
            for (i, c) in self.cols.iter_mut().enumerate() {
                let color = if c.dropped { p.danger } else if c.original.is_none() { p.success } else { p.text };
                ui.add_enabled_ui(!c.dropped, |ui| ui.add_sized([170.0, 22.0], egui::TextEdit::singleline(&mut c.name).text_color(color)));
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!c.dropped, |ui| ui.add_sized([150.0, 22.0], egui::TextEdit::singleline(&mut c.ty).font(FontId::monospace(12.0))));
                    ui.menu_button("…", |ui| {
                        for t in types(d) {
                            if ui.button(*t).clicked() {
                                c.ty = t.to_string();
                                ui.close_menu();
                            }
                        }
                    });
                });
                let mut not_null = !c.nullable;
                if ui.add_enabled(!c.dropped && !c.pk, egui::Checkbox::without_text(&mut not_null)).changed() {
                    c.nullable = !not_null;
                }
                ui.add_enabled(!c.dropped, egui::Checkbox::without_text(&mut c.pk));
                ui.add_enabled_ui(!c.dropped, |ui| ui.add_sized([140.0, 22.0], egui::TextEdit::singleline(&mut c.default).hint_text("none").font(FontId::monospace(12.0))));
                let label = if c.dropped { "Undo" } else { "Drop" };
                if ui.small_button(label).clicked() {
                    if c.original.is_none() {
                        remove = Some(i);
                    } else {
                        c.dropped = !c.dropped;
                    }
                }
                ui.end_row();
            }
            if let Some(i) = remove {
                self.cols.remove(i);
            }
        });
        ui.add_space(4.0);
        if ui.button("+ Add column").clicked() {
            let n = self.cols.len() + 1;
            self.cols.push(ColRow { original: None, name: format!("column_{n}"), ty: types(d).iter().find(|t| t.to_lowercase().contains("text") || t.to_lowercase().starts_with("nvarchar(255")).copied().unwrap_or("text").to_string(), nullable: true, pk: false, default: String::new(), dropped: false });
        }
    }

    fn indexes_ui(&mut self, ui: &mut egui::Ui, p: &Palette) {
        if let Some(pk) = &self.primary_index {
            ui.label(RichText::new(format!("Primary key {} ({}). Change it with the Primary key boxes on the Columns tab.", pk.name, pk.columns.join(", "))).size(12.0).color(p.muted));
            ui.add_space(4.0);
        }
        egui::Grid::new("te-idx").num_columns(4).spacing([8.0, 6.0]).show(ui, |ui| {
            for h in ["Name", "Columns (comma separated)", "Unique", ""] {
                ui.label(RichText::new(h).size(11.5).color(p.muted));
            }
            ui.end_row();
            let mut remove = None;
            for (i, ix) in self.idx.iter_mut().enumerate() {
                ui.add_enabled_ui(!ix.dropped, |ui| ui.add_sized([200.0, 22.0], egui::TextEdit::singleline(&mut ix.name)));
                ui.add_enabled_ui(!ix.dropped, |ui| ui.add_sized([260.0, 22.0], egui::TextEdit::singleline(&mut ix.columns)));
                ui.add_enabled(!ix.dropped, egui::Checkbox::without_text(&mut ix.unique));
                if ui.small_button(if ix.dropped { "Undo" } else { "Drop" }).clicked() {
                    ix.dropped = !ix.dropped;
                    if ix.dropped && ix.name.starts_with("new_index") {
                        remove = Some(i);
                    }
                }
                ui.end_row();
            }
            if let Some(i) = remove {
                self.idx.remove(i);
            }
        });
        ui.add_space(4.0);
        if ui.button("+ Add index").clicked() {
            let first = self.cols.iter().find(|c| !c.dropped && !c.pk).map(|c| c.name.clone()).unwrap_or_default();
            self.idx.push(IdxRow { name: format!("{}_{}_idx", self.name.trim(), first), columns: first, unique: false, dropped: false });
        }
    }

    fn keys_ui(&mut self, ui: &mut egui::Ui, p: &Palette) {
        if self.dialect == Dialect::Sqlite {
            ui.label(RichText::new("SQLite rebuilds the table to change foreign keys; the preview shows the steps.").size(12.0).color(p.muted));
        }
        let tables = self.tables.clone();
        egui::Grid::new("te-fk").num_columns(6).spacing([8.0, 6.0]).show(ui, |ui| {
            for h in ["Name", "Columns", "References table", "Referenced columns", "On delete", ""] {
                ui.label(RichText::new(h).size(11.5).color(p.muted));
            }
            ui.end_row();
            for (i, f) in self.fks.iter_mut().enumerate() {
                ui.add_enabled_ui(!f.dropped, |ui| ui.add_sized([150.0, 22.0], egui::TextEdit::singleline(&mut f.name)));
                ui.add_enabled_ui(!f.dropped, |ui| ui.add_sized([120.0, 22.0], egui::TextEdit::singleline(&mut f.columns)));
                egui::ComboBox::from_id_salt(("fk-ref", i)).selected_text(if f.ref_table.is_empty() { "choose…" } else { &f.ref_table }).width(150.0).show_ui(ui, |ui| {
                    for t in &tables {
                        ui.selectable_value(&mut f.ref_table, t.clone(), t);
                    }
                });
                ui.add_enabled_ui(!f.dropped, |ui| ui.add_sized([120.0, 22.0], egui::TextEdit::singleline(&mut f.ref_columns)));
                egui::ComboBox::from_id_salt(("fk-del", i)).selected_text(if f.on_delete.is_empty() { "NO ACTION" } else { &f.on_delete }).width(110.0).show_ui(ui, |ui| {
                    for o in ["NO ACTION", "CASCADE", "SET NULL", "RESTRICT"] {
                        let v = if o == "NO ACTION" { String::new() } else { o.to_string() };
                        ui.selectable_value(&mut f.on_delete, v, o);
                    }
                });
                if ui.small_button(if f.dropped { "Undo" } else { "Drop" }).clicked() {
                    f.dropped = !f.dropped;
                }
                ui.end_row();
            }
        });
        ui.add_space(4.0);
        if ui.button("+ Add foreign key").clicked() {
            let n = self.fks.len() + 1;
            self.fks.push(FkRow { name: format!("fk_{}_{n}", self.name.trim()), columns: String::new(), ref_table: String::new(), ref_columns: "id".into(), on_delete: String::new(), dropped: false });
        }
    }
}
