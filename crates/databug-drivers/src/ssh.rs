//! SSH tunnels: reach a database through an SSH server (a bastion or jump
//! host). DataBug signs in to the SSH server, listens on a local port on
//! 127.0.0.1 and forwards every connection to it through the SSH session to
//! the database's host and port, as `ssh -L` does. The driver then connects
//! to the local port, so every network engine works unchanged.
//!
//! Sessions with the same SSH server and database target share one tunnel;
//! it closes when the last connection using it goes away.
//!
//! Host keys: the user's `~/.ssh/known_hosts` is checked first, then
//! DataBug's own file. A host found in neither is trusted on first use and
//! remembered in DataBug's file (`ssh -o StrictHostKeyChecking=accept-new`);
//! a key that changed is always refused.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use databug_core::{Capabilities, Catalog, ConnectionConfig, Plan, SshAuth, SshTunnel};
use russh::client;
use russh::keys::{self, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};

use crate::{Canceller, DbConnection, ExecOptions, Sink, SourceRef};

static KNOWN_HOSTS: OnceLock<PathBuf> = OnceLock::new();
static TUNNELS: OnceLock<Mutex<HashMap<String, Weak<Tunnel>>>> = OnceLock::new();

/// Where DataBug remembers the SSH host keys it trusted on first use
/// (`known_hosts` format). Set once at startup.
pub fn set_known_hosts_file(path: PathBuf) {
    let _ = KNOWN_HOSTS.set(path);
}

/// A running tunnel: a local port forwarded to the database through SSH.
pub struct Tunnel {
    pub port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        // stops accepting, and drops the SSH session with the listener task
        self.task.abort();
    }
}

/// Open (or reuse) the tunnel for `cfg`, whose `ssh` must be set. `secret`
/// is the SSH password or the key's passphrase.
pub async fn open(cfg: &ConnectionConfig, secret: Option<&str>) -> anyhow::Result<Arc<Tunnel>> {
    let ssh = cfg.ssh.as_ref().ok_or_else(|| anyhow::anyhow!("No SSH tunnel is set up for this connection"))?;
    if ssh.host.trim().is_empty() {
        anyhow::bail!("Enter the SSH server's host name in the connection's SSH tunnel settings");
    }
    if ssh.user.trim().is_empty() {
        anyhow::bail!("Enter the SSH user name in the connection's SSH tunnel settings");
    }
    let key = format!("{}@{}:{}|{}:{}|{:?}|{}", ssh.user, ssh.host, ssh.port, cfg.host, cfg.port, ssh.auth, ssh.key_path);
    let registry = TUNNELS.get_or_init(Default::default);
    if let Some(t) = registry.lock().unwrap().get(&key).and_then(Weak::upgrade) {
        return Ok(t);
    }
    let t = match tokio::time::timeout(Duration::from_secs(20), start(ssh, &cfg.host, cfg.port, secret)).await {
        Ok(r) => Arc::new(r?),
        Err(_) => anyhow::bail!("The SSH server {}:{} didn't answer within 20 seconds", ssh.host, ssh.port),
    };
    let mut map = registry.lock().unwrap();
    map.retain(|_, w| w.strong_count() > 0);
    map.insert(key, Arc::downgrade(&t));
    Ok(t)
}

async fn start(ssh: &SshTunnel, db_host: &str, db_port: u16, secret: Option<&str>) -> anyhow::Result<Tunnel> {
    let config = Arc::new(client::Config {
        inactivity_timeout: None,
        // keeps idle tunnels alive through firewalls and NAT
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 4,
        ..Default::default()
    });
    let rejected = Arc::new(Mutex::new(None));
    let checker = HostKeys { host: ssh.host.trim().to_string(), port: ssh.port, rejected: rejected.clone() };
    let mut session = match client::connect(config, (ssh.host.trim(), ssh.port), checker).await {
        Ok(s) => s,
        Err(e) => {
            if let Some(why) = rejected.lock().unwrap().take() {
                anyhow::bail!(why);
            }
            anyhow::bail!("Couldn't reach the SSH server {}:{}: {e}", ssh.host, ssh.port);
        }
    };
    let user = ssh.user.trim();
    let signed_in = match ssh.auth {
        SshAuth::Password => session.authenticate_password(user, secret.unwrap_or("")).await?.success(),
        SshAuth::Key => {
            let path = expand_home(&ssh.key_path);
            if ssh.key_path.trim().is_empty() {
                anyhow::bail!("Choose the private key file in the connection's SSH tunnel settings");
            }
            let pass = secret.filter(|s| !s.is_empty());
            let key = keys::load_secret_key(&path, pass).map_err(|e| match e {
                keys::Error::KeyIsEncrypted => anyhow::anyhow!("The key {} is protected by a passphrase: enter it in the SSH tunnel settings", path.display()),
                keys::Error::IO(io) if io.kind() == std::io::ErrorKind::NotFound => anyhow::anyhow!("The key file {} doesn't exist", path.display()),
                e => anyhow::anyhow!("Couldn't read the key {}: {e} (a wrong passphrase also shows up like this)", path.display()),
            })?;
            let hash = session.best_supported_rsa_hash().await?.flatten();
            session.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await?.success()
        }
        SshAuth::Agent => agent_sign_in(&mut session, user).await?,
    };
    if !signed_in {
        let what = match ssh.auth {
            SshAuth::Password => "password",
            SshAuth::Key => "key",
            SshAuth::Agent => "agent's keys",
        };
        anyhow::bail!("The SSH server {} didn't accept the {what} for {user}", ssh.host);
    }
    // the database has to be reachable from the SSH server: check once now,
    // so the message says where it failed
    let probe = session.channel_open_direct_tcpip(db_host, db_port as u32, "127.0.0.1", 0).await.map_err(|e| {
        anyhow::anyhow!("Signed in to the SSH server {}, but it couldn't reach the database at {db_host}:{db_port} ({e}). The database host is looked up from the SSH server, so use the address it knows the database by (often localhost or a private IP).", ssh.host)
    })?;
    let _ = probe.close().await;

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let session = Arc::new(session);
    let db_host = db_host.to_string();
    let task = tokio::spawn(async move {
        while let Ok((mut socket, peer)) = listener.accept().await {
            let session = session.clone();
            let db_host = db_host.clone();
            tokio::spawn(async move {
                let _ = socket.set_nodelay(true);
                if let Ok(channel) = session.channel_open_direct_tcpip(db_host, db_port as u32, "127.0.0.1", peer.port() as u32).await {
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
                }
            });
        }
    });
    Ok(Tunnel { port, task })
}

/// Sign in with a key held by the running SSH agent.
async fn agent_sign_in(session: &mut client::Handle<HostKeys>, user: &str) -> anyhow::Result<bool> {
    #[cfg(unix)]
    {
        let agent = keys::agent::client::AgentClient::connect_env().await.map_err(|_| anyhow::anyhow!("No SSH agent is running (SSH_AUTH_SOCK isn't set). Start one and add your key with ssh-add, or choose a key file"))?;
        try_agent(session, user, agent).await
    }
    #[cfg(windows)]
    {
        match keys::agent::client::AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await {
            Ok(agent) => try_agent(session, user, agent).await,
            Err(_) => {
                let agent = keys::agent::client::AgentClient::connect_pageant().await.map_err(|_| anyhow::anyhow!("No SSH agent is running (neither the OpenSSH agent service nor Pageant). Start one, or choose a key file"))?;
                try_agent(session, user, agent).await
            }
        }
    }
}

async fn try_agent<S>(session: &mut client::Handle<HostKeys>, user: &str, mut agent: keys::agent::client::AgentClient<S>) -> anyhow::Result<bool>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let identities = agent.request_identities().await.map_err(|e| anyhow::anyhow!("Couldn't read the SSH agent's keys: {e}"))?;
    if identities.is_empty() {
        anyhow::bail!("The SSH agent holds no keys: add one with ssh-add");
    }
    let hash = session.best_supported_rsa_hash().await?.flatten();
    for id in identities {
        let key = match id {
            keys::agent::AgentIdentity::PublicKey { key, .. } => key,
            _ => continue,
        };
        if let Ok(r) = session.authenticate_publickey_with(user, key, hash, &mut agent).await {
            if r.success() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// `~/…` in a key path, as typed or as ssh's config writes it.
fn expand_home(path: &str) -> PathBuf {
    let path = path.trim();
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        if let Some(h) = home {
            return PathBuf::from(h).join(rest);
        }
    }
    PathBuf::from(path)
}

struct HostKeys {
    host: String,
    port: u16,
    /// Why the server's key was refused, for the error message.
    rejected: Arc<Mutex<Option<String>>>,
}

impl client::Handler for HostKeys {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let key = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(c) => PublicKey::from(c.public_key().clone()),
        };
        match verify_host(&self.host, self.port, &key) {
            Ok(()) => Ok(true),
            Err(why) => {
                *self.rejected.lock().unwrap() = Some(why);
                Ok(false)
            }
        }
    }
}

/// Accept the SSH server's key: known to ssh or to DataBug, or new (and then
/// remembered). A key that differs from the recorded one is refused.
fn verify_host(host: &str, port: u16, key: &PublicKey) -> Result<(), String> {
    let fingerprint = key.fingerprint(Default::default());
    let changed = |file: &str| {
        format!(
            "The SSH server {host}'s host key changed (it now presents {fingerprint}), so DataBug won't connect: someone may be intercepting the connection. If the server was reinstalled, remove its old entry from {file} and connect again."
        )
    };
    // the user's own known_hosts (what ssh trusts); a missing file is fine
    match keys::check_known_hosts(host, port, key) {
        Ok(true) => return Ok(()),
        Err(keys::Error::KeyChanged { .. }) => return Err(changed("~/.ssh/known_hosts")),
        _ => {}
    }
    let Some(ours) = KNOWN_HOSTS.get() else {
        return Err(format!("The SSH server {host} isn't in ~/.ssh/known_hosts (its key is {fingerprint})"));
    };
    match keys::check_known_hosts_path(host, port, key, ours) {
        Ok(true) => Ok(()),
        Err(keys::Error::KeyChanged { .. }) => Err(changed(&ours.display().to_string())),
        _ => keys::known_hosts::learn_known_hosts_path(host, port, key, ours).map_err(|e| format!("Couldn't save the SSH server's key to {}: {e}", ours.display())),
    }
}

/// A database connection through a tunnel: the tunnel stays open while the
/// connection is in use.
pub(crate) struct Tunneled {
    pub inner: Box<dyn DbConnection>,
    pub _tunnel: Arc<Tunnel>,
}

#[async_trait]
impl DbConnection for Tunneled {
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
    fn canceller(&self) -> Option<Canceller> {
        self.inner.canceller()
    }
    fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }
    async fn execute(&mut self, stmt: &str, opts: &ExecOptions, sink: &Sink) -> anyhow::Result<()> {
        self.inner.execute(stmt, opts, sink).await
    }
    async fn introspect(&mut self) -> anyhow::Result<Catalog> {
        self.inner.introspect().await
    }
    async fn list_databases(&mut self) -> anyhow::Result<(Vec<String>, String)> {
        self.inner.list_databases().await
    }
    async fn explain(&mut self, stmt: &str, analyze: bool) -> anyhow::Result<Plan> {
        self.inner.explain(stmt, analyze).await
    }
    async fn object_source(&mut self, schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
        self.inner.object_source(schema, obj).await
    }
    async fn attachment(&mut self, collection: &str, doc_id: &str, name: &str) -> anyhow::Result<Vec<u8>> {
        self.inner.attachment(collection, doc_id, name).await
    }
}
