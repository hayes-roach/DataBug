use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DbKind {
    Postgres,
    MySql,
    MsSql,
    Sqlite,
    MongoDb,
    Oracle,
    Db2,
    DuckDb,
    Cassandra,
    CouchDb,
    CockroachDb,
    TimescaleDb,
    QuestDb,
    InfluxDb,
    Spark,
    H2,
}

/// What kind of database an engine is, for grouping engines in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DbCategory {
    Relational,
    Analytical,
    Document,
    WideColumn,
    TimeSeries,
    Distributed,
}

impl DbCategory {
    pub const ALL: [DbCategory; 6] = [DbCategory::Relational, DbCategory::Analytical, DbCategory::Document, DbCategory::WideColumn, DbCategory::TimeSeries, DbCategory::Distributed];

    pub fn label(self) -> &'static str {
        match self {
            DbCategory::Relational => "Relational / SQL",
            DbCategory::Analytical => "Analytical",
            DbCategory::Document => "NoSQL · Document",
            DbCategory::WideColumn => "NoSQL · Wide-column",
            DbCategory::TimeSeries => "Time series",
            DbCategory::Distributed => "Big data / distributed",
        }
    }

    /// The engines in this category, in `DbKind::ALL` order.
    pub fn kinds(self) -> impl Iterator<Item = DbKind> {
        DbKind::ALL.into_iter().filter(move |k| k.category() == self)
    }
}

impl DbKind {
    pub const ALL: [DbKind; 16] = [
        DbKind::Postgres,
        DbKind::MySql,
        DbKind::MsSql,
        DbKind::Sqlite,
        DbKind::MongoDb,
        DbKind::Oracle,
        DbKind::Db2,
        DbKind::H2,
        DbKind::DuckDb,
        DbKind::CouchDb,
        DbKind::Cassandra,
        DbKind::TimescaleDb,
        DbKind::QuestDb,
        DbKind::InfluxDb,
        DbKind::CockroachDb,
        DbKind::Spark,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DbKind::Postgres => "PostgreSQL",
            DbKind::MySql => "MySQL / MariaDB",
            DbKind::MsSql => "SQL Server",
            DbKind::Sqlite => "SQLite",
            DbKind::MongoDb => "MongoDB",
            DbKind::Oracle => "Oracle",
            DbKind::Db2 => "IBM Db2",
            DbKind::DuckDb => "DuckDB",
            DbKind::Cassandra => "Cassandra",
            DbKind::CouchDb => "CouchDB",
            DbKind::CockroachDb => "CockroachDB",
            DbKind::TimescaleDb => "TimescaleDB",
            DbKind::QuestDb => "QuestDB",
            DbKind::InfluxDb => "InfluxDB",
            DbKind::Spark => "Spark SQL",
            DbKind::H2 => "H2",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            DbKind::Postgres => "PG",
            DbKind::MySql => "MY",
            DbKind::MsSql => "MS",
            DbKind::Sqlite => "LT",
            DbKind::MongoDb => "MG",
            DbKind::Oracle => "OR",
            DbKind::Db2 => "D2",
            DbKind::DuckDb => "DK",
            DbKind::Cassandra => "CA",
            DbKind::CouchDb => "CO",
            DbKind::CockroachDb => "CR",
            DbKind::TimescaleDb => "TS",
            DbKind::QuestDb => "QD",
            DbKind::InfluxDb => "IX",
            DbKind::Spark => "SP",
            DbKind::H2 => "H2",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            DbKind::Postgres => 5432,
            DbKind::MySql => 3306,
            DbKind::MsSql => 1433,
            DbKind::Sqlite => 0,
            DbKind::MongoDb => 27017,
            DbKind::Oracle => 1521,
            DbKind::Db2 => 50000,
            DbKind::DuckDb => 0,
            DbKind::Cassandra => 9042,
            DbKind::CouchDb => 5984,
            DbKind::CockroachDb => 26257,
            DbKind::TimescaleDb => 5432,
            // QuestDB's PostgreSQL wire protocol port
            DbKind::QuestDb => 8812,
            DbKind::InfluxDb => 8086,
            // Spark Thrift server (HiveServer2 protocol)
            DbKind::Spark => 10000,
            // H2's PostgreSQL server mode (`-pg`)
            DbKind::H2 => 5435,
        }
    }

    /// The console language. Engines without a dialect of their own use the
    /// closest one for highlighting, completion and generated SQL: the
    /// PostgreSQL-compatible engines, Oracle, Db2, DuckDB, Cassandra (CQL)
    /// and InfluxDB (InfluxQL) quote identifiers with `"` like Postgres;
    /// Spark SQL quotes with backticks like MySQL; CouchDB's Mango queries
    /// are written in the mongo-shell form.
    pub fn dialect(self) -> Dialect {
        match self {
            DbKind::Postgres => Dialect::Postgres,
            DbKind::MySql => Dialect::MySql,
            DbKind::MsSql => Dialect::MsSql,
            DbKind::Sqlite => Dialect::Sqlite,
            DbKind::MongoDb => Dialect::Mongo,
            DbKind::Oracle | DbKind::Db2 | DbKind::DuckDb | DbKind::Cassandra | DbKind::CockroachDb | DbKind::TimescaleDb | DbKind::QuestDb | DbKind::InfluxDb | DbKind::H2 => Dialect::Postgres,
            DbKind::Spark => Dialect::MySql,
            DbKind::CouchDb => Dialect::Mongo,
        }
    }

    pub fn category(self) -> DbCategory {
        match self {
            DbKind::Postgres | DbKind::MySql | DbKind::MsSql | DbKind::Sqlite | DbKind::Oracle | DbKind::Db2 | DbKind::H2 => DbCategory::Relational,
            DbKind::DuckDb => DbCategory::Analytical,
            DbKind::MongoDb | DbKind::CouchDb => DbCategory::Document,
            DbKind::Cassandra => DbCategory::WideColumn,
            DbKind::TimescaleDb | DbKind::QuestDb | DbKind::InfluxDb => DbCategory::TimeSeries,
            DbKind::CockroachDb | DbKind::Spark => DbCategory::Distributed,
        }
    }

    /// Database used when the connection doesn't name one.
    pub fn default_database(self) -> &'static str {
        match self {
            DbKind::Postgres | DbKind::TimescaleDb => "postgres",
            DbKind::MongoDb => "test",
            DbKind::CockroachDb => "defaultdb",
            DbKind::QuestDb => "qdb",
            DbKind::Spark => "default",
            _ => "",
        }
    }

    pub fn is_file_based(self) -> bool {
        matches!(self, DbKind::Sqlite | DbKind::DuckDb)
    }

    /// Whether the engine speaks the PostgreSQL wire protocol (and is
    /// reached through the Postgres driver).
    pub fn is_pg_wire(self) -> bool {
        matches!(self, DbKind::Postgres | DbKind::CockroachDb | DbKind::TimescaleDb | DbKind::QuestDb | DbKind::H2)
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

    /// A string literal for this dialect. SQL Server gets `N'…'` so text
    /// outside the database's code page (é, 李, emoji) isn't turned into `?`.
    pub fn quote_literal(self, value: &str) -> String {
        let escaped = value.replace('\'', "''");
        match self {
            Dialect::MySql => format!("'{}'", escaped.replace('\\', "\\\\")),
            Dialect::MsSql => format!("N'{escaped}'"),
            _ => format!("'{escaped}'"),
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
    /// Oracle: `database` is a SID rather than a service name.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub oracle_sid: bool,
    /// InfluxDB 2.x organization (for Flux queries and the bucket list).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub org: String,
    /// Db2 / Spark: the ODBC driver name as registered with the driver
    /// manager, e.g. `IBM DB2 ODBC DRIVER` or `Simba Spark ODBC Driver`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub odbc_driver: String,
    /// Db2 / Spark: extra `key=value;` pairs for the ODBC connection string
    /// (never a password).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub odbc_options: String,
    /// Schemas shown in the explorer. `None` shows every schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_schemas: Option<Vec<String>>,
    /// Extra databases on the same server shown in the explorer, like
    /// schemas. `None` shows only the connection's own database.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_databases: Option<Vec<String>>,
    /// Schema visibility for the extra databases, by database name.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub database_schemas: std::collections::BTreeMap<String, Vec<String>>,
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
            oracle_sid: false,
            org: String::new(),
            odbc_driver: String::new(),
            odbc_options: String::new(),
            visible_schemas: None,
            visible_databases: None,
            database_schemas: Default::default(),
        }
    }

    /// The database to connect to: the configured one, or the engine's
    /// default when none is set (`postgres` on PostgreSQL; SQL Server uses
    /// the login's default database and MySQL connects without one).
    pub fn effective_database(&self) -> String {
        if !self.database.trim().is_empty() {
            return self.database.trim().to_string();
        }
        self.kind.default_database().to_string()
    }

    /// Whether this engine has several databases per server that can be
    /// browsed separately (MySQL's databases are its schemas).
    pub fn supports_databases(&self) -> bool {
        matches!(self.kind, DbKind::Postgres | DbKind::MsSql | DbKind::MongoDb | DbKind::CockroachDb | DbKind::TimescaleDb)
    }

    /// The config for another database on the same server. Its id is
    /// `<connection id>@<database>`, which `base_id` maps back.
    pub fn for_database(&self, db: &str) -> ConnectionConfig {
        let mut c = self.clone();
        c.id = format!("{}@{}", self.id, db);
        c.name = format!("{} · {}", self.name, db);
        c.database = db.to_string();
        c.visible_schemas = self.database_schemas.get(db).cloned();
        c.visible_databases = None;
        c.database_schemas = Default::default();
        c
    }

    /// The saved connection an id belongs to (strips `@database`).
    pub fn base_id(id: &str) -> &str {
        id.split_once('@').map(|(a, _)| a).unwrap_or(id)
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
        (self.kind != DbKind::MsSql || self.ms_auth.needs_password()) && !self.kind.is_file_based()
    }

    pub fn display_target(&self) -> String {
        match self.kind {
            DbKind::Sqlite | DbKind::DuckDb => self.path.clone(),
            DbKind::MongoDb if !self.uri.is_empty() => self.uri.clone(),
            _ => {
                let db = self.effective_database();
                if db.is_empty() {
                    format!("{}:{} (default database)", self.host, self.port)
                } else {
                    format!("{}:{}/{}", self.host, self.port, db)
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_has_one_category() {
        let grouped: Vec<DbKind> = DbCategory::ALL.iter().flat_map(|c| c.kinds()).collect();
        assert_eq!(grouped.len(), DbKind::ALL.len());
        for k in DbKind::ALL {
            assert_eq!(grouped.iter().filter(|g| **g == k).count(), 1, "{k:?}");
        }
        assert_eq!(DbKind::DuckDb.category(), DbCategory::Analytical);
        assert_eq!(DbKind::Cassandra.category(), DbCategory::WideColumn);
        assert_eq!(DbKind::InfluxDb.category(), DbCategory::TimeSeries);
    }

    #[test]
    fn engine_codes_and_labels_are_unique() {
        for (i, a) in DbKind::ALL.iter().enumerate() {
            for b in &DbKind::ALL[i + 1..] {
                assert_ne!(a.short(), b.short());
                assert_ne!(a.label(), b.label());
            }
        }
    }

    #[test]
    fn new_engine_defaults() {
        assert_eq!(DbKind::CockroachDb.default_port(), 26257);
        assert_eq!(DbKind::QuestDb.default_port(), 8812);
        assert_eq!(DbKind::Oracle.default_port(), 1521);
        assert_eq!(DbKind::Spark.dialect(), Dialect::MySql);
        assert_eq!(DbKind::CouchDb.dialect(), Dialect::Mongo);
        assert!(DbKind::DuckDb.is_file_based());
        assert!(DbKind::H2.is_pg_wire() && !DbKind::DuckDb.is_pg_wire());
        let c = ConnectionConfig::new(DbKind::CockroachDb);
        assert_eq!(c.effective_database(), "defaultdb");
        assert!(c.supports_databases());
        let mut d = ConnectionConfig::new(DbKind::DuckDb);
        d.path = "/data/analytics.duckdb".into();
        assert!(!d.needs_password());
        assert_eq!(d.display_target(), "/data/analytics.duckdb");
    }

    #[test]
    fn old_configs_still_load() {
        let json = r#"{"id":"1","name":"store","kind":"Postgres","host":"db.example.com","port":5432}"#;
        let c: ConnectionConfig = serde_json::from_str(json).unwrap();
        assert!(c.org.is_empty() && !c.oracle_sid && c.odbc_driver.is_empty());
        // empty new fields stay out of the saved file
        let out = serde_json::to_string(&c).unwrap();
        assert!(!out.contains("oracle_sid") && !out.contains("odbc_driver"));
    }

    #[test]
    fn literals_per_dialect() {
        assert_eq!(Dialect::Postgres.quote_literal("it's"), "'it''s'");
        assert_eq!(Dialect::Sqlite.quote_literal("a\\b"), "'a\\b'");
        assert_eq!(Dialect::MySql.quote_literal("it's a\\b"), "'it''s a\\\\b'");
        assert_eq!(Dialect::MsSql.quote_literal("Zoë 李's"), "N'Zoë 李''s'");
    }

    #[test]
    fn identifiers_per_dialect() {
        assert_eq!(Dialect::Postgres.quote_ident("orders"), "orders");
        assert_eq!(Dialect::Postgres.quote_ident("Orders"), "\"Orders\"");
        assert_eq!(Dialect::Postgres.quote_ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(Dialect::MySql.quote_ident("a`b"), "`a``b`");
        assert_eq!(Dialect::MsSql.quote_ident("a]b c"), "[a]]b c]");
        assert_eq!(Dialect::Sqlite.quote_ident("1st"), "\"1st\"");
    }
}
