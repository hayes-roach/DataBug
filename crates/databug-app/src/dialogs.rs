//! The connection dialog.

use crate::theme::{bold, connection_color, Palette, SWATCHES};
use eframe::egui::{self, CornerRadius, RichText, Sense, Stroke};
use std::sync::mpsc;
use std::time::Instant;
use databug_core::{ConnectionConfig, DbCategory, DbKind, MsAuth, SslMode};

pub struct ConnDialog {
    pub cfg: ConnectionConfig,
    pub password: String,
    pub save_password: bool,
    pub is_new: bool,
    test: Option<Result<String, String>>,
    test_rx: Option<mpsc::Receiver<Result<String, String>>>,
    error: Option<String>,
    /// Schemas from the last introspection, for the visibility list.
    pub known_schemas: Vec<String>,
    pub default_schema: String,
    /// Engine group shown in the picker.
    category: DbCategory,
}

pub enum DialogOutcome {
    Open,
    Cancel,
    Save,
}

impl ConnDialog {
    pub fn new(cfg: ConnectionConfig, password: String, is_new: bool) -> Self {
        let category = cfg.kind.category();
        ConnDialog { cfg, password, save_password: true, is_new, test: None, test_rx: None, error: None, known_schemas: Vec::new(), default_schema: String::new(), category }
    }

    pub fn show(&mut self, ctx: &egui::Context, p: &Palette, rt: &tokio::runtime::Handle, logos: &crate::icons::Logos) -> DialogOutcome {
        let mut outcome = DialogOutcome::Open;
        if let Some(rx) = &self.test_rx {
            if let Ok(r) = rx.try_recv() {
                self.test = Some(r);
                self.test_rx = None;
            }
        }
        let available = databug_drivers::available();
        let title = if self.is_new { "New connection" } else { "Edit connection" };
        let mut open = true;
        egui::Window::new(RichText::new(title).font(bold(15.0)))
            .id(egui::Id::new("conn-dialog"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size([560.0, 0.0])
            .show(ctx, |ui| {
                // engine picker: one group of engines at a time
                ui.horizontal_wrapped(|ui| {
                    for cat in DbCategory::ALL {
                        let current = cat == self.cfg.kind.category();
                        let text = if current { RichText::new(cat.label()).strong() } else { RichText::new(cat.label()) };
                        ui.selectable_value(&mut self.category, cat, text);
                    }
                });
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    for kind in self.category.kinds() {
                        let enabled = available.contains(&kind);
                        let selected = self.cfg.kind == kind;
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(68.0, 56.0), if enabled { Sense::click() } else { Sense::hover() });
                        let fill = if selected { p.selection } else if resp.hovered() { p.hover } else { p.island };
                        ui.painter().rect(rect, CornerRadius::same(8), fill, Stroke::new(1.0_f32, if selected { p.accent } else { p.border }), egui::StrokeKind::Inside);
                        let logo = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 20.0), egui::vec2(24.0, 24.0));
                        if enabled {
                            logos.paint(ui, logo, kind);
                        } else {
                            ui.painter().rect_filled(logo, CornerRadius::same(4), p.raised);
                        }
                        let name = match kind {
                            DbKind::MySql => "MySQL",
                            k => k.label(),
                        };
                        ui.painter().text(egui::pos2(rect.center().x, rect.bottom() - 10.0), egui::Align2::CENTER_CENTER, name, egui::FontId::proportional(10.5), if enabled { p.text } else { p.faint });
                        if resp.clicked() && !selected {
                            let old_default = self.cfg.kind.default_port();
                            self.cfg.kind = kind;
                            if self.cfg.port == old_default || self.cfg.port == 0 {
                                self.cfg.port = kind.default_port();
                            }
                            // these engines sign in once over either TLS or plain text, so there's no "prefer"
                            if tls_on_or_off(kind) && self.cfg.ssl == SslMode::Prefer {
                                self.cfg.ssl = SslMode::Disable;
                            }
                            self.test = None;
                        }
                        if !enabled {
                            resp.on_hover_text(match kind {
                                DbKind::Oracle => "Not included in this build: build with the `oracle` feature (needs Oracle Instant Client to run)",
                                DbKind::Db2 => "Not included in this build: build with the `db2` feature (needs ODBC and the IBM Db2 ODBC driver)",
                                DbKind::Spark => "Not included in this build: build with the `spark` feature (needs ODBC and a Spark ODBC driver)",
                                _ => "Not included in this build",
                            });
                        }
                    }
                });
                ui.add_space(10.0);
                egui::Grid::new("conn-form").striped(false).num_columns(2).spacing([12.0, 8.0]).min_col_width(110.0).show(ui, |ui| {
                    let field = |ui: &mut egui::Ui, label: &str| {
                        ui.label(RichText::new(label).color(p.muted));
                    };
                    field(ui, "Name");
                    let hint = self.default_name();
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.name).hint_text(hint).desired_width(380.0));
                    ui.end_row();
                    match self.cfg.kind {
                        DbKind::Sqlite | DbKind::DuckDb => {
                            let duck = self.cfg.kind == DbKind::DuckDb;
                            field(ui, "Database file");
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut self.cfg.path).hint_text(if duck { "/path/to/analytics.duckdb or :memory:" } else { "/path/to/database.db" }).desired_width(296.0));
                                if ui.button("Browse…").clicked() {
                                    let dialog = if duck { rfd::FileDialog::new().add_filter("DuckDB", &["duckdb", "ddb", "db"]) } else { rfd::FileDialog::new().add_filter("SQLite", &["db", "sqlite", "sqlite3", "db3"]) };
                                    if let Some(f) = dialog.add_filter("All files", &["*"]).pick_file() {
                                        self.cfg.path = f.display().to_string();
                                    }
                                }
                            });
                            ui.end_row();
                            if duck {
                                field(ui, "Engine");
                                ui.vertical(|ui| duckdb_download_ui(ui, p, rt));
                                ui.end_row();
                            }
                        }
                        kind => {
                            if let Some(hint) = uri_hint(kind) {
                                field(ui, if kind == DbKind::Oracle { "TNS / descriptor" } else { "URI" });
                                ui.add(egui::TextEdit::singleline(&mut self.cfg.uri).hint_text(hint).desired_width(380.0));
                                ui.end_row();
                            }
                            if uri_hint(kind).is_none() || self.cfg.uri.is_empty() {
                                field(ui, "Host");
                                ui.horizontal(|ui| {
                                    ui.add(egui::TextEdit::singleline(&mut self.cfg.host).hint_text("localhost").desired_width(260.0));
                                    ui.label(RichText::new("Port").color(p.muted));
                                    ui.add(egui::DragValue::new(&mut self.cfg.port).range(1..=65535).speed(0));
                                });
                                ui.end_row();
                            }
                            let auth = if kind == DbKind::MsSql { self.cfg.ms_auth } else { MsAuth::SqlServer };
                            if kind == DbKind::MsSql {
                                field(ui, "Authentication");
                                ui.horizontal(|ui| {
                                    for a in MsAuth::ALL {
                                        let r = ui.selectable_value(&mut self.cfg.ms_auth, a, a.label());
                                        let tip = match a {
                                            MsAuth::SqlServer => "A SQL Server login and password",
                                            MsAuth::WindowsIntegrated => {
                                                if cfg!(windows) {
                                                    "Sign in as the Windows account you're logged in with"
                                                } else {
                                                    "Sign in with your Kerberos ticket. Run `kinit you@EXAMPLE.COM` first"
                                                }
                                            }
                                            MsAuth::WindowsCredentials => {
                                                if cfg!(windows) {
                                                    "Sign in as a different Windows/domain account"
                                                } else {
                                                    "Sign in as a domain account. DataBug gets a Kerberos ticket for it with kinit"
                                                }
                                            }
                                        };
                                        if r.on_hover_text(tip).changed() {
                                            self.test = None;
                                        }
                                    }
                                });
                                ui.end_row();
                            }
                            let windows_ok = databug_drivers::windows_auth_available();
                            match auth {
                                MsAuth::WindowsIntegrated => {
                                    field(ui, "User");
                                    let who = if cfg!(windows) {
                                        let dom = std::env::var("USERDOMAIN").unwrap_or_default();
                                        let user = std::env::var("USERNAME").unwrap_or_default();
                                        if dom.is_empty() { user } else { format!("{dom}\\{user}") }
                                    } else {
                                        "your Kerberos ticket (klist)".to_string()
                                    };
                                    ui.label(RichText::new(format!("Signed in as {who}")).color(p.muted));
                                    ui.end_row();
                                }
                                _ => {
                                    let windows = auth == MsAuth::WindowsCredentials;
                                    let influx = kind == DbKind::InfluxDb;
                                    field(ui, if windows { "Windows user" } else { "User" });
                                    let user_hint = if windows {
                                        "DOMAIN\\user or user@example.com"
                                    } else if influx {
                                        "only for InfluxDB 1.x; leave empty to use a token"
                                    } else if matches!(kind, DbKind::Cassandra | DbKind::CouchDb) {
                                        "optional"
                                    } else {
                                        ""
                                    };
                                    ui.add(egui::TextEdit::singleline(&mut self.cfg.user).hint_text(user_hint).desired_width(380.0));
                                    ui.end_row();
                                    field(ui, if influx && self.cfg.user.is_empty() { "API token" } else { "Password" });
                                    ui.horizontal(|ui| {
                                        ui.add(egui::TextEdit::singleline(&mut self.password).password(true).desired_width(220.0));
                                        ui.checkbox(&mut self.save_password, "Save in system keychain");
                                    });
                                    ui.end_row();
                                }
                            }
                            if auth != MsAuth::SqlServer && !windows_ok {
                                ui.label("");
                                ui.label(RichText::new("This build has no Kerberos support, so Windows authentication won't work here. The release downloads include it.").size(11.5).color(p.warning));
                                ui.end_row();
                            } else if auth != MsAuth::SqlServer && !cfg!(windows) {
                                ui.label("");
                                let msg = if auth == MsAuth::WindowsIntegrated {
                                    "Uses your Kerberos ticket. Your Mac or Linux machine needs krb5.conf pointing at the domain."
                                } else {
                                    "DataBug runs kinit for this account, which replaces your current Kerberos ticket."
                                };
                                ui.label(RichText::new(msg).size(11.5).color(p.faint));
                                ui.end_row();
                            }
                            if kind == DbKind::Oracle && self.cfg.uri.is_empty() {
                                field(ui, "Connect by");
                                ui.horizontal(|ui| {
                                    ui.selectable_value(&mut self.cfg.oracle_sid, false, "Service name");
                                    ui.selectable_value(&mut self.cfg.oracle_sid, true, "SID");
                                });
                                ui.end_row();
                            }
                            if kind != DbKind::Oracle || self.cfg.uri.is_empty() {
                                let (label, hint) = database_field(kind, self.cfg.oracle_sid);
                                field(ui, label);
                                ui.add(egui::TextEdit::singleline(&mut self.cfg.database).hint_text(hint).desired_width(380.0));
                                ui.end_row();
                            }
                            if kind == DbKind::InfluxDb {
                                field(ui, "Organization");
                                ui.add(egui::TextEdit::singleline(&mut self.cfg.org).hint_text("InfluxDB 2.x, for Flux queries").desired_width(380.0));
                                ui.end_row();
                            }
                            if matches!(kind, DbKind::Db2 | DbKind::Spark) {
                                field(ui, "ODBC driver");
                                ui.add(egui::TextEdit::singleline(&mut self.cfg.odbc_driver).hint_text(if kind == DbKind::Db2 { "IBM DB2 ODBC DRIVER" } else { "Simba Spark ODBC Driver" }).desired_width(380.0));
                                ui.end_row();
                                field(ui, "ODBC options");
                                ui.add(egui::TextEdit::singleline(&mut self.cfg.odbc_options).hint_text(if kind == DbKind::Db2 { "e.g. CURRENTSCHEMA=STORE" } else { "e.g. ThriftTransport=2;HTTPPath=cliservice" }).desired_width(380.0))
                                    .on_hover_text("Extra key=value; pairs for the ODBC connection string. Don't put the password here.");
                                ui.end_row();
                            }
                            field(ui, "SSL / TLS");
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut self.cfg.ssl, SslMode::Disable, "Off");
                                if !tls_on_or_off(kind) {
                                    ui.selectable_value(&mut self.cfg.ssl, SslMode::Prefer, "Prefer");
                                }
                                ui.selectable_value(&mut self.cfg.ssl, SslMode::Require, "Require");
                                if kind == DbKind::MsSql {
                                    ui.checkbox(&mut self.cfg.trust_server_cert, "Trust server certificate");
                                }
                            });
                            ui.end_row();
                        }
                    }
                    if let Some(note) = engine_note(self.cfg.kind) {
                        ui.label("");
                        ui.label(RichText::new(note).size(11.5).color(p.faint));
                        ui.end_row();
                    }
                    if self.known_schemas.len() > 1 {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.add_space(2.0);
                            ui.label(RichText::new(if self.cfg.kind == DbKind::MySql { "Databases" } else { "Schemas" }).color(p.muted));
                        });
                        ui.vertical(|ui| self.schema_list(ui, p));
                        ui.end_row();
                    }
                    field(ui, "Color");
                    ui.horizontal(|ui| {
                        for sw in SWATCHES {
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), Sense::click());
                            ui.painter().circle_filled(rect.center(), 8.0, connection_color(sw));
                            if self.cfg.color == sw {
                                ui.painter().circle_stroke(rect.center(), 10.0, Stroke::new(1.5_f32, p.text));
                            }
                            if resp.clicked() {
                                self.cfg.color = sw;
                            }
                        }
                    });
                    ui.end_row();
                    field(ui, "Safety");
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut self.cfg.production, "Production").on_hover_text("Ask before running anything that changes data, and show a red marker").changed() && self.cfg.production {
                            self.cfg.color = [247, 84, 100];
                        }
                        ui.checkbox(&mut self.cfg.read_only, "Read-only").on_hover_text("Block statements that modify data or schema");
                    });
                    ui.end_row();
                });
                ui.add_space(12.0);
                if let Some(e) = &self.error {
                    ui.label(RichText::new(e).color(p.danger));
                }
                match &self.test {
                    Some(Ok(m)) => {
                        ui.label(RichText::new(format!("✔ {m}")).color(p.success));
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(format!("✖ {e}")).color(p.danger));
                    }
                    None => {}
                }
                ui.separator();
                ui.horizontal(|ui| {
                    let testing = self.test_rx.is_some();
                    if ui.add_enabled(!testing, egui::Button::new("Test connection")).clicked() {
                        let cfg = self.cfg.clone();
                        let pw = self.password.clone();
                        let (tx, rx) = mpsc::channel();
                        let ctx2 = ui.ctx().clone();
                        rt.spawn(async move {
                            let start = Instant::now();
                            let r = match databug_drivers::connect(&cfg, Some(&pw)).await {
                                Ok(_c) => Ok(format!("Connected in {}", crate::grid::fmt_duration(start.elapsed()))),
                                Err(e) => Err(format!("{e:#}")),
                            };
                            let _ = tx.send(r);
                            ctx2.request_repaint();
                        });
                        self.test_rx = Some(rx);
                        self.test = None;
                    }
                    if testing {
                        ui.spinner();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let save = ui.add(egui::Button::new(RichText::new("Save").color(egui::Color32::WHITE)).fill(p.accent));
                        if save.clicked() {
                            if let Some(e) = self.validate() {
                                self.error = Some(e);
                            } else {
                                if self.cfg.name.trim().is_empty() {
                                    self.cfg.name = self.default_name();
                                }
                                outcome = DialogOutcome::Save;
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            outcome = DialogOutcome::Cancel;
                        }
                    });
                });
            });
        if !open {
            outcome = DialogOutcome::Cancel;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            outcome = DialogOutcome::Cancel;
        }
        outcome
    }

    fn schema_list(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let mut all = self.cfg.visible_schemas.is_none();
        ui.horizontal(|ui| {
            if ui.radio_value(&mut all, true, "All").clicked() {
                self.cfg.visible_schemas = None;
            }
            if ui.radio_value(&mut all, false, "Selected").clicked() && self.cfg.visible_schemas.is_none() {
                self.cfg.visible_schemas = Some(vec![self.default_schema.clone()]);
            }
            if let Some(list) = &self.cfg.visible_schemas {
                ui.label(RichText::new(format!("{} of {}", self.known_schemas.iter().filter(|s| list.contains(s)).count(), self.known_schemas.len())).color(p.faint));
            }
        });
        if let Some(list) = self.cfg.visible_schemas.as_mut() {
            egui::Frame::new().fill(p.canvas).stroke(Stroke::new(1.0_f32, p.border)).corner_radius(CornerRadius::same(6)).inner_margin(6.0).show(ui, |ui| {
                ui.set_width(368.0);
                egui::ScrollArea::vertical().max_height(130.0).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for s in &self.known_schemas {
                            let mut on = list.contains(s);
                            let text = if *s == self.default_schema { format!("{s} (default)") } else { s.clone() };
                            if ui.checkbox(&mut on, text).changed() {
                                if on {
                                    list.push(s.clone());
                                } else {
                                    list.retain(|x| x != s);
                                }
                            }
                        }
                    });
                });
            });
        }
    }

    fn default_name(&self) -> String {
        match self.cfg.kind {
            k @ (DbKind::Sqlite | DbKind::DuckDb) => std::path::Path::new(&self.cfg.path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| k.label().into()),
            _ if !self.cfg.database.is_empty() => format!("{}@{}", self.cfg.database, if self.cfg.host.is_empty() { "localhost" } else { &self.cfg.host }),
            k => format!("{}@{}", k.label(), if self.cfg.host.is_empty() { "localhost" } else { &self.cfg.host }),
        }
    }

    fn validate(&self) -> Option<String> {
        match self.cfg.kind {
            DbKind::Sqlite if self.cfg.path.trim().is_empty() => Some("Choose a database file".into()),
            DbKind::DuckDb if self.cfg.path.trim().is_empty() => Some("Choose a database file, or enter :memory:".into()),
            DbKind::CouchDb | DbKind::InfluxDb if !self.cfg.uri.trim().is_empty() && !self.cfg.uri.starts_with("http://") && !self.cfg.uri.starts_with("https://") => Some("The URI should start with http:// or https://".into()),
            DbKind::CouchDb | DbKind::InfluxDb if self.cfg.uri.contains('@') => Some("Put the user and password in their fields, not the URI, so they stay out of the config file".into()),
            DbKind::Oracle if self.cfg.uri.trim().is_empty() && self.cfg.database.trim().is_empty() => Some(if self.cfg.oracle_sid { "Enter the SID".into() } else { "Enter the service name".into() }),
            DbKind::Db2 if self.cfg.database.trim().is_empty() => Some("Enter the database name".into()),
            DbKind::Db2 | DbKind::Spark if self.cfg.odbc_options.to_ascii_uppercase().contains("PWD=") => Some("Put the password in the Password field, not the ODBC options".into()),
            DbKind::MongoDb if !self.cfg.uri.is_empty() && !self.cfg.uri.starts_with("mongodb") => Some("The URI should start with mongodb:// or mongodb+srv://".into()),
            DbKind::MongoDb if self.cfg.uri.contains('@') && self.cfg.uri.split('@').next().map(|s| s.contains(':') && s.matches(':').count() > 1).unwrap_or(false) => {
                Some("Put the password in the Password field, not the URI, so it stays out of the config file".into())
            }
            DbKind::Sqlite | DbKind::DuckDb | DbKind::MongoDb => None,
            DbKind::CouchDb | DbKind::InfluxDb | DbKind::Oracle if !self.cfg.uri.trim().is_empty() => None,
            _ if self.cfg.host.trim().is_empty() => Some("Enter a host".into()),
            DbKind::MsSql if self.cfg.ms_auth == MsAuth::WindowsCredentials && !self.cfg.user.contains('\\') && !self.cfg.user.contains('@') => {
                Some("Enter the Windows account as DOMAIN\\user or user@example.com".into())
            }
            DbKind::MsSql if self.cfg.ms_auth == MsAuth::SqlServer && self.cfg.user.trim().is_empty() => Some("Enter a SQL Server login".into()),
            _ if self.cfg.visible_schemas.as_ref().map(|l| l.is_empty()).unwrap_or(false) => Some("Pick at least one schema to show".into()),
            _ => None,
        }
    }
}

/// Engines whose servers listen for either TLS or plain text: DataBug signs
/// in once, so they offer Off / Require without Prefer.
fn tls_on_or_off(kind: DbKind) -> bool {
    matches!(kind, DbKind::Cassandra | DbKind::CouchDb | DbKind::InfluxDb | DbKind::Oracle | DbKind::Db2 | DbKind::Spark)
}

/// Engines with an optional URI that replaces host and port, with its hint.
fn uri_hint(kind: DbKind) -> Option<&'static str> {
    match kind {
        DbKind::MongoDb => Some("mongodb+srv://cluster.example.net (optional)"),
        DbKind::CouchDb => Some("https://couch.example.com (optional)"),
        DbKind::InfluxDb => Some("https://influx.example.com (optional, e.g. InfluxDB Cloud)"),
        DbKind::Oracle => Some("store_high or (DESCRIPTION=…) (optional, replaces host and service)"),
        _ => None,
    }
}

/// Label and hint for the database field.
fn database_field(kind: DbKind, sid: bool) -> (&'static str, &'static str) {
    match kind {
        DbKind::Postgres | DbKind::TimescaleDb => ("Database", "postgres"),
        DbKind::MsSql => ("Database", "master"),
        DbKind::MongoDb => ("Database", "test"),
        DbKind::CockroachDb => ("Database", "defaultdb"),
        DbKind::QuestDb => ("Database", "qdb"),
        DbKind::Oracle if sid => ("SID", "FREE"),
        DbKind::Oracle => ("Service name", "FREEPDB1"),
        DbKind::Db2 => ("Database", "STORE"),
        DbKind::Cassandra => ("Keyspace", "optional"),
        DbKind::CouchDb => ("Database", "optional; lists every database you can see"),
        DbKind::InfluxDb => ("Bucket", "the bucket (or 1.x database) to query"),
        DbKind::Spark => ("Database", "default"),
        DbKind::H2 => ("Database path", "~/store, as the H2 server sees it"),
        _ => ("Database", ""),
    }
}

/// A short note about what an engine needs, shown under the form.
fn engine_note(kind: DbKind) -> Option<&'static str> {
    match kind {
        DbKind::Oracle => Some("Needs Oracle Instant Client on this computer (see the README)."),
        DbKind::Db2 => Some("Needs the IBM Db2 ODBC driver registered with the ODBC driver manager."),
        DbKind::Spark => Some("Connects to the Spark Thrift server through a Spark ODBC driver (Simba or Databricks)."),
        DbKind::H2 => Some("Start the H2 server with -pg (PostgreSQL server mode); DataBug connects to that port."),
        DbKind::CouchDb => Some("Queries are Mango selectors in mongo-shell form: db.orders.find({status: \"open\"})."),
        DbKind::InfluxDb => Some("Runs InfluxQL, Flux (InfluxDB 2.x) and INSERT <line protocol>."),
        DbKind::Cassandra => Some("Runs CQL. Keyspaces appear as schemas."),
        DbKind::QuestDb => Some("Connects to QuestDB's PostgreSQL port (8812 by default)."),
        _ => None,
    }
}

/// DuckDB isn't built into DataBug: offer to download its library, and show
/// progress while that runs (the download carries on if the dialog closes).
fn duckdb_download_ui(ui: &mut egui::Ui, p: &Palette, rt: &tokio::runtime::Handle) {
    let Some(d) = databug_drivers::duckdb_download() else {
        ui.label(RichText::new("DuckDB doesn't publish a library for this platform.").color(p.warning));
        return;
    };
    let mb = |b: u64| b as f64 / 1_000_000.0;
    if d.state.running {
        let total = d.state.total.max(1);
        ui.add(egui::ProgressBar::new(d.state.downloaded as f32 / total as f32).desired_width(380.0).text(format!("Downloading DuckDB {}… {:.0} of {:.0} MB", d.version, mb(d.state.downloaded), mb(total))));
        // progress arrives from the download task, which also requests repaints
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        return;
    }
    if d.installed {
        ui.label(RichText::new(format!("DuckDB {} is installed", d.version)).color(p.success));
        return;
    }
    ui.label(RichText::new(format!("DuckDB isn't included in DataBug. Download its library once ({:.0} MB) from DuckDB's GitHub releases; DataBug checks it against a pinned checksum.", mb(d.size))).size(11.5).color(p.muted));
    ui.horizontal(|ui| {
        if ui.button(format!("Download DuckDB {}", d.version)).on_hover_text(d.url.as_str()).clicked() {
            let ctx = ui.ctx().clone();
            databug_drivers::start_duckdb_download(rt, std::sync::Arc::new(move || ctx.request_repaint()));
        }
    });
    if let Some(e) = &d.state.error {
        ui.label(RichText::new(format!("✖ {e}")).size(11.5).color(p.danger));
    }
}
