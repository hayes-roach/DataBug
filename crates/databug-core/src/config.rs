use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DbKind {
    Postgres,
    MySql,
    MsSql,
    Sqlite,
    MongoDb,
}

impl DbKind {
    pub const ALL: [DbKind; 5] = [
        DbKind::Postgres,
        DbKind::MySql,
        DbKind::MsSql,
        DbKind::Sqlite,
        DbKind::MongoDb,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DbKind::Postgres => "PostgreSQL",
            DbKind::MySql => "MySQL / MariaDB",
            DbKind::MsSql => "SQL Server",
            DbKind::Sqlite => "SQLite",
            DbKind::MongoDb => "MongoDB",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            DbKind::Postgres => "PG",
            DbKind::MySql => "MY",
            DbKind::MsSql => "MS",
            DbKind::Sqlite => "LT",
            DbKind::MongoDb => "MG",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            DbKind::Postgres => 5432,
            DbKind::MySql => 3306,
            DbKind::MsSql => 1433,
            DbKind::Sqlite => 0,
            DbKind::MongoDb => 27017,
        }
    }

    pub fn dialect(self) -> Dialect {
        match self {
            DbKind::Postgres => Dialect::Postgres,
            DbKind::MySql => Dialect::MySql,
            DbKind::MsSql => Dialect::MsSql,
            DbKind::Sqlite => Dialect::Sqlite,
            DbKind::MongoDb => Dialect::Mongo,
        }
    }

    pub fn is_file_based(self) -> bool {
        matches!(self, DbKind::Sqlite)
    }
}

/// The language spoken by a connection's console.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Postgres,
    MySql,
    MsSql,
    Sqlite,
    Mongo,
}

impl Dialect {
    pub fn quote_ident(self, name: &str) -> String {
        let plain = !name.is_empty()
            && name
                .chars()
                .next()
                .map(|c| c.is_ascii_alphabetic() || c == '_')
                .unwrap_or(false)
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && (self != Dialect::Postgres || name.chars().all(|c| !c.is_ascii_uppercase()));
        if plain {
            return name.to_string();
        }
        match self {
            Dialect::MySql => format!("`{}`", name.replace('`', "``")),
            Dialect::MsSql => format!("[{}]", name.replace(']', "]]")),
            _ => format!("\"{}\"", name.replace('"', "\"\"")),
        }
    }

    pub fn quote_literal(self, value: &str) -> String {
        let escaped = value.replace('\'', "''");
        if self == Dialect::MySql {
            format!("'{}'", escaped.replace('\\', "\\\\"))
        } else {
            format!("'{escaped}'")
        }
    }

    pub fn begin_sql(self) -> &'static str {
        match self {
            Dialect::MySql => "START TRANSACTION",
            Dialect::MsSql => "BEGIN TRANSACTION",
            _ => "BEGIN",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SslMode {
    Disable,
    #[default]
    Prefer,
    Require,
}

/// How SQL Server checks who you are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MsAuth {
    /// SQL Server login: user name + password.
    #[default]
    SqlServer,
    /// The signed-in Windows account (SSPI on Windows, a Kerberos ticket on macOS/Linux).
    WindowsIntegrated,
    /// A Windows account given explicitly as DOMAIN\user + password (NTLM/Kerberos).
    WindowsCredentials,
}

impl MsAuth {
    pub const ALL: [MsAuth; 3] = [MsAuth::SqlServer, MsAuth::WindowsIntegrated, MsAuth::WindowsCredentials];

    pub fn label(self) -> &'static str {
        match self {
            MsAuth::SqlServer => "SQL Server",
            MsAuth::WindowsIntegrated => "Windows (current user)",
            MsAuth::WindowsCredentials => "Windows credentials",
        }
    }

    pub fn needs_password(self) -> bool {
        self != MsAuth::WindowsIntegrated
    }
}

/// Saved connection settings. Passwords are never stored here; they live in
/// the OS keychain under the connection's `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    pub kind: DbKind,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub database: String,
    /// File path for SQLite.
    #[serde(default)]
    pub path: String,
    /// Full URI for MongoDB (overrides host/port when set). Must not embed a password.
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub ssl: SslMode,
    /// Accept self-signed server certificates (SQL Server dev boxes).
    #[serde(default)]
    pub trust_server_cert: bool,
    #[serde(default)]
    pub production: bool,
    #[serde(default)]
    pub read_only: bool,
    /// Accent color as RGB.
    #[serde(default = "default_color")]
    pub color: [u8; 3],
    /// SQL Server authentication method.
    #[serde(default)]
    pub ms_auth: MsAuth,
    /// Schemas shown in the explorer. `None` shows every schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_schemas: Option<Vec<String>>,
}

fn default_color() -> [u8; 3] {
    [86, 156, 214]
}

impl ConnectionConfig {
    pub fn new(kind: DbKind) -> Self {
        let id = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        ConnectionConfig {
            id,
            name: String::new(),
            kind,
            host: "localhost".into(),
            port: kind.default_port(),
            user: String::new(),
            database: String::new(),
            path: String::new(),
            uri: String::new(),
            ssl: SslMode::Prefer,
            trust_server_cert: false,
            production: false,
            read_only: false,
            color: default_color(),
            ms_auth: MsAuth::SqlServer,
            visible_schemas: None,
        }
    }

    /// Whether the explorer shows this schema. The default schema is always
    /// shown when nothing else would be.
    pub fn schema_visible(&self, name: &str) -> bool {
        match &self.visible_schemas {
            None => true,
            Some(list) => list.iter().any(|s| s.eq_ignore_ascii_case(name)),
        }
    }

    pub fn needs_password(&self) -> bool {
        !(self.kind == DbKind::MsSql && !self.ms_auth.needs_password()) && !self.kind.is_file_based()
    }

    pub fn display_target(&self) -> String {
        match self.kind {
            DbKind::Sqlite => self.path.clone(),
            DbKind::MongoDb if !self.uri.is_empty() => self.uri.clone(),
            _ => format!("{}:{}/{}", self.host, self.port, self.database),
        }
    }
}

/// What a driver can do, so the UI can adapt per engine.
#[derive(Debug, Clone, Copy, Default)]
pub struct Capabilities {
    pub schemas: bool,
    pub transactions: bool,
    pub explain: bool,
    pub explain_analyze: bool,
    pub server_cancel: bool,
    pub sql: bool,
}
