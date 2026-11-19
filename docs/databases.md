# Databases

Every engine DataBug supports, how each is reached, and what the explorer and consoles do for it. Back to the [README](../README.md).

| Engine | Driver crate | Queries and results | Schema introspection | Explain | Server-side cancel | Tested against a live server |
|---|---|---|---|---|---|---|
| PostgreSQL | `tokio-postgres` | ✔ | ✔ (pg_catalog) | JSON tree, `ANALYZE` | ✔ | ✔ Postgres 16 |
| MySQL / MariaDB | `mysql_async` | ✔ | ✔ (information_schema) | JSON tree, `ANALYZE` text | ✔ (`KILL QUERY`) | ✔ MariaDB 10.11 |
| SQLite | `rusqlite` (bundled) | ✔ | ✔ | `EXPLAIN QUERY PLAN` tree | ✔ (interrupt) | ✔ |
| SQL Server | `tiberius` | ✔ | ✔ (sys catalog) | Showplan XML tree | drops connection | ✔ live server |
| MongoDB | `mongodb` | ✔ shell syntax | ✔ (sampled fields) | `explain` tree | drops connection | compiles, not yet run against a server |
| CockroachDB | `tokio-postgres` (PostgreSQL protocol) | ✔ | ✔ (information_schema) | text (`EXPLAIN (VERBOSE)`, `ANALYZE`) | ✔ | ✔ CockroachDB 24.3 (Docker) |
| TimescaleDB | `tokio-postgres` (PostgreSQL protocol) | ✔ | ✔ (pg_catalog; chunks hidden, continuous aggregates as materialized views) | JSON tree, `ANALYZE` | ✔ | ✔ TimescaleDB on Postgres 16 (Docker) |
| QuestDB | `tokio-postgres` (PostgreSQL protocol, port 8812) | ✔ | ✔ (`SHOW TABLES` / `SHOW COLUMNS`) | text | drops connection | ✔ QuestDB (Docker, latest) |
| H2 | `tokio-postgres` (H2's `-pg` server mode) | ✔ | ✔ (information_schema) | text, `ANALYZE` | drops connection | ✔ H2 2.3.232 (`-pg`) |
| DuckDB | official `libduckdb` 2.0.0, downloaded on demand and loaded at run time | ✔ | ✔ (information_schema, macros, sequences) | text, `ANALYZE` | ✔ (interrupt) | ✔ DuckDB 2.0.0 |
| Apache Cassandra | `scylla` | ✔ CQL | ✔ (system_schema; keyspaces as schemas) | — | drops connection | ✔ Cassandra 5 (Docker) |
| CouchDB | `reqwest` (HTTP API) | ✔ Mango in shell syntax | ✔ (databases as collections, sampled fields, indexes) | `_explain` JSON | drops connection | ✔ CouchDB 3 (Docker) |
| Redis | `redis` (RESP2; TLS through rustls) | ✔ commands as in redis-cli, replies shaped per command | ✔ (keys with type, expiry and sampled hash fields) | — | drops connection | ✔ Redis 8 (Docker) |
| InfluxDB | `reqwest` (HTTP API) | ✔ InfluxQL, Flux (2.x), line-protocol `INSERT` | ✔ (measurements, tags, fields) | — | drops connection | ✔ InfluxDB 2.7 (Docker) |
| Oracle | `oracle` (ODPI-C; Oracle Instant Client when connecting) | ✔ | ✔ (ALL_* views) | `DBMS_XPLAN` text | ✔ (break) | compiles, not yet run against a server |
| IBM Db2 | `odbc-api` (driver manager built in; IBM Db2 ODBC driver when connecting) | ✔ | ✔ (SYSCAT) | — | drops connection | compiles, not yet run against a server |
| Spark SQL | `odbc-api` (Thrift server through a Spark ODBC driver) | ✔ | ✔ (`SHOW` / `DESCRIBE`) | `EXPLAIN FORMATTED` text | drops connection | compiles, not yet run against a server |

The new-connection dialog picks the engine from a dropdown that shows each engine's logo and groups them by type: **Relational / SQL** (PostgreSQL, MySQL, SQL Server, SQLite, Oracle, Db2, H2), **Analytical** (DuckDB), **NoSQL · Document** (MongoDB, CouchDB), **NoSQL · Wide-column** (Cassandra), **NoSQL · Key-value** (Redis), **Time series** (TimescaleDB, QuestDB, InfluxDB) and **Big data / distributed** (CockroachDB, Spark SQL).

## What the explorer shows

| | PostgreSQL | MySQL / MariaDB | SQL Server | SQLite | MongoDB |
|---|---|---|---|---|---|
| Tables, columns, keys, indexes, foreign keys | ✔ | ✔ | ✔ | ✔ | collections, sampled fields, indexes |
| Views | ✔ | ✔ | ✔ | ✔ | ✔ |
| Materialized views | ✔ | | | | |
| Stored procedures | ✔ | ✔ | ✔ | | |
| Functions (scalar, table-valued, aggregates) | ✔ (with overloads) | ✔ | ✔ | | |
| Triggers | ✔ | ✔ | ✔ | ✔ | |
| Sequences | ✔ | | ✔ | | |
| Synonyms | | | ✔ | | |
| Events | | ✔ | | | |
| CLR procedures, functions, aggregates, triggers, assemblies | | | ✔ | | |

Every object with a definition has an **edit** button (the pencil that appears on hover or selection). Tables open the table editor. Views, materialized views, procedures, functions, triggers, sequences, synonyms and events open their definition as runnable DDL in a console: `CREATE OR REPLACE` on Postgres and MySQL views, `CREATE OR ALTER` on SQL Server (2016 SP1+), `DROP` + `CREATE` where the engine has no replace (MySQL routines, SQLite, Postgres materialized views). CLR objects open their `EXTERNAL NAME` binding, and assemblies show their details and the objects bound to them.

Each driver is a cargo feature (`postgres`, `mysql`, `mssql`, `sqlite`, `mongodb`, `cockroachdb`, `timescaledb`, `questdb`, `h2`, `duckdb`, `cassandra`, `couchdb`, `influxdb`, `oracle`, `db2`, `spark`), so a build can leave out the ones it doesn't need. Everything except `cassandra-tls` is in the default build (it pulls in rustls' aws-lc backend, a C library), and building needs no system libraries: ODPI-C (Oracle) and the unixODBC driver manager (Db2, Spark on macOS/Linux) are compiled in, and Windows has its own ODBC. DataBug starts without any vendor software installed; three engines need theirs only when you connect:

| Engine | To connect |
|---|---|
| Oracle | [Oracle Instant Client](https://www.oracle.com/database/technologies/instant-client.html) on the library path (`DYLD_LIBRARY_PATH` / `LD_LIBRARY_PATH` / `PATH`) or next to DataBug |
| IBM Db2 | the IBM Data Server Driver for ODBC and CLI, registered as `IBM DB2 ODBC DRIVER` in `odbcinst.ini` (point `ODBCSYSINI` at its folder on macOS/Linux) or set *ODBC driver* in the dialog to the driver library's full path |
| Spark SQL | a Spark ODBC driver (Simba or Databricks), registered the same way, and a running Spark Thrift server; *ODBC options* sets `ThriftTransport`, `HTTPPath` and friends |

**DuckDB is downloaded, not built in.** Choosing DuckDB in the new-connection dialog offers **Download DuckDB**: DataBug fetches DuckDB's official library for your platform (about 40 MB on macOS, 28 MB on Linux, 20 MB on Windows) from [DuckDB's GitHub releases](https://github.com/duckdb/duckdb/releases/tag/v2.0.0) over verified HTTPS, checks it against a SHA-256 pinned in the source, keeps only the library (in DataBug's data folder under `runtimes/duckdb-2.0.0`) and loads it when you connect. Nothing is downloaded unless you click the button. To use a DuckDB library you already have, set `DATABUG_DUCKDB_LIB=/path/to/libduckdb.dylib` (`.so`, `duckdb.dll`).

Engines without a dialect of their own borrow the closest one for highlighting, completion and generated SQL: the PostgreSQL-compatible engines, Oracle, Db2, DuckDB, Cassandra and InfluxDB use the Postgres one (double-quoted identifiers), Spark SQL the MySQL one (backticks), and CouchDB the MongoDB shell syntax. Redis has its own: one command per line, as in redis-cli. Oracle gets `LIMIT n` rewritten to `FETCH FIRST n ROWS ONLY`. PL/SQL and SQL PL blocks run as one statement (packages up to a `/` line).

CouchDB consoles take Mango queries in shell form: `db.orders.find({status: "open"}, {total: 1}).sort({total: -1}).limit(20)`, `findOne`, `countDocuments`, `insertOne/Many`, `createIndex`, `getIndexes`, `drop()`, `db.createCollection('name')`, `show dbs`, and `GET /path` for any read-only API call. `updateOne`, `updateMany`, `replaceOne`, `deleteOne` and `deleteMany` work too: DataBug reads the matching documents with their `_rev`, applies the change and writes them back in one bulk request. Updates take MongoDB's operators (`$set`, `$unset`, `$inc`, `$mul`, `$min`, `$max`, `$rename`, `$currentDate`, `$push`, `$addToSet`, `$pull`, `$pullAll`, `$pop`), with dotted paths for nested fields; a document someone changed in the meantime is reported as a conflict instead of being overwritten.

Results from CouchDB and MongoDB open in a **document view** (switch with **Documents** / **Table** above the result): the documents are listed on the left, and the selected one shows every field, `_id` and `_rev` included, as a tree you can expand, or as raw JSON. **Edit** turns the document into JSON you can change; **Save** sends only what changed, as one `updateOne` with `$set` and `$unset` (on CouchDB it also checks `_rev`, so it never overwrites a newer version). **Delete** asks first, then deletes the document. Both run like any console statement, so read-only and production connections are respected, and the result is reloaded afterwards. CouchDB attachments are listed with their content type, size, revision and digest, and with their link (copy it with **Copy link**; the link never includes credentials). **Open** downloads an attachment through the connection and opens it with the system's default app, **Save…** saves it, and images (PNG, JPEG, GIF) are previewed inline. Downloads are capped at 200 MB. InfluxDB consoles take InfluxQL (`SHOW MEASUREMENTS`, `SELECT mean("total") FROM "orders" GROUP BY time(1h)`), Flux on 2.x (anything starting with `from(` or using `|>`; needs the organization) and `INSERT <line protocol>`; the API token goes in the password field. Cassandra (TLS needs the `cassandra-tls` feature), CouchDB, InfluxDB, Oracle, Db2 and Spark offer TLS *Off* or *Require* only: DataBug signs in once and never retries over the other transport. Cassandra connections all go through the host you enter (nodes often advertise internal addresses that aren't reachable through Docker, NAT or port-forwards); any node can coordinate a request.

Redis consoles run one command per line, written as in redis-cli (`HGETALL user:1`, `SET greeting "hello world" EX 60`; `#` starts a comment). Replies come back as tables shaped for the command: `HGETALL` as field / value, `ZRANGE … WITHSCORES` as member / score, `XRANGE` as one row per entry with each field as a column, `SCAN` with its next cursor, `INFO` as section / key / value, `LRANGE` with indexes, and other replies as values. The explorer lists the database's keys (up to 5,000, found with `SCAN`) with their type and expiry; double-click a key to show its value with the right command for its type, or *Show DDL* for a small script with `TTL`, `MEMORY USAGE` and `OBJECT ENCODING`. Completion knows 185 commands with their arguments and a summary from the Redis documentation, offers keys (those of the type the command works on first), hash fields, and option words like `EX`, `NX` and `WITHSCORES`. The Database field is the logical database number (`0` by default); the user is an ACL user (Redis 6+) or empty. Read-only connections allow only commands that don't write; `FLUSHDB`, `FLUSHALL` and multi-key `DEL` ask for confirmation on production connections. Commands that stream forever (`SUBSCRIBE`, `MONITOR`) aren't allowed in a console.

MongoDB and CouchDB completion follows their documentation: collection and cursor methods with their signatures, query operators (`$gt`, `$in`, `$elemMatch`, and CouchDB's `$beginsWith` and `$allMatch`), update operators in `updateOne` / `updateMany`, and pipeline stages and expressions in `aggregate`, each with a one-line description. The ones that fit the method you're in come first.

MongoDB consoles accept shell-style statements: `db.orders.find({status: "open"}).sort({total: -1}).limit(20)`, `aggregate([...])`, `countDocuments`, `distinct`, `insertOne/Many`, `updateOne/Many`, `deleteOne/Many`, `createIndex`, `getIndexes`, `db.runCommand({...})`, `show collections`, `show dbs`. `ObjectId("…")` and `ISODate("…")` work inside filters.

## SQL Server sign-in

The connection dialog's **Authentication** row offers three options:

| Option | What you enter | Windows | macOS / Linux |
|---|---|---|---|
| SQL Server | login + password | ✔ | ✔ |
| Windows (current user) | nothing | ✔ SSPI, as the logged-in account | ✔ uses your Kerberos ticket (`kinit you@EXAMPLE.COM` first) |
| Windows credentials | `DOMAIN\user` or `user@example.com` + password | ✔ SSPI (NTLM or Kerberos) | ✔ DataBug runs `kinit` for that account, then signs in with the ticket |

On macOS and Linux, Windows sign-in is Kerberos only (no NTLM), so the machine needs a `krb5.conf` that knows your domain, and the **Host** must be the server's full DNS name (e.g. `sql01.example.com`), not an IP, so the `MSSQLSvc/` service name resolves. The release builds include Kerberos; when building yourself add `--features kerberos` (Linux needs `libkrb5-dev` / `krb5-devel` and clang). Passwords still go to the OS keychain, never the config file.
