//! Import a CSV file into an existing table (mapping columns by name) or a
//! new table (types inferred from the data).

use crate::theme::{bold, Palette};
use databug_core::{ColumnInfo, DbKind, Dialect, ObjectKind, TableInfo};
use databug_sql::ddl;
use eframe::egui::{self, Color32, FontId, RichText};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq)]
enum Inferred {
    Int,
    Float,
    Text,
}

pub struct CsvImport {
    pub conn_id: String,
    pub schema: String,
    kind: DbKind,
    path: PathBuf,
    delimiter: u8,
    header: bool,
    empty_is_null: bool,
    rows: Vec<Vec<String>>,
    error: Option<String>,
    tables: Vec<TableInfo>,
    /// Some(table) = existing table, None = new table
    target: Option<String>,
    new_name: String,
    /// csv column → target column index (existing table)
    mapping: Vec<Option<usize>>,
    /// new table columns: (include, name, type)
    new_cols: Vec<(bool, String, String)>,
}

pub enum ImportOutcome {
    Open,
    Cancel,
    Run { stmts: Vec<String>, summary: String },
}

/// A safe column name from a CSV header: "On time %" → "on_time".
fn ident(h: &str) -> String {
    let mut out = String::new();
    for c in h.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        format!("col_{out}")
    } else {
        out
    }
}

fn detect_delimiter(sample: &str) -> u8 {
    let line = sample.lines().next().unwrap_or("");
    (*b",;\t|").into_iter().max_by_key(|d| line.matches(*d as char).count()).unwrap_or(b',')
}

fn type_for(d: Dialect, t: Inferred) -> &'static str {
    match (d, t) {
        (Dialect::Postgres, Inferred::Int) => "bigint",
        (Dialect::Postgres, Inferred::Float) => "double precision",
        (Dialect::Postgres, Inferred::Text) => "text",
        (Dialect::MySql, Inferred::Int) => "BIGINT",
        (Dialect::MySql, Inferred::Float) => "DOUBLE",
        (Dialect::MySql, Inferred::Text) => "TEXT",
        (Dialect::MsSql, Inferred::Int) => "bigint",
        (Dialect::MsSql, Inferred::Float) => "float",
        (Dialect::MsSql, Inferred::Text) => "nvarchar(max)",
        (_, Inferred::Int) => "INTEGER",
        (_, Inferred::Float) => "REAL",
        (_, Inferred::Text) => "TEXT",
    }
}

impl CsvImport {
    pub fn new(conn_id: String, schema: String, kind: DbKind, path: PathBuf, tables: Vec<TableInfo>, target: Option<String>) -> Self {
        let stem = path.file_stem().map(|s| s.to_string_lossy().to_lowercase().replace([' ', '-', '.'], "_")).unwrap_or_else(|| "imported".into());
        let mut me = CsvImport {
            conn_id,
            schema,
            kind,
            path,
            delimiter: b',',
            header: true,
            empty_is_null: true,
            rows: Vec::new(),
            error: None,
            tables: tables.into_iter().filter(|t| matches!(t.kind, ObjectKind::Table | ObjectKind::Collection)).collect(),
            target,
            new_name: stem,
            mapping: Vec::new(),
            new_cols: Vec::new(),
        };
        if let Ok(bytes) = std::fs::read(&me.path) {
            me.delimiter = detect_delimiter(&String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]));
        }
        me.reload();
        me
    }

    fn reload(&mut self) {
        self.error = None;
        let rdr = csv::ReaderBuilder::new().delimiter(self.delimiter).has_headers(false).flexible(true).from_path(&self.path);
        match rdr {
            Ok(mut r) => {
                self.rows = Vec::new();
                for rec in r.records() {
                    match rec {
                        Ok(rec) => self.rows.push(rec.iter().map(|s| s.to_string()).collect()),
                        Err(e) => {
                            self.error = Some(format!("Line {}: {e}", self.rows.len() + 1));
                            break;
                        }
                    }
                }
            }
            Err(e) => self.error = Some(format!("Couldn't read the file: {e}")),
        }
        self.rebuild();
    }

    fn width(&self) -> usize {
        self.rows.iter().map(|r| r.len()).max().unwrap_or(0)
    }

    fn headers(&self) -> Vec<String> {
        let w = self.width();
        (0..w)
            .map(|i| {
                if self.header {
                    self.rows.first().and_then(|r| r.get(i)).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("column_{}", i + 1))
                } else {
                    format!("column_{}", i + 1)
                }
            })
            .collect()
    }

    fn data(&self) -> &[Vec<String>] {
        if self.header && !self.rows.is_empty() {
            &self.rows[1..]
        } else {
            &self.rows
        }
    }

    fn infer(&self, col: usize) -> Inferred {
        let mut t = Inferred::Int;
        let mut seen = false;
        for r in self.data().iter().take(5000) {
            let v = r.get(col).map(|s| s.trim()).unwrap_or("");
            if v.is_empty() {
                continue;
            }
            seen = true;
            if t == Inferred::Int && v.parse::<i64>().is_err() {
                t = Inferred::Float;
            }
            if t == Inferred::Float && v.parse::<f64>().is_err() {
                return Inferred::Text;
            }
        }
        if seen {
            t
        } else {
            Inferred::Text
        }
    }

    fn rebuild(&mut self) {
        let headers = self.headers();
        let d = self.kind.dialect();
        self.new_cols = headers.iter().enumerate().map(|(i, h)| (true, ident(h), type_for(d, self.infer(i)).to_string())).collect();
        self.mapping = match self.target.as_ref().and_then(|t| self.tables.iter().find(|x| &x.name == t)) {
            Some(t) => headers.iter().map(|h| t.columns.iter().position(|c| c.name.eq_ignore_ascii_case(h) || c.name.eq_ignore_ascii_case(&h.replace(' ', "_")))).collect(),
            None => vec![None; headers.len()],
        };
    }

    pub(crate) fn statements(&self) -> Result<(Vec<String>, String), String> {
        let d = self.kind.dialect();
        let data = self.data();
        if data.is_empty() {
            return Err("The file has no data rows".into());
        }
        let cell = |r: &Vec<String>, i: usize| -> Option<String> {
            let v = r.get(i).cloned().unwrap_or_default();
            if self.empty_is_null && v.trim().is_empty() {
                None
            } else {
                Some(v)
            }
        };
        let (table, cols, numeric, picks, mut stmts): (String, Vec<String>, Vec<bool>, Vec<usize>, Vec<String>) = match &self.target {
            Some(name) => {
                let t = self.tables.iter().find(|t| &t.name == name).ok_or("Choose a table")?;
                let picks: Vec<(usize, usize)> = self.mapping.iter().enumerate().filter_map(|(i, m)| m.map(|c| (i, c))).collect();
                if picks.is_empty() {
                    return Err("Map at least one CSV column to a table column".into());
                }
                let cols = picks.iter().map(|(_, c)| t.columns[*c].name.clone()).collect();
                let numeric = picks.iter().map(|(_, c)| databug_core::ColumnMeta::new("", t.columns[*c].data_type.clone()).is_numeric()).collect();
                (t.name.clone(), cols, numeric, picks.iter().map(|p| p.0).collect(), vec![])
            }
            None => {
                if self.new_name.trim().is_empty() {
                    return Err("Enter a name for the new table".into());
                }
                let picks: Vec<usize> = (0..self.new_cols.len()).filter(|i| self.new_cols[*i].0).collect();
                if picks.is_empty() {
                    return Err("Include at least one column".into());
                }
                let mut t = TableInfo::new(self.new_name.trim(), ObjectKind::Table);
                t.columns = picks.iter().map(|&i| ColumnInfo { name: self.new_cols[i].1.clone(), data_type: self.new_cols[i].2.clone(), nullable: true, primary_key: false, default: None }).collect();
                let create = if self.kind == DbKind::MongoDb { vec![] } else { ddl::create_table(d, &self.schema, &t).into_iter().map(|s| s.sql).collect() };
                let numeric = picks.iter().map(|&i| !self.new_cols[i].2.to_lowercase().contains("text") && !self.new_cols[i].2.to_lowercase().contains("char")).collect();
                (t.name.clone(), t.columns.iter().map(|c| c.name.clone()).collect(), numeric, picks, create)
            }
        };
        let rows: Vec<Vec<Option<String>>> = data.iter().map(|r| picks.iter().map(|&i| cell(r, i)).collect()).collect();
        if self.kind == DbKind::MongoDb {
            for chunk in rows.chunks(500) {
                let docs: Vec<serde_json::Value> = chunk
                    .iter()
                    .map(|r| {
                        let mut m = serde_json::Map::new();
                        for (i, v) in r.iter().enumerate() {
                            let val = match v {
                                None => serde_json::Value::Null,
                                Some(s) if numeric[i] => s.trim().parse::<serde_json::Number>().map(serde_json::Value::Number).unwrap_or_else(|_| s.clone().into()),
                                Some(s) => s.clone().into(),
                            };
                            m.insert(cols[i].clone(), val);
                        }
                        serde_json::Value::Object(m)
                    })
                    .collect();
                stmts.push(format!("db.{}.insertMany({})", table, serde_json::Value::Array(docs)));
            }
        } else {
            let target = ddl::qualified(d, &self.schema, &table);
            stmts.push(d.begin_sql().to_string());
            stmts.extend(ddl::insert_batches(d, &target, &cols, &numeric, &rows, if d == Dialect::MsSql { 900 } else { 500 }));
            stmts.push("COMMIT".into());
        }
        Ok((stmts, format!("Imported {} rows into {}", rows.len(), table)))
    }

    pub fn show(&mut self, ctx: &egui::Context, p: &Palette) -> ImportOutcome {
        let mut outcome = ImportOutcome::Open;
        let mut open = true;
        let mut reload = false;
        let mut rebuild = false;
        egui::Window::new(RichText::new("Import CSV").font(bold(15.0)))
            .id(egui::Id::new("csv-import"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([780.0, 560.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(RichText::new(self.path.display().to_string()).font(FontId::monospace(12.0)).color(p.muted));
                let total = self.data().len();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} rows · {} columns", crate::grid::group(total), self.width())).color(p.muted));
                    ui.separator();
                    ui.label("Delimiter");
                    for (d, label) in [(b',', ","), (b';', ";"), (b'\t', "tab"), (b'|', "|")] {
                        if ui.selectable_label(self.delimiter == d, label).clicked() {
                            self.delimiter = d;
                            reload = true;
                        }
                    }
                    if ui.checkbox(&mut self.header, "First row is a header").changed() {
                        rebuild = true;
                    }
                    ui.checkbox(&mut self.empty_is_null, "Empty values are NULL");
                });
                ui.horizontal(|ui| {
                    ui.label("Into");
                    let sel = self.target.clone().unwrap_or_else(|| "a new table".into());
                    egui::ComboBox::from_id_salt("csv-target").selected_text(sel).width(220.0).show_ui(ui, |ui| {
                        if ui.selectable_label(self.target.is_none(), "a new table").clicked() {
                            self.target = None;
                            rebuild = true;
                        }
                        for t in &self.tables {
                            if ui.selectable_label(self.target.as_deref() == Some(t.name.as_str()), &t.name).clicked() {
                                self.target = Some(t.name.clone());
                                rebuild = true;
                            }
                        }
                    });
                    if self.target.is_none() {
                        ui.label("named");
                        ui.add(egui::TextEdit::singleline(&mut self.new_name).desired_width(200.0));
                    }
                });
                ui.add_space(6.0);
                let headers = self.headers();
                egui::ScrollArea::both().id_salt("csv-map").max_height(ui.available_height() - 60.0).show(ui, |ui| {
                    egui::Grid::new("csv-cols").num_columns(3).spacing([12.0, 5.0]).striped(true).show(ui, |ui| {
                        ui.label(RichText::new("CSV column").size(11.5).color(p.muted));
                        ui.label(RichText::new(if self.target.is_some() { "Table column" } else { "New column and type" }).size(11.5).color(p.muted));
                        ui.label(RichText::new("Sample values").size(11.5).color(p.muted));
                        ui.end_row();
                        for (i, h) in headers.iter().enumerate() {
                            ui.label(RichText::new(h).font(FontId::monospace(12.0)));
                            match &self.target {
                                Some(name) => {
                                    let cols: Vec<String> = self.tables.iter().find(|t| &t.name == name).map(|t| t.columns.iter().map(|c| c.name.clone()).collect()).unwrap_or_default();
                                    let cur = self.mapping.get(i).copied().flatten();
                                    egui::ComboBox::from_id_salt(("map", i)).selected_text(cur.and_then(|c| cols.get(c).cloned()).unwrap_or_else(|| "skip".into())).width(200.0).show_ui(ui, |ui| {
                                        if ui.selectable_label(cur.is_none(), "skip").clicked() {
                                            self.mapping[i] = None;
                                        }
                                        for (ci, c) in cols.iter().enumerate() {
                                            if ui.selectable_label(cur == Some(ci), c).clicked() {
                                                self.mapping[i] = Some(ci);
                                            }
                                        }
                                    });
                                }
                                None => {
                                    if let Some(nc) = self.new_cols.get_mut(i) {
                                        ui.horizontal(|ui| {
                                            ui.checkbox(&mut nc.0, "");
                                            ui.add_enabled_ui(nc.0, |ui| ui.add_sized([150.0, 22.0], egui::TextEdit::singleline(&mut nc.1)));
                                            ui.add_enabled_ui(nc.0, |ui| ui.add_sized([130.0, 22.0], egui::TextEdit::singleline(&mut nc.2).font(FontId::monospace(12.0))));
                                        });
                                    }
                                }
                            }
                            let samples: Vec<String> = self.data().iter().take(3).map(|r| r.get(i).cloned().unwrap_or_default().chars().take(24).collect()).collect();
                            ui.label(RichText::new(samples.join("  ·  ")).size(12.0).color(p.faint));
                            ui.end_row();
                        }
                    });
                });
                let plan = self.statements();
                if let Some(e) = &self.error {
                    ui.label(RichText::new(e).color(p.danger));
                }
                if let Err(e) = &plan {
                    ui.label(RichText::new(e).color(p.warning));
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(plan.is_ok(), egui::Button::new(RichText::new("Import").color(Color32::WHITE)).fill(p.accent)).clicked() {
                            if let Ok((stmts, summary)) = plan {
                                outcome = ImportOutcome::Run { stmts, summary };
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            outcome = ImportOutcome::Cancel;
                        }
                        ui.label(RichText::new("Runs in one transaction; if a row fails you can roll back.").size(11.5).color(p.faint));
                    });
                });
            });
        if reload {
            self.reload();
        } else if rebuild {
            self.rebuild();
        }
        if !open {
            outcome = ImportOutcome::Cancel;
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_create_and_inserts() {
        let path = std::env::temp_dir().join(format!("databug-csv-{}.csv", std::process::id()));
        std::fs::write(&path, "Supplier ID;Name;On time %\n1;Acme;97.5\n2;\"Delta; Fab\";\n").unwrap();
        let imp = CsvImport::new("c".into(), "public".into(), DbKind::Postgres, path, vec![], None);
        assert_eq!(imp.delimiter, b';');
        assert_eq!(imp.new_cols.iter().map(|c| c.1.as_str()).collect::<Vec<_>>(), vec!["supplier_id", "name", "on_time"]);
        assert_eq!(imp.new_cols[0].2, "bigint");
        assert_eq!(imp.new_cols[2].2, "double precision");
        let (stmts, summary) = imp.statements().unwrap();
        assert!(stmts[0].starts_with("CREATE TABLE public.suppliers") || stmts[0].starts_with("CREATE TABLE public."), "{}", stmts[0]);
        assert_eq!(stmts[1], "BEGIN");
        assert!(stmts[2].contains("(1, 'Acme', 97.5),\n(2, 'Delta; Fab', NULL)"), "{}", stmts[2]);
        assert_eq!(stmts.last().unwrap(), "COMMIT");
        assert_eq!(summary, format!("Imported 2 rows into {}", imp.new_name));
    }

    #[test]
    fn maps_existing_table_columns_by_name() {
        let path = std::env::temp_dir().join(format!("databug-csv2-{}.csv", std::process::id()));
        std::fs::write(&path, "NAME,ignored,id\nx,y,7\n").unwrap();
        let mut t = TableInfo::new("people", ObjectKind::Table);
        for (n, ty) in [("id", "integer"), ("name", "text")] {
            t.columns.push(ColumnInfo { name: n.into(), data_type: ty.into(), nullable: true, primary_key: false, default: None });
        }
        let imp = CsvImport::new("c".into(), "main".into(), DbKind::Sqlite, path, vec![t], Some("people".into()));
        assert_eq!(imp.mapping, vec![Some(1), None, Some(0)]);
        let (stmts, _) = imp.statements().unwrap();
        assert_eq!(stmts[1], "INSERT INTO people (name, id) VALUES\n('x', 7)");
    }
}
