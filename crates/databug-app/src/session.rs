//! A session owns one database connection on a background Tokio task. The UI
//! sends commands and receives events over channels, so it never blocks.

use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use databug_core::{Capabilities, Catalog, ConnectionConfig, Plan, QueryEvent};
use databug_drivers::{Canceller, DbConnection, ExecOptions, Sink, SourceRef};
use tokio::sync::{mpsc as tmpsc, Notify};

pub type SessionId = u64;

pub enum Cmd {
    Execute { job: u64, stmts: Vec<String>, opts: ExecOptions },
    Explain { job: u64, stmt: String, analyze: bool },
    Introspect,
    /// Fetch definitions of views, routines and other objects.
    Sources { req: u64, schema: String, items: Vec<SourceRef> },
}

pub enum Ev {
    Connecting,
    Connected(Capabilities),
    ConnectFailed(String),
    /// The connection was dropped (after a forced cancel) and will reopen on demand.
    Dropped,
    Catalog(Result<Catalog, String>),
    Query { job: u64, idx: usize, ev: QueryEvent },
    StmtDone { job: u64, idx: usize, elapsed: Duration, error: Option<String> },
    JobDone { job: u64 },
    Plan { job: u64, result: Result<Plan, String>, elapsed: Duration },
    Sources { req: u64, results: Vec<(SourceRef, Result<String, String>)> },
}

pub type EvSender = mpsc::Sender<(SessionId, Ev)>;

pub struct Session {
    pub id: SessionId,
    tx: tmpsc::UnboundedSender<Cmd>,
    abort: Arc<Notify>,
    canceller: Arc<Mutex<Option<Canceller>>>,
    cancel_requested: Option<Instant>,
}

impl Session {
    pub fn spawn(rt: &tokio::runtime::Handle, id: SessionId, cfg: ConnectionConfig, password: Option<String>, out: EvSender, ctx: eframe::egui::Context) -> Session {
        let (tx, rx) = tmpsc::unbounded_channel();
        let abort = Arc::new(Notify::new());
        let canceller: Arc<Mutex<Option<Canceller>>> = Arc::default();
        rt.spawn(worker(id, cfg, password, rx, abort.clone(), canceller.clone(), out, ctx));
        Session { id, tx, abort, canceller, cancel_requested: None }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    /// First press asks the server to cancel; a second press (or an engine
    /// without server-side cancel) abandons the request and reconnects.
    pub fn cancel(&mut self) {
        let server = self.canceller.lock().unwrap().clone();
        match (server, self.cancel_requested) {
            (Some(c), None) => {
                c();
                self.cancel_requested = Some(Instant::now());
            }
            _ => {
                self.abort.notify_waiters();
                self.cancel_requested = None;
            }
        }
    }

    pub fn job_finished(&mut self) {
        self.cancel_requested = None;
    }
}

#[allow(clippy::too_many_arguments)]
async fn worker(
    id: SessionId,
    cfg: ConnectionConfig,
    password: Option<String>,
    mut rx: tmpsc::UnboundedReceiver<Cmd>,
    abort: Arc<Notify>,
    canceller: Arc<Mutex<Option<Canceller>>>,
    out: EvSender,
    ctx: eframe::egui::Context,
) {
    let send = {
        let out = out.clone();
        let ctx = ctx.clone();
        move |ev: Ev| {
            let _ = out.send((id, ev));
            ctx.request_repaint();
        }
    };
    let mut conn: Option<Box<dyn DbConnection>> = None;

    while let Some(cmd) = rx.recv().await {
        if conn.is_none() {
            send(Ev::Connecting);
            match databug_drivers::connect(&cfg, password.as_deref()).await {
                Ok(c) => {
                    *canceller.lock().unwrap() = c.canceller();
                    send(Ev::Connected(c.capabilities()));
                    conn = Some(c);
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    send(Ev::ConnectFailed(msg.clone()));
                    match cmd {
                        Cmd::Execute { job, .. } => {
                            send(Ev::StmtDone { job, idx: 0, elapsed: Duration::ZERO, error: Some(msg) });
                            send(Ev::JobDone { job });
                        }
                        Cmd::Explain { job, .. } => send(Ev::Plan { job, result: Err(msg), elapsed: Duration::ZERO }),
                        Cmd::Introspect => send(Ev::Catalog(Err(msg))),
                        Cmd::Sources { req, items, .. } => send(Ev::Sources { req, results: items.into_iter().map(|i| (i, Err(msg.clone()))).collect() }),
                    }
                    continue;
                }
            }
        }
        let c = conn.as_mut().unwrap();
        match cmd {
            Cmd::Execute { job, stmts, opts } => {
                let mut dropped = false;
                for (idx, stmt) in stmts.iter().enumerate() {
                    let sink: Sink = {
                        let out = out.clone();
                        let ctx = ctx.clone();
                        Arc::new(move |ev| {
                            let _ = out.send((id, Ev::Query { job, idx, ev }));
                            ctx.request_repaint();
                        })
                    };
                    let start = Instant::now();
                    let res = tokio::select! {
                        r = c.execute(stmt, &opts, &sink) => Some(r),
                        _ = abort.notified() => None,
                    };
                    let elapsed = start.elapsed();
                    match res {
                        Some(Ok(())) => send(Ev::StmtDone { job, idx, elapsed, error: None }),
                        Some(Err(e)) => {
                            send(Ev::StmtDone { job, idx, elapsed, error: Some(format!("{e:#}")) });
                            break;
                        }
                        None => {
                            send(Ev::StmtDone { job, idx, elapsed, error: Some("Cancelled; the connection was reset".into()) });
                            dropped = true;
                            break;
                        }
                    }
                }
                if dropped {
                    conn = None;
                    *canceller.lock().unwrap() = None;
                    send(Ev::Dropped);
                }
                send(Ev::JobDone { job });
            }
            Cmd::Explain { job, stmt, analyze } => {
                let start = Instant::now();
                let res = tokio::select! {
                    r = c.explain(&stmt, analyze) => Some(r),
                    _ = abort.notified() => None,
                };
                match res {
                    Some(r) => send(Ev::Plan { job, result: r.map_err(|e| format!("{e:#}")), elapsed: start.elapsed() }),
                    None => {
                        conn = None;
                        send(Ev::Plan { job, result: Err("Cancelled".into()), elapsed: start.elapsed() });
                        send(Ev::Dropped);
                    }
                }
            }
            Cmd::Introspect => {
                let r = c.introspect().await.map_err(|e| format!("{e:#}"));
                send(Ev::Catalog(r));
            }
            Cmd::Sources { req, schema, items } => {
                let mut results = Vec::with_capacity(items.len());
                for item in items {
                    let r = c.object_source(&schema, &item).await.map_err(|e| format!("{e:#}"));
                    results.push((item, r));
                }
                send(Ev::Sources { req, results });
            }
        }
    }
}
