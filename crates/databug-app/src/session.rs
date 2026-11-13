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
    /// List the databases on the server.
    ListDatabases,
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
    /// Databases on the server and the one this connection uses.
    Databases(Result<(Vec<String>, String), String>),
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

    /// Abandon whatever is running (used when disconnecting).
    pub fn abort(&self) {
        self.abort.notify_waiters();
    }

    pub fn job_finished(&mut self) {
        self.cancel_requested = None;
    }
}

/// Whether an error means the connection itself is gone (server restarted,
/// network dropped, idle timeout), so the next command must reconnect.
fn connection_lost(c: &dyn DbConnection, err: &str) -> bool {
    c.is_closed() || lost_message(err)
}

fn lost_message(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    [
        "connection closed",
        "connection reset",
        "connection refused",
        "broken pipe",
        "server has gone away",
        "lost connection",
        "connection was closed",
        "not connected",
        "connection aborted",
        "unexpected eof",
        "end of file",
        "socket closed",
        "timed out",
    ]
    .iter()
    .any(|k| e.contains(k))
}

/// Answer a command that can't run because the connection failed.
fn fail(send: &impl Fn(Ev), cmd: Cmd, msg: &str) {
    let msg = msg.to_string();
    match cmd {
        Cmd::Execute { job, .. } => {
            send(Ev::StmtDone { job, idx: 0, elapsed: Duration::ZERO, error: Some(msg) });
            send(Ev::JobDone { job });
        }
        Cmd::Explain { job, .. } => send(Ev::Plan { job, result: Err(msg), elapsed: Duration::ZERO }),
        Cmd::Introspect => send(Ev::Catalog(Err(msg))),
        Cmd::ListDatabases => send(Ev::Databases(Err(msg))),
        Cmd::Sources { req, items, .. } => send(Ev::Sources { req, results: items.into_iter().map(|i| (i, Err(msg.clone()))).collect() }),
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
                    // Fail this command and everything already queued with the
                    // same error instead of signing in again for each one:
                    // repeated failed logins can lock the account.
                    fail(&send, cmd, &msg);
                    while let Ok(queued) = rx.try_recv() {
                        fail(&send, queued, &msg);
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
                            let msg = format!("{e:#}");
                            dropped = connection_lost(c.as_ref(), &msg);
                            send(Ev::StmtDone { job, idx, elapsed, error: Some(msg) });
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
                    Some(r) => {
                        let r = r.map_err(|e| format!("{e:#}"));
                        let lost = r.as_ref().err().map(|m| connection_lost(c.as_ref(), m)).unwrap_or(false);
                        send(Ev::Plan { job, result: r, elapsed: start.elapsed() });
                        if lost {
                            conn = None;
                            *canceller.lock().unwrap() = None;
                            send(Ev::Dropped);
                        }
                    }
                    None => {
                        conn = None;
                        send(Ev::Plan { job, result: Err("Cancelled".into()), elapsed: start.elapsed() });
                        send(Ev::Dropped);
                    }
                }
            }
            Cmd::Introspect => {
                let r = c.introspect().await.map_err(|e| format!("{e:#}"));
                let lost = r.as_ref().err().map(|m| connection_lost(c.as_ref(), m)).unwrap_or(false);
                send(Ev::Catalog(r));
                if lost {
                    conn = None;
                    *canceller.lock().unwrap() = None;
                    send(Ev::Dropped);
                }
            }
            Cmd::ListDatabases => {
                let r = c.list_databases().await.map_err(|e| format!("{e:#}"));
                send(Ev::Databases(r));
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

#[cfg(test)]
mod tests {
    use super::lost_message;

    #[test]
    fn lost_connections() {
        assert!(lost_message("db error: connection closed"));
        assert!(lost_message("MySQL server has gone away"));
        assert!(lost_message("Broken pipe (os error 32)"));
        assert!(!lost_message("relation \"nope\" does not exist"));
        assert!(!lost_message("FOREIGN KEY constraint failed"));
    }
}
