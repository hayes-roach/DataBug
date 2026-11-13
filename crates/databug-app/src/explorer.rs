//! The database explorer: connections → schemas → object groups → objects.
//! A custom tree so the selected row can be highlighted across its full
//! width and editable objects get an inline edit button.

use crate::icons::{self, Logos};
use crate::theme::{bold, connection_color, Palette};
use databug_core::{Catalog, ConnectionConfig, DbKind, DbObject, ObjectKind, SchemaInfo, TableInfo};
use databug_drivers::SourceRef;
use databug_sql::rename::RenameTarget;
use eframe::egui::{self, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub enum Action {
    NewConsole(String),
    SelectTop(String, String, String),
    OpenDdl(String, String, String),
    Refresh(String),
    /// Sign in again after a failed attempt (only ever from a click).
    Retry(String),
    EditConn(String),
    DuplicateConn(String),
    DeleteConn(String),
    Copy(String),
    Insert(String),
    NewConnection,
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
    /// Change which databases of a server the explorer shows (`None` = only the connection's own).
    SetDatabases(String, Option<Vec<String>>),
    /// Fetch the list of databases on a connection's server.
    LoadDatabases(String),
    /// Close every session of a connection but keep it in the list.
    Disconnect(String),
    Connect(String),
    /// Move a connection to this position in the visible list (drag and drop).
    MoveConn(String, usize),
    /// Open an EXEC / CALL / SELECT template for a routine.
    ExecRoutine(String, String, DbObject),
    /// Open "Find in database".
    SearchDatabase,
    SwitchProject(Option<String>),
    /// Create (id `None`) or rename a project.
    SaveProject(Option<String>, String),
    DeleteProject(String),
    AddToProject(String, String),
    RemoveFromProject(String),
}

/// Databases on a connection's server, as listed by the driver.
pub struct DbList {
    pub names: Vec<String>,
    pub current: String,
    pub loading: bool,
    pub error: Option<String>,
}

/// Everything about connections the explorer shows beyond their config.
pub struct ExplorerCtx<'a> {
    /// Connections with an open session.
    pub connected: &'a HashSet<String>,
    /// Connections the user disconnected.
    pub disconnected: &'a HashSet<String>,
    /// Connections whose last sign-in failed, with the error.
    pub failed: &'a HashMap<String, String>,
    pub databases: &'a HashMap<String, DbList>,
    pub projects: &'a [crate::store::Project],
    pub current_project: Option<&'a str>,
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
    /// Open the database picker for this connection on the next frame.
    db_picker_request: Option<String>,
    db_picker_search: String,
    /// Connection being dragged to a new position.
    dragging: Option<String>,
    /// Project name being edited: (project id or None for a new one, name).
    project_edit: Option<(Option<String>, String)>,
    scroll_to_selected: bool,
    /// Indexes of each group's objects (all, and those matching the filter),
    /// built once per catalog load instead of on every frame.
    groups: HashMap<String, Rc<Vec<u32>>>,
    groups_filter: String,
    /// Lay out every row, even off-screen ones (tests compare the two).
    full_layout: bool,
    /// Height of the tree's content and its scroll offset in the last frame.
    content_height: f32,
    scroll_y: f32,
}

impl ExplorerState {
    /// Open the schema picker for a connection (from a menu or command).
    pub fn request_picker(&mut self, conn_id: &str) {
        self.open.insert(format!("c:{conn_id}"));
        self.picker_request = Some(conn_id.to_string());
    }

    /// Indexes into `s.tables` (relation kinds) or `s.objects` of the objects
    /// of `kind` whose name contains `filter` (already lower case).
    fn group_items(&mut self, conn: &str, generation: u64, s: &SchemaInfo, kind: ObjectKind, filter: &str) -> Rc<Vec<u32>> {
        if self.groups_filter != filter || self.groups.len() > 4096 {
            self.groups.clear();
            self.groups_filter = filter.to_string();
        }
        // the generation changes on every catalog load; the counts are a backstop
        let key = format!("{conn}\u{1}{generation}\u{1}{}\u{1}{}\u{1}{}\u{1}{kind:?}\u{1}{filter}", s.name, s.tables.len(), s.objects.len());
        self.groups
            .entry(key)
            .or_insert_with(|| {
                let keep = |name: &str| filter.is_empty() || name.to_lowercase().contains(filter);
                Rc::new(if kind.is_relation() {
                    s.tables.iter().enumerate().filter(|(_, t)| t.kind == kind && keep(&t.name)).map(|(i, _)| i as u32).collect()
                } else {
                    s.objects.iter().enumerate().filter(|(_, o)| o.kind == kind && keep(&o.name)).map(|(i, _)| i as u32).collect()
                })
            })
            .clone()
    }

    fn schema_matches(&mut self, conn: &str, generation: u64, s: &SchemaInfo, filter: &str) -> bool {
        ObjectKind::DISPLAY_ORDER.iter().any(|&k| !self.group_items(conn, generation, s, k, filter).is_empty())
    }

    pub fn open_key(&mut self, key: impl Into<String>) {
        self.open.insert(key.into());
    }

    /// Expand the path to an object and select it.
    pub fn reveal_object(&mut self, conn_id: &str, base_id: &str, schema: &str, group: Option<ObjectKind>, key: String) {
        self.open.insert(format!("c:{base_id}"));
        if conn_id != base_id {
            if let Some((_, db)) = conn_id.split_once('@') {
                self.open.insert(format!("d:{base_id}:{db}"));
            }
        } else {
            self.open.insert(format!("d:{base_id}:"));
        }
        self.open.insert(format!("s:{conn_id}:{schema}"));
        if let Some(k) = group {
            self.open.insert(format!("g:{conn_id}:{schema}:{k:?}"));
        }
        self.selected = Some(key);
        self.scroll_to_selected = true;
    }
}

pub struct CatalogView<'a> {
    pub catalog: &'a Catalog,
    /// Bumped on every catalog load.
    pub generation: u64,
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
    Database,
    Schema,
    Group(ObjectKind),
    Kind(ObjectKind),
    Column(bool, bool),
    Index(bool),
    Folder,
}

const ROW_H: f32 = 22.0;

/// Leaves out rows that are scrolled out of view: instead of laying them
/// out, it adds their height as empty space, so the scroll bar and the
/// position of everything else stay the same. Large databases have tens of
/// thousands of rows; only the few dozen on screen are built each frame.
struct Skipper {
    on: bool,
    pitch: f32,
    pending: f32,
}

impl Skipper {
    fn new(ui: &egui::Ui, st: &ExplorerState) -> Self {
        // revealing an object needs its row laid out to scroll to it
        Skipper { on: !st.scroll_to_selected && !st.full_layout, pitch: ROW_H + ui.spacing().item_spacing.y, pending: 0.0 }
    }

    /// Whether a block of `rows` rows would be on screen. If not, its
    /// height is set aside and the caller skips it.
    fn visible(&mut self, ui: &mut egui::Ui, rows: usize) -> bool {
        if !self.on {
            return true;
        }
        let h = rows as f32 * self.pitch;
        let top = ui.cursor().top() + self.pending;
        let clip = ui.clip_rect();
        if top + h < clip.top() - 2.0 * ROW_H || top > clip.bottom() + 2.0 * ROW_H {
            self.pending += h;
            false
        } else {
            self.finish(ui);
            true
        }
    }

    fn finish(&mut self, ui: &mut egui::Ui) {
        if self.pending > 0.0 {
            ui.add_space(self.pending);
            self.pending = 0.0;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn row(ui: &mut egui::Ui, p: &Palette, st: &mut ExplorerState, logos: &Logos, key: &str, depth: usize, expandable: bool, glyph: Glyph, label: &str, detail: &str, editable: bool, strong: bool) -> RowOut {
    row_ex(ui, p, st, logos, key, depth, expandable, glyph, label, detail, editable, strong, Sense::click(), false)
}

#[allow(clippy::too_many_arguments)]
fn row_ex(ui: &mut egui::Ui, p: &Palette, st: &mut ExplorerState, logos: &Logos, key: &str, depth: usize, expandable: bool, glyph: Glyph, label: &str, detail: &str, editable: bool, strong: bool, sense: Sense, dim: bool) -> RowOut {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, ROW_H), sense);
    if st.scroll_to_selected && st.selected.as_deref() == Some(key) {
        resp.scroll_to_me(Some(egui::Align::Center));
        st.scroll_to_selected = false;
    }
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
        Glyph::Database => {
            let c = icon.center();
            let col = p.syn_function;
            painter.rect_stroke(Rect::from_center_size(c, egui::vec2(10.0, 11.0)), CornerRadius::same(3), Stroke::new(1.3_f32, col), egui::StrokeKind::Inside);
            painter.line_segment([c + egui::vec2(-5.0, -1.5), c + egui::vec2(5.0, -1.5)], Stroke::new(1.0_f32, col));
            painter.line_segment([c + egui::vec2(-5.0, 1.8), c + egui::vec2(5.0, 1.8)], Stroke::new(1.0_f32, col));
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
    let color = if dim {
        p.muted
    } else if strong || selected {
        p.text
    } else {
        Color32::from_rgba_unmultiplied(p.text.r(), p.text.g(), p.text.b(), 225)
    };
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

pub fn show(ui: &mut egui::Ui, p: &Palette, st: &mut ExplorerState, logos: &Logos, connections: &[ConnectionConfig], catalogs: &HashMap<String, CatalogView<'_>>, cx: &ExplorerCtx<'_>) -> Vec<Action> {
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Database explorer").font(bold(13.0)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if icon_button(ui, p, HeaderIcon::Plus, true).on_hover_text("New connection").clicked() {
                actions.push(Action::NewConnection);
            }
            if icon_button(ui, p, HeaderIcon::Search, false).on_hover_text("Find in database: tables, columns, routines, definitions and data  (Ctrl/⌘ + F outside the editor)").clicked() {
                actions.push(Action::SearchDatabase);
            }
        });
    });
    ui.add_space(4.0);
    project_bar(ui, p, st, cx, &mut actions);
    ui.add_space(4.0);
    ui.add(egui::TextEdit::singleline(&mut st.filter).hint_text("🔍  Filter objects").desired_width(f32::INFINITY));
    ui.add_space(4.0);
    if let Some(id) = st.reveal.take() {
        st.open.insert(format!("c:{}", ConnectionConfig::base_id(&id)));
    }
    let filter = st.filter.to_lowercase();
    let filtering = !filter.is_empty();
    let out = egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        if connections.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                if cx.current_project.is_some() {
                    ui.label(RichText::new("No connections in this project").color(p.muted));
                    ui.label(RichText::new("New connections are added to the current project").size(11.0).color(p.faint));
                } else {
                    ui.label(RichText::new("No connections yet").color(p.muted));
                }
                ui.add_space(6.0);
                if ui.button("New connection").clicked() {
                    actions.push(Action::NewConnection);
                }
            });
        }
        let mut headers: Vec<Rect> = Vec::new();
        for cfg in connections {
            let ck = format!("c:{}", cfg.id);
            let view = catalogs.get(&cfg.id);
            let disconnected = cx.disconnected.contains(&cfg.id);
            let failed = cx.failed.get(&cfg.id);
            let connected = cx.connected.contains(&cfg.id);
            let engine = match cfg.kind {
                DbKind::Postgres => "postgres",
                DbKind::MySql => "mysql",
                DbKind::MsSql => "sql server",
                DbKind::Sqlite => "sqlite",
                DbKind::MongoDb => "mongodb",
                k => k.label(),
            };
            let detail = if disconnected {
                "disconnected".to_string()
            } else if failed.is_some() {
                "sign-in failed".to_string()
            } else if connected {
                format!("{engine} · connected")
            } else {
                engine.to_string()
            };
            let sense = if filtering { Sense::click() } else { Sense::click_and_drag() };
            let r = row_ex(ui, p, st, logos, &ck, 0, true, Glyph::Engine(cfg.kind, cfg.color), &cfg.name, &detail, false, true, sense, disconnected);
            headers.push(r.resp.rect);
            if connected && !disconnected {
                // green "live" dot at the right edge of the row
                let c = egui::pos2(r.resp.rect.right() - 9.0, r.resp.rect.center().y);
                ui.painter().circle_filled(c, 3.0, p.success);
            }
            if r.resp.drag_started() {
                st.dragging = Some(cfg.id.clone());
            }
            if r.double {
                actions.push(if disconnected { Action::Connect(cfg.id.clone()) } else { Action::NewConsole(cfg.id.clone()) });
            }
            if r.clicked && view.is_none() && !disconnected && failed.is_none() {
                actions.push(Action::Refresh(cfg.id.clone()));
            }
            let dbs = cx.databases.get(&cfg.id);
            let multi_db = multi_db(cfg, dbs);
            let primary = primary_db(cfg, dbs);
            let default_schema = view.as_ref().map(|v| v.catalog.default_schema.clone()).unwrap_or_default();
            let multi_schema = view.map(|v| v.catalog.schemas.len() > 1).unwrap_or(false);
            let mut st_picker = false;
            let mut db_picker = false;
            let hover = if disconnected { format!("{}\nDisconnected — double-click or right-click → Connect", cfg.display_target()) } else { format!("{}\nDrag to reorder", cfg.display_target()) };
            r.resp.on_hover_text(hover).context_menu(|ui| {
                if disconnected {
                    if ui.button(RichText::new("Connect").color(p.success)).clicked() {
                        actions.push(Action::Connect(cfg.id.clone()));
                        ui.close_menu();
                    }
                    ui.separator();
                }
                for (label, a) in [("New console", Action::NewConsole(cfg.id.clone())), ("Refresh schema", Action::Retry(cfg.id.clone()))] {
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
                if cfg.supports_databases() && ui.button("Choose databases…").clicked() {
                    db_picker = true;
                    ui.close_menu();
                }
                if multi_schema && !multi_db && ui.button("Choose schemas…").clicked() {
                    st_picker = true;
                    ui.close_menu();
                }
                ui.separator();
                if !disconnected && ui.button("Disconnect").on_hover_text("Close its sessions; the connection stays in the list").clicked() {
                    actions.push(Action::Disconnect(cfg.id.clone()));
                    ui.close_menu();
                }
                if ui.button("Edit connection…").clicked() {
                    actions.push(Action::EditConn(cfg.id.clone()));
                    ui.close_menu();
                }
                if ui.button("Duplicate").clicked() {
                    actions.push(Action::DuplicateConn(cfg.id.clone()));
                    ui.close_menu();
                }
                if !cx.projects.is_empty() {
                    ui.menu_button("Add to project", |ui| {
                        for pr in cx.projects {
                            let has = pr.connections.iter().any(|c| c == &cfg.id);
                            if ui.add_enabled(!has, egui::Button::new(&pr.name)).clicked() {
                                actions.push(Action::AddToProject(cfg.id.clone(), pr.id.clone()));
                                ui.close_menu();
                            }
                        }
                    });
                }
                if cx.current_project.is_some() && ui.button("Remove from project").on_hover_text("Keeps the connection; it stays in “All connections”").clicked() {
                    actions.push(Action::RemoveFromProject(cfg.id.clone()));
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
            if db_picker {
                st.open.insert(ck.clone());
                st.db_picker_request = Some(cfg.id.clone());
                if dbs.map(|d| d.names.is_empty() && !d.loading).unwrap_or(true) && failed.is_none() {
                    actions.push(Action::LoadDatabases(cfg.id.clone()));
                }
            }
            if !is_open(st, &ck, filtering) {
                continue;
            }
            if let (Some(err), false) = (failed, disconnected) {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(30.0);
                    ui.label(RichText::new(err.chars().take(160).collect::<String>()).size(11.5).color(p.danger));
                });
                ui.horizontal(|ui| {
                    ui.add_space(30.0);
                    ui.label(RichText::new("Not retrying automatically").size(11.0).color(p.faint)).on_hover_text("Repeated failed sign-ins can lock an account, so DataBug waits for you. Fix the settings or retry when ready.");
                    if ui.small_button("Retry").clicked() {
                        actions.push(Action::Retry(cfg.id.clone()));
                    }
                    if ui.small_button("Edit…").clicked() {
                        actions.push(Action::EditConn(cfg.id.clone()));
                    }
                });
                ui.add_space(2.0);
                continue;
            }
            if disconnected {
                ui.horizontal(|ui| {
                    ui.add_space(30.0);
                    ui.label(RichText::new("Disconnected").size(11.5).color(p.faint));
                    if ui.small_button("Connect").clicked() {
                        actions.push(Action::Connect(cfg.id.clone()));
                    }
                });
                ui.add_space(2.0);
                continue;
            }
            if cfg.supports_databases() {
                db_picker_row(ui, p, st, cfg, dbs, &primary, &mut actions);
            }
            if !multi_db {
                conn_body(ui, p, st, logos, cfg, view, 0, &filter, &mut actions);
                continue;
            }
            // one node per database: the connection's own first, then the others
            let mut list = vec![primary.clone()];
            for d in cfg.visible_databases.iter().flatten() {
                if !list.iter().any(|x| x.eq_ignore_ascii_case(d)) {
                    list.push(d.clone());
                }
            }
            for db in list {
                let is_primary = db == primary;
                let dk = format!("d:{}:{}", cfg.id, if is_primary { "" } else { db.as_str() });
                let vcfg = if is_primary { cfg.clone() } else { cfg.for_database(&db) };
                let vview = catalogs.get(&vcfg.id);
                if is_primary && !st.open.contains(&format!("{dk}:seen")) {
                    st.open.insert(dk.clone());
                    st.open.insert(format!("{dk}:seen"));
                }
                if filtering && !vview.map(|v| v.catalog.schemas.iter().any(|s| st.schema_matches(&vcfg.id, v.generation, s, &filter))).unwrap_or(false) {
                    continue;
                }
                let detail = match vview {
                    Some(v) if v.loading => "loading…".to_string(),
                    Some(v) => format!("{}{}", if is_primary { "default · " } else { "" }, v.catalog.table_count()),
                    None if is_primary => "default".to_string(),
                    None => String::new(),
                };
                let label = if db.is_empty() { "(default database)".to_string() } else { db.clone() };
                let r = row(ui, p, st, logos, &dk, 1, true, Glyph::Database, &label, &detail, false, false);
                if (r.clicked || r.double) && vview.is_none() && failed.is_none() {
                    actions.push(Action::Refresh(vcfg.id.clone()));
                }
                let vid = vcfg.id.clone();
                let dbname = db.clone();
                let base = cfg.id.clone();
                let visible = cfg.visible_databases.clone().unwrap_or_default();
                r.resp.context_menu(|ui| {
                    if ui.button("New console").clicked() {
                        actions.push(Action::NewConsole(vid.clone()));
                        ui.close_menu();
                    }
                    if ui.button("Refresh schema").clicked() {
                        actions.push(Action::Retry(vid.clone()));
                        ui.close_menu();
                    }
                    if !is_primary && ui.button("Hide database").clicked() {
                        let rest: Vec<String> = visible.iter().filter(|d| **d != dbname).cloned().collect();
                        actions.push(Action::SetDatabases(base.clone(), if rest.is_empty() { None } else { Some(rest) }));
                        ui.close_menu();
                    }
                });
                if !is_open(st, &dk, filtering) {
                    continue;
                }
                if vview.is_none() && failed.is_none() && !st.open.contains(&format!("{dk}:req")) {
                    // opened for the first time: load its schema
                    st.open.insert(format!("{dk}:req"));
                    actions.push(Action::Refresh(vcfg.id.clone()));
                }
                conn_body(ui, p, st, logos, &vcfg, vview, 1, &filter, &mut actions);
            }
        }
        // drag and drop to reorder connections
        if let Some(d) = st.dragging.clone() {
            let released = ui.input(|i| i.pointer.any_released() || !i.pointer.any_down());
            if let Some(pos) = ui.ctx().pointer_latest_pos() {
                let idx = headers.iter().filter(|r| r.center().y < pos.y).count();
                let y = if idx < headers.len() { headers[idx].top() } else { ui.min_rect().bottom() };
                let x = ui.min_rect().x_range();
                ui.painter().hline(x, y, Stroke::new(2.0_f32, p.accent));
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                if released {
                    actions.push(Action::MoveConn(d, idx));
                }
            }
            if released {
                st.dragging = None;
            }
        }
    });
    st.content_height = out.content_size.y;
    st.scroll_y = out.state.offset.y;
    actions
}

#[derive(Clone, Copy)]
enum HeaderIcon {
    Search,
    Plus,
}

/// A 26×26 rounded icon button for the explorer header.
fn icon_button(ui: &mut egui::Ui, p: &Palette, icon: HeaderIcon, primary: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), Sense::click());
    let hovered = resp.hovered();
    let pressed = resp.is_pointer_button_down_on();
    let (fill, fg) = if primary {
        (if pressed { p.accent.gamma_multiply(0.8) } else if hovered { p.accent } else { p.accent.gamma_multiply(0.9) }, Color32::WHITE)
    } else {
        (if pressed { p.selection } else if hovered { p.hover } else { p.raised }, if hovered { p.text } else { p.muted })
    };
    let painter = ui.painter_at(rect.expand(1.0));
    painter.rect(rect, CornerRadius::same(7), fill, if primary { Stroke::NONE } else { Stroke::new(1.0_f32, p.border.gamma_multiply(0.7)) }, egui::StrokeKind::Inside);
    let c = rect.center();
    let st = Stroke::new(1.7_f32, fg);
    match icon {
        HeaderIcon::Search => {
            let o = c + egui::vec2(-1.5, -1.5);
            painter.circle_stroke(o, 4.6, st);
            painter.line_segment([o + egui::vec2(3.4, 3.4), o + egui::vec2(7.0, 7.0)], Stroke::new(2.0_f32, fg));
        }
        HeaderIcon::Plus => {
            painter.line_segment([c + egui::vec2(-5.0, 0.0), c + egui::vec2(5.0, 0.0)], st);
            painter.line_segment([c + egui::vec2(0.0, -5.0), c + egui::vec2(0.0, 5.0)], st);
        }
    }
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

fn primary_db(cfg: &ConnectionConfig, dbs: Option<&DbList>) -> String {
    let own = cfg.effective_database();
    if !own.is_empty() {
        return own;
    }
    dbs.map(|d| d.current.clone()).unwrap_or_default()
}

/// Whether a connection shows database nodes (it has extra databases picked).
fn multi_db(cfg: &ConnectionConfig, dbs: Option<&DbList>) -> bool {
    if !cfg.supports_databases() {
        return false;
    }
    let primary = primary_db(cfg, dbs);
    cfg.visible_databases.as_ref().map(|l| l.iter().any(|d| !d.eq_ignore_ascii_case(&primary))).unwrap_or(false)
}

/// The project switcher above the tree: "All connections" or a named
/// project, plus new / rename / delete.
fn project_bar(ui: &mut egui::Ui, p: &Palette, st: &mut ExplorerState, cx: &ExplorerCtx<'_>, actions: &mut Vec<Action>) {
    let current = cx.current_project.and_then(|id| cx.projects.iter().find(|pr| pr.id == id));
    let popup_id = ui.id().with("project-menu");
    let open = ui.memory(|m| m.is_popup_open(popup_id));
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), Sense::click());
    let hovered = resp.hovered();
    let painter = ui.painter_at(rect);
    painter.rect(rect, CornerRadius::same(7), if open || hovered { p.hover } else { p.raised }, Stroke::new(1.0_f32, if open { p.accent } else { p.border.gamma_multiply(0.7) }), egui::StrokeKind::Inside);
    // folder glyph
    let f = Rect::from_center_size(egui::pos2(rect.left() + 16.0, rect.center().y + 0.5), egui::vec2(13.0, 10.0));
    let folder = if current.is_some() { p.accent } else { p.muted };
    painter.rect_filled(Rect::from_min_size(f.left_top() + egui::vec2(0.0, -2.0), egui::vec2(5.5, 3.0)), CornerRadius::same(1), folder);
    painter.rect_filled(f, CornerRadius::same(2), folder.gamma_multiply(0.85));
    let (label, count) = match current {
        Some(pr) => (pr.name.clone(), pr.connections.len()),
        None => ("All connections".to_string(), usize::MAX),
    };
    let caption = painter.layout_no_wrap("PROJECT".into(), FontId::proportional(9.5), p.faint);
    let x = rect.left() + 30.0;
    let name = painter.layout_no_wrap(label, bold(12.5), p.text);
    let avail = rect.right() - 30.0 - x;
    painter.galley(egui::pos2(x, rect.center().y - caption.size().y / 2.0), caption.clone(), p.faint);
    let nx = x + caption.size().x + 7.0;
    painter.with_clip_rect(Rect::from_min_max(egui::pos2(nx, rect.top()), egui::pos2(x + avail, rect.bottom()))).galley(egui::pos2(nx, rect.center().y - name.size().y / 2.0), name.clone(), p.text);
    if count != usize::MAX && nx + name.size().x + 30.0 < x + avail {
        let badge = painter.layout_no_wrap(count.to_string(), FontId::proportional(10.5), p.muted);
        let br = Rect::from_min_size(egui::pos2(nx + name.size().x + 7.0, rect.center().y - 8.0), egui::vec2(badge.size().x + 10.0, 16.0));
        painter.rect_filled(br, CornerRadius::same(8), p.island);
        painter.galley(br.center() - badge.size() / 2.0, badge, p.muted);
    }
    // chevron
    let c = egui::pos2(rect.right() - 15.0, rect.center().y);
    let pts = if open { [c + egui::vec2(-4.0, 2.0), c + egui::vec2(0.0, -2.0), c + egui::vec2(4.0, 2.0)] } else { [c + egui::vec2(-4.0, -2.0), c + egui::vec2(0.0, 2.0), c + egui::vec2(4.0, -2.0)] };
    painter.add(egui::Shape::line(pts.to_vec(), Stroke::new(1.5_f32, if hovered || open { p.text } else { p.muted })));
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let resp = resp.on_hover_text("Switch project: each project keeps its own list of connections");
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }
    egui::popup_below_widget(ui, popup_id, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
        ui.set_min_width(rect.width() - 12.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(RichText::new("Projects").size(11.0).color(p.faint));
        let item = |ui: &mut egui::Ui, on: bool, text: &str, n: Option<usize>| -> bool {
            let (r, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), Sense::click());
            if resp.hovered() || on {
                ui.painter().rect_filled(r, CornerRadius::same(5), if on { p.selection.gamma_multiply(0.6) } else { p.hover });
            }
            if on {
                let c = egui::pos2(r.left() + 12.0, r.center().y);
                ui.painter().add(egui::Shape::line(vec![c + egui::vec2(-4.0, 0.0), c + egui::vec2(-1.0, 3.0), c + egui::vec2(4.5, -3.5)], Stroke::new(1.6_f32, p.accent)));
            }
            ui.painter().text(egui::pos2(r.left() + 26.0, r.center().y), egui::Align2::LEFT_CENTER, text, if on { bold(12.5) } else { FontId::proportional(12.5) }, p.text);
            if let Some(n) = n {
                ui.painter().text(egui::pos2(r.right() - 8.0, r.center().y), egui::Align2::RIGHT_CENTER, n.to_string(), FontId::proportional(11.0), p.faint);
            }
            resp.clicked()
        };
        if item(ui, current.is_none(), "All connections", None) {
            actions.push(Action::SwitchProject(None));
        }
        for pr in cx.projects {
            let on = current.map(|c| c.id == pr.id).unwrap_or(false);
            if item(ui, on, &pr.name, Some(pr.connections.len())) {
                actions.push(Action::SwitchProject(Some(pr.id.clone())));
            }
        }
        ui.separator();
        if item(ui, false, "New project…", None) {
            st.project_edit = Some((None, String::new()));
        }
        if let Some(pr) = current {
            if item(ui, false, "Rename project…", None) {
                st.project_edit = Some((Some(pr.id.clone()), pr.name.clone()));
            }
            if item(ui, false, "Delete project", None) {
                actions.push(Action::DeleteProject(pr.id.clone()));
            }
        }
    });
    let mut done: Option<bool> = None;
    if let Some((_, name)) = st.project_edit.as_mut() {
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(name).hint_text("Project name").desired_width((ui.available_width() - 90.0).max(60.0)));
            if !r.has_focus() && !r.lost_focus() && name.is_empty() {
                r.request_focus();
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                done = Some(true);
            }
            if ui.small_button("Save").clicked() {
                done = Some(true);
            }
            if ui.small_button("✕").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
            }
        });
    }
    if let Some(save) = done {
        if let Some((id, name)) = st.project_edit.take() {
            if save && !name.trim().is_empty() {
                actions.push(Action::SaveProject(id, name.trim().to_string()));
            }
        }
    }
}

/// The "2 of 9 databases" row under a connection, and its checklist.
fn db_picker_row(ui: &mut egui::Ui, p: &Palette, st: &mut ExplorerState, cfg: &ConnectionConfig, dbs: Option<&DbList>, primary: &str, actions: &mut Vec<Action>) {
    let requested = st.db_picker_request.as_deref() == Some(cfg.id.as_str());
    let Some(list) = dbs.filter(|d| d.names.len() > 1 || d.loading || d.error.is_some() || requested) else { return };
    let shown: Vec<String> = {
        let mut v = vec![primary.to_string()];
        for d in cfg.visible_databases.iter().flatten() {
            if !v.iter().any(|x| x.eq_ignore_ascii_case(d)) {
                v.push(d.clone());
            }
        }
        v
    };
    let total = list.names.len();
    let text = if list.loading && total == 0 {
        "Loading databases…".to_string()
    } else if let Some(e) = &list.error {
        format!("Couldn't list databases: {}", e.chars().take(60).collect::<String>())
    } else {
        format!("{} of {total} databases", shown.iter().filter(|d| list.names.iter().any(|n| n.eq_ignore_ascii_case(d))).count().max(1))
    };
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 20.0), Sense::click());
    let popup_id = ui.id().with(("db-picker", &cfg.id));
    let painter = ui.painter_at(rect);
    let open = ui.memory(|m| m.is_popup_open(popup_id));
    let x = rect.left() + 26.0;
    let color = if list.error.is_some() { p.danger } else if resp.hovered() || open { p.accent } else { p.muted };
    let g = painter.layout_no_wrap(text, FontId::proportional(11.5), color);
    let pill = Rect::from_min_size(egui::pos2(x - 6.0, rect.center().y - g.size().y / 2.0 - 2.0), g.size() + egui::vec2(26.0, 4.0));
    if resp.hovered() || open {
        painter.rect_filled(pill, CornerRadius::same(4), p.hover);
    }
    let arrow = Rect::from_center_size(egui::pos2(x + g.size().x + 9.0, rect.center().y), egui::vec2(10.0, 10.0));
    painter.galley(egui::pos2(x, rect.center().y - g.size().y / 2.0), g, color);
    icons::paint_arrow(&painter, arrow, true, color);
    let resp = resp.on_hover_text("Choose which databases on this server the explorer shows");
    if resp.clicked() || (requested && total > 0) {
        st.db_picker_request = None;
        st.db_picker_search.clear();
        ui.memory_mut(|m| m.toggle_popup(popup_id));
        if list.error.is_some() {
            actions.push(Action::LoadDatabases(cfg.id.clone()));
        }
    }
    egui::popup_below_widget(ui, popup_id, &resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.set_min_width(250.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Show databases").font(bold(12.5)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("⟳").on_hover_text("Reload the list").clicked() {
                    actions.push(Action::LoadDatabases(cfg.id.clone()));
                }
            });
        });
        ui.add_space(2.0);
        if total > 8 {
            ui.add(egui::TextEdit::singleline(&mut st.db_picker_search).hint_text("Filter").desired_width(f32::INFINITY));
        }
        ui.horizontal(|ui| {
            if ui.small_button("All").clicked() {
                actions.push(Action::SetDatabases(cfg.id.clone(), Some(list.names.clone())));
            }
            if ui.small_button("Default only").on_hover_text(format!("Only {primary}")).clicked() {
                actions.push(Action::SetDatabases(cfg.id.clone(), None));
            }
        });
        ui.separator();
        let search = st.db_picker_search.to_lowercase();
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            for name in &list.names {
                if !search.is_empty() && !name.to_lowercase().contains(&search) {
                    continue;
                }
                let is_primary = name.eq_ignore_ascii_case(primary);
                let mut on = is_primary || shown.iter().any(|d| d.eq_ignore_ascii_case(name));
                ui.horizontal(|ui| {
                    let r = ui.add_enabled(!is_primary, egui::Checkbox::new(&mut on, name.as_str()));
                    if r.changed() {
                        let mut l: Vec<String> = shown.iter().filter(|d| !d.eq_ignore_ascii_case(primary)).cloned().collect();
                        if on {
                            l.push(name.clone());
                        } else {
                            l.retain(|d| !d.eq_ignore_ascii_case(name));
                        }
                        actions.push(Action::SetDatabases(cfg.id.clone(), if l.is_empty() { None } else { Some(l) }));
                    }
                    if is_primary {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new("default").size(11.0).color(p.faint));
                        });
                    }
                });
            }
        });
    });
}

/// Schemas and objects of one database (`d0` = extra depth when the
/// database has its own node).
#[allow(clippy::too_many_arguments)]
fn conn_body(ui: &mut egui::Ui, p: &Palette, st: &mut ExplorerState, logos: &Logos, cfg: &ConnectionConfig, view: Option<&CatalogView<'_>>, d0: usize, filter: &str, actions: &mut Vec<Action>) {
    let filtering = !filter.is_empty();
    let pad = 30.0 + d0 as f32 * 14.0;
    let Some(v) = view else {
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.label(RichText::new("Loading schema…").size(12.0).color(p.muted));
        });
        return;
    };
    if let Some(err) = v.error {
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.label(RichText::new(err.chars().take(140).collect::<String>()).size(11.5).color(p.danger));
        });
        ui.horizontal(|ui| {
            ui.add_space(pad);
            if ui.small_button("Retry").clicked() {
                actions.push(Action::Retry(cfg.id.clone()));
            }
        });
    }
    if v.catalog.schemas.is_empty() && v.loading {
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.label(RichText::new("Loading schema…").size(12.0).color(p.muted));
        });
    }
    let single = v.catalog.schemas.len() == 1;
    if !single && !v.catalog.schemas.is_empty() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(d0 as f32 * 14.0);
            ui.vertical(|ui| schema_picker(ui, p, st, cfg, v.catalog, actions));
        });
    }
    for s in &v.catalog.schemas {
        if !single && !cfg.schema_visible(&s.name) {
            continue;
        }
        if filtering && !st.schema_matches(&cfg.id, v.generation, s, filter) {
            continue;
        }
        let depth = if single {
            1 + d0
        } else {
            let sk = format!("s:{}:{}", cfg.id, s.name);
            let is_default = s.name == v.catalog.default_schema;
            if is_default && !st.open.contains(&format!("{sk}:seen")) {
                st.open.insert(sk.clone());
                st.open.insert(format!("{sk}:seen"));
            }
            let n = s.tables.len() + s.objects.len();
            let r = row(ui, p, st, logos, &sk, 1 + d0, true, Glyph::Schema, &s.name, &if n == 0 { "empty".to_string() } else { n.to_string() }, false, false);
            schema_menu(r.resp, cfg, &s.name, actions);
            if !is_open(st, &sk, filtering) {
                continue;
            }
            2 + d0
        };
        schema_body(ui, p, st, logos, cfg, v.generation, s, depth, filter, actions);
    }
    if v.catalog.introspected_at > 0 && !filtering {
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.label(RichText::new(format!("{} objects · {}", v.catalog.table_count(), v.age)).size(10.5).color(p.faint));
        });
        ui.add_space(2.0);
    }
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
#[allow(clippy::too_many_arguments)]
fn schema_body(ui: &mut egui::Ui, p: &Palette, st: &mut ExplorerState, logos: &Logos, cfg: &ConnectionConfig, generation: u64, s: &SchemaInfo, depth: usize, filter: &str, actions: &mut Vec<Action>) {
    let filtering = !filter.is_empty();
    for kind in ObjectKind::DISPLAY_ORDER {
        let all = st.group_items(&cfg.id, generation, s, kind, "");
        let n = all.len();
        if n == 0 {
            continue;
        }
        let gk = format!("g:{}:{}:{:?}", cfg.id, s.name, kind);
        if kind == ObjectKind::Table && !st.open.contains(&format!("{gk}:seen")) {
            st.open.insert(gk.clone());
            st.open.insert(format!("{gk}:seen"));
        }
        let items = if filtering { st.group_items(&cfg.id, generation, s, kind, filter) } else { all };
        if items.is_empty() {
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
        let mut skip = Skipper::new(ui, st);
        if kind.is_relation() {
            // expanded tables (usually few) are taller than one row
            let prefix = format!("t:{}:{}:", cfg.id, s.name);
            let open: HashSet<String> = st.open.iter().filter_map(|k| k.strip_prefix(prefix.as_str())).map(str::to_string).collect();
            for &i in items.iter() {
                let t = &s.tables[i as usize];
                let rows = if open.contains(&t.name) { table_rows(&open, s, t) } else { 1 };
                if skip.visible(ui, rows) {
                    table_node(ui, p, st, logos, cfg, &s.name, s, t, depth + 1, actions);
                }
            }
        } else {
            for &i in items.iter() {
                if !skip.visible(ui, 1) {
                    continue;
                }
                let o = &s.objects[i as usize];
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
                let runnable = databug_sql::ddl::exec_template(cfg.kind.dialect(), &s.name, &o.name, o.kind, &o.detail).is_some();
                let obj = o.clone();
                r.resp.on_hover_text(format!("{} {}.{}{}", o.kind.label(), s.name, o.name, if o.detail.is_empty() { String::new() } else { format!(" {}", detail) })).context_menu(|ui| {
                    if runnable {
                        let label = match (cfg.kind, obj.kind) {
                            (DbKind::MsSql, ObjectKind::Procedure | ObjectKind::ClrProcedure) => "Execute (EXEC)…",
                            (_, ObjectKind::Procedure | ObjectKind::ClrProcedure) => "Execute (CALL)…",
                            _ => "Run in console…",
                        };
                        if ui.button(RichText::new(label).color(p.success)).clicked() {
                            actions.push(Action::ExecRoutine(cfg.id.clone(), s.name.clone(), obj.clone()));
                            ui.close_menu();
                        }
                    }
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
        skip.finish(ui);
    }
}

/// Rows a table takes when expanded, matching what `table_node` draws.
/// `open` holds the open keys under this schema, without the `t:conn:schema:` prefix.
fn table_rows(open: &HashSet<String>, s: &SchemaInfo, t: &TableInfo) -> usize {
    let mut n = 1 + t.columns.len();
    let mut folder = |count: usize, suffix: &str| {
        if count > 0 {
            n += 1 + if open.contains(&format!("{}:{suffix}", t.name)) { count } else { 0 };
        }
    };
    folder(t.indexes.iter().filter(|i| !i.primary || t.kind == ObjectKind::Collection).count(), "idx");
    folder(t.foreign_keys.len(), "fk");
    folder(s.objects.iter().filter(|o| matches!(o.kind, ObjectKind::Trigger | ObjectKind::ClrTrigger) && o.table.as_deref() == Some(t.name.as_str())).count(), "trg");
    n
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
        // a double-click opens the rows; undo the expand/collapse that the
        // shared row code does on double-click
        if st.open.contains(&key) {
            st.open.remove(&key);
        } else {
            st.open.insert(key.clone());
        }
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
    let mut skip = Skipper::new(ui, st);
    for c in &t.columns {
        if !skip.visible(ui, 1) {
            continue;
        }
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
    skip.finish(ui);
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

#[cfg(test)]
mod tests {
    use super::*;
    use databug_core::{ColumnInfo, ForeignKey, IndexInfo};

    /// A schema with `tables` tables (8 columns, an index and a foreign key
    /// each), `funcs` functions and `triggers` triggers.
    fn catalog(tables: usize, funcs: usize, triggers: usize) -> Catalog {
        let mut s = SchemaInfo { name: "public".into(), ..Default::default() };
        for i in 0..tables {
            let mut t = TableInfo::new(format!("table_{i:05}"), ObjectKind::Table);
            for c in 0..8 {
                t.columns.push(ColumnInfo { name: format!("col_{c}"), data_type: "integer".into(), nullable: true, primary_key: c == 0, default: None });
            }
            t.indexes.push(IndexInfo { name: format!("ix_{i}"), columns: vec!["col_1".into()], unique: false, primary: false });
            t.foreign_keys.push(ForeignKey { name: format!("fk_{i}"), columns: vec!["col_2".into()], ref_schema: "public".into(), ref_table: "table_00000".into(), ref_columns: vec!["col_0".into()], on_delete: String::new() });
            s.tables.push(t);
        }
        for i in 0..funcs {
            s.objects.push(DbObject { name: format!("fn_{i:05}"), kind: ObjectKind::Function, detail: "a integer, b text".into(), ident: String::new(), table: None });
        }
        for i in 0..triggers {
            s.objects.push(DbObject { name: format!("trg_{i:05}"), kind: ObjectKind::Trigger, detail: String::new(), ident: String::new(), table: Some(format!("table_{:05}", i % tables.max(1))) });
        }
        Catalog { default_schema: "public".into(), introspected_at: 1, schemas: vec![s] }
    }

    struct Harness {
        ctx: egui::Context,
        logos: Option<Logos>,
        cfg: ConnectionConfig,
        cat: Catalog,
        st: ExplorerState,
    }

    impl Harness {
        fn new(cat: Catalog) -> Self {
            let ctx = egui::Context::default();
            crate::theme::install_fonts(&ctx);
            let mut cfg = ConnectionConfig::new(DbKind::Postgres);
            cfg.id = "c1".into();
            let mut st = ExplorerState::default();
            for k in ["c:c1", "d:c1:", "s:c1:public", "g:c1:public:Table", "g:c1:public:Function", "g:c1:public:Trigger"] {
                st.open.insert(k.into());
            }
            Harness { ctx, logos: None, cfg, cat, st }
        }

        /// One frame; `wheel` scrolls by that many points.
        fn frame(&mut self, wheel: f32) {
            let p = crate::theme::palette(crate::theme::ThemeKind::Dark, &Default::default());
            let (a, b, c, d) = (HashSet::new(), HashSet::new(), HashMap::new(), HashMap::new());
            let cx = ExplorerCtx { connected: &a, disconnected: &b, failed: &c, databases: &d, projects: &[], current_project: None };
            let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 1100.0))), ..Default::default() };
            if wheel != 0.0 {
                input.events.push(egui::Event::PointerMoved(egui::pos2(150.0, 500.0)));
                input.events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -wheel), modifiers: Default::default() });
            }
            let Harness { ctx, logos, cfg, cat, st } = self;
            let _ = ctx.run(input, |ctx| {
                let l = logos.get_or_insert_with(|| Logos::discover(ctx));
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut views = HashMap::new();
                    views.insert("c1".to_string(), CatalogView { catalog: cat, generation: 1, loading: false, error: None, age: String::new() });
                    show(ui, &p, st, l, std::slice::from_ref(cfg), &views, &cx);
                });
            });
        }
    }

    #[test]
    fn skipping_rows_keeps_the_layout() {
        let mut virt = Harness::new(catalog(400, 300, 120));
        // a mix of expanded tables and sub-folders, near the top, middle and end
        for i in [0, 3, 150, 151, 399] {
            virt.st.open.insert(format!("t:c1:public:table_{i:05}"));
        }
        virt.st.open.insert("t:c1:public:table_00003:idx".into());
        virt.st.open.insert("t:c1:public:table_00150:fk".into());
        virt.st.open.insert("t:c1:public:table_00150:trg".into());
        let mut full = Harness::new(virt.cat.clone());
        full.st.open = virt.st.open.clone();
        full.st.full_layout = true;
        for wheel in [0.0, 3000.0, 4000.0, 9000.0, 100_000.0] {
            for _ in 0..3 {
                virt.frame(wheel);
                full.frame(wheel);
            }
            assert!(virt.st.content_height > 10_000.0);
            assert_eq!(virt.st.scroll_y, full.st.scroll_y);
            assert!(wheel == 0.0 || virt.st.scroll_y > 0.0, "didn't scroll");
            assert_eq!(virt.st.content_height, full.st.content_height, "after scrolling {wheel}");
        }
    }

    #[test]
    fn filter_uses_cached_groups() {
        let mut h = Harness::new(catalog(50, 20, 5));
        h.st.filter = "fn_0001".into();
        h.frame(0.0);
        let s = h.cat.schemas[0].clone();
        assert_eq!(h.st.group_items("c1", 1, &s, ObjectKind::Function, "fn_0001").len(), 10);
        assert!(h.st.group_items("c1", 1, &s, ObjectKind::Table, "fn_0001").is_empty());
        assert_eq!(h.st.group_items("c1", 1, &s, ObjectKind::Table, "").len(), 50);
        // a new catalog load (generation) is never served from the old lists
        let mut s2 = s.clone();
        s2.objects.truncate(3);
        assert_eq!(h.st.group_items("c1", 2, &s2, ObjectKind::Function, "fn_0001").len(), 0);
    }

    /// `cargo test --release -p databug explorer_frame_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn explorer_frame_cost() {
        for (t, f, g) in [(50, 20, 10), (1000, 500, 200), (5000, 2000, 1000), (20000, 5000, 3000)] {
            let mut h = Harness::new(catalog(t, f, g));
            let mut times = Vec::new();
            for i in 0..12 {
                let start = std::time::Instant::now();
                h.frame(if i == 6 { 50_000.0 } else { 0.0 });
                if i >= 2 {
                    times.push(start.elapsed().as_secs_f64() * 1000.0);
                }
            }
            times.sort_by(|a, b| a.partial_cmp(b).unwrap());
            eprintln!("tables {t:>6} functions {f:>5} triggers {g:>5}: {:>7.2} ms per frame", times[times.len() / 2]);
        }
    }
}
