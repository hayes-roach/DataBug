use crate::compare::{CompareAction, CompareView, Phase};
use crate::csv_import::{CsvImport, ImportOutcome};
use crate::dialogs::{ConnDialog, DialogOutcome};
use crate::er::{ErAction, ErView};
use crate::explorer::{Action, CatalogView, ExplorerState};
use crate::icons::Logos;
use crate::rename_dialog::{RenameDialog, RenameOutcome};
use crate::table_editor::{EditorOutcome, TableEditor};
use crate::editor::Editor;
use crate::grid::{fmt_duration, group, EditTarget, GridAction, ResultView};
use crate::plan_view::PlanView;
use crate::session::{Cmd, Ev, EvSender, Session, SessionId};
use crate::store::{HistoryEntry, Settings, Store};
use crate::theme::{self, bold, connection_color, palette, Palette, ThemeKind};
use eframe::egui::{self, Color32, CornerRadius, FontId, Key, KeyboardShortcut, Margin, Modifiers, RichText, Sense, Stroke};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use databug_core::{Capabilities, Catalog, ConnectionConfig, DbKind, Dialect, ObjectKind, QueryEvent, TableInfo};
use databug_drivers::{ExecOptions, SourceRef};
use databug_sql::rename::{rename_references, target_at, RenameTarget};
use databug_sql::{classify, dangers, single_table_source, split_statements, statement_at, StatementKind};

const META_BIT: u64 = 1 << 62;

#[derive(Clone, PartialEq)]
enum ConnState {
    Idle,
    Connecting,
    Connected,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq)]
enum BottomTab {
    Output,
    Result(usize),
    Plan,
    History,
}

#[derive(Clone, Copy, PartialEq)]
enum LogKind {
    Info,
    Ok,
    Warn,
    Error,
}

struct LogLine {
    at: String,
    kind: LogKind,
    text: String,
    sql: Option<String>,
}

struct Running {
    job: u64,
    started: Instant,
    stmts: Vec<(String, Range<usize>)>,
    done: usize,
    explain: bool,
    analyze: bool,
    edit_refresh: Option<usize>,
    had_error: bool,
    /// reload the connection's schema when the job succeeds (DDL ran)
    refresh_after: bool,
    done_toast: Option<String>,
    /// result views created for this job (by statement index)
    result_for_stmt: HashMap<usize, usize>,
}

struct Tab {
    id: u64,
    conn_id: String,
    title: String,
    editor: Editor,
    results: Vec<ResultView>,
    bottom: BottomTab,
    output: Vec<LogLine>,
    plan: Option<PlanView>,
    running: Option<Running>,
    manual_tx: bool,
    in_tx: bool,
    session: Option<Session>,
    state: ConnState,
    caps: Capabilities,
    tool: Option<Tool>,
    /// (reload schema, toast) applied to the next job dispatched in this tab
    after_job: Option<(bool, Option<String>)>,
}

struct CatalogEntry {
    catalog: Catalog,
    generation: u64,
    loading: bool,
    error: Option<String>,
}

/// A full-area tool shown in a tab instead of a console.
enum Tool {
    Er(ErView),
    Compare(CompareView),
}

/// Why a batch of object definitions was requested.
enum SourcePurpose {
    Edit { conn_id: String },
    Compare { tab_id: u64, side: u8 },
    Rename { conn_id: String, schema: String, target: RenameTarget, new_name: String, script: String },
}

struct Confirm {
    tab_id: u64,
    title: String,
    reasons: Vec<String>,
    stmts: Vec<(String, Range<usize>)>,
    explain: Option<bool>,
    edit_refresh: Option<usize>,
}

struct Toast {
    text: String,
    until: Instant,
}

/// Steps for `--screenshot`: open the sample database, run a query, show completion.
struct ScreenshotPlan {
    path: std::path::PathBuf,
    step: u32,
    wait_frames: u32,
    /// main (completion), light, plan, dialog
    scene: String,
}

pub struct DataBugApp {
    rt: tokio::runtime::Runtime,
    store: Store,
    settings: Settings,
    connections: Vec<ConnectionConfig>,
    catalogs: HashMap<String, CatalogEntry>,
    refreshed: HashSet<String>,
    meta: HashMap<String, Session>,
    tabs: Vec<Tab>,
    active: usize,
    tx: EvSender,
    rx: mpsc::Receiver<(SessionId, Ev)>,
    next_id: u64,
    history: Vec<HistoryEntry>,
    history_filter: String,
    explorer: ExplorerState,
    logos: Logos,
    table_editor: Option<TableEditor>,
    csv: Option<CsvImport>,
    rename: Option<RenameDialog>,
    pending_sources: HashMap<u64, SourcePurpose>,
    dialog: Option<ConnDialog>,
    editing_conn: Option<String>,
    confirm: Option<Confirm>,
    toasts: Vec<Toast>,
    screenshot: Option<ScreenshotPlan>,
    mem: (Instant, Option<u64>),
    applied_zoom: Option<f32>,
    applied_theme: Option<(ThemeKind, f32)>,
    ctx: egui::Context,
}

fn now_hms() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let local = secs % 86_400;
    format!("{:02}:{:02}:{:02}", local / 3600, (local / 60) % 60, local % 60)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn ago(secs: u64) -> String {
    let d = now_secs().saturating_sub(secs);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86_399 => format!("{} h ago", d / 3600),
        _ => format!("{} d ago", d / 86_400),
    }
}

fn rss_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: u64 = s.split_whitespace().nth(1)?.parse().ok()?;
        Some(pages * 4096)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn one_line(sql: &str, max: usize) -> String {
    let s: String = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > max {
        s.chars().take(max).collect::<String>() + "…"
    } else {
        s
    }
}

pub fn select_top(dialect: Dialect, schema: &str, table: &str, default_schema: &str) -> String {
    let q = |s: &str| dialect.quote_ident(s);
    let name = if schema.is_empty() || schema.eq_ignore_ascii_case(default_schema) { q(table) } else { format!("{}.{}", q(schema), q(table)) };
    match dialect {
        Dialect::MsSql => format!("SELECT TOP 100 * FROM {name};"),
        Dialect::Mongo => format!("db.{table}.find({{}}).limit(100)"),
        _ => format!("SELECT * FROM {name} LIMIT 100;"),
    }
}

pub fn ddl(dialect: Dialect, schema: &str, t: &TableInfo) -> String {
    if dialect == Dialect::Mongo {
        let fields: Vec<String> = t.columns.iter().map(|c| format!("//   {}: {}", c.name, c.data_type)).collect();
        return format!("// Collection {} — fields seen in a sample of documents:\n{}\ndb.{}.find({{}}).limit(20)", t.name, fields.join("\n"), t.name);
    }
    databug_sql::ddl::script(dialect, &databug_sql::ddl::create_table(dialect, schema, t))
}

const DEMO_SQL: &str = "-- Top customers by lifetime value\nSELECT c.name, c.city, COUNT(o.id) AS orders, SUM(o.total) AS lifetime_total\nFROM customers c\nJOIN orders o ON o.customer_id = c.id\nWHERE o.status = 'shipped'\nGROUP BY c.name, c.city\nORDER BY lifetime_total DESC\nLIMIT 200;\n";

impl DataBugApp {
    pub fn new(cc: &eframe::CreationContext<'_>, screenshot: Option<std::path::PathBuf>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().thread_name("databug-io").build().expect("tokio runtime");
        let store = Store::open();
        let settings = store.load_settings();
        let connections = store.load_connections();
        let history = store.load_history(2000);
        let (tx, rx) = mpsc::channel();
        let mut app = DataBugApp {
            rt,
            store,
            settings,
            connections,
            catalogs: HashMap::new(),
            refreshed: HashSet::new(),
            meta: HashMap::new(),
            tabs: Vec::new(),
            active: 0,
            tx,
            rx,
            next_id: 1,
            history,
            history_filter: String::new(),
            explorer: ExplorerState::default(),
            logos: Logos::default(),
            table_editor: None,
            csv: None,
            rename: None,
            pending_sources: HashMap::new(),
            dialog: None,
            editing_conn: None,
            confirm: None,
            toasts: Vec::new(),
            screenshot: screenshot.map(|path| ScreenshotPlan { path, step: 0, wait_frames: 0, scene: std::env::var("DATABUG_SHOT_SCENE").unwrap_or_else(|_| "main".into()) }),
            mem: (Instant::now() - Duration::from_secs(10), None),
            applied_theme: None,
            applied_zoom: None,
            ctx: cc.egui_ctx.clone(),
        };
        egui_extras::install_image_loaders(&cc.egui_ctx);
        app.logos = Logos::discover(&cc.egui_ctx);
        for c in app.connections.clone() {
            if let Some(cat) = app.store.load_catalog(&c.id) {
                app.catalogs.insert(c.id.clone(), CatalogEntry { catalog: cat, generation: 1, loading: false, error: None });
            }
        }
        app
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast { text: text.into(), until: Instant::now() + Duration::from_secs(4) });
    }

    fn conn(&self, id: &str) -> Option<&ConnectionConfig> {
        self.connections.iter().find(|c| c.id == id)
    }

    fn catalog(&self, conn_id: &str) -> Option<&Catalog> {
        self.catalogs.get(conn_id).map(|e| &e.catalog)
    }

    // ---------- sessions and catalogs ----------

    fn spawn_session(&self, id: SessionId, cfg: &ConnectionConfig, ctx: &egui::Context) -> Session {
        let pw = self.store.password(&cfg.id);
        Session::spawn(self.rt.handle(), id, cfg.clone(), pw, self.tx.clone(), ctx.clone())
    }

    fn refresh_catalog(&mut self, conn_id: &str, ctx: &egui::Context) {
        let Some(cfg) = self.conn(conn_id).cloned() else { return };
        if !self.meta.contains_key(conn_id) {
            let sid = META_BIT | self.id();
            let s = self.spawn_session(sid, &cfg, ctx);
            self.meta.insert(conn_id.to_string(), s);
        }
        self.meta[conn_id].send(Cmd::Introspect);
        self.refreshed.insert(conn_id.to_string());
        let e = self.catalogs.entry(conn_id.to_string()).or_insert(CatalogEntry { catalog: Catalog::default(), generation: 0, loading: false, error: None });
        e.loading = true;
        e.error = None;
    }

    fn ensure_catalog(&mut self, conn_id: &str, ctx: &egui::Context) {
        if !self.refreshed.contains(conn_id) {
            self.refresh_catalog(conn_id, ctx);
        }
    }

    // ---------- tabs ----------

    fn open_console(&mut self, conn_id: &str, text: Option<String>, ctx: &egui::Context) -> usize {
        let Some(cfg) = self.conn(conn_id).cloned() else { return self.active };
        let id = self.id();
        let n = self.tabs.iter().filter(|t| t.conn_id == conn_id).count();
        let title = if n == 0 { cfg.name.clone() } else { format!("{} ({})", cfg.name, n + 1) };
        let text = text.unwrap_or_else(|| if cfg.kind == DbKind::MongoDb { "db.".into() } else { String::new() });
        let mut editor = Editor::new(egui::Id::new(("editor", id)), text, cfg.kind.dialect());
        editor.set_text(editor.text.clone());
        self.tabs.push(Tab {
            id,
            conn_id: conn_id.to_string(),
            title,
            editor,
            results: Vec::new(),
            bottom: BottomTab::Output,
            output: Vec::new(),
            plan: None,
            running: None,
            manual_tx: false,
            in_tx: false,
            session: None,
            state: ConnState::Idle,
            caps: Capabilities::default(),
            tool: None,
            after_job: None,
        });
        self.active = self.tabs.len() - 1;
        self.ensure_catalog(conn_id, ctx);
        self.explorer.reveal = Some(conn_id.to_string());
        self.active
    }

    fn close_tab(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            if self.tabs[idx].in_tx {
                self.toast("The open transaction was rolled back when the console closed");
            }
            self.tabs.remove(idx);
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len().saturating_sub(1);
            }
        }
    }

    // ---------- running ----------

    /// Statements the run command applies to: the selection, the whole
    /// script, or the statement under the cursor.
    fn targets(tab: &Tab, script: bool) -> Vec<(String, Range<usize>)> {
        let d = tab.editor.dialect;
        let text = &tab.editor.text;
        if let Some(sel) = tab.editor.selection.clone().filter(|_| tab.editor.selected_text().is_some()) {
            let part = &text[sel.clone()];
            return split_statements(part, d).into_iter().map(|r| (part[r.clone()].to_string(), sel.start + r.start..sel.start + r.end)).collect();
        }
        if script {
            return split_statements(text, d).into_iter().map(|r| (text[r.clone()].to_string(), r)).collect();
        }
        statement_at(text, tab.editor.cursor, d).map(|r| vec![(text[r.clone()].to_string(), r)]).unwrap_or_default()
    }

    fn request_run(&mut self, script: bool, explain: Option<bool>, ctx: &egui::Context) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        if tab.running.is_some() {
            self.toast("A query is already running in this console");
            return;
        }
        let mut stmts = Self::targets(tab, script);
        if explain.is_some() {
            stmts.truncate(1);
        }
        if stmts.is_empty() {
            self.toast("Nothing to run here");
            return;
        }
        let tab_id = tab.id;
        self.check_and_dispatch(tab_id, stmts, explain, None, ctx);
    }

    fn check_and_dispatch(&mut self, tab_id: u64, stmts: Vec<(String, Range<usize>)>, explain: Option<bool>, edit_refresh: Option<usize>, ctx: &egui::Context) {
        let Some(ti) = self.tabs.iter().position(|t| t.id == tab_id) else { return };
        let Some(cfg) = self.conn(&self.tabs[ti].conn_id).cloned() else { return };
        let d = cfg.kind.dialect();
        let analyze_runs = explain == Some(true);
        let mut reasons = Vec::new();
        for (s, _) in &stmts {
            let kind = classify(s, d);
            if explain.is_some() && !analyze_runs {
                continue;
            }
            if cfg.read_only && kind.modifies() && !(analyze_runs && kind != StatementKind::Ddl) {
                let t = &mut self.tabs[ti];
                t.output.push(LogLine { at: now_hms(), kind: LogKind::Error, text: format!("Blocked: {} is read-only", cfg.name), sql: Some(one_line(s, 160)) });
                t.bottom = BottomTab::Output;
                return;
            }
            for dz in dangers(s, d) {
                reasons.push(format!("{} — {}", dz.message(), one_line(s, 80)));
            }
            if cfg.production && kind.modifies() && !analyze_runs {
                reasons.push(format!("Changes data on production connection “{}” — {}", cfg.name, one_line(s, 80)));
            }
        }
        reasons.dedup();
        if !reasons.is_empty() {
            self.confirm = Some(Confirm { tab_id, title: if cfg.production { "Run on production?".into() } else { "Run this statement?".into() }, reasons, stmts, explain, edit_refresh });
            return;
        }
        self.dispatch(ti, stmts, explain, edit_refresh, ctx);
    }

    fn dispatch(&mut self, ti: usize, mut stmts: Vec<(String, Range<usize>)>, explain: Option<bool>, edit_refresh: Option<usize>, ctx: &egui::Context) {
        let job = self.id();
        let Some(cfg) = self.conn(&self.tabs[ti].conn_id).cloned() else { return };
        if self.tabs[ti].session.is_none() {
            let sid = self.tabs[ti].id;
            let s = self.spawn_session(sid, &cfg, ctx);
            self.tabs[ti].session = Some(s);
        }
        let d = cfg.kind.dialect();
        let limit = self.settings.row_limit;
        let tab = &mut self.tabs[ti];
        if let Some(analyze) = explain {
            let (sql, range) = stmts.remove(0);
            tab.editor.running = Some(range.clone());
            tab.output.push(LogLine { at: now_hms(), kind: LogKind::Info, text: if analyze { "Explain analyze".into() } else { "Explain plan".into() }, sql: Some(one_line(&sql, 200)) });
            tab.running = Some(Running { job, started: Instant::now(), stmts: vec![(sql.clone(), range)], done: 0, explain: true, analyze, edit_refresh: None, had_error: false, refresh_after: false, done_toast: None, result_for_stmt: HashMap::new() });
            tab.session.as_ref().unwrap().send(Cmd::Explain { job, stmt: sql, analyze });
            return;
        }
        // manual transaction mode: open a transaction before the first change
        if tab.manual_tx && !tab.in_tx && (tab.caps.transactions || d != Dialect::Mongo) && stmts.iter().any(|(s, _)| classify(s, d).modifies()) {
            let r = stmts[0].1.start..stmts[0].1.start;
            stmts.insert(0, (d.begin_sql().to_string(), r));
        }
        if edit_refresh.is_none() {
            tab.results.clear();
            tab.plan = None;
        }
        let first = stmts.first().map(|s| s.1.start).unwrap_or(0);
        let last = stmts.last().map(|s| s.1.end).unwrap_or(0);
        tab.editor.running = Some(first..last);
        tab.editor.error = None;
        let n = stmts.len();
        tab.output.push(LogLine {
            at: now_hms(),
            kind: LogKind::Info,
            text: if n == 1 { "Running".into() } else { format!("Running {n} statements") },
            sql: (n == 1).then(|| one_line(&stmts[0].0, 200)),
        });
        tab.session.as_ref().unwrap().send(Cmd::Execute { job, stmts: stmts.iter().map(|s| s.0.clone()).collect(), opts: ExecOptions { row_limit: limit } });
        let (refresh_after, done_toast) = tab.after_job.take().unwrap_or((false, None));
        tab.running = Some(Running { job, started: Instant::now(), stmts, done: 0, explain: false, analyze: false, edit_refresh, had_error: false, refresh_after, done_toast, result_for_stmt: HashMap::new() });
    }

    fn tx_command(&mut self, sql: &str, ctx: &egui::Context) {
        if let Some(t) = self.tabs.get(self.active) {
            if t.running.is_some() {
                return;
            }
            let ti = self.active;
            let r = t.editor.cursor..t.editor.cursor;
            self.dispatch(ti, vec![(sql.to_string(), r)], None, Some(usize::MAX), ctx);
        }
    }

    // ---------- events ----------

    fn pump_events(&mut self, ctx: &egui::Context) {
        let mut sources_done = Vec::new();
        self.pump_inner(&mut sources_done);
        for (req, results) in sources_done {
            self.sources_arrived(req, results, ctx);
        }
        self.poll_compare(ctx);
    }

    fn pump_inner(&mut self, sources_done: &mut Vec<(u64, Vec<(SourceRef, Result<String, String>)>)>) {
        while let Ok((sid, ev)) = self.rx.try_recv() {
            if sid & META_BIT != 0 {
                let Some(conn_id) = self.meta.iter().find(|(_, s)| s.id == sid).map(|(k, _)| k.clone()) else { continue };
                match ev {
                    Ev::Catalog(Ok(cat)) => {
                        self.store.save_catalog(&conn_id, &cat);
                        let e = self.catalogs.entry(conn_id.clone()).or_insert(CatalogEntry { catalog: Catalog::default(), generation: 0, loading: false, error: None });
                        e.catalog = cat;
                        e.generation += 1;
                        e.loading = false;
                        e.error = None;
                    }
                    Ev::Sources { req, results } => {
                        sources_done.push((req, results));
                    }
                    Ev::Catalog(Err(msg)) | Ev::ConnectFailed(msg) => {
                        if let Some(e) = self.catalogs.get_mut(&conn_id) {
                            e.loading = false;
                            e.error = Some(msg);
                        }
                        self.meta.remove(&conn_id);
                    }
                    _ => {}
                }
                continue;
            }
            let Some(ti) = self.tabs.iter().position(|t| t.id == sid) else { continue };
            let conn_id = self.tabs[ti].conn_id.clone();
            let cfg = self.conn(&conn_id).cloned();
            let mut history: Option<HistoryEntry> = None;
            let mut follow_refresh = false;
            let mut follow_toast: Option<String> = None;
            {
                let catalog = self.catalogs.get(&conn_id).map(|e| &e.catalog);
                let tab = &mut self.tabs[ti];
                let d = tab.editor.dialect;
                match ev {
                    Ev::Connecting => tab.state = ConnState::Connecting,
                    Ev::Connected(caps) => {
                        tab.state = ConnState::Connected;
                        tab.caps = caps;
                    }
                    Ev::ConnectFailed(msg) => {
                        tab.state = ConnState::Failed(msg.clone());
                        tab.output.push(LogLine { at: now_hms(), kind: LogKind::Error, text: format!("Couldn't connect: {msg}"), sql: None });
                        tab.session = None;
                    }
                    Ev::Dropped => {
                        tab.state = ConnState::Idle;
                        if tab.in_tx {
                            tab.output.push(LogLine { at: now_hms(), kind: LogKind::Warn, text: "The connection was reset, so the open transaction was rolled back".into(), sql: None });
                        }
                        tab.in_tx = false;
                    }
                    Ev::Catalog(_) | Ev::Sources { .. } => {}
                    Ev::Query { job, idx, ev } => {
                        let Some(run) = tab.running.as_mut().filter(|r| r.job == job) else { continue };
                        match ev {
                            QueryEvent::Columns(cols) => {
                                let n = tab.results.len() + 1;
                                let sql = run.stmts.get(idx).map(|s| s.0.clone()).unwrap_or_default();
                                let src = single_table_source(&sql, d);
                                let title = src.as_ref().map(|t| t.name.clone()).unwrap_or_else(|| format!("Result {n}"));
                                let mut view = ResultView::new(title, cols, d, run.job * 100 + idx as u64);
                                if let (Some(src), Some(cat)) = (src, catalog) {
                                    if let Some(t) = cat.find_table(src.schema.as_deref(), &src.name) {
                                        let pk_cols: Vec<usize> = t.primary_key().iter().filter_map(|pk| view.columns.iter().position(|c| c.name.eq_ignore_ascii_case(&pk.name))).collect();
                                        if !pk_cols.is_empty() && pk_cols.len() == t.primary_key().len() && t.kind == ObjectKind::Table {
                                            let schema = cat.tables().find(|(_, tt)| std::ptr::eq(*tt, t)).map(|(s, _)| s.name.clone()).unwrap_or_default();
                                            let qualified = if d == Dialect::Sqlite && schema == "main" { d.quote_ident(&t.name) } else { format!("{}.{}", d.quote_ident(&schema), d.quote_ident(&t.name)) };
                                            let numeric = view.columns.iter().map(|c| c.is_numeric() || t.column(&c.name).map(|ci| databug_core::ColumnMeta::new("", ci.data_type.clone()).is_numeric()).unwrap_or(false)).collect();
                                            view.edit_target = Some(EditTarget { qualified, pk_cols, numeric });
                                        }
                                    }
                                }
                                tab.results.push(view);
                                run.result_for_stmt.insert(idx, tab.results.len() - 1);
                                if !matches!(tab.bottom, BottomTab::Result(_)) || tab.results.len() == 1 {
                                    tab.bottom = BottomTab::Result(tab.results.len() - 1);
                                }
                            }
                            QueryEvent::Rows(rows) => {
                                if let Some(&ri) = run.result_for_stmt.get(&idx) {
                                    tab.results[ri].append(rows);
                                }
                            }
                            QueryEvent::Truncated => {
                                if let Some(&ri) = run.result_for_stmt.get(&idx) {
                                    tab.results[ri].truncated = true;
                                }
                            }
                            QueryEvent::Affected(_) if run.stmts.get(idx).map(|s| matches!(classify(&s.0, d), StatementKind::Transaction | StatementKind::Ddl)).unwrap_or(false) => {}
                            QueryEvent::Affected(n) => {
                                tab.output.push(LogLine { at: now_hms(), kind: LogKind::Ok, text: format!("{} row{} affected", group(n as usize), if n == 1 { "" } else { "s" }), sql: None });
                            }
                            QueryEvent::Message(m) => tab.output.push(LogLine { at: now_hms(), kind: LogKind::Info, text: m, sql: None }),
                        }
                    }
                    Ev::StmtDone { job, idx, elapsed, error } => {
                        let Some(run) = tab.running.as_mut().filter(|r| r.job == job) else { continue };
                        run.done = idx + 1;
                        let (sql, range) = run.stmts.get(idx).cloned().unwrap_or_default();
                        if let Some(&ri) = run.result_for_stmt.get(&idx) {
                            tab.results[ri].elapsed = Some(elapsed);
                            let rows = tab.results[ri].row_count();
                            tab.output.push(LogLine { at: now_hms(), kind: LogKind::Ok, text: format!("{} row{} in {}{}", group(rows), if rows == 1 { "" } else { "s" }, fmt_duration(elapsed), if tab.results[ri].truncated { " (row limit reached)" } else { "" }), sql: None });
                        } else if error.is_none() {
                            tab.output.push(LogLine { at: now_hms(), kind: LogKind::Ok, text: format!("Completed in {}", fmt_duration(elapsed)), sql: Some(one_line(&sql, 120)) });
                        }
                        match &error {
                            Some(e) => {
                                run.had_error = true;
                                tab.output.push(LogLine { at: now_hms(), kind: LogKind::Error, text: e.clone(), sql: Some(one_line(&sql, 200)) });
                                tab.editor.error = Some(range);
                                tab.bottom = BottomTab::Output;
                            }
                            None => {
                                if classify(&sql, d) == StatementKind::Transaction {
                                    let up = sql.trim_start().to_ascii_uppercase();
                                    tab.in_tx = up.starts_with("BEGIN") || up.starts_with("START");
                                }
                            }
                        }
                        if run.edit_refresh != Some(usize::MAX) {
                            history = Some(HistoryEntry { at: now_secs(), connection: cfg.as_ref().map(|c| c.name.clone()).unwrap_or_default(), sql: sql.clone(), ms: elapsed.as_millis() as u64, ok: error.is_none() });
                        }
                    }
                    Ev::JobDone { job } => {
                        if tab.running.as_ref().map(|r| r.job == job).unwrap_or(false) {
                            let run = tab.running.take().unwrap();
                            if let Some(ri) = run.edit_refresh.filter(|i| *i != usize::MAX) {
                                if !run.had_error {
                                    if let Some(v) = tab.results.get_mut(ri) {
                                        v.edits_applied();
                                    }
                                    tab.output.push(LogLine { at: now_hms(), kind: LogKind::Ok, text: "Changes saved".into(), sql: None });
                                    tab.bottom = BottomTab::Result(ri);
                                }
                            }
                            if tab.results.is_empty() && !run.had_error && !matches!(tab.bottom, BottomTab::History) {
                                tab.bottom = BottomTab::Output;
                            }
                            tab.editor.running = None;
                            if let Some(s) = tab.session.as_mut() {
                                s.job_finished();
                            }
                            if !run.had_error {
                                if run.refresh_after {
                                    follow_refresh = true;
                                }
                                if let Some(t) = run.done_toast {
                                    follow_toast = Some(t);
                                }
                            }
                        }
                    }
                    Ev::Plan { job, result, elapsed } => {
                        if tab.running.as_ref().map(|r| r.job == job).unwrap_or(false) {
                            let run = tab.running.take().unwrap();
                            tab.editor.running = None;
                            match result {
                                Ok(plan) => {
                                    tab.plan = Some(PlanView::new(plan, run.analyze, elapsed));
                                    tab.bottom = BottomTab::Plan;
                                    tab.output.push(LogLine { at: now_hms(), kind: LogKind::Ok, text: format!("Plan ready in {}", fmt_duration(elapsed)), sql: None });
                                }
                                Err(e) => {
                                    tab.output.push(LogLine { at: now_hms(), kind: LogKind::Error, text: e, sql: None });
                                    tab.bottom = BottomTab::Output;
                                }
                            }
                            if let Some(s) = tab.session.as_mut() {
                                s.job_finished();
                            }
                        }
                    }
                }
                if tab.output.len() > 2000 {
                    tab.output.drain(..500);
                }
            }
            if follow_refresh {
                self.refreshed.remove(&conn_id);
                self.refresh_after_ddl(&conn_id);
            }
            if let Some(t) = follow_toast {
                self.toast(t);
            }
            if let Some(h) = history {
                self.store.append_history(&h);
                self.history.push(h);
                if self.history.len() > 5000 {
                    self.history.drain(..1000);
                }
            }
        }
    }

    // ---------- actions ----------

    fn apply(&mut self, a: Action, ctx: &egui::Context) {
        match a {
            Action::NewConsole(id) => {
                self.open_console(&id, None, ctx);
            }
            Action::SelectTop(id, schema, table) => {
                let Some(cfg) = self.conn(&id).cloned() else { return };
                let default = self.catalog(&id).map(|c| c.default_schema.clone()).unwrap_or_default();
                let sql = select_top(cfg.kind.dialect(), &schema, &table, &default);
                let ti = match self.tabs.get(self.active).filter(|t| t.conn_id == id && t.running.is_none()) {
                    Some(_) => self.active,
                    None => self.open_console(&id, None, ctx),
                };
                let tab = &mut self.tabs[ti];
                let mut text = tab.editor.text.trim_end().to_string();
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                let start = text.len();
                text.push_str(&sql);
                tab.editor.set_text(text);
                let end = tab.editor.text.len();
                tab.editor.cursor = end;
                tab.editor.selection = None;
                let range = start..end;
                let stmt = sql.trim_end_matches(';').to_string();
                let tab_id = tab.id;
                self.check_and_dispatch(tab_id, vec![(stmt, range)], None, None, ctx);
            }
            Action::OpenDdl(id, schema, table) => {
                let Some(cfg) = self.conn(&id).cloned() else { return };
                let text = self.catalog(&id).and_then(|c| c.find_table(Some(&schema), &table)).map(|t| ddl(cfg.kind.dialect(), &schema, t));
                if let Some(text) = text {
                    let ti = self.open_console(&id, Some(text), ctx);
                    self.tabs[ti].title = format!("{table} DDL");
                }
            }
            Action::Refresh(id) => self.refresh_catalog(&id, ctx),
            Action::SetSchemas(id, list) => {
                if let Some(c) = self.connections.iter_mut().find(|c| c.id == id) {
                    c.visible_schemas = list;
                    self.store.save_connections(&self.connections);
                }
            }
            Action::EditConn(id) => {
                if let Some(cfg) = self.conn(&id).cloned() {
                    let pw = self.store.password(&id).unwrap_or_default();
                    let mut d = ConnDialog::new(cfg, pw, false);
                    if let Some(cat) = self.catalog(&id) {
                        d.known_schemas = cat.schemas.iter().map(|s| s.name.clone()).collect();
                        d.default_schema = cat.default_schema.clone();
                    }
                    self.dialog = Some(d);
                    self.editing_conn = Some(id);
                }
            }
            Action::DuplicateConn(id) => {
                if let Some(mut cfg) = self.conn(&id).cloned() {
                    let pw = self.store.password(&id).unwrap_or_default();
                    cfg.id = ConnectionConfig::new(cfg.kind).id;
                    cfg.name = format!("{} copy", cfg.name);
                    self.dialog = Some(ConnDialog::new(cfg, pw, true));
                    self.editing_conn = None;
                }
            }
            Action::DeleteConn(id) => {
                self.connections.retain(|c| c.id != id);
                self.store.save_connections(&self.connections);
                self.store.forget_password(&id);
                self.store.delete_catalog(&id);
                self.catalogs.remove(&id);
                self.meta.remove(&id);
                self.tabs.retain(|t| t.conn_id != id);
                self.active = self.active.min(self.tabs.len().saturating_sub(1));
            }
            Action::Copy(s) => {
                ctx.copy_text(s);
                self.toast("Copied");
            }
            Action::Insert(s) => {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    t.editor.insert(&s);
                }
            }
            Action::NewConnection => {
                let kind = databug_drivers::available().first().copied().unwrap_or(DbKind::Postgres);
                self.dialog = Some(ConnDialog::new(ConnectionConfig::new(kind), String::new(), true));
                self.editing_conn = None;
            }
            Action::OpenSample => self.open_sample(ctx),
            Action::EditTable(id, schema, table) => {
                let Some(cfg) = self.conn(&id).cloned() else { return };
                let Some(cat) = self.catalog(&id) else { return };
                let names: Vec<String> = cat.schema(&schema).map(|s| s.tables.iter().filter(|t| t.kind == ObjectKind::Table).map(|t| t.name.clone()).collect()).unwrap_or_default();
                match cat.find_table(Some(&schema), &table).cloned() {
                    Some(t) => self.table_editor = Some(TableEditor::new(id, schema, cfg.kind.dialect(), Some(t), names)),
                    None => self.toast(format!("{table} isn't in the loaded schema; refresh and try again")),
                }
            }
            Action::NewTable(id, schema) => {
                let Some(cfg) = self.conn(&id).cloned() else { return };
                let names: Vec<String> = self.catalog(&id).and_then(|c| c.schema(&schema)).map(|s| s.tables.iter().filter(|t| t.kind == ObjectKind::Table).map(|t| t.name.clone()).collect()).unwrap_or_default();
                self.table_editor = Some(TableEditor::new(id, schema, cfg.kind.dialect(), None, names));
            }
            Action::EditObject(id, schema, r) => {
                let name = r.name.clone();
                self.request_sources(&id, &schema, vec![r], SourcePurpose::Edit { conn_id: id.clone() }, ctx);
                self.toast(format!("Loading the definition of {name}…"));
            }
            Action::DropObject(id, schema, r) => {
                let Some(cfg) = self.conn(&id).cloned() else { return };
                let d = cfg.kind.dialect();
                let stmt = if r.kind.is_relation() {
                    databug_sql::ddl::drop_relation(d, &schema, &r.name, r.kind)
                } else {
                    databug_sql::diff::drop_object(d, &schema, &databug_core::DbObject { name: r.name.clone(), kind: r.kind, detail: r.detail.clone(), ident: r.ident.clone(), table: r.table.clone() })
                };
                let text = format!("-- Review before running: this permanently removes {} {}\n{}", r.kind.label(), r.name, databug_sql::ddl::script(d, &[stmt]));
                self.open_script(&id, &format!("Drop {}", r.name), text, ctx);
            }
            Action::ErDiagram(id, schema) => {
                let Some(s) = self.catalog(&id).and_then(|c| c.schema(&schema)).cloned() else {
                    self.toast("Load the schema first");
                    return;
                };
                let title = format!("ER · {}", schema);
                self.open_tool(&id, title, Tool::Er(ErView::new(id.clone(), &s)));
            }
            Action::Compare(id, schema) => {
                // default target: another connection of the same kind, else the same connection
                let kind = self.conn(&id).map(|c| c.kind);
                let other = self.connections.iter().find(|c| c.id != id && Some(c.kind) == kind).map(|c| c.id.clone()).unwrap_or_else(|| id.clone());
                let other_schema = self.catalog(&other).map(|c| c.default_schema.clone()).unwrap_or_else(|| schema.clone());
                self.open_tool(&id, "Schema compare".into(), Tool::Compare(CompareView::new((id.clone(), schema), (other, other_schema))));
            }
            Action::ImportCsv(id, schema, table) => {
                let Some(cfg) = self.conn(&id).cloned() else { return };
                let Some(path) = rfd::FileDialog::new().add_filter("CSV", &["csv", "tsv", "txt"]).add_filter("All files", &["*"]).pick_file() else { return };
                let tables = self.catalog(&id).and_then(|c| c.schema(&schema)).map(|s| s.tables.clone()).unwrap_or_default();
                self.csv = Some(CsvImport::new(id, schema, cfg.kind, path, tables, table));
            }
            Action::Rename(id, schema, target) => {
                self.rename = Some(RenameDialog::new(id, schema, target, None));
            }
        }
    }

    fn open_sample(&mut self, ctx: &egui::Context) {
        let path = self.store.data_dir().join("sample-store.db");
        if !path.exists() {
            let mut cfg = ConnectionConfig::new(DbKind::Sqlite);
            cfg.path = path.display().to_string();
            let res: anyhow::Result<()> = self.rt.block_on(async {
                let mut c = databug_drivers::connect(&cfg, None).await?;
                let sink: databug_drivers::Sink = std::sync::Arc::new(|_| {});
                let sql = include_str!("../assets/sample.sql");
                for r in split_statements(sql, Dialect::Sqlite) {
                    c.execute(&sql[r], &ExecOptions::default(), &sink).await?;
                }
                Ok(())
            });
            if let Err(e) = res {
                let _ = std::fs::remove_file(&path);
                self.toast(format!("Couldn't create the sample database: {e:#}"));
                return;
            }
        }
        let path_s = path.display().to_string();
        let id = match self.connections.iter().find(|c| c.kind == DbKind::Sqlite && c.path == path_s) {
            Some(c) => c.id.clone(),
            None => {
                let mut cfg = ConnectionConfig::new(DbKind::Sqlite);
                cfg.name = "Sample store".into();
                cfg.path = path_s;
                cfg.color = [95, 184, 101];
                let id = cfg.id.clone();
                self.connections.push(cfg);
                self.store.save_connections(&self.connections);
                id
            }
        };
        self.open_console(&id, Some(DEMO_SQL.into()), ctx);
    }


    // ---------- schema-changing work ----------

    fn refresh_after_ddl(&mut self, conn_id: &str) {
        let ctx = self.ctx.clone();
        self.refresh_catalog(conn_id, &ctx);
    }

    fn ensure_meta(&mut self, conn_id: &str, ctx: &egui::Context) -> bool {
        let Some(cfg) = self.conn(conn_id).cloned() else { return false };
        if !self.meta.contains_key(conn_id) {
            let sid = META_BIT | self.id();
            let s = self.spawn_session(sid, &cfg, ctx);
            self.meta.insert(conn_id.to_string(), s);
        }
        true
    }

    fn request_sources(&mut self, conn_id: &str, schema: &str, items: Vec<SourceRef>, purpose: SourcePurpose, ctx: &egui::Context) -> u64 {
        let req = self.id();
        if self.ensure_meta(conn_id, ctx) {
            self.meta[conn_id].send(Cmd::Sources { req, schema: schema.to_string(), items });
            self.pending_sources.insert(req, purpose);
        }
        req
    }

    /// Put a script in a console for this connection and run it.
    fn run_in_console(&mut self, conn_id: &str, script: String, refresh: bool, toast: Option<String>, ctx: &egui::Context) {
        let Some(cfg) = self.conn(conn_id).cloned() else { return };
        let d = cfg.kind.dialect();
        let ti = match self.tabs.get(self.active).filter(|t| t.conn_id == conn_id && t.running.is_none() && t.tool.is_none()) {
            Some(_) => self.active,
            None => self.open_console(conn_id, None, ctx),
        };
        let tab = &mut self.tabs[ti];
        let mut text = tab.editor.text.trim_end().to_string();
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        let base = text.len();
        text.push_str(&script);
        tab.editor.set_text(text);
        let stmts: Vec<(String, Range<usize>)> = split_statements(&script, d).into_iter().map(|r| (script[r.clone()].to_string(), base + r.start..base + r.end)).collect();
        if stmts.is_empty() {
            return;
        }
        tab.after_job = Some((refresh, toast));
        let tab_id = tab.id;
        self.active = ti;
        self.check_and_dispatch(tab_id, stmts, None, None, ctx);
    }

    fn open_script(&mut self, conn_id: &str, title: &str, script: String, ctx: &egui::Context) {
        let ti = self.open_console(conn_id, Some(script), ctx);
        self.tabs[ti].title = title.to_string();
        self.tabs[ti].editor.select(0..0);
    }

    fn sources_arrived(&mut self, req: u64, results: Vec<(SourceRef, Result<String, String>)>, ctx: &egui::Context) {
        let Some(purpose) = self.pending_sources.remove(&req) else { return };
        match purpose {
            SourcePurpose::Edit { conn_id } => {
                for (r, res) in results {
                    match res {
                        Ok(text) => {
                            self.open_script(&conn_id, &r.name, text, ctx);
                            self.toast(format!("Editing {} {} — run it with Ctrl/⌘ + Shift + Enter to save", r.kind.label(), r.name));
                        }
                        Err(e) => self.toast(format!("Couldn't load {}: {e}", r.name)),
                    }
                }
            }
            SourcePurpose::Compare { tab_id, side } => {
                let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else { return };
                let Some(Tool::Compare(cv)) = tab.tool.as_mut() else { return };
                let (src_schema, dst_schema) = (cv.left.1.clone(), cv.right.1.clone());
                let src_d = self.connections.iter().find(|c| c.id == cv.left.0).map(|c| c.kind.dialect()).unwrap_or(Dialect::Postgres);
                for (r, res) in results {
                    let Ok(mut text) = res else { continue };
                    if side == 0 && src_schema != dst_schema {
                        // point the definition at the target schema
                        text = text.replace(&format!("{}.", src_d.quote_ident(&src_schema)), &format!("{}.", src_d.quote_ident(&dst_schema)));
                    }
                    let key = databug_sql::diff::source_key(r.kind, &r.name, &r.detail);
                    if side == 0 {
                        cv.src_sources.insert(key, text);
                    } else {
                        cv.dst_sources.insert(key, text);
                    }
                }
                cv.pending.remove(&req);
            }
            SourcePurpose::Rename { conn_id, schema, target, new_name, mut script } => {
                let d = self.conn(&conn_id).map(|c| c.kind.dialect()).unwrap_or(Dialect::Postgres);
                let catalog = self.catalog(&conn_id).cloned();
                let mut changed = Vec::new();
                for (r, res) in results {
                    let Ok(text) = res else { continue };
                    let (new_text, n) = rename_references(&text, d, &target, &new_name, catalog.as_ref());
                    if n > 0 {
                        changed.push(r.name.clone());
                        script.push_str(&format!("\n-- {} {} ({} reference{})\n", r.kind.label(), r.name, n, if n == 1 { "" } else { "s" }));
                        script.push_str(new_text.trim_end());
                        script.push('\n');
                    }
                }
                let title = format!("Rename {}", target.current_name());
                self.open_script(&conn_id, &title, script, ctx);
                self.toast(if changed.is_empty() { "Review the rename script, then run it with Ctrl/⌘ + Shift + Enter".to_string() } else { format!("Also updates {} — review, then run with Ctrl/⌘ + Shift + Enter", changed.join(", ")) });
                let _ = schema;
            }
        }
    }

    fn start_compare(&mut self, tab_id: u64, ctx: &egui::Context) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == tab_id) else { return };
        let Some(Tool::Compare(cv)) = tab.tool.as_ref() else { return };
        let (l, r) = (cv.left.0.clone(), cv.right.0.clone());
        let gens = (self.catalogs.get(&l).map(|e| e.generation).unwrap_or(0), self.catalogs.get(&r).map(|e| e.generation).unwrap_or(0));
        self.refresh_catalog(&l, ctx);
        if r != l {
            self.refresh_catalog(&r, ctx);
        }
        if let Some(Tool::Compare(cv)) = self.tabs.iter_mut().find(|t| t.id == tab_id).and_then(|t| t.tool.as_mut()) {
            cv.wait_gens = gens;
            cv.phase = Phase::Catalogs;
            cv.status = "Loading both schemas…".into();
            cv.src_sources.clear();
            cv.dst_sources.clear();
            cv.pending.clear();
        }
    }

    fn poll_compare(&mut self, ctx: &egui::Context) {
        let ids: Vec<u64> = self.tabs.iter().filter(|t| matches!(&t.tool, Some(Tool::Compare(c)) if matches!(c.phase, Phase::Catalogs | Phase::Definitions))).map(|t| t.id).collect();
        for id in ids {
            let (phase, left, right, gens, with_defs, pending) = {
                let Some(Tool::Compare(cv)) = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.tool.as_ref()) else { continue };
                (cv.phase, cv.left.clone(), cv.right.clone(), cv.wait_gens, cv.with_definitions, cv.pending.len())
            };
            let ready = |conn: &str, gen: u64| self.catalogs.get(conn).map(|e| !e.loading && (e.generation > gen || e.error.is_some())).unwrap_or(false);
            let same = left.0 == right.0;
            if phase == Phase::Catalogs {
                if !(ready(&left.0, gens.0) && (same || ready(&right.0, gens.1)) || same && ready(&left.0, gens.0)) {
                    continue;
                }
                if let Some(err) = self.catalogs.get(&left.0).and_then(|e| e.error.clone()).or_else(|| self.catalogs.get(&right.0).and_then(|e| e.error.clone())) {
                    if let Some(Tool::Compare(cv)) = self.tabs.iter_mut().find(|t| t.id == id).and_then(|t| t.tool.as_mut()) {
                        cv.phase = Phase::Idle;
                        cv.status = format!("Couldn't load a schema: {err}");
                    }
                    continue;
                }
                if with_defs {
                    let lreq = self.catalog(&left.0).map(|c| CompareView::definition_requests(c, &left.1)).unwrap_or_default();
                    let rreq = self.catalog(&right.0).map(|c| CompareView::definition_requests(c, &right.1)).unwrap_or_default();
                    let a = self.request_sources(&left.0, &left.1, lreq, SourcePurpose::Compare { tab_id: id, side: 0 }, ctx);
                    let b = self.request_sources(&right.0, &right.1, rreq, SourcePurpose::Compare { tab_id: id, side: 1 }, ctx);
                    if let Some(Tool::Compare(cv)) = self.tabs.iter_mut().find(|t| t.id == id).and_then(|t| t.tool.as_mut()) {
                        cv.pending.insert(a);
                        cv.pending.insert(b);
                        cv.phase = Phase::Definitions;
                        cv.status = "Fetching definitions…".into();
                    }
                    continue;
                }
            } else if pending > 0 {
                continue;
            }
            let src = self.catalog(&left.0).cloned().unwrap_or_default();
            let dst = self.catalog(&right.0).cloned().unwrap_or_default();
            let d = self.conn(&right.0).map(|c| c.kind.dialect()).unwrap_or(Dialect::Postgres);
            if let Some(Tool::Compare(cv)) = self.tabs.iter_mut().find(|t| t.id == id).and_then(|t| t.tool.as_mut()) {
                cv.finish(&src, &dst, d);
            }
        }
    }

    fn open_tool(&mut self, conn_id: &str, title: String, tool: Tool) {
        let id = self.id();
        let d = self.conn(conn_id).map(|c| c.kind.dialect()).unwrap_or(Dialect::Postgres);
        self.tabs.push(Tab {
            id,
            conn_id: conn_id.to_string(),
            title,
            editor: Editor::new(egui::Id::new(("editor", id)), String::new(), d),
            results: Vec::new(),
            bottom: BottomTab::Output,
            output: Vec::new(),
            plan: None,
            running: None,
            manual_tx: false,
            in_tx: false,
            session: None,
            state: ConnState::Idle,
            caps: Capabilities::default(),
            tool: Some(tool),
            after_job: None,
        });
        self.active = self.tabs.len() - 1;
    }

    fn apply_rename(&mut self, ctx: &egui::Context) {
        let Some(r) = self.rename.take() else { return };
        let Some(cfg) = self.conn(&r.conn_id).cloned() else { return };
        let d = cfg.kind.dialect();
        let new = r.new_name.trim().to_string();
        let catalog = self.catalog(&r.conn_id).cloned();
        if r.update_console || matches!(r.target, RenameTarget::Alias { .. }) {
            if let Some(tab) = r.from_tab.and_then(|id| self.tabs.iter_mut().find(|t| t.id == id)) {
                let (text, n) = rename_references(&tab.editor.text, d, &r.target, &new, catalog.as_ref());
                if n > 0 {
                    let cursor = tab.editor.cursor;
                    tab.editor.set_text(text);
                    tab.editor.select(cursor..cursor);
                }
                self.toast(format!("Renamed {n} reference{} in the console", if n == 1 { "" } else { "s" }));
            }
        }
        if !r.rename_in_db || matches!(r.target, RenameTarget::Alias { .. }) {
            return;
        }
        let schema = match &r.target {
            RenameTarget::Table { schema: Some(s), .. } | RenameTarget::Column { schema: Some(s), .. } => s.clone(),
            _ => r.schema.clone(),
        };
        let stmt = match &r.target {
            RenameTarget::Table { name, .. } => databug_sql::ddl::rename_table(d, &schema, name, &new),
            RenameTarget::Column { table, column, .. } => {
                let def = catalog.as_ref().and_then(|c| c.find_table(Some(&schema), table)).and_then(|t| t.column(column)).cloned();
                databug_sql::ddl::rename_column(d, &schema, table, column, &new, def.as_ref())
            }
            RenameTarget::Alias { .. } => return,
        };
        let script = format!("-- Rename {} to {}\n{}", r.target.describe(), new, databug_sql::ddl::script(d, &[stmt]));
        let deps: Vec<SourceRef> = if r.update_dependents {
            catalog
                .as_ref()
                .and_then(|c| c.schema(&schema))
                .map(|s| {
                    let mut v: Vec<SourceRef> = s.tables.iter().filter(|t| matches!(t.kind, ObjectKind::View | ObjectKind::MaterializedView)).map(|t| SourceRef::relation(&t.name, t.kind)).collect();
                    v.extend(s.objects.iter().filter(|o| o.kind.has_source() && !o.kind.is_clr() && !matches!(o.kind, ObjectKind::Sequence | ObjectKind::Synonym | ObjectKind::Aggregate)).map(SourceRef::from_object));
                    v
                })
                .unwrap_or_default()
        } else {
            vec![]
        };
        if deps.is_empty() {
            let title = format!("Rename {}", r.target.current_name());
            self.open_script(&r.conn_id, &title, script, ctx);
            self.toast("Review the rename script, then run it with Ctrl/⌘ + Shift + Enter");
        } else {
            self.toast(format!("Checking {} views and routines for references…", deps.len()));
            self.request_sources(&r.conn_id, &schema, deps, SourcePurpose::Rename { conn_id: r.conn_id.clone(), schema: schema.clone(), target: r.target.clone(), new_name: new, script }, ctx);
        }
    }

    // ---------- UI ----------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() || self.confirm.is_some() || self.table_editor.is_some() || self.csv.is_some() || self.rename.is_some() {
            return;
        }
        if self.tabs.get(self.active).map(|t| t.tool.is_some()).unwrap_or(false) {
            return;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::SHIFT, Key::F6))) {
            self.start_rename_from_editor();
            return;
        }
        let sc = |m: Modifiers, k: Key| KeyboardShortcut::new(m, k);
        let (run_all, run, format, new_tab, close_tab, explain) = ctx.input_mut(|i| {
            let run_all = i.consume_shortcut(&sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::Enter));
            let run = i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Enter));
            let format = i.consume_shortcut(&sc(Modifiers::CTRL | Modifiers::ALT, Key::L)) || i.consume_shortcut(&sc(Modifiers::COMMAND | Modifiers::ALT, Key::L)) || i.consume_shortcut(&sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::F));
            let new_tab = i.consume_shortcut(&sc(Modifiers::COMMAND, Key::T));
            let close_tab = i.consume_shortcut(&sc(Modifiers::COMMAND, Key::W));
            let explain = i.consume_shortcut(&sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::E));
            (run_all, run, format, new_tab, close_tab, explain)
        });
        if run_all {
            self.request_run(true, None, ctx);
        } else if run {
            self.request_run(false, None, ctx);
        }
        if explain {
            self.request_run(false, Some(false), ctx);
        }
        if format {
            self.format_active();
        }
        if new_tab {
            if let Some(id) = self.tabs.get(self.active).map(|t| t.conn_id.clone()) {
                self.open_console(&id, None, ctx);
            }
        }
        if close_tab {
            self.close_tab(self.active);
        }
    }

    fn start_rename_from_editor(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let catalog = self.catalogs.get(&tab.conn_id).map(|e| &e.catalog);
        match target_at(&tab.editor.text, tab.editor.cursor, catalog, tab.editor.dialect) {
            Some(target) => {
                let schema = catalog.map(|c| c.default_schema.clone()).unwrap_or_default();
                self.rename = Some(RenameDialog::new(tab.conn_id.clone(), schema, target, Some(tab.id)));
            }
            None => self.toast("Put the cursor on a table, column or alias to rename it"),
        }
    }

    fn format_active(&mut self) {
        if let Some(t) = self.tabs.get_mut(self.active) {
            let f = databug_sql::format_sql(&t.editor.text, t.editor.dialect);
            if !f.trim().is_empty() {
                t.editor.set_text(f);
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, p: &Palette, ctx: &egui::Context) {
        let mut actions = Vec::new();
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // logo: a small bug with a split shell
            let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), Sense::hover());
            let c = rect.center() + egui::vec2(0.0, 1.5);
            let leg = Stroke::new(1.4_f32, p.muted);
            for dy in [-3.5_f32, 0.0, 3.5] {
                ui.painter().line_segment([c + egui::vec2(-4.0, dy), c + egui::vec2(-9.0, dy + dy.signum() * 1.5)], leg);
                ui.painter().line_segment([c + egui::vec2(4.0, dy), c + egui::vec2(9.0, dy + dy.signum() * 1.5)], leg);
            }
            ui.painter().circle_filled(c + egui::vec2(0.0, -7.0), 3.0, p.syn_keyword);
            ui.painter().rect_filled(egui::Rect::from_center_size(c, egui::vec2(11.0, 13.0)), CornerRadius::same(5), p.accent);
            ui.painter().line_segment([c + egui::vec2(0.0, -6.0), c + egui::vec2(0.0, 6.0)], Stroke::new(1.2_f32, p.canvas));
            ui.label(RichText::new("DataBug").font(bold(14.0)));
            ui.add_space(12.0);

            let active = self.tabs.get(self.active).filter(|t| t.tool.is_none());
            let running = active.map(|t| t.running.is_some()).unwrap_or(false);
            if let Some(t) = active {
                if let Some(cfg) = self.conn(&t.conn_id) {
                    egui::Frame::new().fill(p.island).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(8, 3)).show(ui, |ui| {
                        self.logos.show(ui, cfg.kind, 15.0);
                        crate::grid::painted_dot(ui, connection_color(cfg.color), 4.0);
                        ui.label(RichText::new(&cfg.name).color(p.text));
                        ui.label(RichText::new(cfg.kind.label()).size(11.5).color(p.muted));
                    });
                    if cfg.production {
                        egui::Frame::new().fill(p.danger.gamma_multiply(0.2)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(7, 3)).show(ui, |ui| {
                            ui.label(RichText::new("PRODUCTION").size(11.0).font(bold(11.0)).color(p.danger));
                        });
                    }
                    if cfg.read_only {
                        egui::Frame::new().fill(p.raised).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(7, 3)).show(ui, |ui| {
                            ui.label(RichText::new("read-only").size(11.0).color(p.muted));
                        });
                    }
                }
                ui.add_space(8.0);
                let run_btn = egui::Button::new(RichText::new("▶  Run").color(if running { p.faint } else { p.success })).min_size(egui::vec2(0.0, 26.0));
                if ui.add_enabled(!running, run_btn).on_hover_text("Run the statement at the cursor or the selection  (Ctrl/⌘ + Enter)").clicked() {
                    actions.push(1);
                }
                if ui.add_enabled(!running, egui::Button::new("Run script").min_size(egui::vec2(0.0, 26.0))).on_hover_text("Run every statement in this console  (Ctrl/⌘ + Shift + Enter)").clicked() {
                    actions.push(2);
                }
                ui.add_enabled_ui(!running, |ui| {
                    ui.menu_button("Explain", |ui| {
                        if ui.button("Explain plan").on_hover_text("Ctrl/⌘ + Shift + E").clicked() {
                            actions.push(3);
                            ui.close_menu();
                        }
                        if ui.button("Explain analyze").on_hover_text("Runs the statement; changes are rolled back").clicked() {
                            actions.push(4);
                            ui.close_menu();
                        }
                    });
                });
                let stop = egui::Button::new(RichText::new("■  Cancel").color(if running { p.danger } else { p.faint })).min_size(egui::vec2(0.0, 26.0));
                if ui.add_enabled(running, stop).on_hover_text("Cancel on the server; press again to drop the connection").clicked() {
                    actions.push(5);
                }
                ui.separator();
                let t = &self.tabs[self.active];
                let manual = t.manual_tx;
                let in_tx = t.in_tx;
                ui.label(RichText::new("Tx").color(p.muted));
                if ui.selectable_label(!manual, "Auto").on_hover_text("Each statement commits on its own").clicked() {
                    actions.push(6);
                }
                if ui.selectable_label(manual, "Manual").on_hover_text("Changes stay in an open transaction until you commit").clicked() {
                    actions.push(7);
                }
                if in_tx {
                    egui::Frame::new().fill(p.warning.gamma_multiply(0.2)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(7, 3)).show(ui, |ui| {
                        ui.label(RichText::new("transaction open").size(11.5).color(p.warning));
                    });
                    if ui.button(RichText::new("Commit").color(p.success)).clicked() {
                        actions.push(8);
                    }
                    if ui.button("Rollback").clicked() {
                        actions.push(9);
                    }
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let icon = if self.settings.theme == ThemeKind::Dark { "☀" } else { "🌙" };
                if ui.button(icon).on_hover_text("Switch theme").clicked() {
                    self.settings.theme = if self.settings.theme == ThemeKind::Dark { ThemeKind::Light } else { ThemeKind::Dark };
                    self.store.save_settings(&self.settings);
                }
                ui.menu_button("⚙", |ui| {
                    ui.set_min_width(240.0);
                    ui.label(RichText::new("Settings").font(bold(13.0)));
                    ui.add_space(4.0);
                    let mut changed = false;
                    ui.horizontal(|ui| {
                        ui.label("Row limit");
                        changed |= ui.add(egui::DragValue::new(&mut self.settings.row_limit).range(100..=1_000_000).speed(100)).changed();
                    });
                    ui.horizontal(|ui| {
                        ui.label("Text size");
                        let mut pct = (self.settings.ui_scale * 100.0).round() as i32;
                        if ui.small_button("−").on_hover_text("Smaller  (Ctrl/⌘ −)").clicked() {
                            pct -= 10;
                        }
                        ui.add(egui::DragValue::new(&mut pct).range(70..=200).suffix("%").speed(1));
                        if ui.small_button("+").on_hover_text("Larger  (Ctrl/⌘ +)").clicked() {
                            pct += 10;
                        }
                        if ui.small_button("Reset").on_hover_text("100%  (Ctrl/⌘ 0)").clicked() {
                            pct = 100;
                        }
                        let new = (pct.clamp(70, 200) as f32) / 100.0;
                        if (new - self.settings.ui_scale).abs() > 0.001 {
                            self.settings.ui_scale = new;
                            changed = true;
                        }
                    });
                    ui.label(RichText::new("Scales everything: menus, explorer, results and editor.").size(11.0).color(p.faint));
                    ui.horizontal(|ui| {
                        ui.label("Editor font size");
                        changed |= ui.add(egui::DragValue::new(&mut self.settings.code_font_size).range(10.0..=22.0).speed(0.25)).changed();
                    });
                    changed |= ui.checkbox(&mut self.settings.auto_complete, "Show completion while typing").changed();
                    if changed {
                        self.store.save_settings(&self.settings);
                    }
                });
                if !self.tabs.is_empty() && ui.button("Format").on_hover_text("Reformat the console  (Ctrl + Alt + L)").clicked() {
                    actions.push(10);
                }
            });
        });
        for a in actions {
            match a {
                1 => self.request_run(false, None, ctx),
                2 => self.request_run(true, None, ctx),
                3 => self.request_run(false, Some(false), ctx),
                4 => self.request_run(false, Some(true), ctx),
                5 => {
                    if let Some(s) = self.tabs.get_mut(self.active).and_then(|t| t.session.as_mut()) {
                        s.cancel();
                    }
                }
                6 => {
                    if let Some(t) = self.tabs.get_mut(self.active) {
                        if t.in_tx {
                            self.toast("Commit or roll back the open transaction first");
                        } else {
                            t.manual_tx = false;
                        }
                    }
                }
                7 => {
                    if let Some(t) = self.tabs.get_mut(self.active) {
                        t.manual_tx = true;
                    }
                }
                8 => self.tx_command("COMMIT", ctx),
                9 => self.tx_command("ROLLBACK", ctx),
                10 => self.format_active(),
                _ => {}
            }
        }
    }

    fn tab_strip(&mut self, ui: &mut egui::Ui, p: &Palette) -> (Option<usize>, Option<usize>, bool) {
        let mut select = None;
        let mut close = None;
        let mut new_tab = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            egui::ScrollArea::horizontal().id_salt("tabs").max_width(ui.available_width() - 34.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, t) in self.tabs.iter().enumerate() {
                        let active = i == self.active;
                        let color = self.conn(&t.conn_id).map(|c| connection_color(c.color)).unwrap_or(p.muted);
                        let title_galley = ui.painter().layout_no_wrap(t.title.clone(), if active { bold(12.5) } else { FontId::proportional(12.5) }, if active { p.text } else { p.muted });
                        let w = title_galley.size().x + 52.0;
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 30.0), Sense::click());
                        if active {
                            ui.painter().rect_filled(rect.shrink2(egui::vec2(0.0, 2.0)), CornerRadius::same(6), p.raised);
                            ui.painter().line_segment([egui::pos2(rect.left() + 8.0, rect.bottom() - 1.0), egui::pos2(rect.right() - 8.0, rect.bottom() - 1.0)], Stroke::new(2.0_f32, p.accent));
                        } else if resp.hovered() {
                            ui.painter().rect_filled(rect.shrink2(egui::vec2(0.0, 2.0)), CornerRadius::same(6), p.hover);
                        }
                        ui.painter().circle_filled(egui::pos2(rect.left() + 12.0, rect.center().y), 3.5, color);
                        ui.painter().galley(egui::pos2(rect.left() + 22.0, rect.center().y - title_galley.size().y / 2.0), title_galley, p.text);
                        if t.running.is_some() {
                            ui.painter().circle_filled(egui::pos2(rect.right() - 14.0, rect.center().y), 3.0, p.accent);
                        } else {
                            let x_rect = egui::Rect::from_center_size(egui::pos2(rect.right() - 14.0, rect.center().y), egui::vec2(16.0, 16.0));
                            let x_resp = ui.interact(x_rect, ui.id().with(("close", t.id)), Sense::click());
                            if resp.hovered() || x_resp.hovered() || active {
                                if x_resp.hovered() {
                                    ui.painter().rect_filled(x_rect, CornerRadius::same(4), p.hover);
                                }
                                ui.painter().text(x_rect.center(), egui::Align2::CENTER_CENTER, "×", FontId::proportional(14.0), p.muted);
                            }
                            if x_resp.clicked() {
                                close = Some(i);
                            }
                        }
                        if resp.clicked() {
                            select = Some(i);
                        }
                        if resp.middle_clicked() {
                            close = Some(i);
                        }
                    }
                });
            });
            if ui.add(egui::Button::new(RichText::new("+").size(16.0).color(p.muted)).frame(false)).on_hover_text("New console  (Ctrl/⌘ + T)").clicked() {
                new_tab = true;
            }
        });
        (select, close, new_tab)
    }

    fn welcome(&mut self, ui: &mut egui::Ui, p: &Palette) -> Vec<Action> {
        let mut actions = Vec::new();
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.25).max(20.0));
            ui.label(RichText::new("DataBug").font(bold(30.0)));
            ui.label(RichText::new("A fast, memory-light database client").size(14.0).color(p.muted));
            ui.add_space(22.0);
            let big = |t: &str| egui::Button::new(RichText::new(t).size(14.0)).min_size(egui::vec2(220.0, 34.0));
            if ui.add(big("New connection")).clicked() {
                actions.push(Action::NewConnection);
            }
            ui.add_space(4.0);
            if ui.add(big("Open sample database")).clicked() {
                actions.push(Action::OpenSample);
            }
            if let Some(c) = self.connections.first() {
                ui.add_space(4.0);
                if ui.add(big(&format!("Open console on {}", c.name))).clicked() {
                    actions.push(Action::NewConsole(c.id.clone()));
                }
            }
            ui.add_space(28.0);
            for (k, v) in [("Ctrl/⌘ + Enter", "Run statement at cursor"), ("Ctrl/⌘ + Shift + Enter", "Run script"), ("Ctrl + Space", "Completion"), ("Ctrl + Alt + L", "Format"), ("Ctrl/⌘ + /", "Toggle comment")] {
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 360.0).max(0.0) / 2.0);
                    ui.add_sized([170.0, 18.0], egui::Label::new(RichText::new(k).font(FontId::monospace(12.0)).color(p.muted)));
                    ui.label(RichText::new(v).color(p.faint));
                });
            }
        });
        actions
    }

    fn bottom_panel(&mut self, ui: &mut egui::Ui, p: &Palette, ctx: &egui::Context) {
        let mut grid_action = GridAction::None;
        let mut insert: Option<String> = None;
        let mut toast: Option<String> = None;
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let chip = |ui: &mut egui::Ui, selected: bool, text: String| -> bool {
                let rt = RichText::new(text).size(12.5).color(if selected { p.text } else { p.muted });
                ui.add(egui::Button::new(rt).fill(if selected { p.raised } else { Color32::TRANSPARENT }).stroke(Stroke::NONE).min_size(egui::vec2(0.0, 24.0))).clicked()
            };
            let errors = tab.output.iter().rev().take(50).any(|l| l.kind == LogKind::Error);
            let out_label = if errors && tab.bottom != BottomTab::Output { "Output •".to_string() } else { "Output".to_string() };
            if chip(ui, tab.bottom == BottomTab::Output, out_label) {
                tab.bottom = BottomTab::Output;
            }
            for i in 0..tab.results.len() {
                let label = format!("{}  {}", tab.results[i].title, group(tab.results[i].row_count()));
                if chip(ui, tab.bottom == BottomTab::Result(i), label) {
                    tab.bottom = BottomTab::Result(i);
                }
            }
            if tab.plan.is_some() && chip(ui, tab.bottom == BottomTab::Plan, "Plan".into()) {
                tab.bottom = BottomTab::Plan;
            }
            if chip(ui, tab.bottom == BottomTab::History, "History".into()) {
                tab.bottom = BottomTab::History;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(r) = &tab.running {
                    ui.label(RichText::new(fmt_duration(r.started.elapsed())).font(FontId::monospace(12.0)).color(p.muted));
                    ui.add(egui::Spinner::new().size(13.0));
                    let label = if r.explain { "Explaining…".to_string() } else if r.stmts.len() > 1 { format!("Running {}/{}…", (r.done + 1).min(r.stmts.len()), r.stmts.len()) } else { "Running…".to_string() };
                    ui.label(RichText::new(label).color(p.accent));
                }
            });
        });
        ui.add_space(2.0);
        ui.separator();
        match tab.bottom {
            BottomTab::Result(i) if i < tab.results.len() => {
                grid_action = tab.results[i].show(ui, p);
                if let GridAction::Run(stmts) = &grid_action {
                    let r = tab.editor.cursor..tab.editor.cursor;
                    let stmts = stmts.iter().map(|s| (s.clone(), r.clone())).collect();
                    let tab_id = tab.id;
                    self.check_and_dispatch(tab_id, stmts, None, Some(i), ctx);
                    return;
                }
            }
            BottomTab::Plan if tab.plan.is_some() => {
                toast = tab.plan.as_mut().unwrap().show(ui, p);
            }
            BottomTab::History => {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.history_filter).hint_text("Search history…").desired_width(260.0));
                    ui.label(RichText::new("Double-click to insert into the console").size(11.5).color(p.faint));
                });
                let f = self.history_filter.to_lowercase();
                let items: Vec<&HistoryEntry> = self.history.iter().rev().filter(|h| f.is_empty() || h.sql.to_lowercase().contains(&f) || h.connection.to_lowercase().contains(&f)).collect();
                egui::ScrollArea::vertical().id_salt("history").auto_shrink([false, false]).show_rows(ui, 24.0, items.len(), |ui, range| {
                    for h in &items[range] {
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::same(4), p.hover);
                        }
                        ui.painter().circle_filled(egui::pos2(rect.left() + 8.0, rect.center().y), 3.0, if h.ok { p.success } else { p.danger });
                        ui.painter().text(egui::pos2(rect.left() + 20.0, rect.center().y), egui::Align2::LEFT_CENTER, ago(h.at), FontId::proportional(11.5), p.faint);
                        ui.painter().text(egui::pos2(rect.left() + 100.0, rect.center().y), egui::Align2::LEFT_CENTER, one_line(&h.connection, 18), FontId::proportional(11.5), p.muted);
                        ui.painter().text(egui::pos2(rect.left() + 240.0, rect.center().y), egui::Align2::LEFT_CENTER, one_line(&h.sql, ((rect.width() - 340.0) / 7.3).max(10.0) as usize), FontId::monospace(12.0), p.text);
                        ui.painter().text(egui::pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, format!("{} ms", h.ms), FontId::monospace(11.5), p.faint);
                        if resp.double_clicked() {
                            insert = Some(h.sql.clone());
                        }
                        resp.on_hover_text(h.sql.chars().take(1500).collect::<String>());
                    }
                });
            }
            _ => {
                egui::ScrollArea::vertical().id_salt(("output", tab.id)).stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                    if tab.output.is_empty() {
                        ui.label(RichText::new("Run a statement with Ctrl/⌘ + Enter. Messages, timings and errors show up here.").color(p.faint));
                    }
                    for l in &tab.output {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&l.at).font(FontId::monospace(11.5)).color(p.faint));
                            let (icon, color) = match l.kind {
                                LogKind::Info => ("›", p.muted),
                                LogKind::Ok => ("✔", p.success),
                                LogKind::Warn => ("!", p.warning),
                                LogKind::Error => ("✖", p.danger),
                            };
                            ui.label(RichText::new(icon).color(color));
                            ui.label(RichText::new(&l.text).color(if l.kind == LogKind::Error { p.danger } else { p.text }));
                            if let Some(sql) = &l.sql {
                                ui.label(RichText::new(sql).font(FontId::monospace(12.0)).color(p.muted));
                            }
                        });
                    }
                });
            }
        }
        if let GridAction::Toast(t) = grid_action {
            toast = Some(t);
        }
        if let Some(s) = insert {
            if let Some(t) = self.tabs.get_mut(self.active) {
                let mut text = t.editor.text.trim_end().to_string();
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(&s);
                t.editor.set_text(text);
                t.editor.focus();
            }
        }
        if let Some(t) = toast {
            self.toast(t);
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let small = |t: String, c: Color32| RichText::new(t).size(11.5).color(c);
            if let Some(t) = self.tabs.get(self.active) {
                let (txt, c) = match &t.state {
                    ConnState::Idle => ("Not connected".to_string(), p.faint),
                    ConnState::Connecting => ("Connecting…".to_string(), p.warning),
                    ConnState::Connected => ("Connected".to_string(), p.success),
                    ConnState::Failed(_) => ("Connection failed".to_string(), p.danger),
                };
                ui.label(small(format!("● {txt}"), c));
                if let Some(e) = self.catalogs.get(&t.conn_id) {
                    if e.loading {
                        ui.label(small("Introspecting…".into(), p.muted));
                    } else if e.catalog.introspected_at > 0 {
                        ui.label(small(format!("Schema: {} objects, {}", e.catalog.table_count(), ago(e.catalog.introspected_at)), p.muted));
                    }
                }
                let diags = t.editor.diagnostics();
                let errs = diags.iter().filter(|d| d.severity == databug_sql::Severity::Error).count();
                let warns = diags.len() - errs;
                if errs > 0 {
                    ui.label(small(format!("{errs} problem{}", if errs == 1 { "" } else { "s" }), p.danger));
                }
                if warns > 0 {
                    ui.label(small(format!("{warns} warning{}", if warns == 1 { "" } else { "s" }), p.warning));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.mem.0.elapsed() > Duration::from_secs(1) {
                    self.mem = (Instant::now(), rss_bytes());
                }
                if let Some(b) = self.mem.1 {
                    ui.label(small(format!("{} MB", b / (1024 * 1024)), p.faint)).on_hover_text("Resident memory of this process");
                }
                if let Some(t) = self.tabs.get(self.active) {
                    let bytes: usize = t.results.iter().map(|r| r.heap_bytes()).sum();
                    if bytes > 0 {
                        ui.label(small(format!("results {}", human_bytes(bytes)), p.faint));
                    }
                    let (l, c) = t.editor.line_col();
                    ui.label(small(format!("Ln {l}, Col {c}"), p.muted));
                    ui.label(small(match t.editor.dialect {
                        Dialect::Postgres => "PostgreSQL",
                        Dialect::MySql => "MySQL",
                        Dialect::MsSql => "T-SQL",
                        Dialect::Sqlite => "SQLite",
                        Dialect::Mongo => "Mongo shell",
                    }.into(), p.muted));
                }
            });
        });
    }

    fn confirm_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(c) = &self.confirm else { return };
        let mut decision: Option<bool> = None;
        egui::Window::new(RichText::new(&c.title).font(bold(15.0)))
            .id(egui::Id::new("confirm"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
            .fixed_size([520.0, 0.0])
            .show(ctx, |ui| {
                for r in &c.reasons {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("⚠").color(p.warning));
                        ui.label(r);
                    });
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(RichText::new("Run anyway").color(Color32::WHITE)).fill(p.danger)).clicked() {
                            decision = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            decision = Some(false);
                        }
                    });
                });
                if ui.input(|i| i.key_pressed(Key::Escape)) {
                    decision = Some(false);
                }
            });
        if let Some(go) = decision {
            let c = self.confirm.take().unwrap();
            if go {
                if let Some(ti) = self.tabs.iter().position(|t| t.id == c.tab_id) {
                    self.dispatch(ti, c.stmts, c.explain, c.edit_refresh, ctx);
                }
            }
        }
    }

    fn dialog_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(d) = self.dialog.as_mut() else { return };
        match d.show(ctx, p, self.rt.handle(), &self.logos) {
            DialogOutcome::Open => {}
            DialogOutcome::Cancel => {
                self.dialog = None;
                self.editing_conn = None;
            }
            DialogOutcome::Save => {
                let d = self.dialog.take().unwrap();
                let cfg = d.cfg.clone();
                if !d.password.is_empty() {
                    let ok = self.store.set_password(&cfg.id, &d.password, d.save_password);
                    if !ok {
                        self.toast("The system keychain isn't available, so the password is kept for this session only");
                    }
                } else if !d.is_new {
                    self.store.forget_password(&cfg.id);
                }
                let only_view = self.conn(&cfg.id).map(|old| {
                    let mut a = serde_json::to_value(old).ok();
                    let mut b = serde_json::to_value(&cfg).ok();
                    for v in [&mut a, &mut b].into_iter().flatten() {
                        if let Some(o) = v.as_object_mut() {
                            o.remove("visible_schemas");
                            o.remove("name");
                            o.remove("color");
                        }
                    }
                    a == b
                }).unwrap_or(false) && d.password == self.store.password(&cfg.id).unwrap_or_default();
                match self.connections.iter_mut().find(|c| c.id == cfg.id) {
                    Some(existing) => *existing = cfg.clone(),
                    None => self.connections.push(cfg.clone()),
                }
                self.store.save_connections(&self.connections);
                if only_view {
                    self.editing_conn = None;
                    return;
                }
                // settings changed: drop open sessions so they reconnect with the new values
                for t in self.tabs.iter_mut().filter(|t| t.conn_id == cfg.id && t.running.is_none()) {
                    t.session = None;
                    t.state = ConnState::Idle;
                    t.in_tx = false;
                    t.editor.dialect = cfg.kind.dialect();
                }
                self.meta.remove(&cfg.id);
                self.refreshed.remove(&cfg.id);
                if d.is_new {
                    self.open_console(&cfg.id, None, ctx);
                } else {
                    self.refresh_catalog(&cfg.id, ctx);
                }
                self.editing_conn = None;
            }
        }
    }


    fn tool_ui(&mut self, ui: &mut egui::Ui, p: &Palette, ctx: &egui::Context) {
        let ti = self.active;
        let conns = self.connections.clone();
        let catalogs = &self.catalogs;
        let lookup = |id: &str| catalogs.get(id).map(|e| e.catalog.clone());
        let mut er_action = ErAction::None;
        let mut cmp_action = CompareAction::None;
        match self.tabs[ti].tool.as_mut() {
            Some(Tool::Er(v)) => er_action = v.show(ui, p),
            Some(Tool::Compare(v)) => cmp_action = v.show(ui, p, &conns, &lookup),
            None => {}
        }
        let tab_id = self.tabs[ti].id;
        match er_action {
            ErAction::EditTable(t) => {
                if let Some(Tool::Er(v)) = &self.tabs[ti].tool {
                    let (c, s) = (v.conn_id.clone(), v.schema.clone());
                    self.apply(Action::EditTable(c, s, t), ctx);
                }
            }
            ErAction::Copy(text, msg) => {
                ctx.copy_text(text);
                self.toast(msg);
            }
            ErAction::Refresh => {
                let (c, s) = match &self.tabs[ti].tool {
                    Some(Tool::Er(v)) => (v.conn_id.clone(), v.schema.clone()),
                    _ => return,
                };
                if let Some(schema) = self.catalog(&c).and_then(|cat| cat.schema(&s)).cloned() {
                    if let Some(Tool::Er(v)) = self.tabs[ti].tool.as_mut() {
                        v.load(&schema);
                    }
                }
            }
            ErAction::None => {}
        }
        match cmp_action {
            CompareAction::Start => self.start_compare(tab_id, ctx),
            CompareAction::OpenScript(script) => {
                if let Some(Tool::Compare(v)) = &self.tabs[ti].tool {
                    let target = v.right.0.clone();
                    self.open_script(&target, "Migration", script, ctx);
                }
            }
            CompareAction::RunScript(script) => {
                if let Some(Tool::Compare(v)) = &self.tabs[ti].tool {
                    let target = v.right.0.clone();
                    self.run_in_console(&target, script, true, Some("Migration applied; compare again to confirm".into()), ctx);
                }
            }
            CompareAction::None => {}
        }
    }

    fn modal_windows(&mut self, ctx: &egui::Context, p: &Palette) {
        if let Some(ed) = self.table_editor.as_mut() {
            match ed.show(ctx, p) {
                EditorOutcome::Open => {}
                EditorOutcome::Cancel => self.table_editor = None,
                EditorOutcome::Execute(script) => {
                    let ed = self.table_editor.take().unwrap();
                    self.run_in_console(&ed.conn_id, script, true, Some("Table updated".into()), ctx);
                }
                EditorOutcome::OpenInConsole(script) => {
                    let ed = self.table_editor.take().unwrap();
                    self.open_script(&ed.conn_id, "Table DDL", script, ctx);
                }
            }
        }
        if let Some(imp) = self.csv.as_mut() {
            match imp.show(ctx, p) {
                ImportOutcome::Open => {}
                ImportOutcome::Cancel => self.csv = None,
                ImportOutcome::Run { stmts, summary } => {
                    let imp = self.csv.take().unwrap();
                    self.run_import(&imp.conn_id, stmts, summary, ctx);
                }
            }
        }
        if let Some(r) = self.rename.as_mut() {
            match r.show(ctx, p) {
                RenameOutcome::Open => {}
                RenameOutcome::Cancel => self.rename = None,
                RenameOutcome::Apply => self.apply_rename(ctx),
            }
        }
    }

    /// CSV imports can be thousands of statements: run them as one job in a
    /// dedicated console without pasting them into the editor.
    fn run_import(&mut self, conn_id: &str, stmts: Vec<String>, summary: String, ctx: &egui::Context) {
        let ti = self.open_console(conn_id, Some(format!("-- {summary}\n-- ({} statements)\n", stmts.len())), ctx);
        self.tabs[ti].title = "Import".into();
        self.tabs[ti].after_job = Some((true, Some(summary)));
        let stmts = stmts.into_iter().map(|s| (s, 0..0)).collect();
        let tab_id = self.tabs[ti].id;
        self.check_and_dispatch(tab_id, stmts, None, None, ctx);
    }

    fn toasts(&mut self, ctx: &egui::Context, p: &Palette) {
        let now = Instant::now();
        self.toasts.retain(|t| t.until > now);
        if self.toasts.is_empty() {
            return;
        }
        let next = self.toasts.iter().map(|t| t.until).min().unwrap();
        ctx.request_repaint_after(next - now);
        egui::Area::new(egui::Id::new("toasts")).anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -40.0]).order(egui::Order::Foreground).show(ctx, |ui| {
            for t in &self.toasts {
                egui::Frame::popup(ui.style()).fill(p.raised).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                    ui.set_max_width(420.0);
                    ui.label(&t.text);
                });
                ui.add_space(6.0);
            }
        });
    }

    fn drive_screenshot(&mut self, ctx: &egui::Context) {
        let compare_done = self.tabs.last().map(|t| matches!(&t.tool, Some(Tool::Compare(c)) if c.phase == Phase::Done)).unwrap_or(false);
        let Some(plan) = self.screenshot.as_mut() else { return };
        ctx.request_repaint();
        plan.wait_frames += 1;
        match plan.step {
            0 if plan.wait_frames > 3 => {
                if plan.scene == "light" {
                    self.settings.theme = ThemeKind::Light;
                }
                plan.step = 1;
                plan.wait_frames = 0;
                if plan.scene.starts_with("pg") {
                    let id = self.connections.first().map(|c| c.id.clone()).unwrap_or_default();
                    self.open_console(&id, Some(DEMO_SQL.into()), ctx);
                } else {
                    self.open_sample(ctx);
                }
                if let Some(t) = self.tabs.get_mut(self.active) {
                    t.editor.cursor = DEMO_SQL.len() - 2;
                    t.editor.selection = None;
                }
                self.request_run(false, None, ctx);
            }
            1 => {
                if plan.wait_frames > 600 {
                    eprintln!("screenshot: timed out waiting for the sample query");
                    eprintln!("connections: {}", self.connections.len());
                    if let Some(t) = self.tabs.get(self.active) {
                        for l in &t.output {
                            eprintln!("  output: {}", l.text);
                        }
                        if let Some(e) = self.catalogs.get(&t.conn_id) {
                            eprintln!("  catalog: loading={} tables={} error={:?}", e.loading, e.catalog.table_count(), e.error);
                        }
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                let ready = self.tabs.get(self.active).map(|t| t.running.is_none() && !t.results.is_empty()).unwrap_or(false);
                let cat_ready = self.tabs.get(self.active).and_then(|t| self.catalogs.get(&t.conn_id)).map(|e| !e.loading && e.catalog.table_count() > 0).unwrap_or(false);
                if ready && cat_ready && plan.wait_frames > 5 {
                    plan.step = 2;
                    plan.wait_frames = 0;
                    let scene = plan.scene.clone();
                    match scene.as_str() {
                        "pg-explorer" => {
                            let id = self.connections.first().map(|c| c.id.clone()).unwrap_or_default();
                            self.explorer.open_key(format!("s:{id}:public"));
                            for k in ["Table", "MaterializedView", "Procedure", "Function", "TableFunction", "Trigger", "Sequence"] {
                                self.explorer.open_key(format!("g:{id}:public:{k}"));
                            }
                            self.explorer.open_key(format!("t:{id}:public:orders"));
                            self.explorer.selected = Some(format!("t:{id}:public:orders"));
                        }
                        "pg-compare" => {
                            let id = self.connections.first().map(|c| c.id.clone()).unwrap_or_default();
                            self.apply(Action::Compare(id.clone(), "public".into()), ctx);
                            if let Some(Tool::Compare(cv)) = self.tabs.last_mut().and_then(|t| t.tool.as_mut()) {
                                cv.right = (id, "staging".into());
                            }
                            let tid = self.tabs.last().map(|t| t.id).unwrap_or(0);
                            self.start_compare(tid, ctx);
                        }
                        "table" => {
                            let id = self.tabs[self.active].conn_id.clone();
                            self.apply(Action::EditTable(id, "main".into(), "orders".into()), ctx);
                            if let Some(ed) = self.table_editor.as_mut() {
                                ed.add_column("shipped_at", "TEXT");
                            }
                        }
                        "er" => {
                            let id = self.tabs[self.active].conn_id.clone();
                            self.apply(Action::ErDiagram(id, "main".into()), ctx);
                        }
                        "csv" => {
                            let id = self.tabs[self.active].conn_id.clone();
                            let path = std::env::temp_dir().join("suppliers.csv");
                            let _ = std::fs::write(&path, "Supplier ID,Name,Country,Rating,On time %\n1,Acme Wire,US,4.6,97.5\n2,Delta Fab,MX,4.1,91\n3,Northern Copper,CA,4.8,99.1\n4,Gulf Cable,US,,88\n");
                            let tables = self.catalog(&id).and_then(|c| c.schema("main")).map(|s| s.tables.clone()).unwrap_or_default();
                            self.csv = Some(CsvImport::new(id, "main".into(), DbKind::Sqlite, path, tables, None));
                        }
                        "csv-run" => {
                            let id = self.tabs[self.active].conn_id.clone();
                            let path = std::env::temp_dir().join("suppliers.csv");
                            let _ = std::fs::write(&path, "Supplier ID,Name,Country,Rating\n1,Acme Wire,US,4.6\n2,Delta Fab,MX,4.1\n3,Northern Copper,CA,4.8\n4,Gulf Cable,US,\n");
                            let imp = CsvImport::new(id.clone(), "main".into(), DbKind::Sqlite, path, vec![], None);
                            if let Ok((stmts, summary)) = imp.statements() {
                                self.run_import(&id, stmts, summary, ctx);
                            }
                        }
                        "table-exec" => {
                            let id = self.tabs[self.active].conn_id.clone();
                            self.apply(Action::EditTable(id.clone(), "main".into(), "orders".into()), ctx);
                            if let Some(mut ed) = self.table_editor.take() {
                                ed.add_column("shipped_at", "TEXT");
                                let script = ed.script();
                                self.run_in_console(&id, script, true, Some("Table updated".into()), ctx);
                            }
                        }
                        "edit-object" => {
                            let id = self.tabs[self.active].conn_id.clone();
                            self.apply(Action::EditObject(id, "main".into(), SourceRef::relation("customer_totals", ObjectKind::View)), ctx);
                        }
                        "rename-apply" | "rename" => {
                            if let Some(t) = self.tabs.get_mut(self.active) {
                                let text = "SELECT o.id, o.total, c.name\nFROM orders o\nJOIN customers c ON c.id = o.customer_id\nWHERE o.total > 500;\n".to_string();
                                let cursor = text.find("o.total").unwrap() + 4;
                                t.editor.set_text(text);
                                t.editor.select(cursor..cursor);
                            }
                        }
                        "pg" => {
                            if let Some(t) = self.tabs.get_mut(self.active) {
                                t.editor.cursor = 40;
                            }
                            self.request_run(false, Some(true), ctx);
                        }
                        "plan" => {
                            if let Some(t) = self.tabs.get_mut(self.active) {
                                t.editor.cursor = 40;
                            }
                            self.request_run(false, Some(false), ctx);
                        }
                        "confirm" | "tx" | "edit" => {
                            if let Some(t) = self.tabs.get_mut(self.active) {
                                let sql = match scene.as_str() {
                                    "confirm" => "DELETE FROM order_items;",
                                    "tx" => "UPDATE products SET stock = stock - 1 WHERE id = 3;",
                                    _ => "SELECT * FROM products WHERE id <= 12;",
                                };
                                t.editor.set_text(sql.into());
                                t.editor.select(0..0);
                                t.manual_tx = scene == "tx";
                            }
                        }
                        "flow" | "error" => {
                            if let Some(t) = self.tabs.get_mut(self.active) {
                                if scene == "error" {
                                    t.editor.set_text("SELECT * FROM nope;\n\nDELETE FROM orders;\n".into());
                                    t.editor.select(0..0);
                                } else {
                                    t.editor.set_text(String::new());
                                }
                            }
                        }
                        "pg-schemas" => {
                            let id = self.connections.first().map(|c| c.id.clone()).unwrap_or_default();
                            self.apply(Action::SetSchemas(id.clone(), Some(vec!["public".into(), "analytics".into(), "reporting".into()])), ctx);
                            self.explorer.open_key(format!("s:{id}:analytics"));
                            self.explorer.open_key(format!("s:{id}:reporting"));
                            self.explorer.request_picker(&id);
                        }
                        "ms-dialog" | "ms-dialog-integrated" => {
                            let mut cfg = ConnectionConfig::new(DbKind::MsSql);
                            cfg.name = "Store (SQL Server)".into();
                            cfg.host = "sql01.example.com".into();
                            cfg.database = "store".into();
                            cfg.ms_auth = if scene == "ms-dialog" { databug_core::MsAuth::WindowsCredentials } else { databug_core::MsAuth::WindowsIntegrated };
                            cfg.user = "EXAMPLE\\alex".into();
                            cfg.trust_server_cert = true;
                            let mut d = ConnDialog::new(cfg, "secret".into(), false);
                            d.known_schemas = ["dbo", "sales", "inventory", "reporting", "staging", "archive"].map(String::from).to_vec();
                            d.default_schema = "dbo".into();
                            d.cfg.visible_schemas = Some(vec!["dbo".into(), "sales".into(), "inventory".into()]);
                            self.dialog = Some(d);
                        }
                        "dialog" => {
                            let mut cfg = ConnectionConfig::new(DbKind::Postgres);
                            cfg.host = "db.internal.example.com".into();
                            cfg.user = "analyst".into();
                            cfg.database = "warehouse".into();
                            cfg.production = true;
                            cfg.color = [247, 84, 100];
                            self.dialog = Some(ConnDialog::new(cfg, "secret".into(), true));
                        }
                        _ => {
                            if let Some(t) = self.tabs.get_mut(self.active) {
                                let mut text = DEMO_SQL.to_string();
                                text.push_str("\nSELECT p.name, p.\nFROM products p\nWHERE p.price > 100;\n");
                                let cursor = text.find("p.\nFROM").unwrap() + 2;
                                t.editor.set_text(text);
                                t.editor.select(cursor..cursor);
                            }
                        }
                    }
                }

            }
            2 if plan.scene == "flow" && plan.wait_frames < 60 => {
                let f = plan.wait_frames;
                let key = |k: Key, m: Modifiers| egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m };
                let ev = match f {
                    5 => Some(egui::Event::Text("SELECT * FROM cu".into())),
                    9 => Some(key(Key::Enter, Modifiers::NONE)),
                    12 => Some(egui::Event::Text(" c WHERE c.".into())),
                    15 => Some(egui::Event::Text("ci".into())),
                    18 => Some(key(Key::Enter, Modifiers::NONE)),
                    21 => Some(egui::Event::Text(" = 'Macon'".into())),
                    24 => Some(key(Key::Enter, Modifiers::COMMAND)),
                    _ => None,
                };
                if let Some(ev) = ev {
                    ctx.input_mut(|i| i.events.push(ev));
                }
            }
            2 if matches!(plan.scene.as_str(), "error" | "confirm" | "tx" | "edit") && plan.wait_frames == 5 => {
                ctx.input_mut(|i| i.events.push(egui::Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND }));
            }
            2 if plan.scene == "edit" && plan.wait_frames == 20 => {
                // edit two cells the way a double-click would, then submit
                if let Some(t) = self.tabs.get_mut(self.active) {
                    if let Some(v) = t.results.get_mut(0) {
                        v.set_cell(1, 2, Some("Copper wire 2 (edited)".into()));
                        v.set_cell(2, 5, Some("999".into()));
                        if let Ok(stmts) = v.update_statements() {
                            let r = 0..0;
                            let tab_id = t.id;
                            let stmts = stmts.into_iter().map(|s| (s, r.clone())).collect();
                            self.check_and_dispatch(tab_id, stmts, None, Some(0), ctx);
                        }
                    }
                }
            }
            2 if plan.scene == "edit" && plan.wait_frames == 35 => {
                ctx.input_mut(|i| i.events.push(egui::Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND }));
            }
            2 if (plan.scene == "rename" || plan.scene == "rename-apply") && plan.wait_frames == 4 => {
                let apply = plan.scene == "rename-apply";
                self.start_rename_from_editor();
                if let Some(r) = self.rename.as_mut() {
                    r.new_name = "amount".into();
                }
                if apply {
                    self.apply_rename(ctx);
                }
            }
            2 if plan.scene == "pg-compare" && plan.wait_frames < 400 && !compare_done => {
                plan.wait_frames = plan.wait_frames.min(5);
            }
            2 if plan.wait_frames == 4 && (plan.scene == "main" || plan.scene == "light") => {
                // open completion the way Ctrl+Space would
                ctx.input_mut(|i| {
                    i.events.push(egui::Event::Key { key: Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL });
                });
            }
            2 if plan.wait_frames > match plan.scene.as_str() { "flow" | "edit" | "csv-run" | "table-exec" | "rename-apply" | "edit-object" => 60, "pg-compare" => 30, _ => 14 } && self.tabs.get(self.active).map(|t| t.running.is_none()).unwrap_or(true) => {
                plan.step = 3;
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
            3 => {
                let shot = ctx.input(|i| {
                    i.events.iter().find_map(|e| match e {
                        egui::Event::Screenshot { image, .. } => Some(image.clone()),
                        _ => None,
                    })
                });
                if let Some(img) = shot {
                    if let Err(e) = save_png(&plan.path, &img) {
                        eprintln!("screenshot: {e}");
                    } else {
                        eprintln!("screenshot saved to {}", plan.path.display());
                    }
                    if let Some(b) = rss_bytes() {
                        eprintln!("resident memory: {} MB", b / (1024 * 1024));
                    }
                    if std::env::var_os("DATABUG_SHOT_REPORT").is_some() {
                        if let Some(t) = self.tabs.get(self.active) {
                            eprintln!("== active tab: {}\n{}", t.title, t.editor.text);
                            for l in &t.output {
                                eprintln!("   output: {}", l.text);
                            }
                            if let Some(e) = self.catalogs.get(&t.conn_id) {
                                for (s, tb) in e.catalog.tables() {
                                    eprintln!("   table {}.{}: {}", s.name, tb.name, tb.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", "));
                                }
                            }
                        }
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            _ => {}
        }
    }
}

fn human_bytes(b: usize) -> String {
    if b < 1024 * 1024 {
        format!("{:.0} KB", b as f64 / 1024.0)
    } else {
        format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
    }
}

fn save_png(path: &std::path::Path, img: &egui::ColorImage) -> anyhow::Result<()> {
    let f = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), img.size[0] as u32, img.size[1] as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header()?;
    w.write_image_data(img.as_raw())?;
    Ok(())
}

impl DataBugApp {
    /// Apply the "Text size" setting as egui's zoom factor, and keep the
    /// setting in step when the user zooms with Ctrl/⌘ +, − or 0.
    fn sync_zoom(&mut self, ctx: &egui::Context) {
        let want = self.settings.ui_scale.clamp(0.7, 2.0);
        let current = ctx.zoom_factor();
        match self.applied_zoom {
            Some(applied) if (applied - want).abs() < 0.001 && (current - applied).abs() > 0.001 => {
                // changed from the keyboard
                let z = (current.clamp(0.7, 2.0) * 100.0).round() / 100.0;
                self.settings.ui_scale = z;
                self.applied_zoom = Some(z);
                if (z - current).abs() > 0.001 {
                    ctx.set_zoom_factor(z);
                }
                self.store.save_settings(&self.settings);
            }
            Some(applied) if (applied - want).abs() < 0.001 => {}
            _ => {
                ctx.set_zoom_factor(want);
                self.applied_zoom = Some(want);
            }
        }
    }
}

impl eframe::App for DataBugApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let key = (self.settings.theme, self.settings.code_font_size);
        if self.applied_theme != Some(key) {
            theme::apply(ctx, key.0, key.1);
            self.applied_theme = Some(key);
        }
        self.sync_zoom(ctx);
        let p = *palette(self.settings.theme);
        self.pump_events(ctx);
        self.drive_screenshot(ctx);
        self.shortcuts(ctx);
        if self.tabs.iter().any(|t| t.running.is_some()) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        egui::TopBottomPanel::top("toolbar").exact_height(44.0).frame(egui::Frame::new().fill(p.canvas).inner_margin(Margin::symmetric(12, 6))).show(ctx, |ui| self.toolbar(ui, &p, ctx));
        egui::TopBottomPanel::bottom("status").exact_height(24.0).frame(egui::Frame::new().fill(p.canvas).inner_margin(Margin::symmetric(14, 2))).show(ctx, |ui| self.status_bar(ui, &p));

        let island = egui::Frame::new().fill(p.island).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(8));
        let mut actions = Vec::new();
        let width = self.settings.explorer_width;
        let views: HashMap<String, CatalogView> = self
            .catalogs
            .iter()
            .map(|(k, e)| (k.clone(), CatalogView { catalog: &e.catalog, loading: e.loading, error: e.error.as_deref(), age: if e.catalog.introspected_at > 0 { ago(e.catalog.introspected_at) } else { String::new() } }))
            .collect();
        let side = egui::SidePanel::left("explorer")
            .resizable(true)
            .default_width(width)
            .width_range(180.0..=520.0)
            .frame(egui::Frame::new().fill(p.canvas).inner_margin(Margin { left: 8, right: 3, top: 0, bottom: 0 }))
            .show_separator_line(false)
            .show(ctx, |ui| {
                island.show(ui, |ui| {
                    ui.set_min_height(ui.available_height());
                    let mut acts = crate::explorer::show(ui, &p, &mut self.explorer, &self.logos, &self.connections, &views);
                    // don't start another refresh while one is loading
                    acts.retain(|a| match a {
                        Action::Refresh(id) => !views.get(id).map(|e| e.loading).unwrap_or(false),
                        _ => true,
                    });
                    actions.extend(acts);
                });
            });
        drop(views);
        let w = side.response.rect.width();
        if (w - self.settings.explorer_width).abs() > 4.0 && !ctx.input(|i| i.pointer.any_down()) {
            self.settings.explorer_width = w;
            self.store.save_settings(&self.settings);
        }

        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.canvas).inner_margin(Margin { left: 3, right: 8, top: 0, bottom: 0 })).show(ctx, |ui| {
            if self.tabs.is_empty() {
                island.show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    actions.extend(self.welcome(ui, &p));
                });
                return;
            }
            if self.tabs.get(self.active).map(|t| t.tool.is_some()).unwrap_or(false) {
                island.show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    let (select, close, _) = self.tab_strip(ui, &p);
                    if let Some(i) = select {
                        self.active = i;
                    }
                    if let Some(i) = close {
                        self.close_tab(i);
                        return;
                    }
                    ui.add_space(4.0);
                    self.tool_ui(ui, &p, ctx);
                });
                return;
            }
            let h = ui.available_height();
            egui::TopBottomPanel::bottom("results")
                .resizable(true)
                .default_height(h * 0.45)
                .height_range(120.0..=(h - 140.0).max(160.0))
                .frame(egui::Frame::new().inner_margin(Margin { left: 0, right: 0, top: 6, bottom: 0 }))
                .show_separator_line(false)
                .show_inside(ui, |ui| {
                    island.show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        self.bottom_panel(ui, &p, ctx);
                    });
                });
            egui::CentralPanel::default().frame(egui::Frame::new()).show_inside(ui, |ui| {
                island.show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    let (select, close, new_tab) = self.tab_strip(ui, &p);
                    if let Some(i) = select {
                        self.active = i;
                        self.tabs[i].editor.focus();
                    }
                    if new_tab {
                        if let Some(id) = self.tabs.get(self.active).map(|t| t.conn_id.clone()) {
                            actions.push(Action::NewConsole(id));
                        }
                    }
                    ui.add_space(2.0);
                    if let Some(i) = close {
                        self.close_tab(i);
                    }
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        let entry = self.catalogs.get(&tab.conn_id);
                        let gen = entry.map(|e| e.generation).unwrap_or(0);
                        tab.editor.show(ui, &p, entry.map(|e| &e.catalog), gen, self.settings.auto_complete, self.settings.code_font_size);
                    }
                });
            });
        });

        for a in actions {
            self.apply(a, ctx);
        }
        self.confirm_window(ctx, &p);
        self.dialog_window(ctx, &p);
        self.modal_windows(ctx, &p);
        self.toasts(ctx, &p);
    }
}
