//! The database explorer: connections → schemas → object groups → objects.
//! A custom tree so the selected row can be highlighted across its full
//! width and editable objects get an inline edit button.

use crate::icons::{self, Logos};
use crate::theme::{bold, connection_color, Palette};
use databug_core::{Catalog, ConnectionConfig, DbKind, ObjectKind, SchemaInfo, TableInfo};
use databug_drivers::SourceRef;
use databug_sql::rename::RenameTarget;
use eframe::egui::{self, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke};
use std::collections::{HashMap, HashSet};

pub enum Action {
    NewConsole(String),
    SelectTop(String, String, String),
    OpenDdl(String, String, String),
    Refresh(String),
    EditConn(String),
    DuplicateConn(String),
    DeleteConn(String),
    Copy(String),
    Insert(String),
    NewConnection,
    OpenSample,
    EditTable(String, String, String),
    NewTable(String, String),
    EditObject(String, String, SourceRef),
    DropObject(String, String, SourceRef),
    ErDiagram(String, String),
    Compare(String, String),
    ImportCsv(String, String, Option<String>),
    Rename(String, String, RenameTarget),
    /// Change which schemas the explorer shows (`None` = all).
    SetSchemas(String, Option<Vec<String>>),
}

#[derive(Default)]
pub struct ExplorerState {
    pub filter: String,
    open: HashSet<String>,
    pub selected: Option<String>,
    /// Open this connection's node on the next frame.
    pub reveal: Option<String>,
    /// Open the schema picker for this connection on the next frame.
    picker_request: Option<String>,
    picker_search: String,
}

impl ExplorerState {
    /// Open the schema picker for a connection (from a menu or command).
    pub fn request_picker(&mut self, conn_id: &str) {
        self.open.insert(format!("c:{conn_id}"));
        self.picker_request = Some(conn_id.to_string());
    }

    pub fn open_key(&mut self, key: impl Into<String>) {
        self.open.insert(key.into());
    }
}

pub struct CatalogView<'a> {
    pub catalog: &'a Catalog,
    pub loading: bool,
    pub error: Option<&'a str>,
    pub age: String,
}

struct RowOut {
    clicked: bool,
    double: bool,
    edit: bool,
    resp: egui::Response,
}

enum Glyph {
    Engine(DbKind, [u8; 3]),
    Schema,
    Group(ObjectKind),
    Kind(ObjectKind),
    Column(bool, bool),
    Index(bool),
    Folder,
}

const ROW_H: f32 = 22.0;

#[allow(clippy::too_many_arguments)]
fn row(ui: &mut egui::Ui, p: &Palette, st: &mut ExplorerState, logos: &Logos, key: &str, depth: usize, expandable: bool, glyph: Glyph, label: &str, detail: &str, editable: bool, strong: bool) -> RowOut {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, ROW_H), Sense::click());
    let selected = st.selected.as_deref() == Some(key);
    let open = st.open.contains(key);
    let painter = ui.painter_at(rect.expand2(egui::vec2(0.0, 0.0)));
    if selected {
        painter.rect_filled(rect, CornerRadius::same(5), p.tree_selected);
        painter.rect_filled(Rect::from_min_size(rect.min + egui::vec2(0.0, 3.0), egui::vec2(2.5, ROW_H - 6.0)), CornerRadius::same(1), p.accent);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(5), p.hover);
    }
    let x0 = rect.left() + 6.0 + depth as f32 * 14.0;
    let arrow = Rect::from_center_size(egui::pos2(x0 + 6.0, rect.center().y), egui::vec2(12.0, 12.0));
    let mut toggled = false;
    if expandable {
        icons::paint_arrow(&painter, arrow, open, if selected { p.text } else { p.muted });
        if let Some(pos) = resp.interact_pointer_pos() {
            if resp.clicked() && pos.x <= arrow.right() + 2.0 {
                toggled = true;
            }
        }
    }
    let icon = Rect::from_center_size(egui::pos2(x0 + 21.0, rect.center().y), egui::vec2(15.0, 15.0));
    match glyph {
        Glyph::Engine(kind, color) => {
            logos.paint(ui, icon.shrink(0.5), kind);
            // connection color dot, ringed so it stays separate from the logo
            let dot = egui::pos2(icon.right() + 1.5, icon.bottom() - 0.5);
            painter.circle_filled(dot, 3.6, p.island);
            painter.circle_filled(dot, 2.6, connection_color(color));
        }
        Glyph::Schema => {
            painter.add(egui::Shape::convex_polygon(
                vec![icon.center() + egui::vec2(0.0, -5.5), icon.center() + egui::vec2(5.0, 0.0), icon.center() + egui::vec2(0.0, 5.5), icon.center() + egui::vec2(-5.0, 0.0)],
                p.warning.gamma_multiply(0.85),
                Stroke::NONE,
            ));
        }
        Glyph::Group(k) | Glyph::Kind(k) => icons::paint_kind(&painter, icon, k, p),
        Glyph::Column(pk, fk) => icons::paint_column(&painter, icon, pk, fk, p),
        Glyph::Index(u) => icons::paint_index(&painter, icon, u, p),
        Glyph::Folder => {
            let r = Rect::from_center_size(icon.center(), egui::vec2(12.0, 9.0));
            painter.rect_stroke(r, CornerRadius::same(2), Stroke::new(1.2_f32, p.muted), egui::StrokeKind::Inside);
        }
    }
    // label and detail
    let show_edit = editable && (resp.hovered() || selected);
    let right_pad = if show_edit { 28.0 } else { 6.0 };
    let text_x = icon.right() + 7.0;
    let avail = (rect.right() - right_pad - text_x).max(10.0);
    let font = if strong { bold(13.0) } else { FontId::proportional(12.8) };
    let color = if strong || selected { p.text } else { Color32::from_rgba_unmultiplied(p.text.r(), p.text.g(), p.text.b(), 225) };
    let g = ui.painter().layout(label.to_string(), font, color, f32::INFINITY);
    let label_w = g.size().x.min(avail);
    painter.with_clip_rect(Rect::from_min_max(egui::pos2(text_x, rect.top()), egui::pos2(text_x + avail, rect.bottom()))).galley(egui::pos2(text_x, rect.center().y - g.size().y / 2.0), g, color);
    if !detail.is_empty() && label_w + 20.0 < avail {
        let dg = ui.painter().layout_no_wrap(detail.to_string(), FontId::proportional(11.0), p.faint);
        painter
            .with_clip_rect(Rect::from_min_max(egui::pos2(text_x + label_w + 7.0, rect.top()), egui::pos2(text_x + avail, rect.bottom())))
            .galley(egui::pos2(text_x + label_w + 7.0, rect.center().y - dg.size().y / 2.0), dg, p.faint);
    }
    // inline edit button
    let mut edit = false;
    if show_edit {
        let b = Rect::from_center_size(egui::pos2(rect.right() - 14.0, rect.center().y), egui::vec2(20.0, 18.0));
        let br = ui.interact(b, ui.id().with(("edit", key)), Sense::click());
        if br.hovered() {
            painter.rect_filled(b, CornerRadius::same(4), p.raised);
        }
        icons::paint_pencil(&painter, b, if br.hovered() { p.accent } else { p.muted });
        if br.clicked() {
            edit = true;
        }
        br.on_hover_text("Edit");
    }
    if toggled || (resp.double_clicked() && expandable) {
        if open {
            st.open.remove(key);
        } else {
            st.open.insert(key.to_string());
        }
    }
    if resp.clicked() || edit || resp.secondary_clicked() {
        st.selected = Some(key.to_string());
    }
    RowOut { clicked: resp.clicked() && !toggled, double: resp.double_clicked(), edit, resp }
}

fn is_open(st: &ExplorerState, key: &str, filtering: bool) -> bool {
    filtering || st.open.contains(key)
}

pub fn show(ui: &mut egui::Ui, p: &Palette, st: &mut ExplorerState, logos: &Logos, connections: &[ConnectionConfig], catalogs: &HashMap<String, CatalogView<'_>>) -> Vec<Action> {
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Database explorer").font(bold(13.0)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("+").on_hover_text("New connection").clicked() {
                actions.push(Action::NewConnection);
            }
        });
    });
    ui.add_space(2.0);
    ui.add(egui::TextEdit::singleline(&mut st.filter).hint_text("🔍  Filter objects").desired_width(f32::INFINITY));
    ui.add_space(4.0);
    if let Some(id) = st.reveal.take() {
        st.open.insert(format!("c:{id}"));
    }
    let filter = st.filter.to_lowercase();
    let filtering = !filter.is_empty();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        if connections.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No connections yet").color(p.muted));
                ui.add_space(6.0);
                if ui.button("New connection").clicked() {
                    actions.push(Action::NewConnection);
                }
                if ui.button("Open sample database").clicked() {
                    actions.push(Action::OpenSample);
                }
            });
        }
        for cfg in connections {
            let ck = format!("c:{}", cfg.id);
            let view = catalogs.get(&cfg.id);
            let detail = match cfg.kind {
                DbKind::Postgres => "postgres",
                DbKind::MySql => "mysql",
                DbKind::MsSql => "sql server",
                DbKind::Sqlite => "sqlite",
                DbKind::MongoDb => "mongodb",
            };
            let r = row(ui, p, st, logos, &ck, 0, true, Glyph::Engine(cfg.kind, cfg.color), &cfg.name, detail, false, true);
            if r.double {
                actions.push(Action::NewConsole(cfg.id.clone()));
            }
            if r.clicked && view.is_none() {
                actions.push(Action::Refresh(cfg.id.clone()));
            }
            let default_schema = view.as_ref().map(|v| v.catalog.default_schema.clone()).unwrap_or_default();
            let multi_schema = view.map(|v| v.catalog.schemas.len() > 1).unwrap_or(false);
            let mut st_picker = false;
            r.resp.on_hover_text(cfg.display_target()).context_menu(|ui| {
                for (label, a) in [
                    ("New console", Action::NewConsole(cfg.id.clone())),
                    ("Refresh schema", Action::Refresh(cfg.id.clone())),
                ] {
                    if ui.button(label).clicked() {
                        actions.push(a);
                        ui.close_menu();
                    }
                }
                if cfg.kind != DbKind::MongoDb {
                    ui.separator();
                    if ui.button("ER diagram").clicked() {
                        actions.push(Action::ErDiagram(cfg.id.clone(), default_schema.clone()));
                        ui.close_menu();
                    }
                    if ui.button("Compare schema with…").clicked() {
                        actions.push(Action::Compare(cfg.id.clone(), default_schema.clone()));
                        ui.close_menu();
                    }
                    if ui.button("New table…").clicked() {
                        actions.push(Action::NewTable(cfg.id.clone(), default_schema.clone()));
                        ui.close_menu();
                    }
                }
                if ui.button("Import CSV…").clicked() {
                    actions.push(Action::ImportCsv(cfg.id.clone(), default_schema.clone(), None));
                    ui.close_menu();
                }
                if multi_schema {
                    if ui.button("Choose schemas…").clicked() {
                        st_picker = true;
                        ui.close_menu();
                    }
                }
                ui.separator();
                if ui.button("Edit connection…").clicked() {
                    actions.push(Action::EditConn(cfg.id.clone()));
                    ui.close_menu();
                }
                if ui.button("Duplicate").clicked() {
                    actions.push(Action::DuplicateConn(cfg.id.clone()));
                    ui.close_menu();
                }
                if ui.button(RichText::new("Delete connection").color(p.danger)).clicked() {
                    actions.push(Action::DeleteConn(cfg.id.clone()));
                    ui.close_menu();
                }
            });
            if st_picker {
                st.open.insert(ck.clone());
                st.picker_request = Some(cfg.id.clone());
            }
            if !is_open(st, &ck, filtering) {
                continue;
            }
            let Some(v) = view else {
                ui.label(RichText::new("    Loading schema…").size(12.0).color(p.muted));
                continue;
            };
            if let Some(err) = v.error {
                ui.horizontal(|ui| {
                    ui.add_space(30.0);
                    ui.label(RichText::new(err.chars().take(140).collect::<String>()).size(11.5).color(p.danger));
                });
                ui.horizontal(|ui| {
                    ui.add_space(30.0);
                    if ui.small_button("Retry").clicked() {
                        actions.push(Action::Refresh(cfg.id.clone()));
                    }
                });
            }
            if v.catalog.schemas.is_empty() && v.loading {
                ui.label(RichText::new("      Loading schema…").size(12.0).color(p.muted));
            }
            let single = v.catalog.schemas.len() == 1;
            if !single && !v.catalog.schemas.is_empty() {
                schema_picker(ui, p, st, cfg, v.catalog, &mut actions);
            }
            for s in &v.catalog.schemas {
                if !single && !cfg.schema_visible(&s.name) {
                    continue;
                }
                if filtering && !schema_matches(s, &filter) {
                    continue;
                }
                let depth = if single {
                    1
                } else {
                    let sk = format!("s:{}:{}", cfg.id, s.name);
                    let is_default = s.name == v.catalog.default_schema;
                    if is_default && !st.open.contains(&format!("{sk}:seen")) {
                        st.open.insert(sk.clone());
                        st.open.insert(format!("{sk}:seen"));
                    }
                    let n = s.tables.len() + s.objects.len();
                    let r = row(ui, p, st, logos, &sk, 1, true, Glyph::Schema, &s.name, &if n == 0 { "empty".to_string() } else { n.to_string() }, false, false);
                    schema_menu(r.resp, cfg, &s.name, &mut actions);
                    if !is_open(st, &sk, filtering) {
                        continue;
                    }
                    2
                };
                schema_body(ui, p, st, logos, cfg, s, depth, &filter, &mut actions);
            }
            if v.catalog.introspected_at > 0 && !filtering {
                ui.horizontal(|ui| {
                    ui.add_space(30.0);
                    ui.label(RichText::new(format!("{} objects · {}", v.catalog.table_count(), v.age)).size(10.5).color(p.faint));
                });
                ui.add_space(2.0);
            }
        }
    });
    actions
}

/// The "3 of 12 schemas" row under a connection, and its checkbox popup —
/// DataGrip's schema selection.
fn schema_picker(ui: &mut egui::Ui, p: &Palette, st: &mut ExplorerState, cfg: &ConnectionConfig, cat: &Catalog, actions: &mut Vec<Action>) {
    let total = cat.schemas.len();
    let shown = cat.schemas.iter().filter(|s| cfg.schema_visible(&s.name)).count();
    let noun = if cfg.kind == DbKind::MySql { "databases" } else { "schemas" };
    let text = if cfg.visible_schemas.is_none() { format!("All {total} {noun}") } else { format!("{shown} of {total} {noun}") };
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 20.0), Sense::click());
    let popup_id = ui.id().with(("schema-picker", &cfg.id));
    let painter = ui.painter_at(rect);
    let open = ui.memory(|m| m.is_popup_open(popup_id));
    let x = rect.left() + 26.0;
    let color = if resp.hovered() || open { p.accent } else { p.muted };
    let g = painter.layout_no_wrap(text, FontId::proportional(11.5), color);
    let pill = Rect::from_min_size(egui::pos2(x - 6.0, rect.center().y - g.size().y / 2.0 - 2.0), g.size() + egui::vec2(26.0, 4.0));
    if resp.hovered() || open {
        painter.rect_filled(pill, CornerRadius::same(4), p.hover);
    }
    let arrow = Rect::from_center_size(egui::pos2(x + g.size().x + 9.0, rect.center().y), egui::vec2(10.0, 10.0));
    painter.galley(egui::pos2(x, rect.center().y - g.size().y / 2.0), g, color);
    icons::paint_arrow(&painter, arrow, true, color);
    let resp = resp.on_hover_text(format!("Choose which {noun} the explorer shows"));
    if resp.clicked() || st.picker_request.as_deref() == Some(cfg.id.as_str()) {
        st.picker_request = None;
        st.picker_search.clear();
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }
    egui::popup_below_widget(ui, popup_id, &resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.set_min_width(250.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Show {noun}")).font(bold(12.5)));
        });
        ui.add_space(2.0);
        if total > 8 {
            ui.add(egui::TextEdit::singleline(&mut st.picker_search).hint_text("Filter").desired_width(f32::INFINITY));
        }
        ui.horizontal(|ui| {
            if ui.small_button("All").clicked() {
                actions.push(Action::SetSchemas(cfg.id.clone(), None));
            }
            if ui.small_button("Default only").on_hover_text(format!("Only {}", cat.default_schema)).clicked() {
                actions.push(Action::SetSchemas(cfg.id.clone(), Some(vec![cat.default_schema.clone()])));
            }
            if ui.small_button("Non-empty").on_hover_text("Hide schemas with no objects").clicked() {
                let list = cat.schemas.iter().filter(|s| (s.tables.len() + s.objects.len()) > 0 || s.name == cat.default_schema).map(|s| s.name.clone()).collect();
                actions.push(Action::SetSchemas(cfg.id.clone(), Some(list)));
            }
        });
        ui.separator();
        let search = st.picker_search.to_lowercase();
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            for s in &cat.schemas {
                if !search.is_empty() && !s.name.to_lowercase().contains(&search) {
                    continue;
                }
                let mut on = cfg.schema_visible(&s.name);
                let n = s.tables.len() + s.objects.len();
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut on, &s.name).changed() {
                        let mut list: Vec<String> = cat.schemas.iter().filter(|x| cfg.schema_visible(&x.name)).map(|x| x.name.clone()).collect();
                        if on {
                            list.push(s.name.clone());
                        } else {
                            list.retain(|x| x != &s.name);
                        }
                        // everything ticked is the same as "all", which also picks up new schemas
                        let all = cat.schemas.iter().all(|x| list.contains(&x.name));
                        actions.push(Action::SetSchemas(cfg.id.clone(), if all { None } else { Some(list) }));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if s.name == cat.default_schema { format!("default · {n}") } else { n.to_string() };
                        ui.label(RichText::new(label).size(11.0).color(p.faint));
                    });
                });
            }
        });
    });
}

fn schema_matches(s: &SchemaInfo, f: &str) -> bool {
    s.tables.iter().any(|t| t.name.to_lowercase().contains(f)) || s.objects.iter().any(|o| o.name.to_lowercase().contains(f))
}

fn schema_menu(resp: egui::Response, cfg: &ConnectionConfig, schema: &str, actions: &mut Vec<Action>) {
    resp.context_menu(|ui| {
        if cfg.kind != DbKind::MongoDb {
            if ui.button("New table…").clicked() {
                actions.push(Action::NewTable(cfg.id.clone(), schema.to_string()));
                ui.close_menu();
            }
            if ui.button("ER diagram").clicked() {
                actions.push(Action::ErDiagram(cfg.id.clone(), schema.to_string()));
                ui.close_menu();
            }
            if ui.button("Compare with…").clicked() {
                actions.push(Action::Compare(cfg.id.clone(), schema.to_string()));
                ui.close_menu();
            }
        }
        if ui.button("Import CSV…").clicked() {
            actions.push(Action::ImportCsv(cfg.id.clone(), schema.to_string(), None));
            ui.close_menu();
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn schema_body(ui: &mut egui::Ui, p: &Palette, st: &mut ExplorerState, logos: &Logos, cfg: &ConnectionConfig, s: &SchemaInfo, depth: usize, filter: &str, actions: &mut Vec<Action>) {
    let filtering = !filter.is_empty();
    for kind in ObjectKind::DISPLAY_ORDER {
        let n = s.count(kind);
        if n == 0 {
            continue;
        }
        let gk = format!("g:{}:{}:{:?}", cfg.id, s.name, kind);
        if kind == ObjectKind::Table && !st.open.contains(&format!("{gk}:seen")) {
            st.open.insert(gk.clone());
            st.open.insert(format!("{gk}:seen"));
        }
        let matching: usize = if kind.is_relation() {
            s.tables.iter().filter(|t| t.kind == kind && t.name.to_lowercase().contains(filter)).count()
        } else {
            s.objects.iter().filter(|o| o.kind == kind && o.name.to_lowercase().contains(filter)).count()
        };
        if filtering && matching == 0 {
            continue;
        }
        let r = row(ui, p, st, logos, &gk, depth, true, Glyph::Group(kind), kind.group_label(), &n.to_string(), false, false);
        if kind == ObjectKind::Table && cfg.kind != DbKind::MongoDb {
            r.resp.context_menu(|ui| {
                if ui.button("New table…").clicked() {
                    actions.push(Action::NewTable(cfg.id.clone(), s.name.clone()));
                    ui.close_menu();
                }
            });
        }
        if !is_open(st, &gk, filtering) {
            continue;
        }
        if kind.is_relation() {
            for t in s.tables.iter().filter(|t| t.kind == kind && (!filtering || t.name.to_lowercase().contains(filter))) {
                table_node(ui, p, st, logos, cfg, &s.name, s, t, depth + 1, actions);
            }
        } else {
            for o in s.objects.iter().filter(|o| o.kind == kind && (!filtering || o.name.to_lowercase().contains(filter))) {
                let key = format!("o:{}:{}:{:?}:{}:{}:{}", cfg.id, s.name, o.kind, o.name, o.detail, o.table.as_deref().unwrap_or(""));
                let detail = match o.kind {
                    ObjectKind::Procedure | ObjectKind::Function | ObjectKind::TableFunction | ObjectKind::Aggregate if !o.detail.is_empty() => format!("({})", o.detail),
                    _ => o.detail.clone(),
                };
                let r = row(ui, p, st, logos, &key, depth + 1, false, Glyph::Kind(o.kind), &o.name, &detail, o.kind.has_source(), false);
                let sref = SourceRef::from_object(o);
                if r.edit || r.double {
                    actions.push(Action::EditObject(cfg.id.clone(), s.name.clone(), sref.clone()));
                }
                let name = o.name.clone();
                r.resp.on_hover_text(format!("{} {}.{}{}", o.kind.label(), s.name, o.name, if o.detail.is_empty() { String::new() } else { format!(" {}", detail) })).context_menu(|ui| {
                    if ui.button(if o.kind.is_clr() { "View definition" } else { "Edit definition" }).clicked() {
                        actions.push(Action::EditObject(cfg.id.clone(), s.name.clone(), sref.clone()));
                        ui.close_menu();
                    }
                    if ui.button("Copy name").clicked() {
                        actions.push(Action::Copy(name.clone()));
                        ui.close_menu();
                    }
                    if ui.button("Generate DROP").clicked() {
                        actions.push(Action::DropObject(cfg.id.clone(), s.name.clone(), sref.clone()));
                        ui.close_menu();
                    }
                });
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn table_node(ui: &mut egui::Ui, p: &Palette, st: &mut ExplorerState, logos: &Logos, cfg: &ConnectionConfig, schema: &str, s: &SchemaInfo, t: &TableInfo, depth: usize, actions: &mut Vec<Action>) {
    let key = format!("t:{}:{}:{}", cfg.id, schema, t.name);
    let editable = cfg.kind != DbKind::MongoDb || t.kind == ObjectKind::View;
    let r = row(ui, p, st, logos, &key, depth, true, Glyph::Kind(t.kind), &t.name, "", editable, false);
    let edit_action = || match t.kind {
        ObjectKind::Table => Action::EditTable(cfg.id.clone(), schema.to_string(), t.name.clone()),
        k => Action::EditObject(cfg.id.clone(), schema.to_string(), SourceRef::relation(&t.name, k)),
    };
    if r.edit {
        actions.push(edit_action());
    }
    if r.double {
        actions.push(Action::SelectTop(cfg.id.clone(), schema.to_string(), t.name.clone()));
    }
    let sql = cfg.kind != DbKind::MongoDb;
    r.resp
        .on_hover_text(format!("{} {}.{} · {} columns\nDouble-click to open the first 100 rows", t.kind.label(), schema, t.name, t.columns.len()))
        .context_menu(|ui| {
            if ui.button("Open first 100 rows").clicked() {
                actions.push(Action::SelectTop(cfg.id.clone(), schema.to_string(), t.name.clone()));
                ui.close_menu();
            }
            if editable && ui.button(if t.kind == ObjectKind::Table { "Modify table…" } else { "Edit definition" }).clicked() {
                actions.push(edit_action());
                ui.close_menu();
            }
            if sql && t.kind == ObjectKind::Table {
                if ui.button("Open DDL").clicked() {
                    actions.push(Action::OpenDdl(cfg.id.clone(), schema.to_string(), t.name.clone()));
                    ui.close_menu();
                }
                if ui.button("Rename…").clicked() {
                    actions.push(Action::Rename(cfg.id.clone(), schema.to_string(), RenameTarget::Table { schema: Some(schema.to_string()), name: t.name.clone() }));
                    ui.close_menu();
                }
            }
            if matches!(t.kind, ObjectKind::Table | ObjectKind::Collection) && ui.button("Import CSV…").clicked() {
                actions.push(Action::ImportCsv(cfg.id.clone(), schema.to_string(), Some(t.name.clone())));
                ui.close_menu();
            }
            if sql && ui.button("ER diagram of schema").clicked() {
                actions.push(Action::ErDiagram(cfg.id.clone(), schema.to_string()));
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Copy name").clicked() {
                actions.push(Action::Copy(t.name.clone()));
                ui.close_menu();
            }
            if ui.button("Insert name into console").clicked() {
                actions.push(Action::Insert(cfg.kind.dialect().quote_ident(&t.name)));
                ui.close_menu();
            }
            if sql && ui.button(RichText::new(format!("Generate DROP {}", t.kind.label().to_uppercase())).color(p.danger)).clicked() {
                actions.push(Action::DropObject(cfg.id.clone(), schema.to_string(), SourceRef::relation(&t.name, t.kind)));
                ui.close_menu();
            }
        });
    if !st.open.contains(&key) {
        return;
    }
    for c in &t.columns {
        let ckey = format!("{key}:col:{}", c.name);
        let detail = format!("{}{}", c.data_type, if c.nullable || c.primary_key { "" } else { " not null" });
        let r = row(ui, p, st, logos, &ckey, depth + 1, false, Glyph::Column(c.primary_key, t.is_fk_column(&c.name)), &c.name, &detail, false, false);
        if r.double {
            actions.push(Action::Insert(cfg.kind.dialect().quote_ident(&c.name)));
        }
        let col = c.name.clone();
        r.resp
            .on_hover_text(format!("{} {}{}{}", c.name, c.data_type, if c.nullable { "" } else { " NOT NULL" }, c.default.as_ref().map(|d| format!(" DEFAULT {d}")).unwrap_or_default()))
            .context_menu(|ui| {
                if ui.button("Insert into console").clicked() {
                    actions.push(Action::Insert(cfg.kind.dialect().quote_ident(&col)));
                    ui.close_menu();
                }
                if sql && t.kind == ObjectKind::Table && ui.button("Rename…").clicked() {
                    actions.push(Action::Rename(cfg.id.clone(), schema.to_string(), RenameTarget::Column { schema: Some(schema.to_string()), table: t.name.clone(), column: col.clone() }));
                    ui.close_menu();
                }
                if ui.button("Copy name").clicked() {
                    actions.push(Action::Copy(col.clone()));
                    ui.close_menu();
                }
            });
    }
    let secondary: Vec<_> = t.indexes.iter().filter(|i| !i.primary || t.kind == ObjectKind::Collection).collect();
    if !secondary.is_empty() {
        let k = format!("{key}:idx");
        row(ui, p, st, logos, &k, depth + 1, true, Glyph::Folder, "indexes", &secondary.len().to_string(), false, false);
        if st.open.contains(&k) {
            for i in secondary {
                row(ui, p, st, logos, &format!("{k}:{}", i.name), depth + 2, false, Glyph::Index(i.unique), &i.name, &format!("({}){}", i.columns.join(", "), if i.unique { " unique" } else { "" }), false, false);
            }
        }
    }
    if !t.foreign_keys.is_empty() {
        let k = format!("{key}:fk");
        row(ui, p, st, logos, &k, depth + 1, true, Glyph::Folder, "foreign keys", &t.foreign_keys.len().to_string(), false, false);
        if st.open.contains(&k) {
            for f in &t.foreign_keys {
                row(ui, p, st, logos, &format!("{k}:{}", f.name), depth + 2, false, Glyph::Column(false, true), &f.name, &format!("({}) → {}({})", f.columns.join(", "), f.ref_table, f.ref_columns.join(", ")), false, false);
            }
        }
    }
    let triggers: Vec<_> = s.objects.iter().filter(|o| matches!(o.kind, ObjectKind::Trigger | ObjectKind::ClrTrigger) && o.table.as_deref() == Some(t.name.as_str())).collect();
    if !triggers.is_empty() {
        let k = format!("{key}:trg");
        row(ui, p, st, logos, &k, depth + 1, true, Glyph::Group(ObjectKind::Trigger), "triggers", &triggers.len().to_string(), false, false);
        if st.open.contains(&k) {
            for o in triggers {
                let r = row(ui, p, st, logos, &format!("{k}:{}", o.name), depth + 2, false, Glyph::Kind(o.kind), &o.name, "", true, false);
                if r.edit || r.double {
                    actions.push(Action::EditObject(cfg.id.clone(), schema.to_string(), SourceRef::from_object(o)));
                }
            }
        }
    }
}
