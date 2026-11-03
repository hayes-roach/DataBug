use crate::{build_catalog, cell, collect, plan, replace_create, scalar, split_list, truthy, Batcher, Canceller, DbConnection, ExecOptions, Extras, Sink, SourceRef};
use async_trait::async_trait;
use futures_util::TryStreamExt;
use std::fmt::Write;
use databug_core::{Capabilities, Catalog, ColumnMeta, ConnectionConfig, DbObject, Dialect, ForeignKey, IndexInfo, MsAuth, ObjectKind, Plan, QueryEvent, SslMode};
use tiberius::{AuthMethod, Client, ColumnData, Config, EncryptionLevel, FromSql, QueryItem};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

pub struct MsConn {
    client: Client<Compat<TcpStream>>,
}

fn ms_error(e: tiberius::error::Error) -> anyhow::Error {
    match e {
        tiberius::error::Error::Server(t) => anyhow::anyhow!("Msg {}, Level {}, Line {}: {}", t.code(), t.class(), t.line(), t.message()),
        other => anyhow::anyhow!("{other}"),
    }
}

/// Turn the saved auth choice into tiberius' method.
async fn auth_method(cfg: &ConnectionConfig, password: &str) -> anyhow::Result<AuthMethod> {
    match cfg.ms_auth {
        MsAuth::SqlServer => {
            if cfg.user.trim().is_empty() {
                anyhow::bail!("Enter a SQL Server login, or switch Authentication to Windows");
            }
            Ok(AuthMethod::sql_server(&cfg.user, password))
        }
        MsAuth::WindowsIntegrated => integrated(),
        MsAuth::WindowsCredentials => {
            let user = cfg.user.trim();
            if !user.contains('\\') && !user.contains('@') {
                anyhow::bail!("Enter the Windows account as DOMAIN\\user or user@domain.com");
            }
            windows_credentials(user, password).await
        }
    }
}

#[cfg(any(windows, all(unix, feature = "kerberos")))]
fn integrated() -> anyhow::Result<AuthMethod> {
    Ok(AuthMethod::Integrated)
}

#[cfg(not(any(windows, all(unix, feature = "kerberos"))))]
fn integrated() -> anyhow::Result<AuthMethod> {
    anyhow::bail!("This build of DataBug has no Kerberos support. Use the release download, or build with `--features kerberos`")
}

/// On Windows, SSPI signs in with the typed account (NTLM or Kerberos).
#[cfg(windows)]
async fn windows_credentials(user: &str, password: &str) -> anyhow::Result<AuthMethod> {
    Ok(AuthMethod::windows(user, password))
}

/// On macOS/Linux there is no SSPI, so get a Kerberos ticket for the typed
/// account with `kinit` and then sign in with that ticket.
#[cfg(all(unix, feature = "kerberos"))]
async fn windows_credentials(user: &str, password: &str) -> anyhow::Result<AuthMethod> {
    kinit(&kerberos_principal(user), password).await?;
    Ok(AuthMethod::Integrated)
}

#[cfg(not(any(windows, all(unix, feature = "kerberos"))))]
async fn windows_credentials(_user: &str, _password: &str) -> anyhow::Result<AuthMethod> {
    anyhow::bail!("This build of DataBug has no Kerberos support, which Windows credentials need on macOS/Linux. Use the release download, or build with `--features kerberos`")
}

/// `DOMAIN\user` → `user` (the machine's default realm); `user@corp.example.com`
/// → `user@CORP.EXAMPLE.COM` (Kerberos realms are upper case).
pub fn kerberos_principal(user: &str) -> String {
    if let Some((_, name)) = user.split_once('\\') {
        return name.to_string();
    }
    match user.rsplit_once('@') {
        Some((name, realm)) => format!("{name}@{}", realm.to_ascii_uppercase()),
        None => user.to_string(),
    }
}

#[cfg(all(unix, feature = "kerberos"))]
async fn kinit(principal: &str, password: &str) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut cmd = tokio::process::Command::new("kinit");
    // macOS ships Heimdal, which only reads a piped password when asked to;
    // MIT Kerberos (most Linux distros) reads it from stdin by default.
    if cfg!(target_os = "macos") {
        cmd.arg("--password-file=STDIN");
    }
    cmd.arg(principal).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("Couldn't run kinit ({e}). Install Kerberos tools (krb5-user / krb5-workstation)"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(password.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
    }
    let out = tokio::time::timeout(std::time::Duration::from_secs(20), child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!("kinit timed out reaching the domain controller"))??;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        let msg = msg.lines().filter(|l| !l.trim().is_empty() && !l.contains("Password for")).last().unwrap_or("kinit failed").trim().to_string();
        anyhow::bail!("Kerberos sign-in for {principal} failed: {msg}");
    }
    Ok(())
}

/// Whether "Windows (current user)" works in this build.
pub fn integrated_auth_available() -> bool {
    cfg!(any(windows, all(unix, feature = "kerberos")))
}

impl MsConn {
    pub async fn connect(cfg: &ConnectionConfig, password: &str) -> anyhow::Result<Self> {
        let mut config = Config::new();
        config.host(&cfg.host);
        config.port(cfg.port);
        config.authentication(auth_method(cfg, password).await?);
        config.application_name("DataBug");
        if !cfg.database.is_empty() {
            config.database(&cfg.database);
        }
        config.encryption(match cfg.ssl {
            SslMode::Disable => EncryptionLevel::NotSupported,
            SslMode::Prefer => EncryptionLevel::Off,
            SslMode::Require => EncryptionLevel::Required,
        });
        if cfg.trust_server_cert {
            config.trust_cert();
        }
        let client = match Self::open(config.clone()).await {
            // Azure SQL may redirect to another node.
            Err(tiberius::error::Error::Routing { host, port }) => {
                config.host(&host);
                config.port(port);
                Self::open(config).await.map_err(ms_error)?
            }
            other => other.map_err(ms_error)?,
        };
        Ok(MsConn { client })
    }

    async fn open(config: Config) -> tiberius::Result<Client<Compat<TcpStream>>> {
        let tcp = TcpStream::connect(config.get_addr()).await?;
        tcp.set_nodelay(true)?;
        Client::connect(config, tcp.compat_write()).await
    }

    async fn results(&mut self, sql: &str) -> anyhow::Result<Vec<Vec<tiberius::Row>>> {
        self.client.simple_query(sql).await.map_err(ms_error)?.into_results().await.map_err(ms_error)
    }

    async fn simple_strings(&mut self, sql: &str) -> anyhow::Result<Vec<Vec<String>>> {
        let results = self.client.simple_query(sql).await.map_err(ms_error)?.into_results().await.map_err(ms_error)?;
        Ok(results
            .into_iter()
            .map(|rows| rows.into_iter().filter_map(|r| r.get::<&str, _>(0).map(str::to_string)).collect())
            .collect())
    }
}

fn type_name(c: &tiberius::Column) -> String {
    let t = format!("{:?}", c.column_type()).to_ascii_lowercase();
    match t.as_str() {
        "int1" => "tinyint".into(),
        "int2" => "smallint".into(),
        "int4" | "intn" => "int".into(),
        "int8" => "bigint".into(),
        "float4" => "real".into(),
        "float8" | "floatn" => "float".into(),
        "bitn" => "bit".into(),
        "decimaln" | "numericn" => "decimal".into(),
        "datetimen" => "datetime".into(),
        "bigvarchar" => "varchar".into(),
        "bigchar" => "char".into(),
        "bigvarbin" => "varbinary".into(),
        _ => t,
    }
}

fn cell_text(d: &ColumnData<'static>, out: &mut String) -> bool {
    macro_rules! w {
        ($v:expr) => {{
            let _ = write!(out, "{}", $v);
            true
        }};
    }
    match d {
        ColumnData::U8(Some(v)) => w!(v),
        ColumnData::I16(Some(v)) => w!(v),
        ColumnData::I32(Some(v)) => w!(v),
        ColumnData::I64(Some(v)) => w!(v),
        ColumnData::F32(Some(v)) => w!(v),
        ColumnData::F64(Some(v)) => w!(v),
        ColumnData::Bit(Some(v)) => w!(if *v { "1" } else { "0" }),
        ColumnData::String(Some(v)) => {
            out.push_str(v);
            true
        }
        ColumnData::Guid(Some(v)) => w!(v.to_string().to_ascii_uppercase()),
        ColumnData::Numeric(Some(v)) => w!(v),
        ColumnData::Xml(Some(v)) => w!(v),
        ColumnData::Binary(Some(b)) => {
            out.push_str("0x");
            for byte in b.iter().take(4096) {
                let _ = write!(out, "{byte:02X}");
            }
            true
        }
        ColumnData::DateTime(Some(_)) | ColumnData::SmallDateTime(Some(_)) | ColumnData::DateTime2(Some(_)) => {
            match chrono::NaiveDateTime::from_sql(d) {
                Ok(Some(v)) => w!(v.format("%Y-%m-%d %H:%M:%S%.f")),
                _ => false,
            }
        }
        ColumnData::Date(Some(_)) => match chrono::NaiveDate::from_sql(d) {
            Ok(Some(v)) => w!(v),
            _ => false,
        },
        ColumnData::Time(Some(_)) => match chrono::NaiveTime::from_sql(d) {
            Ok(Some(v)) => w!(v),
            _ => false,
        },
        ColumnData::DateTimeOffset(Some(_)) => match chrono::DateTime::<chrono::FixedOffset>::from_sql(d) {
            Ok(Some(v)) => w!(v.format("%Y-%m-%d %H:%M:%S%.f %:z")),
            _ => false,
        },
        _ => false,
    }
}

/// DML without OUTPUT goes through `execute` so we get the affected row count;
/// everything else runs as a plain batch.
fn wants_row_count(stmt: &str) -> bool {
    let toks: Vec<_> = databug_sql::tokenize(stmt, Dialect::MsSql).into_iter().filter(|t| t.kind == databug_sql::TokKind::Keyword).collect();
    let first = toks.first().map(|t| t.text(stmt).to_ascii_uppercase());
    matches!(first.as_deref(), Some("INSERT" | "UPDATE" | "DELETE" | "MERGE"))
        && !stmt.to_ascii_uppercase().contains("OUTPUT")
        && databug_sql::split_statements(stmt, Dialect::MsSql).len() == 1
}

#[async_trait]
impl DbConnection for MsConn {
    fn capabilities(&self) -> Capabilities {
        Capabilities { schemas: true, transactions: true, explain: true, explain_analyze: true, server_cancel: false, sql: true }
    }

    fn canceller(&self) -> Option<Canceller> {
        None
    }

    async fn execute(&mut self, stmt: &str, opts: &ExecOptions, sink: &Sink) -> anyhow::Result<()> {
        if wants_row_count(stmt) {
            let r = self.client.execute(stmt, &[]).await.map_err(ms_error)?;
            sink(QueryEvent::Affected(r.total()));
            return Ok(());
        }
        let mut stream = self.client.simple_query(stmt).await.map_err(ms_error)?;
        let mut batch: Option<Batcher> = None;
        let mut keep = true;
        let mut any = false;
        while let Some(item) = stream.try_next().await.map_err(ms_error)? {
            match item {
                QueryItem::Metadata(m) => {
                    drop(batch.take());
                    let cols = m.columns().iter().map(|c| ColumnMeta::new(c.name(), type_name(c))).collect();
                    batch = Some(Batcher::new(sink, cols, opts.row_limit));
                    keep = true;
                    any = true;
                }
                QueryItem::Row(row) => {
                    if !keep {
                        continue;
                    }
                    let Some(b) = batch.as_mut() else { continue };
                    for (_, data) in row.cells() {
                        let rs = b.cells();
                        let mut is_value = false;
                        rs.push_with(|s| is_value = cell_text(data, s));
                        if !is_value {
                            // push_with already appended an empty cell; mark it null by re-pushing
                            rs.mark_last_null();
                        }
                    }
                    keep = b.row_done();
                }
            }
        }
        drop(batch);
        if !any {
            sink(QueryEvent::Message("Commands completed".into()));
        }
        Ok(())
    }

    async fn introspect(&mut self) -> anyhow::Result<Catalog> {
        let rows = collect(self, INTROSPECT).await?;
        let schemas = collect(self, "SELECT name FROM sys.schemas WHERE schema_id < 16384 AND name NOT IN ('sys','INFORMATION_SCHEMA','guest') ORDER BY name").await?;
        let mut extras = Extras::default();
        for r in collect(self, INDEXES).await? {
            extras.indexes.push((cell(&r, 0), cell(&r, 1), IndexInfo { name: cell(&r, 2), unique: truthy(r[3].as_deref()), primary: truthy(r[4].as_deref()), columns: split_list(r[5].as_ref()) }));
        }
        for r in collect(self, FOREIGN_KEYS).await? {
            let rule = cell(&r, 7).replace('_', " ");
            extras.foreign_keys.push((
                cell(&r, 0),
                cell(&r, 1),
                ForeignKey { name: cell(&r, 2), columns: split_list(r[3].as_ref()), ref_schema: cell(&r, 4), ref_table: cell(&r, 5), ref_columns: split_list(r[6].as_ref()), on_delete: if rule == "NO ACTION" { String::new() } else { rule } },
            ));
        }
        for r in collect(self, OBJECTS).await? {
            let kind = match cell(&r, 2).trim() {
                "P" => ObjectKind::Procedure,
                "FN" => ObjectKind::Function,
                "IF" | "TF" => ObjectKind::TableFunction,
                "PC" => ObjectKind::ClrProcedure,
                "FS" => ObjectKind::ClrFunction,
                "FT" => ObjectKind::ClrTableFunction,
                "AF" => ObjectKind::ClrAggregate,
                "TA" => ObjectKind::ClrTrigger,
                "TR" => ObjectKind::Trigger,
                "SO" => ObjectKind::Sequence,
                "SN" => ObjectKind::Synonym,
                _ => continue,
            };
            let parent = cell(&r, 3);
            let detail = match kind {
                ObjectKind::Trigger | ObjectKind::ClrTrigger => format!("on {parent}"),
                ObjectKind::Synonym => cell(&r, 4),
                _ => String::new(),
            };
            let table = matches!(kind, ObjectKind::Trigger | ObjectKind::ClrTrigger).then_some(parent).filter(|p| !p.is_empty());
            extras.objects.push((cell(&r, 0), DbObject { name: cell(&r, 1), kind, detail, ident: String::new(), table }));
        }
        let current = collect(self, "SELECT SCHEMA_NAME()").await?;
        let default = current.first().and_then(|r| r[0].clone()).unwrap_or_else(|| "dbo".into());
        for r in collect(self, "SELECT name, permission_set_desc FROM sys.assemblies WHERE is_user_defined = 1").await.unwrap_or_default() {
            extras.objects.push((default.clone(), DbObject { name: cell(&r, 0), kind: ObjectKind::Assembly, detail: cell(&r, 1).to_lowercase().replace('_', " "), ident: String::new(), table: None }));
        }
        Ok(build_catalog(rows, default, schemas.into_iter().filter_map(|r| r[0].clone()).collect(), extras))
    }

    async fn object_source(&mut self, schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
        let d = Dialect::MsSql;
        let q = format!("{}.{}", d.quote_ident(schema), d.quote_ident(&obj.name));
        let id = format!("OBJECT_ID({})", d.quote_literal(&q));
        match obj.kind {
            ObjectKind::View | ObjectKind::Procedure | ObjectKind::Function | ObjectKind::TableFunction | ObjectKind::Trigger => {
                let def = scalar(self, &format!("SELECT OBJECT_DEFINITION({id})")).await?.ok_or_else(|| anyhow::anyhow!("The definition of {q} is encrypted or you don't have VIEW DEFINITION permission"))?;
                // CREATE OR ALTER needs SQL Server 2016 SP1 or later
                Ok(replace_create(def.trim(), d, "CREATE OR ALTER") + "\nGO\n")
            }
            ObjectKind::ClrProcedure | ObjectKind::ClrFunction | ObjectKind::ClrTableFunction | ObjectKind::ClrTrigger => {
                let m = collect(self, &format!("SELECT a.name, am.assembly_class, am.assembly_method FROM sys.assembly_modules am JOIN sys.assemblies a ON a.assembly_id = am.assembly_id WHERE am.object_id = {id}")).await?;
                let m = m.first().ok_or_else(|| anyhow::anyhow!("CLR module not found"))?;
                let params = collect(
                    self,
                    &format!(
                        "SELECT p.name, TYPE_NAME(p.user_type_id), p.max_length, p.precision, p.scale, p.is_output, p.parameter_id FROM sys.parameters p WHERE p.object_id = {id} ORDER BY p.parameter_id"
                    ),
                )
                .await?;
                let ty = |r: &[Option<String>]| {
                    let t = cell(r, 1);
                    let len: i64 = cell(r, 2).parse().unwrap_or(0);
                    match t.as_str() {
                        "nvarchar" | "nchar" => format!("{t}({})", if len < 0 { "max".into() } else { (len / 2).to_string() }),
                        "varchar" | "char" | "varbinary" | "binary" => format!("{t}({})", if len < 0 { "max".into() } else { len.to_string() }),
                        "decimal" | "numeric" => format!("{t}({},{})", cell(r, 3), cell(r, 4)),
                        _ => t,
                    }
                };
                let args: Vec<String> = params.iter().filter(|r| cell(r, 6) != "0").map(|r| format!("{} {}{}", cell(r, 0), ty(r), if truthy(r[5].as_deref()) { " OUTPUT" } else { "" })).collect();
                let external = format!("EXTERNAL NAME {}.{}.{}", d.quote_ident(&cell(m, 0)), d.quote_ident(&cell(m, 1)), d.quote_ident(&cell(m, 2)));
                let body = match obj.kind {
                    ObjectKind::ClrProcedure => format!("CREATE OR ALTER PROCEDURE {q}\n    {}\nAS {external};", args.join(",\n    ")),
                    ObjectKind::ClrFunction => {
                        let ret = params.iter().find(|r| cell(r, 6) == "0").map(|r| ty(r)).unwrap_or_else(|| "sql_variant".into());
                        format!("CREATE OR ALTER FUNCTION {q} ({})\nRETURNS {ret}\nAS {external};", args.join(", "))
                    }
                    ObjectKind::ClrTableFunction => {
                        let cols = collect(self, &format!("SELECT c.name, TYPE_NAME(c.user_type_id) FROM sys.columns c WHERE c.object_id = {id} ORDER BY c.column_id")).await?;
                        let cols: Vec<String> = cols.iter().map(|c| format!("{} {}", d.quote_ident(&cell(c, 0)), cell(c, 1))).collect();
                        format!("CREATE OR ALTER FUNCTION {q} ({})\nRETURNS TABLE ({})\nAS {external};", args.join(", "), cols.join(", "))
                    }
                    _ => format!("-- CLR trigger {q}\n-- {external}"),
                };
                Ok(format!("-- CLR object: the code lives in assembly {}; this changes only the T-SQL binding.\n{body}\nGO\n", cell(m, 0)))
            }
            ObjectKind::Assembly => {
                let rows = collect(self, &format!("SELECT a.permission_set_desc, a.create_date, a.modify_date, a.clr_name FROM sys.assemblies a WHERE a.name = {}", d.quote_literal(&obj.name))).await?;
                let r = rows.first().ok_or_else(|| anyhow::anyhow!("Assembly not found"))?;
                let modules = collect(self, &format!("SELECT SCHEMA_NAME(o.schema_id) + '.' + o.name, o.type_desc FROM sys.assembly_modules am JOIN sys.objects o ON o.object_id = am.object_id JOIN sys.assemblies a ON a.assembly_id = am.assembly_id WHERE a.name = {}", d.quote_literal(&obj.name))).await?;
                let mut s = format!(
                    "-- Assembly {}\n-- {}\n-- permission set: {}\n-- created {}, modified {}\n-- Assemblies are compiled .NET code, so they can't be edited as text.\n-- To replace it: ALTER ASSEMBLY {} FROM 'path\\to\\new.dll';\n\n-- Objects bound to this assembly:\n",
                    obj.name,
                    cell(r, 3),
                    cell(r, 0),
                    cell(r, 1),
                    cell(r, 2),
                    d.quote_ident(&obj.name)
                );
                for m in modules {
                    s.push_str(&format!("--   {} ({})\n", cell(&m, 0), cell(&m, 1).to_lowercase()));
                }
                Ok(s)
            }
            ObjectKind::Sequence => {
                let rows = collect(self, &format!("SELECT CAST(start_value AS varchar(40)), CAST(increment AS varchar(40)), CAST(minimum_value AS varchar(40)), CAST(maximum_value AS varchar(40)), is_cycling, CAST(current_value AS varchar(40)) FROM sys.sequences WHERE object_id = {id}")).await?;
                let r = rows.first().ok_or_else(|| anyhow::anyhow!("Sequence not found"))?;
                Ok(format!(
                    "-- current value: {}\nALTER SEQUENCE {q}\n    RESTART WITH {}\n    INCREMENT BY {}\n    MINVALUE {}\n    MAXVALUE {}\n    {}CYCLE;\n",
                    cell(r, 5),
                    cell(r, 0),
                    cell(r, 1),
                    cell(r, 2),
                    cell(r, 3),
                    if truthy(r[4].as_deref()) { "" } else { "NO " }
                ))
            }
            ObjectKind::Synonym => {
                let base = scalar(self, &format!("SELECT base_object_name FROM sys.synonyms WHERE object_id = {id}")).await?.unwrap_or_default();
                Ok(format!("DROP SYNONYM IF EXISTS {q};\nGO\nCREATE SYNONYM {q} FOR {base};\nGO\n"))
            }
            k => anyhow::bail!("SQL Server has no {} objects", k.label()),
        }
    }

    async fn explain(&mut self, stmt: &str, analyze: bool) -> anyhow::Result<Plan> {
        let xml = if analyze {
            let modifies = databug_sql::classify(stmt, Dialect::MsSql).modifies();
            if modifies {
                self.simple_strings("BEGIN TRANSACTION").await?;
            }
            self.simple_strings("SET STATISTICS XML ON").await?;
            let res = self.results(stmt).await;
            let _ = self.simple_strings("SET STATISTICS XML OFF").await;
            if modifies {
                let _ = self.simple_strings("ROLLBACK").await;
            }
            res?.into_iter()
                .rev()
                .find_map(|rows| rows.first().and_then(|r| r.get::<&str, _>(0).filter(|s| s.contains("ShowPlanXML")).map(str::to_string)))
                .unwrap_or_default()
        } else {
            self.simple_strings("SET SHOWPLAN_XML ON").await?;
            let res = self.simple_strings(stmt).await;
            let _ = self.simple_strings("SET SHOWPLAN_XML OFF").await;
            res?.into_iter().flatten().collect::<Vec<_>>().join("")
        };
        Ok(Plan { root: plan::from_mssql_xml(&xml), raw: xml, format: "xml" })
    }
}

const INTROSPECT: &str = "SELECT s.name, o.name, CASE o.type WHEN 'V' THEN 'VIEW' ELSE 'TABLE' END, c.name, \
TYPE_NAME(c.user_type_id) + CASE \
 WHEN TYPE_NAME(c.user_type_id) IN ('varchar','char','varbinary','binary') THEN '(' + CASE WHEN c.max_length = -1 THEN 'max' ELSE CAST(c.max_length AS varchar(10)) END + ')' \
 WHEN TYPE_NAME(c.user_type_id) IN ('nvarchar','nchar') THEN '(' + CASE WHEN c.max_length = -1 THEN 'max' ELSE CAST(c.max_length / 2 AS varchar(10)) END + ')' \
 WHEN TYPE_NAME(c.user_type_id) IN ('decimal','numeric') THEN '(' + CAST(c.precision AS varchar(10)) + ',' + CAST(c.scale AS varchar(10)) + ')' \
 ELSE '' END, \
CAST(c.is_nullable AS varchar(1)), dc.definition, CASE WHEN ic.column_id IS NULL THEN 'false' ELSE 'true' END \
FROM sys.objects o JOIN sys.schemas s ON s.schema_id = o.schema_id JOIN sys.columns c ON c.object_id = o.object_id \
LEFT JOIN sys.default_constraints dc ON dc.object_id = c.default_object_id \
LEFT JOIN sys.indexes i ON i.object_id = o.object_id AND i.is_primary_key = 1 \
LEFT JOIN sys.index_columns ic ON ic.object_id = o.object_id AND ic.index_id = i.index_id AND ic.column_id = c.column_id \
WHERE o.type IN ('U','V') AND o.is_ms_shipped = 0 \
ORDER BY s.name, o.name, c.column_id";

const INDEXES: &str = "SELECT s.name, t.name, i.name, CAST(i.is_unique AS varchar(1)), CAST(i.is_primary_key AS varchar(1)), \
STUFF((SELECT ',' + c.name FROM sys.index_columns ic JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
 WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id AND ic.is_included_column = 0 ORDER BY ic.key_ordinal FOR XML PATH('')), 1, 1, '') \
FROM sys.indexes i JOIN sys.tables t ON t.object_id = i.object_id JOIN sys.schemas s ON s.schema_id = t.schema_id \
WHERE i.type > 0 AND t.is_ms_shipped = 0";

const FOREIGN_KEYS: &str = "SELECT s.name, t.name, fk.name, \
STUFF((SELECT ',' + c.name FROM sys.foreign_key_columns fc JOIN sys.columns c ON c.object_id = fc.parent_object_id AND c.column_id = fc.parent_column_id \
 WHERE fc.constraint_object_id = fk.object_id ORDER BY fc.constraint_column_id FOR XML PATH('')), 1, 1, ''), \
rs.name, rt.name, \
STUFF((SELECT ',' + c.name FROM sys.foreign_key_columns fc JOIN sys.columns c ON c.object_id = fc.referenced_object_id AND c.column_id = fc.referenced_column_id \
 WHERE fc.constraint_object_id = fk.object_id ORDER BY fc.constraint_column_id FOR XML PATH('')), 1, 1, ''), \
fk.delete_referential_action_desc \
FROM sys.foreign_keys fk JOIN sys.tables t ON t.object_id = fk.parent_object_id JOIN sys.schemas s ON s.schema_id = t.schema_id \
JOIN sys.tables rt ON rt.object_id = fk.referenced_object_id JOIN sys.schemas rs ON rs.schema_id = rt.schema_id";

const OBJECTS: &str = "SELECT SCHEMA_NAME(o.schema_id), o.name, o.type, ISNULL(OBJECT_NAME(o.parent_object_id), ''), ISNULL(sn.base_object_name, '') \
FROM sys.objects o LEFT JOIN sys.synonyms sn ON sn.object_id = o.object_id \
WHERE o.type IN ('P','PC','FN','IF','TF','FS','FT','AF','TR','TA','SO','SN') AND o.is_ms_shipped = 0";

#[cfg(test)]
mod auth_tests {
    use super::*;

    #[test]
    fn principals() {
        assert_eq!(kerberos_principal("CORP\\hayes"), "hayes");
        assert_eq!(kerberos_principal("hayes@corp.example.com"), "hayes@CORP.EXAMPLE.COM");
        assert_eq!(kerberos_principal("hayes"), "hayes");
    }

    fn cfg(auth: MsAuth, user: &str) -> ConnectionConfig {
        let mut c = ConnectionConfig::new(databug_core::DbKind::MsSql);
        c.host = "127.0.0.1".into();
        c.port = 1; // nothing listens: getting a connection error means auth prep succeeded
        c.user = user.into();
        c.ms_auth = auth;
        c
    }

    #[tokio::test]
    async fn auth_validation() {
        let e = MsConn::connect(&cfg(MsAuth::WindowsCredentials, "hayes"), "x").await.err().unwrap().to_string();
        assert!(e.contains("DOMAIN\\user"), "{e}");
        let e = MsConn::connect(&cfg(MsAuth::SqlServer, ""), "x").await.err().unwrap().to_string();
        assert!(e.contains("SQL Server login"), "{e}");
    }

    /// Needs a reachable KDC: DATABUG_TEST_KRB="user@REALM:password".
    #[cfg(all(unix, feature = "kerberos"))]
    #[tokio::test]
    async fn kerberos_credentials() {
        let Ok(spec) = std::env::var("DATABUG_TEST_KRB") else { return };
        let (user, pw) = spec.split_once(':').unwrap();
        let e = MsConn::connect(&cfg(MsAuth::WindowsCredentials, user), "wrong-password").await.err().unwrap().to_string();
        assert!(e.contains("Kerberos sign-in"), "{e}");
        let e = MsConn::connect(&cfg(MsAuth::WindowsCredentials, user), pw).await.err().unwrap().to_string();
        assert!(!e.contains("Kerberos"), "kinit should succeed, then the TCP connect fails: {e}");
        let out = std::process::Command::new("klist").output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains(&kerberos_principal(user)));
    }
}
